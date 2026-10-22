use crate::{
    config::{Config, Paths},
    engine::Command,
    model::{Capture, Kind},
    platform::{self, Foreground},
    storage::{Store, StoredFile},
};
use anyhow::{Context, Result, bail};
use arboard::{Clipboard, ImageData};
use crossbeam_channel::{Receiver, Sender, bounded};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static STATUS: OnceLock<(Sender<String>, Receiver<String>)> = OnceLock::new();
pub fn status_receiver() -> &'static Receiver<String> {
    &STATUS.get_or_init(|| bounded(32)).1
}
fn status(s: String) {
    let _ = STATUS.get_or_init(|| bounded(32)).0.try_send(s);
}
pub enum ClipboardAction {
    Restore(i64, bool),
}
pub struct ClipboardWorker;
impl ClipboardWorker {
    pub fn start(
        paths: Paths,
        config: Arc<RwLock<Config>>,
        stop: Arc<AtomicBool>,
        commands: Sender<Command>,
        focus: Arc<Mutex<Option<Foreground>>>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Sender<ClipboardAction>> {
        let (tx, rx) = bounded(8);
        thread::Builder::new().name("clipvault-clipboard".into()).spawn(move||{
            let mut clipboard=match Clipboard::new(){Ok(c)=>c,Err(e)=>{status(format!("Clipboard unavailable: {e}"));repaint();return;}};
            let store=match Store::open(paths){Ok(s)=>s,Err(e)=>{status(format!("Clipboard database: {e}"));repaint();return;}};
            let mut last:Option<Vec<u8>>=None;
            let mut deadline=Instant::now();
            let mut warned_unknown=false;
            let mut suppress_until=Instant::now();
            while !stop.load(Ordering::Relaxed){
                match rx.recv_timeout(Duration::from_millis(80)){
                    Ok(ClipboardAction::Restore(id,paste))=>{
                        let result:Result<()>=(||{
                            let e=store.get(id)?.context("History item no longer exists")?;
                            match e.kind {
                                Kind::Text=>clipboard.set_text(e.text)?,
                                Kind::Image=>{
                                    let path=store.blob_path(e.blob.as_deref().context("Missing image")?)?;
                                    let img=image::open(path)?.into_rgba8();
                                    clipboard.set_image(ImageData {width:img.width() as usize,height:img.height() as usize,bytes:Cow::Owned(img.into_raw())})?;
                                }
                                Kind::Files=>{
                                    let path=store.blob_path(e.blob.as_deref().context("Missing file manifest")?)?;
                                    let manifest:Vec<StoredFile>=serde_json::from_slice(&std::fs::read(path)?)?;
                                    let files=manifest.iter().map(|f|store.blob_path(&f.stored)).collect::<Result<Vec<_>>>()?;
                                    if files.iter().any(|p|!p.is_file()){bail!("Managed file is missing");}
                                    clipboard.set().file_list(&files)?;
                                }
                            }
                            suppress_until=Instant::now()+Duration::from_secs(2);
                            if let Ok(Some(capture))=read_capture(&mut clipboard,&config.read().unwrap()) {last=Some(fingerprint(&capture));}
                            if paste {let target=focus.lock().unwrap().clone();thread::sleep(Duration::from_millis(120));platform::paste(target.as_ref())?;}
                            Ok(())
                        })();
                        match result{Ok(())=>status("Clipboard restored.".into()),Err(e)=>status(format!("Restore: {e:#}"))};repaint();
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected)=>break,Err(_)=>{}
                }
                if Instant::now()<deadline{continue;}
                let c=config.read().unwrap().clone();deadline=Instant::now()+Duration::from_millis(c.poll_ms);
                if c.paused || Instant::now()<suppress_until{continue;}
                let before=platform::foreground();
                let source=before.as_ref().map(|a|a.app.as_str());
                if source.is_some_and(|s|s.to_lowercase().contains("clipvault")){continue;}
                if c.excludes(source){if source.is_none()&&!warned_unknown{status("Capture paused: foreground app is unknown. Check OS permissions, or explicitly allow unknown sources in Settings.".into());repaint();warned_unknown=true;}continue;}
                warned_unknown=false;
                if let Some(app)=&before{*focus.lock().unwrap()=Some(app.clone());}
                if let Ok(Some(capture))=read_capture(&mut clipboard,&c){
                    let hash=fingerprint(&capture);
                    if last.as_ref()==Some(&hash){continue;}
                    let after=platform::foreground();
                    if before.as_ref().map(|a|(&a.app,a.token))!=after.as_ref().map(|a|(&a.app,a.token)){continue;}
                    let source=source.unwrap_or("Unknown app").to_owned();
                    if commands.try_send(Command::Capture(capture,source)).is_ok(){last=Some(hash);}
                }
            }
        })?;
        Ok(tx)
    }
}
fn read_capture(clipboard: &mut Clipboard, c: &Config) -> Result<Option<Capture>> {
    if c.capture_files {
        if let Ok(files) = clipboard.get().file_list() {
            if !files.is_empty() {
                return Ok(Some(Capture::Files(files)));
            }
        }
    }
    if let Ok(image) = clipboard.get_image() {
        let expected = (image.width as u64)
            .checked_mul(image.height as u64)
            .and_then(|v| v.checked_mul(4));
        if expected.is_none_or(|n| n > c.max_item_mb * 1024 * 1024 || n != image.bytes.len() as u64)
        {
            return Ok(None);
        }
        let width = u32::try_from(image.width)?;
        let height = u32::try_from(image.height)?;
        return Ok(Some(Capture::Image {
            width,
            height,
            rgba: image.bytes.into_owned(),
        }));
    }
    if let Ok(text) = clipboard.get_text() {
        if !text.trim().is_empty() && text.len() as u64 <= c.max_item_mb * 1024 * 1024 {
            return Ok(Some(Capture::Text(text)));
        }
    }
    Ok(None)
}
fn fingerprint(capture: &Capture) -> Vec<u8> {
    let mut h = Sha256::new();
    match capture {
        Capture::Text(t) => {
            h.update(b"text");
            h.update(t.as_bytes());
        }
        Capture::Image {
            width,
            height,
            rgba,
        } => {
            h.update(b"image");
            h.update(width.to_le_bytes());
            h.update(height.to_le_bytes());
            h.update(rgba);
        }
        Capture::Files(files) => {
            h.update(b"files");
            for file in files {
                h.update(file.to_string_lossy().as_bytes());
                if let Ok(meta) = std::fs::metadata(file) {
                    h.update(meta.len().to_le_bytes());
                    if let Ok(t) = meta.modified().and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map_err(std::io::Error::other)
                    }) {
                        h.update(t.as_nanos().to_le_bytes());
                    }
                }
            }
        }
    };
    h.finalize().to_vec()
}
/// Runs only in disposable CI. Deliberately replaces clipboard contents.
pub fn self_test() -> Result<()> {
    let mut c = Clipboard::new()?;
    c.set_text("ClipVault clipboard CI 🦀")?;
    thread::sleep(Duration::from_millis(150));
    if c.get_text()? != "ClipVault clipboard CI 🦀" {
        bail!("Clipboard text did not round trip");
    }
    c.set_image(ImageData {
        width: 2,
        height: 2,
        bytes: Cow::Owned([255, 0, 0, 255].repeat(4)),
    })?;
    thread::sleep(Duration::from_millis(150));
    let image = c.get_image()?;
    if (image.width, image.height) != (2, 2) {
        bail!("Clipboard image did not round trip");
    }
    let dir = std::env::temp_dir().join(format!("clipvault-ci-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("clipboard-test.txt");
    std::fs::write(&file, "clipboard file round trip")?;
    c.set().file_list(&[&file])?;
    thread::sleep(Duration::from_millis(150));
    let paths = c.get().file_list()?;
    if paths.len() != 1 || paths[0].file_name() != file.file_name() {
        bail!("Clipboard file list did not round trip");
    }
    std::fs::remove_dir_all(dir)?;
    println!("Clipboard text, image, and file round trips passed.");
    Ok(())
}
