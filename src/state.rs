//! Persisted application config (`config.toml` under `~/.config/whale-nest`).
//!
//! Cross-platform location:
//!   - Unix:    $HOME/.config/whale-nest/config.toml
//!   - Windows: %USERPROFILE%\.config\whale-nest\config.toml
//!
//! The preferred port lives here (default 3080, user-overridable). On first
//! run after upgrading from the old single-`cwd` schema, a legacy top-level
//! `cwd` (and `recent_dirs`) is migrated into the per-profile map.
//!
//! 从 Tauri 版原样移植：数据模型升级为 profile 中心化（active_profile +
//! profile_cwds），删除旧全局 cwd / recent_dirs。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Config dir resolved once at startup.
static CONFIG_DIR: LazyLock<PathBuf> = LazyLock::new(whale_nest_config_dir);

/// Persisted app state, loaded at startup and saved on every change.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppState {
    /// Currently active profile (default `web`).
    #[serde(default = "default_profile")]
    pub active_profile: String,
    /// Per-profile working directory (dsh session archive key).
    #[serde(default)]
    pub profile_cwds: HashMap<String, PathBuf>,
    /// Preferred port, default 3080; user-overridable in config.toml.
    #[serde(default = "default_port")]
    pub preferred_port: u16,
    /// Autostart toggle, default off.
    #[serde(default)]
    pub autostart: bool,
    /// Lock the preferred port (fail instead of drifting when taken), default off.
    #[serde(default)]
    pub lock_port: bool,
    /// Whether user has completed initial onboarding wizard. Default false (triggers wizard on first run).
    #[serde(default)]
    pub initialized: bool,
}

/// Runtime-scanned profile summary (never persisted).
#[derive(Clone, Debug)]
pub struct ProfileInfo {
    pub name: String,
    /// Profile dir under `$DSH_HOME/profiles/`.
    pub path: PathBuf,
    /// Whether this profile is a web-type profile (hosts `dsh-web-app`).
    pub is_web_type: bool,
    /// Number of user plugins in `dsh.profile.bundles` (non-`@deepseek-ai/`).
    pub plugin_count: usize,
    /// Working directory bound to this profile (from `profile_cwds`, or default).
    pub cwd: PathBuf,
    /// Number of session subdirs under the cwd's session archive dir.
    pub session_count: usize,
    /// mtime of the session archive dir (last session activity), if any.
    pub last_session_time: Option<SystemTime>,
}

impl ProfileInfo {
    /// Whether this is a web-type profile (can be launched by the dashboard).
    pub fn is_web(&self) -> bool {
        self.is_web_type
    }
}

/// Default preferred port.
pub const DEFAULT_PORT: u16 = 3080;

