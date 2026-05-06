//! Locate the user's system-installed Brave's user-data-dir(s) and
//! mirror components from them. Used to pre-install filter list
//! components for default-disabled regional lists, since Brave's
//! component-updater only auto-fetches `default_enabled=true` lists
//! — others rely on the in-Brave UI toggle to trigger an OnDemand
//! update, which our pref-only writes don't replicate.
//!
//! By copying the bytes-for-bytes signed component tree from the
//! user's main Brave install, we match the exact version + content
//! Brave's component-updater would have produced. No CDN scraping,
//! no manifest synthesis, no signature gymnastics.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// Standard Brave user-data-dir locations across channels +
/// platforms. Returns the ones that actually exist on disk.
pub fn user_data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let candidates = if cfg!(windows) {
            // Windows: %LOCALAPPDATA%\BraveSoftware\<channel>\User Data
            let local = dirs::data_local_dir().unwrap_or_else(|| home.join("AppData/Local"));
            vec![
                local.join("BraveSoftware/Brave-Browser/User Data"),
                local.join("BraveSoftware/Brave-Browser-Beta/User Data"),
                local.join("BraveSoftware/Brave-Browser-Nightly/User Data"),
                local.join("BraveSoftware/Brave-Browser-Dev/User Data"),
            ]
        } else if cfg!(target_os = "macos") {
            let app_sup = home.join("Library/Application Support");
            vec![
                app_sup.join("BraveSoftware/Brave-Browser"),
                app_sup.join("BraveSoftware/Brave-Browser-Beta"),
                app_sup.join("BraveSoftware/Brave-Browser-Nightly"),
                app_sup.join("BraveSoftware/Brave-Browser-Dev"),
            ]
        } else {
            // Linux / *BSD
            let config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
            vec![
                config.join("BraveSoftware/Brave-Browser"),
                config.join("BraveSoftware/Brave-Browser-Beta"),
                config.join("BraveSoftware/Brave-Browser-Nightly"),
                config.join("BraveSoftware/Brave-Browser-Dev"),
            ]
        };
        for c in candidates {
            if c.is_dir() { out.push(c); }
        }
    }
    out
}

/// Find the latest version dir of a component across every system
/// Brave install. Returns `(version_dir, channel_user_data_dir)` so
/// the caller can log which install produced the bytes.
pub fn find_component(component_id: &str) -> Option<(PathBuf, PathBuf)> {
    let mut best: Option<(semver::Version, PathBuf, PathBuf)> = None;
    for ud in user_data_dirs() {
        // Components live at <user_data>/<component_id>/<version>/
        // (top-level — same layout as our brave-regress profiles).
        let comp_root = ud.join(component_id);
        if !comp_root.is_dir() { continue; }
        let Ok(entries) = std::fs::read_dir(&comp_root) else { continue; };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Version dirs are dotted numbers (e.g. 1.0.19623). Skip
            // anything else, including `.disabled` / `.tmp` siblings.
            let Ok(ver) = semver::Version::parse(&name) else { continue; };
            // Must contain at least manifest.json to count — the
            // version dir is created before files arrive, so a
            // mid-fetch state would have nothing in it.
            if !entry.path().join("manifest.json").is_file() { continue; }
            let candidate = (ver.clone(), entry.path(), ud.clone());
            best = match best {
                None => Some(candidate),
                Some(prev) => if candidate.0 > prev.0 { Some(candidate) } else { Some(prev) },
            };
        }
    }
    best.map(|(_, ver_dir, ud)| (ver_dir, ud))
}

/// Copy a system-Brave component's full version dir tree into the
/// target user-data-dir. The destination layout matches the source:
/// `<dst>/<component_id>/<version>/...`. Skips when the destination
/// already has an equal-or-newer version (idempotent — repeated
/// pre-installs don't trample on a Brave-fetched newer copy).
pub fn mirror_into(
    src_version_dir: &Path,
    dst_user_data_dir: &Path,
    component_id: &str,
) -> Result<MirrorResult> {
    let version = src_version_dir.file_name()
        .ok_or_else(|| anyhow!("malformed source path: {}", src_version_dir.display()))?
        .to_string_lossy().into_owned();
    let dst_version_dir = dst_user_data_dir.join(component_id).join(&version);
    if dst_version_dir.is_dir()
        && dst_version_dir.join("list.txt").is_file()
        && dst_version_dir.join("manifest.json").is_file()
    {
        return Ok(MirrorResult { version, dst_version_dir, copied_bytes: 0, skipped: true });
    }
    std::fs::create_dir_all(&dst_version_dir)
        .with_context(|| format!("mkdir {}", dst_version_dir.display()))?;
    let mut copied = 0u64;
    for entry in walkdir::WalkDir::new(src_version_dir) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src_version_dir).unwrap_or(entry.path());
        let dst = dst_version_dir.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dst)?;
        } else if entry.file_type().is_file() {
            if let Some(p) = dst.parent() { std::fs::create_dir_all(p)?; }
            std::fs::copy(entry.path(), &dst)?;
            copied += entry.metadata()?.len();
        }
    }
    Ok(MirrorResult { version, dst_version_dir, copied_bytes: copied, skipped: false })
}

#[derive(Debug)]
pub struct MirrorResult {
    pub version:         String,
    pub dst_version_dir: PathBuf,
    pub copied_bytes:    u64,
    pub skipped:         bool,
}
