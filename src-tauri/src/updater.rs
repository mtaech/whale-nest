//! dsh version update detection.
//!
//! Pure-std module: reads the locally installed dsh version and asks the npm
//! registry for the latest one, then compares them. Deliberately lightweight —
//! no semver crate, no HTTP client; we shell out to `dsh --version` and
//! `npm view` because those already exist on the user's machine and inherit
//! their npm registry configuration (mirrors included).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Result of a version check.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// Locally installed dsh version (e.g. "0.1.0-rc.6").
    pub current: String,
    /// Latest version published to the registry.
    pub latest: String,
    /// Whether the registry has a newer version than the local one.
    pub has_update: bool,
}

/// How long we wait for the registry query before giving up (network can be slow).
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Run `dsh --version` and return the trimmed version string.
fn local_dsh_version() -> Option<String> {
    let mut child = Command::new("dsh")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    let mut out = child.stdout.take()?;
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut out, &mut buf);
    let v = buf.trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Query the npm registry for the latest version of @deepseek-ai/dsh.
/// Uses `npm view <pkg> version` so the user's configured registry (and any
/// mirror) is honored automatically.
fn remote_latest_version() -> Option<String> {
    let mut child = Command::new("npm")
        .args(["view", "@deepseek-ai/dsh", "version"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + CHECK_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    let mut out = child.stdout.take()?;
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut out, &mut buf);
    // npm may emit color codes / trailing newline / warnings; keep the first
    // line that looks like a semver.
    buf.lines()
        .map(str::trim)
        .find(|line| looks_like_version(line))
        .map(str::to_string)
}

/// Best-effort check whether a string looks like a semantic version.
fn looks_like_version(s: &str) -> bool {
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let parts: Vec<&str> = core.split('.').collect();
    // x.y.z with all-numeric parts (npm prints exactly this shape).
    parts.len() >= 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Compare two version strings. Returns `true` when `a` is newer than `b`.
/// Handles the `x.y.z` core and a `-rc.N` / `-preview.N` style prerelease:
/// a prerelease of the same core is older than the release itself.
fn version_newer(a: &str, b: &str) -> bool {
    fn core_and_pre(s: &str) -> (Vec<u64>, Option<String>) {
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (s, None),
        };
        let nums = core
            .split('.')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect::<Vec<_>>();
        (nums, pre)
    }
    let (a_core, a_pre) = core_and_pre(a);
    let (b_core, b_pre) = core_and_pre(b);

    // Compare core numeric parts, padding with zeros.
    let len = a_core.len().max(b_core.len());
    for i in 0..len {
        let av = a_core.get(i).copied().unwrap_or(0);
        let bv = b_core.get(i).copied().unwrap_or(0);
        if av != bv {
            return av > bv;
        }
    }
    // Same core: a release (no pre) beats a prerelease; compare prerelease
    // numerically when both are prereleases (only the numeric tail matters).
    match (a_pre.as_deref(), b_pre.as_deref()) {
        (None, Some(_)) => true,  // 1.0.0 > 1.0.0-rc.1
        (Some(_), None) => false, // 1.0.0-rc.1 < 1.0.0
        (Some(ap), Some(bp)) => {
            let an = ap.rsplit('.').next().and_then(|p| p.parse::<u64>().ok());
            let bn = bp.rsplit('.').next().and_then(|p| p.parse::<u64>().ok());
            match (an, bn) {
                (Some(a), Some(b)) => a > b,
                _ => ap > bp, // fall back to string compare
            }
        }
        (None, None) => false, // identical
    }
}

/// Run a full update check: local version vs registry latest.
/// Returns `None` when either side could not be determined (offline, dsh or
/// npm missing) — the caller should stay silent rather than nag.
pub fn check_for_update() -> Option<UpdateInfo> {
    let current = local_dsh_version()?;
    let latest = remote_latest_version()?;
    let has_update = version_newer(&latest, &current);
    Some(UpdateInfo {
        current,
        latest,
        has_update,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_version_accepts_semver() {
        assert!(looks_like_version("0.1.0"));
        assert!(looks_like_version("0.1.0-rc.6"));
        assert!(looks_like_version("1.2.3-beta.2"));
        assert!(looks_like_version("2.0.0"));
    }

    #[test]
    fn looks_like_version_rejects_junk() {
        assert!(!looks_like_version(""));
        assert!(!looks_like_version("abc"));
        assert!(!looks_like_version("v1.2.3"));
        assert!(!looks_like_version("0.1"));
        assert!(!looks_like_version("npm warn deprecated foo"));
    }

    #[test]
    fn release_beats_prerelease_of_same_core() {
        assert!(version_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!version_newer("1.0.0-rc.1", "1.0.0"));
    }

    #[test]
    fn core_minor_and_patch_compare() {
        assert!(version_newer("0.2.0", "0.1.9"));
        assert!(version_newer("0.1.10", "0.1.9"));
        assert!(!version_newer("0.1.9", "0.1.10"));
        assert!(!version_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn prerelease_rc_number_compare() {
        assert!(version_newer("0.1.0-rc.7", "0.1.0-rc.6"));
        assert!(!version_newer("0.1.0-rc.6", "0.1.0-rc.7"));
    }
}