/// Default (and reserved) profile name.
pub const DEFAULT_PROFILE: &str = "web";

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_profile() -> String {
    DEFAULT_PROFILE.to_string()
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_profile: default_profile(),
            profile_cwds: HashMap::new(),
            preferred_port: DEFAULT_PORT,
            autostart: false,
            lock_port: false,
            initialized: false,
        }
    }
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
pub fn init_config_dir() -> PathBuf {
    let dir = CONFIG_DIR.clone();
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Absolute path of config.toml.
pub fn config_path() -> PathBuf {
    CONFIG_DIR.join("config.toml")
}

/// dsh home: `$DSH_HOME` override, else `~/.dsh`.
pub fn dsh_home() -> PathBuf {
    if let Some(h) = std::env::var_os("DSH_HOME") {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.map(|h| h.join(".dsh")).unwrap_or_else(|| PathBuf::from(".dsh"))
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
            if let Ok(mut state) = toml::from_str::<AppState>(&text) {
                state.migrate_legacy_cwd(&text);
                return state;
            }
            // Corrupt TOML: fall back to defaults (do not destroy the file).
        } else if !path.exists() {
            // New install or upgrade: try migrating the legacy JSON config.
            let legacy = legacy_json_path();
            if let Ok(text) = fs::read_to_string(&legacy) {
                if let Ok(mut state) = serde_json::from_str::<AppState>(&text) {
                    state.migrate_legacy_cwd(&text);
                    let _ = state.save();
                    return state;
                }
            }
        }
        Self::default()
    }

    /// Migrate a legacy top-level `cwd` into `profile_cwds["web"]` when the new
    /// map has no entry for `web` yet (upgrade path from the single-cwd schema).
    fn migrate_legacy_cwd(&mut self, raw: &str) {
        if self.profile_cwds.contains_key(DEFAULT_PROFILE) {
            return;
        }
        if let Some(cwd) = extract_legacy_cwd(raw) {
            self.profile_cwds.insert(DEFAULT_PROFILE.to_string(), cwd);
        }
    }

    /// The working directory to boot for a profile: its remembered cwd, or the
    /// first non-empty default (home dir).
    pub fn profile_cwd(&self, profile: &str) -> PathBuf {
        self.profile_cwds
            .get(profile)
            .cloned()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(default_cwd)
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

/// Pull a legacy top-level `cwd` out of a TOML or JSON config body.
fn extract_legacy_cwd(raw: &str) -> Option<PathBuf> {
    if let Ok(v) = raw.parse::<toml::Value>() {
        if let Some(s) = v.get("cwd").and_then(|x| x.as_str()) {
            return Some(PathBuf::from(s));
        }
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(s) = v.get("cwd").and_then(|x| x.as_str()) {
            return Some(PathBuf::from(s));
        }
    }
    None
}

pub(crate) fn default_cwd() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Encode a working directory into dsh's session archive dir name:
/// `--<path>--` where path separators become `-` (leading/trailing trimmed) and
/// non-ASCII chars become `~XXXX` (UTF-16 code unit, uppercase hex).
///
/// Examples:
///   `/home/huang/proj`     → `--home-huang-proj--`
///   `/a/项目/中`           → `--a-~9879~76EE-~4E0A--`
//
// ponytail: mirrors dsh's on-disk encoding for the common ASCII case. The
// non-ASCII branch is best-effort (UTF-16 unit form); if dsh ever changes its
// scheme only paths with non-ASCII chars would drift — acceptable ceiling.
pub fn encode_session_dir_name(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let mut out = String::new();
    for ch in s.chars() {
        if ch == '/' || ch == '\\' {
            out.push('-');
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for cu in ch.encode_utf16(&mut buf) {
                out.push('~');
                out.push_str(&format!("{:04X}", *cu));
            }
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    format!("--{trimmed}--")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_3080() {
        let state = AppState::default();
        assert_eq!(state.preferred_port, 3080);
        assert_eq!(state.active_profile, "web");
    }

    #[test]
    fn roundtrip_toml() {
        let mut state = AppState::default();
        state.preferred_port = 4321;
        state.autostart = true;
        state.active_profile = "work".into();
        state
            .profile_cwds
            .insert("work".into(), PathBuf::from("/tmp/a"));
        state
            .profile_cwds
            .insert("web".into(), PathBuf::from("/tmp/b"));
        let text = toml::to_string_pretty(&state).expect("serialize");
        let back: AppState = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.preferred_port, 4321);
        assert!(back.autostart);
        assert_eq!(back.active_profile, "work");
        assert_eq!(back.profile_cwds.len(), 2);
        assert_eq!(back.profile_cwds.get("web").unwrap(), &PathBuf::from("/tmp/b"));
    }

    #[test]
    fn missing_fields_default() {
        let text = "active_profile = \"work\"\n";
        let state: AppState = toml::from_str(text).expect("deserialize");
        assert_eq!(state.preferred_port, DEFAULT_PORT);
        assert_eq!(state.active_profile, "work");
        assert!(!state.autostart);
        assert!(!state.lock_port);
        assert!(state.profile_cwds.is_empty());
    }

    #[test]
    fn migrates_legacy_cwd_to_web() {
        let text = "cwd = \"/tmp/old\"\npreferred_port = 3080\n";
        let mut state: AppState = toml::from_str(text).expect("deserialize");
        state.migrate_legacy_cwd(text);
        assert_eq!(
            state.profile_cwds.get("web").unwrap(),
            &PathBuf::from("/tmp/old")
        );
    }

    #[test]
    fn does_not_override_existing_web_cwd() {
        let text = "cwd = \"/tmp/old\"\n";
        let mut state: AppState = toml::from_str(text).expect("deserialize");
        state
            .profile_cwds
            .insert("web".into(), PathBuf::from("/tmp/new"));
        state.migrate_legacy_cwd(text);
        assert_eq!(
            state.profile_cwds.get("web").unwrap(),
            &PathBuf::from("/tmp/new")
        );
    }

    #[test]
    fn encodes_ascii_cwd_like_dsh() {
        // Must match a real session dir name observed on disk.
        assert_eq!(
            encode_session_dir_name(Path::new("/home/huang/Personal/Dev/Code/melon")),
            "--home-huang-Personal-Dev-Code-melon--"
        );
    }

    #[test]
    fn encodes_non_ascii_as_utf16_hex() {
        // 项=U+9879 目=U+76EE 中=U+4E2D
        assert_eq!(
            encode_session_dir_name(Path::new("/a/项目/中")),
            "--a-~9879~76EE-~4E2D--"
        );
    }

    #[test]
    fn profile_cwd_falls_back_to_home() {
        let state = AppState::default();
        let cwd = state.profile_cwd("missing");
        assert!(!cwd.as_os_str().is_empty());
    }
}
