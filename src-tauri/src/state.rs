//! Persisted application config (`config.toml` under `~/.config/whale-nest`).
//!
//! Cross-platform location:
//!   - Unix:    $HOME/.config/whale-nest/config.toml
//!   - Windows: %USERPROFILE%\.config\whale-nest\config.toml
//!
//! The preferred port lives here (default 3080, user-overridable). On first
//! run after upgrading from the old JSON location, an existing legacy
//! `config.json` is migrated so settings (cwd / autostart / port) are kept.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// Config dir resolved once at setup.
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Persisted app state, loaded at startup and saved on every change.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppState {
    /// Last working directory (dsh session archive key).
    pub cwd: PathBuf,
    /// Preferred port, default 3080; user-overridable in config.toml.
    #[serde(default = "default_port")]
    pub preferred_port: u16,
    /// Autostart toggle, default off.
    #[serde(default)]
    pub autostart: bool,
    /// Lock the preferred port (fail instead of drifting when taken), default off.
    #[serde(default)]
    pub lock_port: bool,
    /// Recently used working directories, most recent first (tray quick-switch).
    #[serde(default)]
    pub recent_dirs: Vec<PathBuf>,
    /// Whether user has completed initial onboarding wizard. Default false (triggers wizard on first run).
    #[serde(default)]
    pub initialized: bool,
}

/// Default preferred port.
pub const DEFAULT_PORT: u16 = 3080;

fn default_port() -> u16 {
    DEFAULT_PORT
}

/// Max entries kept in `recent_dirs`.
pub const MAX_RECENT_DIRS: usize = 6;

impl AppState {
    /// Record a working directory as recently used: dedupe, move to front,
    /// trim to `MAX_RECENT_DIRS`. Returns true when the list changed.
    pub fn push_recent_dir(&mut self, dir: PathBuf) -> bool {
        let mut changed = false;
        self.recent_dirs.retain(|d| {
            let keep = d != &dir;
            if !keep {
                changed = true;
            }
            keep
        });
        self.recent_dirs.insert(0, dir);
        changed = true;
        if self.recent_dirs.len() > MAX_RECENT_DIRS {
            self.recent_dirs.truncate(MAX_RECENT_DIRS);
        }
        changed
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            cwd: default_cwd(),
            preferred_port: DEFAULT_PORT,
            autostart: false,
            lock_port: false,
            recent_dirs: Vec::new(),
            initialized: false,
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

/// `~/.config/whale-nest` — the app's config directory (independent of the
/// Tauri platform config dir, per project requirement).
fn whale_nest_config_dir() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = home
        .map(|h| h.join(".config"))
        .unwrap_or_else(std::env::temp_dir);
    base.join("whale-nest")
}

/// Resolve and create the config dir, remembering it for later use.
pub fn init_config_dir(_app: &AppHandle) -> PathBuf {
    let dir = whale_nest_config_dir();
    let _ = fs::create_dir_all(&dir);
    let _ = CONFIG_DIR.set(dir.clone());
    dir
}

/// Absolute path of config.toml.
pub fn config_path() -> PathBuf {
    CONFIG_DIR
        .get()
        .cloned()
        .unwrap_or_else(whale_nest_config_dir)
        .join("config.toml")
}

/// Legacy JSON path (old versions), for one-time migration.
fn legacy_json_path() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("dev.whalenest.desktop").join("config.json")
}

impl AppState {
    /// Load from disk; missing/corrupt file falls back to defaults.
    /// Migrates a legacy JSON config on first run after an upgrade.
    pub fn load() -> Self {
        let path = config_path();
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(state) = toml::from_str::<AppState>(&text) {
                return state;
            }
            // Corrupt TOML: fall back to defaults (do not destroy the file).
        } else if !path.exists() {
            // New install or upgrade: try migrating the legacy JSON config.
            let legacy = legacy_json_path();
            if let Ok(text) = fs::read_to_string(&legacy) {
                if let Ok(state) = serde_json::from_str::<AppState>(&text) {
                    let _ = state.save();
                    return state;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let toml_text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, toml_text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_3080() {
        let state = AppState::default();
        assert_eq!(state.preferred_port, 3080);
    }

    #[test]
    fn roundtrip_toml() {
        let mut state = AppState::default();
        state.preferred_port = 4321;
        state.autostart = true;
        state.recent_dirs = vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")];
        let text = toml::to_string_pretty(&state).expect("serialize");
        let back: AppState = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.preferred_port, 4321);
        assert!(back.autostart);
        assert_eq!(back.recent_dirs.len(), 2);
    }

    #[test]
    fn missing_fields_default() {
        let text = "cwd = \"/tmp\"\n";
        let state: AppState = toml::from_str(text).expect("deserialize");
        assert_eq!(state.preferred_port, DEFAULT_PORT);
        assert!(!state.autostart);
        assert!(!state.lock_port);
        assert!(state.recent_dirs.is_empty());
    }
}
