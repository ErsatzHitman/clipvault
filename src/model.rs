use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Image,
    Files,
}
impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Files => "files",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "image" => Self::Image,
            "files" => Self::Files,
            _ => Self::Text,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Capture {
    Text(String),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Files(Vec<PathBuf>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub kind: Kind,
    pub title: String,
    pub text: String,
    pub blob: Option<String>,
    pub source_app: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub pinned: bool,
    pub bytes: u64,
    pub index_status: String,
    pub index_error: Option<String>,
    /// JSON `EntryMeta` for card rendering (file name/ext/size, thumbnail path); `None` on old rows.
    pub meta: Option<String>,
}
impl Entry {
    pub fn meta_info(&self) -> EntryMeta {
        self.meta
            .as_deref()
            .and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_default()
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMeta {
    pub name: String,
    pub ext: String,
    pub size: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntryMeta {
    #[serde(default)]
    pub files: Vec<FileMeta>,
    /// Blob-relative path of a small PNG thumbnail, if one exists.
    #[serde(default)]
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Keyword,
    Hybrid,
    Semantic,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub entry: Entry,
    pub score: f32,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
