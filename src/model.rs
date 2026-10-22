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
