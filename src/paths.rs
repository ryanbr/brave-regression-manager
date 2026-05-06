use std::path::PathBuf;

/// Root directory for everything brave-regress owns:
///   versions/        — extracted Brave Nightly installs (one folder per tag)
///   profiles/        — isolated --user-data-dir for each named profile
///   cache/downloads/ — installer artifacts pulled from GitHub
///   db/              — verdicts.sqlite, updates.sqlite
///   config.toml
pub fn data_root() -> PathBuf {
    if let Ok(p) = std::env::var("BRAVE_REGRESS_HOME") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    { dirs::data_local_dir().unwrap().join("brave-regress") }
    #[cfg(target_os = "macos")]
    { dirs::data_dir().unwrap().join("brave-regress") }
    #[cfg(all(unix, not(target_os = "macos")))]
    { dirs::data_local_dir().unwrap_or_else(|| dirs::home_dir().unwrap().join(".local/share")).join("brave-regress") }
}

/// Optional override for `versions_dir()` set from the Settings UI at
/// startup. When `Some(path)`, every Brave install / uninstall / launch
/// path resolution uses this directory instead of the default
/// `<data_root>/versions`. Useful for putting installs on a different
/// drive (e.g. C: → D:) without moving anything else. `OnceLock` so the
/// override is set once after config load and read lock-free thereafter.
static VERSIONS_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_versions_dir_override(p: PathBuf) {
    let _ = VERSIONS_DIR_OVERRIDE.set(p);
}

pub fn versions_dir() -> PathBuf {
    if let Some(p) = VERSIONS_DIR_OVERRIDE.get() {
        return p.clone();
    }
    data_root().join("versions")
}
pub fn profiles_dir() -> PathBuf  { data_root().join("profiles") }
pub fn downloads_dir() -> PathBuf { data_root().join("cache/downloads") }
/// Pre-extracted Brave version trees parked here on Uninstall so a
/// subsequent re-install can atomic-rename them straight back into
/// `versions/` instead of re-running the slow zip extract phase.
pub fn extracted_cache_dir() -> PathBuf { data_root().join("cache/extracted") }
pub fn extracted_cache_for(tag: &str) -> PathBuf { extracted_cache_dir().join(tag) }
pub fn db_dir() -> PathBuf        { data_root().join("db") }
pub fn config_path() -> PathBuf   { data_root().join("config.toml") }
pub fn releases_cache_path() -> PathBuf { data_root().join("cache/releases.json") }

pub fn version_dir(tag: &str) -> PathBuf  { versions_dir().join(tag) }
pub fn profile_dir(name: &str) -> PathBuf { profiles_dir().join(name) }

pub fn brave_binary(tag: &str) -> PathBuf {
    let root = version_dir(tag);
    #[cfg(windows)]
    {
        // Windows portable .zip puts brave.exe at the install root for all
        // channels — channel of the install is encoded only in the GitHub
        // release tag, not the binary name.
        root.join("brave.exe")
    }
    #[cfg(target_os = "macos")]
    {
        // .app bundle name varies by channel: `Brave Browser Nightly.app`,
        // `Brave Browser Beta.app`, `Brave Browser.app`. Pick whichever
        // exists; fall back to Nightly for the error path so the message
        // points at a sensible location.
        for app in ["Brave Browser Nightly.app", "Brave Browser Beta.app", "Brave Browser.app"] {
            let bin_name = app.trim_end_matches(".app");
            let p = root.join(app).join("Contents/MacOS").join(bin_name);
            if p.exists() { return p; }
        }
        root.join("Brave Browser Nightly.app/Contents/MacOS/Brave Browser Nightly")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // .deb layout puts the binary at
        //   `opt/brave.com/brave-<channel>/brave-browser-<channel>`
        // (stable: `opt/brave.com/brave/brave-browser`).
        // Portable Linux .zip flattens to a root layout with the binary
        // file directly under the install dir.
        let candidates: [&str; 7] = [
            // .deb-extracted layouts
            "opt/brave.com/brave-nightly/brave-browser-nightly",
            "opt/brave.com/brave-beta/brave-browser-beta",
            "opt/brave.com/brave/brave-browser",
            // portable .zip layouts (after flatten)
            "brave-browser-nightly",
            "brave-browser-beta",
            "brave-browser",
            "brave",
        ];
        for c in candidates {
            let p = root.join(c);
            if p.exists() { return p; }
        }
        root.join("opt/brave.com/brave-nightly/brave-browser-nightly")
    }
}

pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [versions_dir(), profiles_dir(), downloads_dir(), db_dir()] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}
