use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenRouter,
    Fireworks,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageEmbedding {
    Native,
    Caption,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: Provider,
    pub embedding_model: String,
    pub vision_model: String,
    pub image_embedding: ImageEmbedding,
    pub cloud_enabled: bool,
    pub max_entries: usize,
    pub max_age_days: u32,
    pub max_storage_mb: u64,
    pub max_item_mb: u64,
    pub poll_ms: u64,
    pub excluded_apps: Vec<String>,
    pub capture_when_app_unknown: bool,
    pub paused: bool,
    pub hotkey: String,
    pub auto_paste: bool,
    pub capture_files: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            provider: Provider::OpenRouter,
            embedding_model: "google/gemini-embedding-2".into(),
            vision_model: "google/gemini-2.5-flash-lite".into(),
            image_embedding: ImageEmbedding::Native,
            cloud_enabled: false,
            max_entries: 10000,
            max_age_days: 90,
            max_storage_mb: 1024,
            max_item_mb: 20,
            poll_ms: 700,
            excluded_apps: vec![
                "1password".into(),
                "bitwarden".into(),
                "keepass".into(),
                "lastpass".into(),
                "securityagent".into(),
            ],
            capture_when_app_unknown: false,
            paused: false,
            hotkey: "Control+Shift+Space".into(),
            auto_paste: true,
            capture_files: true,
        }
    }
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let config: Self = toml::from_str(&fs::read_to_string(path)?)
            .context("Invalid settings.toml; existing settings were not overwritten")?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.max_entries == 0 || self.max_entries > 1_000_000 {
            bail!("History count must be 1–1,000,000");
        }
        if self.max_item_mb == 0 || self.max_item_mb > 100 {
            bail!("Item size must be 1–100 MiB");
        }
        if self.max_storage_mb > 102400 {
            bail!("Storage limit must be at most 102400 MiB");
        }
        if self.max_storage_mb < self.max_item_mb {
            bail!("Storage limit must fit at least one item");
        }
        if self.poll_ms < 200 || self.poll_ms > 60000 {
            bail!("Capture interval must be 200–60000 ms");
        }
        if self.provider == Provider::Fireworks && self.image_embedding == ImageEmbedding::Native {
            bail!(
                "Fireworks uses caption-mediated images; select Caption mode and compatible text/vision model IDs"
            );
        }
        if self.embedding_model.trim().is_empty() {
            bail!("An embedding model is required");
        }
        if self.hotkey.trim().is_empty() {
            bail!("A global shortcut is required");
        }
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, toml::to_string_pretty(self)?)?;
        private_file(&temp)?;
        #[cfg(not(windows))]
        fs::rename(temp, path)?;
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::{
                Win32::Storage::FileSystem::{
                    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
                },
                core::PCWSTR,
            };
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }?;
        }
        Ok(())
    }
    pub fn api_key_env(&self) -> &'static str {
        match self.provider {
            Provider::OpenRouter => "OPENROUTER_API_KEY",
            Provider::Fireworks => "FIREWORKS_API_KEY",
        }
    }
    pub fn model_key(&self) -> String {
        format!(
            "{:?}:{}:{:?}:{}",
            self.provider, self.embedding_model, self.image_embedding, self.vision_model
        )
    }
    pub fn excludes(&self, app: Option<&str>) -> bool {
        match app {
            None => !self.capture_when_app_unknown,
            Some(app) => {
                let app = app.to_lowercase();
                self.excluded_apps
                    .iter()
                    .any(|s| !s.trim().is_empty() && app.contains(&s.to_lowercase()))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub db: PathBuf,
    pub blobs: PathBuf,
    pub config: PathBuf,
}
impl Paths {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        private_dir(&root)?;
        let blobs = root.join("blobs");
        fs::create_dir_all(&blobs)?;
        private_dir(&blobs)?;
        Ok(Self {
            db: root.join("history.sqlite3"),
            config: root.join("settings.toml"),
            root,
            blobs,
        })
    }
}
pub fn private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}
pub fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exclusions_fail_closed() {
        let c = Config::default();
        assert!(c.excludes(None));
        assert!(c.excludes(Some("com.1password.1password")));
        assert!(!c.excludes(Some("org.mozilla.firefox")));
    }
    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::open(dir.path().join("vault")).unwrap();
        Config::default().save(&p.config).unwrap();
        assert_eq!(Config::load(&p.config).unwrap().max_entries, 10000);
    }
}
