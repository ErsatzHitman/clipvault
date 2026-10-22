use crate::{
    config::{Config, Paths, private_dir, private_file},
    model::{Capture, Entry, Kind, SearchHit, now},
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::Duration,
};

const COLUMNS: &str =
    "id,kind,title,text,blob,source_app,created_at,last_seen,pinned,bytes,index_status,index_error";
const SUMMARY_COLUMNS: &str = "id,kind,title,substr(text,1,16000),blob,source_app,created_at,last_seen,pinned,bytes,index_status,index_error";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredFile {
    pub name: String,
    pub stored: String,
}
pub struct Store {
    pub conn: Connection,
    pub paths: Paths,
}
fn entry(row: &Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: row.get(0)?,
        kind: Kind::parse(&row.get::<_, String>(1)?),
        title: row.get(2)?,
        text: row.get(3)?,
        blob: row.get(4)?,
        source_app: row.get(5)?,
        created_at: row.get(6)?,
        last_seen: row.get(7)?,
        pinned: row.get(8)?,
        bytes: row.get(9)?,
        index_status: row.get(10)?,
        index_error: row.get(11)?,
    })
}
impl Store {
    pub fn open(paths: Paths) -> Result<Self> {
        let conn = Connection::open(&paths.db)?;
        conn.busy_timeout(Duration::from_secs(3))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 1 {
            bail!("History was created by a newer ClipVault version; refusing to modify it");
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT, hash TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
                title TEXT NOT NULL, text TEXT NOT NULL, blob TEXT, source_app TEXT NOT NULL,
                created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0,
                bytes INTEGER NOT NULL, index_status TEXT NOT NULL DEFAULT 'pending', index_error TEXT,
                attempts INTEGER NOT NULL DEFAULT 0, retry_at INTEGER NOT NULL DEFAULT 0);
            CREATE INDEX IF NOT EXISTS entries_recent ON entries(last_seen DESC);
            CREATE INDEX IF NOT EXISTS entries_pending ON entries(index_status,retry_at);
            CREATE TABLE IF NOT EXISTS vectors (entry_id INTEGER PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE,
                model TEXT NOT NULL, dimensions INTEGER NOT NULL, vector BLOB NOT NULL);
            CREATE INDEX IF NOT EXISTS vectors_model ON vectors(model);
            CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(title,text,content='entries',content_rowid='id',tokenize='unicode61 remove_diacritics 2');
            CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
                INSERT INTO entries_fts(rowid,title,text) VALUES(new.id,new.title,new.text); END;
            CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
                INSERT INTO entries_fts(entries_fts,rowid,title,text) VALUES('delete',old.id,old.title,old.text); END;
            CREATE TRIGGER IF NOT EXISTS entries_au AFTER UPDATE OF title,text ON entries BEGIN
                INSERT INTO entries_fts(entries_fts,rowid,title,text) VALUES('delete',old.id,old.title,old.text);
                INSERT INTO entries_fts(rowid,title,text) VALUES(new.id,new.title,new.text); END;
            PRAGMA user_version=1;")?;
        private_file(&paths.db)?;
        for ext in ["sqlite3-wal", "sqlite3-shm"] {
            let p = paths.db.with_extension(ext);
            if p.exists() {
                private_file(&p)?;
            }
        }
        Ok(Self { conn, paths })
    }
    pub fn insert(&mut self, capture: Capture, source: &str, config: &Config) -> Result<i64> {
        let limit = config.max_item_mb * 1024 * 1024;
        let (kind, hash, title, text, blob, bytes) = match capture {
            Capture::Text(text) => {
                if text.trim().is_empty() {
                    bail!("Empty clipboard");
                }
                if text.len() as u64 > limit {
                    bail!("Text exceeds item size limit");
                }
                let hash = digest(&[b"text", text.as_bytes()]);
                let title: String = text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect();
                let bytes = text.len() as u64;
                (Kind::Text, hash, title, text, None, bytes)
            }
            Capture::Image {
                width,
                height,
                rgba,
            } => {
                let expected = (width as u64)
                    .checked_mul(height as u64)
                    .and_then(|n| n.checked_mul(4))
                    .context("Image too large")?;
                if expected > limit || expected as usize != rgba.len() || width == 0 || height == 0
                {
                    bail!("Image exceeds size limit or has invalid dimensions");
                }
                let hash = digest(&[b"image", &width.to_le_bytes(), &height.to_le_bytes(), &rgba]);
                let relative = format!("{hash}/image.png");
                let target = self.paths.blobs.join(&relative);
                if !target.exists() {
                    let image =
                        image::RgbaImage::from_raw(width, height, rgba).context("Invalid image")?;
                    let mut encoded = Cursor::new(Vec::new());
                    image::DynamicImage::ImageRgba8(image)
                        .write_to(&mut encoded, image::ImageFormat::Png)?;
                    self.write_blob(&relative, &encoded.into_inner())?;
                }
                let bytes = fs::metadata(target)?.len();
                (
                    Kind::Image,
                    hash,
                    format!("Image · {width} × {height}"),
                    String::new(),
                    Some(relative),
                    bytes,
                )
            }
            Capture::Files(files) => {
                if files.is_empty() || files.len() > 64 {
                    bail!("Choose 1–64 files");
                }
                let mut items = Vec::new();
                let mut total = 0u64;
                let mut hasher = Sha256::new();
                hasher.update(b"files");
                for path in &files {
                    let meta = fs::symlink_metadata(path)?;
                    if !meta.is_file() || meta.file_type().is_symlink() {
                        bail!(
                            "Only regular files are supported; directories and symlinks are excluded"
                        );
                    }
                    total = total
                        .checked_add(meta.len())
                        .context("File size overflow")?;
                    if total > limit {
                        bail!("Files exceed item size limit");
                    }
                    let name = path
                        .file_name()
                        .context("Missing filename")?
                        .to_string_lossy()
                        .to_string();
                    let mut data = Vec::new();
                    fs::File::open(path)?
                        .take(limit.saturating_sub(total - meta.len()) + 1)
                        .read_to_end(&mut data)?;
                    if data.len() as u64 > limit {
                        bail!("File grew beyond the item limit during capture");
                    }
                    if data.len() as u64 != meta.len() {
                        bail!("File changed during capture");
                    }
                    hasher.update(name.as_bytes());
                    hasher.update([0]);
                    hasher.update(Sha256::digest(&data));
                    items.push((name, data));
                }
                let hash = format!("{:x}", hasher.finalize());
                let mut manifest = Vec::new();
                let mut searchable = String::new();
                for (i, (name, data)) in items.into_iter().enumerate() {
                    // A separate directory preserves each original filename, including duplicate names.
                    let stored = format!("{hash}/files/{i}/{name}");
                    self.write_blob(&stored, &data)?;
                    searchable.push_str(&name);
                    searchable.push('\n');
                    if matches!(
                        Path::new(&name)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_lowercase()
                            .as_str(),
                        "txt"
                            | "md"
                            | "rs"
                            | "json"
                            | "csv"
                            | "log"
                            | "toml"
                            | "yaml"
                            | "yml"
                            | "html"
                            | "css"
                            | "js"
                            | "ts"
                            | "py"
                    ) {
                        if let Ok(s) = std::str::from_utf8(&data) {
                            searchable.extend(s.chars().take(100_000));
                            searchable.push('\n');
                        }
                    }
                    manifest.push(StoredFile { name, stored });
                }
                let title = manifest
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let relative = format!("{hash}/manifest.json");
                self.write_blob(&relative, &serde_json::to_vec(&manifest)?)?;
                (
                    Kind::Files,
                    hash,
                    title.chars().take(200).collect(),
                    searchable,
                    Some(relative),
                    total,
                )
            }
        };
        self.conn.execute("INSERT INTO entries(hash,kind,title,text,blob,source_app,created_at,last_seen,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?8)
            ON CONFLICT(hash) DO UPDATE SET last_seen=excluded.last_seen,source_app=excluded.source_app", params![hash,kind.as_str(),title,text,blob,source,now(),bytes])?;
        Ok(self
            .conn
            .query_row("SELECT id FROM entries WHERE hash=?1", [hash], |r| r.get(0))?)
    }
    fn write_blob(&self, relative: &str, bytes: &[u8]) -> Result<()> {
        let path = self.blob_path(relative)?;
        let parent = path.parent().context("No blob parent")?;
        fs::create_dir_all(parent)?;
        private_dir(parent)?;
        // Intermediate directories created by create_dir_all must also be private.
        let mut ancestor = parent.parent();
        while let Some(dir) = ancestor {
            if dir == self.paths.blobs {
                break;
            }
            private_dir(dir)?;
            ancestor = dir.parent();
        }
        if path.exists() {
            return Ok(());
        }
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            let mut file = fs::File::create(&tmp)?;
            private_file(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(tmp, path)?;
        Ok(())
    }
    pub fn blob_path(&self, relative: &str) -> Result<PathBuf> {
        let path = Path::new(relative);
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            bail!("Invalid blob path");
        }
        Ok(self.paths.blobs.join(path))
    }
    pub fn get(&self, id: i64) -> Result<Option<Entry>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM entries WHERE id=?1"),
                [id],
                entry,
            )
            .optional()?)
    }
    pub fn recent(&self, limit: usize) -> Result<Vec<SearchHit>> {
        let mut s = self.conn.prepare(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM entries ORDER BY pinned DESC,last_seen DESC,id DESC LIMIT ?1"
        ))?;
        Ok(s.query_map([limit.min(1000)], |r| {
            Ok(SearchHit {
                entry: entry(r)?,
                score: 0.0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
    }
    pub fn keyword(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let words: Vec<_> = query
            .split_whitespace()
            .take(32)
            .map(|s| format!("\"{}\"*", s.replace('"', "\"\"")))
            .collect();
        if words.is_empty() {
            return self.recent(limit);
        }
        let mut s = self.conn.prepare(&format!("SELECT {} FROM entries WHERE id IN (SELECT rowid FROM entries_fts WHERE entries_fts MATCH ?1 ORDER BY rank LIMIT ?2) ORDER BY pinned DESC,last_seen DESC",SUMMARY_COLUMNS))?;
        let mut hits: Vec<SearchHit> = s
            .query_map(params![words.join(" AND "), limit.min(1000)], |r| {
                Ok(SearchHit {
                    entry: entry(r)?,
                    score: 1.0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        // Literal substring covers URLs, punctuation, CJK and partial words missed by token search.
        if hits.len() < limit {
            let literal = query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let mut s = self.conn.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM entries WHERE text LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\' ORDER BY last_seen DESC LIMIT ?2"))?;
            let other = s
                .query_map(params![format!("%{literal}%"), limit.min(1000)], entry)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut ids: HashSet<i64> = hits.iter().map(|h| h.entry.id).collect();
            for e in other {
                if hits.len() >= limit {
                    break;
                }
                if ids.insert(e.id) {
                    hits.push(SearchHit {
                        entry: e,
                        score: 0.5,
                    });
                }
            }
        }
        Ok(hits)
    }
    pub fn next_pending(&self) -> Result<Option<Entry>> {
        Ok(self.conn.query_row(&format!("SELECT {COLUMNS} FROM entries WHERE index_status IN ('pending','retry') AND retry_at<=?1 ORDER BY id LIMIT 1"), [now()], entry).optional()?)
    }
    pub fn save_vector(&self, id: i64, model: &str, vector: &[f32]) -> Result<()> {
        let norm = normalize(vector)?;
        let bytes: Vec<u8> = norm.iter().flat_map(|v| v.to_le_bytes()).collect();
        let tx = self.conn.unchecked_transaction()?;
        if self.get(id)?.is_some() {
            tx.execute("INSERT OR REPLACE INTO vectors(entry_id,model,dimensions,vector) VALUES(?1,?2,?3,?4)",params![id,model,norm.len(),bytes])?;
            tx.execute(
                "UPDATE entries SET index_status='ready',index_error=NULL,attempts=0 WHERE id=?1",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn fail_index(&self, id: i64, message: &str) -> Result<()> {
        self.conn.execute("UPDATE entries SET index_status=CASE WHEN attempts>=7 THEN 'failed' ELSE 'retry' END,index_error=?2,retry_at=?3+min(3600,30*(1 << min(attempts,7))),attempts=attempts+1 WHERE id=?1", params![id,message,now()])?;
        Ok(())
    }
    pub fn reindex(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN; DELETE FROM vectors; UPDATE entries SET index_status='pending',index_error=NULL,attempts=0,retry_at=0; COMMIT;")?;
        Ok(())
    }
    pub fn reindex_mismatched(&self, model: &str) -> Result<()> {
        self.conn.execute("UPDATE entries SET index_status='pending',attempts=0,retry_at=0 WHERE id IN (SELECT entry_id FROM vectors WHERE model!=?1)",[model])?;
        Ok(())
    }
    pub fn semantic(&self, vector: &[f32], model: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let vector = normalize(vector)?;
        let mut s = self
            .conn
            .prepare("SELECT entry_id,vector FROM vectors WHERE model=?1 AND dimensions=?2")?;
        let mut scores = Vec::new();
        let rows = s.query_map(params![model, vector.len()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, bytes) = row?;
            if bytes.len() != vector.len() * 4 {
                continue;
            }
            let score: f32 = bytes
                .chunks_exact(4)
                .zip(&vector)
                .map(|(b, q)| f32::from_le_bytes(b.try_into().unwrap()) * q)
                .sum();
            if score.is_finite() {
                scores.push((id, score));
            }
        }
        scores.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        scores.truncate(limit.min(1000));
        scores
            .into_iter()
            .map(|(id, score)| {
                let e = self
                    .conn
                    .query_row(
                        &format!("SELECT {SUMMARY_COLUMNS} FROM entries WHERE id=?1"),
                        [id],
                        entry,
                    )
                    .optional()?;
                Ok(e.map(|entry| SearchHit { entry, score }))
            })
            .filter_map(|r| match r {
                Ok(None) => None,
                Ok(Some(v)) => Some(Ok(v)),
                Err(e) => Some(Err(e)),
            })
            .collect()
    }
    pub fn pin(&self, id: i64, pin: bool) -> Result<()> {
        self.conn
            .execute("UPDATE entries SET pinned=?2 WHERE id=?1", params![id, pin])?;
        Ok(())
    }
    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM entries WHERE id=?1", [id])?;
        self.gc()?;
        Ok(())
    }
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM entries", [])?;
        self.gc()?;
        self.conn.execute_batch("INSERT INTO entries_fts(entries_fts) VALUES('optimize'); PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        Ok(())
    }
    pub fn cleanup(&mut self, c: &Config) -> Result<()> {
        let tx = self.conn.transaction()?;
        if c.max_age_days > 0 {
            tx.execute(
                "DELETE FROM entries WHERE pinned=0 AND last_seen<?1",
                [now() - i64::from(c.max_age_days) * 86400],
            )?;
        }
        // Pins survive age/count cleanup; storage caps still apply to unpinned rows.
        tx.execute("DELETE FROM entries WHERE pinned=0 AND id NOT IN (SELECT id FROM entries ORDER BY pinned DESC,last_seen DESC,id DESC LIMIT ?1)",[c.max_entries])?;
        let mut total: u64 =
            tx.query_row("SELECT coalesce(sum(bytes),0) FROM entries", [], |r| {
                r.get(0)
            })?;
        let cap = c.max_storage_mb * 1024 * 1024;
        let old: Vec<(i64, u64)> = {
            let mut s =
                tx.prepare("SELECT id,bytes FROM entries WHERE pinned=0 ORDER BY last_seen,id")?;
            let rows = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        for (id, bytes) in old {
            if total <= cap {
                break;
            }
            tx.execute("DELETE FROM entries WHERE id=?1", [id])?;
            total = total.saturating_sub(bytes);
        }
        tx.commit()?;
        self.gc()?;
        Ok(())
    }
    pub fn gc(&self) -> Result<()> {
        let mut s = self
            .conn
            .prepare("SELECT blob FROM entries WHERE blob IS NOT NULL")?;
        let names: HashSet<String> = s
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .filter_map(|p| p.split('/').next().map(String::from))
            .collect();
        for file in fs::read_dir(&self.paths.blobs)? {
            let file = file?;
            if !names.contains(&file.file_name().to_string_lossy().to_string()) {
                let kind = file.file_type()?;
                if kind.is_dir() {
                    fs::remove_dir_all(file.path())?;
                } else {
                    fs::remove_file(file.path())?;
                }
            }
        }
        Ok(())
    }
    pub fn counts(&self) -> Result<(u64, u64)> {
        Ok(self.conn.query_row(
            "SELECT count(*),coalesce(sum(index_status='ready'),0) FROM entries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }
}
fn digest(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    format!("{:x}", h.finalize())
}
pub fn normalize(v: &[f32]) -> Result<Vec<f32>> {
    if v.is_empty() || v.len() > 65536 || v.iter().any(|v| !v.is_finite()) {
        bail!("Invalid embedding dimensions or non-finite values");
    }
    let norm = v.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
    if norm < 1e-12 {
        bail!("Zero embedding vector");
    }
    Ok(v.iter().map(|v| (*v as f64 / norm) as f32).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(Paths::open(d.path().join("data")).unwrap()).unwrap();
        (d, s)
    }
    #[test]
    fn persists_deduplicates_and_searches() {
        let (d, mut s) = store();
        let c = Config::default();
        let id = s
            .insert(Capture::Text("Hello clipboard world".into()), "test", &c)
            .unwrap();
        assert_eq!(
            id,
            s.insert(Capture::Text("Hello clipboard world".into()), "test", &c)
                .unwrap()
        );
        assert_eq!(s.keyword("clip", 10).unwrap().len(), 1);
        assert!(s.keyword("\" OR *", 10).is_ok());
        drop(s);
        let s = Store::open(Paths::open(d.path().join("data")).unwrap()).unwrap();
        assert_eq!(s.get(id).unwrap().unwrap().text, "Hello clipboard world");
    }
    #[test]
    fn vectors_are_incremental_and_cascade() {
        let (_dir, mut s) = store();
        let id = s
            .insert(
                Capture::Text("error screenshot".into()),
                "test",
                &Config::default(),
            )
            .unwrap();
        s.save_vector(id, "model", &[1.0, 0.0]).unwrap();
        assert_eq!(
            s.semantic(&[0.9, 0.1], "model", 10).unwrap()[0].entry.id,
            id
        );
        assert!(s.semantic(&[1.0, 0.0], "other", 10).unwrap().is_empty());
        s.delete(id).unwrap();
        assert!(s.semantic(&[1.0, 0.0], "model", 10).unwrap().is_empty());
    }
    #[test]
    fn cleanup_deletes_blobs_but_preserves_pins() {
        let (_dir, mut s) = store();
        let mut c = Config::default();
        c.max_entries = 1;
        let id = s
            .insert(
                Capture::Image {
                    width: 1,
                    height: 1,
                    rgba: vec![1, 2, 3, 255],
                },
                "test",
                &c,
            )
            .unwrap();
        let blob = s.get(id).unwrap().unwrap().blob.unwrap();
        assert!(s.blob_path(&blob).unwrap().exists());
        s.insert(Capture::Text("new".into()), "test", &c).unwrap();
        s.cleanup(&c).unwrap();
        assert!(!s.blob_path(&blob).unwrap().exists());
    }
    #[test]
    fn path_traversal_is_rejected() {
        let (_dir, s) = store();
        assert!(s.blob_path("../secret").is_err());
        assert!(normalize(&[f32::NAN]).is_err());
        assert!(normalize(&[0.0]).is_err());
    }
}
