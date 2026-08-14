//! Persisted application config (config.json in the app config dir).

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Config dir resolved once at setup from app.path().app_config_dir().
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Persisted app state, loaded at startup and saved on every change.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppState {
    /// Last working directory (dsh session archive key).
    pub cwd: PathBuf,
    /// Preferred port, default 3080; 0 = always auto-pick.
    pub preferred_port: u16,
    /// Autostart toggle, default off.
    pub autostart: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            cwd: default_cwd(),
            preferred_port: 3080,
            autostart: false,
        }
    }
}

fn default_cwd() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn fallback_config_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.unwrap_or_else(std::env::temp_dir).join("whalenest")
}

/// Resolve the config dir from the tauri path resolver and remember it.
pub fn init_config_dir(app: &AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| fallback_config_dir());
    let _ = fs::create_dir_all(&dir);
    let _ = CONFIG_DIR.set(dir.clone());
    dir
}

/// Absolute path of config.json.
pub fn config_path() -> PathBuf {
    CONFIG_DIR
        .get()
        .cloned()
        .unwrap_or_else(fallback_config_dir)
        .join("config.json")
}

impl AppState {
    /// Load from disk; missing/corrupt file falls back to defaults.
    pub fn load() -> Self {
        fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }
}
