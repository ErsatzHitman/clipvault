use crate::{
    cloud::Cloud,
    config::{Config, Paths},
    model::{Capture, Entry, SearchHit, SearchMode},
    storage::Store,
};
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
pub struct SearchRequest {
    pub generation: u64,
    pub query: String,
    pub mode: SearchMode,
}
pub enum Command {
    Capture(Capture, String),
    Delete(i64),
    Pin(i64, bool),
    Clear,
    Reindex,
    Save(Config),
    Preview(i64),
}
pub enum Event {
    Changed,
    Status(String),
    Counts(u64, u64),
    Results {
        generation: u64,
        hits: Vec<SearchHit>,
        semantic: bool,
        error: Option<String>,
    },
    Preview {
        id: i64,
        image: Option<(usize, usize, Vec<u8>)>,
        text: String,
    },
}
pub struct Engine {
    pub commands: Sender<Command>,
    pub queries: Sender<SearchRequest>,
    pub events: Receiver<Event>,
    pub config: Arc<RwLock<Config>>,
    pub stop: Arc<AtomicBool>,
}
impl Engine {
    pub fn start(paths: Paths, c: Config, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        // Initialize schema once before threads start.
        let mut store = Store::open(paths.clone())?;
        store.cleanup(&c)?;
        store.reindex_mismatched(&c.model_key())?;
        let config = Arc::new(RwLock::new(c));
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, rx) = bounded(32);
        let (queries, qrx) = bounded::<SearchRequest>(4);
        let (tx, events) = bounded(128);
        let emit = {
            let tx = tx.clone();
            let repaint = repaint.clone();
            move |e| {
                let _ = tx.try_send(e);
                repaint();
            }
        };
        let cfg = config.clone();
        let end = stop.clone();
        let storage_paths = paths.clone();
        thread::Builder::new().name("clipvault-storage".into()).spawn(move || {
            let mut last_cleanup=std::time::Instant::now();
            let mut changed=true;
            while !end.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(command)=> {
                        let result:Result<()>= (|| { match command {
                            Command::Capture(capture,source)=> { let c=cfg.read().unwrap().clone(); if !c.paused && !c.excludes(if source=="Unknown app" {None}else{Some(&source)}) { store.insert(capture,&source,&c)?; changed=true; } }
                            Command::Delete(id)=> {store.delete(id)?;changed=true;}
                            Command::Pin(id,pin)=> {store.pin(id,pin)?;changed=true;}
                            Command::Clear=> {store.clear()?;changed=true;}
                            Command::Reindex=> {store.reindex()?;changed=true;}
                            Command::Save(c)=> {c.save(&storage_paths.config)?;store.reindex_mismatched(&c.model_key())?;*cfg.write().unwrap()=c;store.cleanup(&cfg.read().unwrap())?;changed=true;emit(Event::Status("Settings saved. Shortcut changes take effect after restart.".into()));}
                            Command::Preview(id)=> { if let Some(e)=store.get(id)? { let image=if e.kind==crate::model::Kind::Image { let path=store.blob_path(e.blob.as_deref().unwrap_or(""))?; let img=image::open(path)?.thumbnail(900,700).into_rgba8(); Some((img.width() as usize,img.height() as usize,img.into_raw())) } else {None}; emit(Event::Preview {id,image,text:e.text.chars().take(100_000).collect()}); } }
                        } Ok(()) })();
                        if let Err(error)=result {emit(Event::Status(format!("Storage: {error:#}")));}
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected)=>break,
                    Err(_)=>{}
                }
                if last_cleanup.elapsed()>Duration::from_secs(60) { if let Err(e)=store.cleanup(&cfg.read().unwrap()) {emit(Event::Status(format!("Cleanup: {e}")));} last_cleanup=std::time::Instant::now(); }
                if changed { emit(Event::Changed); if let Ok((all,ready))=store.counts(){emit(Event::Counts(all,ready));} changed=false; }
            }
            let _=store.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        })?;
        let (semantic_tx, semantic_rx) = bounded::<(SearchRequest, Vec<SearchHit>)>(1);
        let replace_stale = semantic_rx.clone();
        let end = stop.clone();
        let keyword_paths = paths.clone();
        let keyword_tx = tx.clone();
        let keyword_paint = repaint.clone();
        thread::Builder::new()
            .name("clipvault-keyword".into())
            .spawn(move || {
                let Ok(store) = Store::open(keyword_paths) else {
                    return;
                };
                while !end.load(Ordering::Relaxed) {
                    let Ok(mut request) = qrx.recv_timeout(Duration::from_millis(200)) else {
                        continue;
                    };
                    while let Ok(new) = qrx.try_recv() {
                        request = new;
                    }
                    let hits = if request.query.trim().is_empty() {
                        store.recent(300)
                    } else {
                        store.keyword(&request.query, 300)
                    };
                    match hits {
                        Ok(hits) => {
                            let _ = keyword_tx.try_send(Event::Results {
                                generation: request.generation,
                                hits: hits.clone(),
                                semantic: false,
                                error: None,
                            });
                            keyword_paint();
                            if request.mode != SearchMode::Keyword
                                && !request.query.trim().is_empty()
                            {
                                let _ = replace_stale.try_recv();
                                let _ = semantic_tx.try_send((request, hits));
                            }
                        }
                        Err(e) => {
                            let _ =
                                keyword_tx.try_send(Event::Status(format!("Keyword search: {e}")));
                            keyword_paint();
                        }
                    }
                }
            })?;
        let cfg = config.clone();
        let end = stop.clone();
        let search_paths = paths.clone();
        let etx = tx.clone();
        let paint = repaint.clone();
        thread::Builder::new()
            .name("clipvault-search".into())
            .spawn(move || {
                let emit = |e| {
                    let _ = etx.try_send(e);
                    paint();
                };
                let Ok(store) = Store::open(search_paths) else {
                    emit(Event::Status("Cannot open search database".into()));
                    return;
                };
                let cloud = Cloud::new();
                let mut cache: Option<(String, String, Vec<f32>)> = None;
                while !end.load(Ordering::Relaxed) {
                    let Ok((request, keyword)) =
                        semantic_rx.recv_timeout(Duration::from_millis(200))
                    else {
                        continue;
                    };
                    let c = cfg.read().unwrap().clone();
                    let semantic: Result<Vec<SearchHit>> = (|| {
                        let key = c.model_key();
                        if !c.cloud_enabled {anyhow::bail!("Cloud semantic search is disabled in Settings");}
                        if store.vector_count(&key,None)?==0 {anyhow::bail!("Semantic index is empty for this model; enable cloud indexing or rebuild the index");}
                        let vector = if let Some((q, m, v)) = &cache {
                            if *q == request.query && *m == key {
                                v.clone()
                            } else {
                                cloud
                                    .as_ref()
                                    .map_err(|e| anyhow::anyhow!(e.to_string()))?
                                    .text(&c, &request.query)?
                            }
                        } else {
                            cloud
                                .as_ref()
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                                .text(&c, &request.query)?
                        };
                        cache = Some((request.query.clone(), key.clone(), vector.clone()));
                        if store.vector_count(&key,Some(vector.len()))?==0 {anyhow::bail!("Embedding dimensions changed; rebuild the semantic index");}
                        store.semantic(&vector, &key, 300)
                    })();
                    match semantic {
                        Ok(hits) => {
                            let hits = if request.mode == SearchMode::Hybrid {
                                fuse(keyword, hits)
                            } else {
                                hits
                            };
                            emit(Event::Results {
                                generation: request.generation,
                                hits,
                                semantic: true,
                                error: None,
                            });
                        }
                        Err(e) => emit(Event::Results {
                            generation: request.generation,
                            hits: keyword,
                            semantic: false,
                            error: if request.mode==SearchMode::Hybrid && !c.cloud_enabled {None}else{Some(e.to_string())},
                        }),
                    }
                }
            })?;
        let cfg = config.clone();
        let end = stop.clone();
        let paint = repaint;
        thread::Builder::new()
            .name("clipvault-index".into())
            .spawn(move || {
                let emit = |e| {
                    let _ = tx.try_send(e);
                    paint();
                };
                let Ok(store) = Store::open(paths) else {
                    emit(Event::Status("Cannot open indexing database".into()));
                    return;
                };
                let cloud = match Cloud::new() {
                    Ok(c) => c,
                    Err(e) => {
                        emit(Event::Status(e.to_string()));
                        return;
                    }
                };
                while !end.load(Ordering::Relaxed) {
                    let c = cfg.read().unwrap().clone();
                    if c.paused
                        || !c.cloud_enabled
                        || std::env::var(c.api_key_env())
                            .unwrap_or_default()
                            .is_empty()
                    {
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    let next = store.next_pending();
                    match next {
                        Ok(Some(e)) => {
                            let path = e.blob.as_ref().and_then(|b| store.blob_path(b).ok());
                            match cloud.entry(&c, &e, path.as_deref()) {
                                Ok(vector) => {
                                    // Discard work if privacy settings or model identity changed while HTTP was in flight.
                                    let current = cfg.read().unwrap().clone();
                                    if current.cloud_enabled
                                        && !current.paused
                                        && current.model_key() == c.model_key()
                                    {
                                        if let Err(error) =
                                            store.save_vector(e.id, &c.model_key(), &vector)
                                        {
                                            emit(Event::Status(format!("Index: {error}")));
                                        }
                                        if let Ok((all, ready)) = store.counts() {
                                            emit(Event::Counts(all, ready));
                                        }
                                    }
                                }
                                Err(error) => {
                                    let msg = error.to_string();
                                    let _ = store.fail_index(e.id, &msg);
                                    emit(Event::Status(format!("Semantic indexing: {msg}")));
                                    thread::sleep(Duration::from_secs(2));
                                }
                            }
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(600)),
                        Err(e) => {
                            emit(Event::Status(format!("Index database: {e}")));
                            thread::sleep(Duration::from_secs(2));
                        }
                    }
                }
            })?;
        Ok(Self {
            commands,
            queries,
            events,
            config,
            stop,
        })
    }
    pub fn send(&self, c: Command) -> bool {
        self.commands.try_send(c).is_ok()
    }
    pub fn search(&self, q: SearchRequest) -> bool {
        self.queries.try_send(q).is_ok()
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
pub fn fuse(keyword: Vec<SearchHit>, semantic: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut scores: HashMap<i64, (Entry, f32)> = HashMap::new();
    for list in [keyword, semantic] {
        for (rank, hit) in list.into_iter().enumerate() {
            let s = 1.0 / (60.0 + rank as f32);
            scores
                .entry(hit.entry.id)
                .and_modify(|v| v.1 += s)
                .or_insert((hit.entry, s));
        }
    }
    let mut hits: Vec<_> = scores
        .into_values()
        .map(|(entry, score)| SearchHit { entry, score })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.entry.last_seen.cmp(&a.entry.last_seen))
    });
    hits.truncate(300);
    hits
}
