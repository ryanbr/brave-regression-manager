use anyhow::{anyhow, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum ResetScope {
    /// Wipe the entire user-data-dir.
    Full,
    /// Cookies, cache, history, storage. Keep prefs + lists.
    BrowsingData,
    /// Preferences + Local State only. Keep browsing data + lists.
    PrefsOnly,
    /// Wipe component-updater list cache so lists re-pull on next seed.
    Lists,
}

impl ResetScope {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "full"            => Self::Full,
            "browsing"        => Self::BrowsingData,
            "prefs"           => Self::PrefsOnly,
            "lists"           => Self::Lists,
            other => return Err(anyhow!("unknown reset scope: {other}")),
        })
    }
}

pub fn reset_profile(profile_dir: &Path, scope: ResetScope) -> Result<()> {
    if !profile_dir.exists() { return Err(anyhow!("profile not found: {}", profile_dir.display())); }
    match scope {
        ResetScope::Full         => wipe_dir(profile_dir),
        ResetScope::BrowsingData => wipe_browsing(profile_dir),
        ResetScope::PrefsOnly    => wipe_prefs(profile_dir),
        ResetScope::Lists        => wipe_list_components(profile_dir),
    }
}

fn wipe_dir(p: &Path) -> Result<()> {
    std::fs::remove_dir_all(p)?;
    std::fs::create_dir_all(p)?;
    Ok(())
}

fn wipe_browsing(p: &Path) -> Result<()> {
    let default = p.join("Default");
    for sub in ["Cache", "Code Cache", "GPUCache", "IndexedDB", "Local Storage",
                "Service Worker", "Session Storage", "Cookies", "Cookies-journal",
                "History", "History-journal", "Network"] {
        rm_path(&default.join(sub))?;
    }
    Ok(())
}

fn wipe_prefs(p: &Path) -> Result<()> {
    rm_path(&p.join("Default").join("Preferences"))?;
    rm_path(&p.join("Default").join("Secure Preferences"))?;
    rm_path(&p.join("Local State"))?;
    Ok(())
}

/// Wipe adblock-related parsed caches and component subfolders.
/// We deliberately leave the regional catalog component alone unless caller
/// passes a fresh seed afterwards.
fn wipe_list_components(p: &Path) -> Result<()> {
    // Brave caches the parsed engine DAT under <profile>/AdBlock/
    rm_path(&p.join("AdBlock"))?;
    rm_path(&p.join("Default").join("AdBlock"))?;
    // Component-updater list folders: walk top-level + Default and remove anything
    // matching a component-id-shaped name (32 lowercase letters).
    for root in [p, &p.join("Default")] {
        if !root.exists() { continue; }
        for e in std::fs::read_dir(root)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if is_component_id(&name) {
                rm_path(&e.path())?;
            }
        }
    }
    Ok(())
}

fn is_component_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_lowercase())
}

fn rm_path(p: &Path) -> Result<()> {
    if !p.exists() { return Ok(()); }
    let meta = std::fs::symlink_metadata(p)?;
    if meta.is_dir() { std::fs::remove_dir_all(p)?; } else { std::fs::remove_file(p)?; }
    Ok(())
}
