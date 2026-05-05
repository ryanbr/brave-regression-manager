use anyhow::{anyhow, Result};
use std::path::PathBuf;

use crate::cli::ProfileCmd;
use crate::paths;

pub mod reset;
pub mod seed;

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub dir:  PathBuf,
}

pub fn list() -> Result<Vec<Profile>> {
    let dir = paths::profiles_dir();
    if !dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hide throwaway-<tag>-<unix-ts> dirs created by the
            // Settings → Clean profile per launch flow. They're
            // single-use, never picked manually, and clutter the
            // profile dropdown.
            if name.starts_with("throwaway-") { continue; }
            out.push(Profile { name, dir: entry.path() });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Wipe every `throwaway-*` profile dir whose modtime is older than
/// `max_age`. Best-effort: every error is silently ignored — these are
/// disposable folders by design. Returns `(count, freed_bytes)` so the
/// caller can log a one-line summary.
pub fn purge_stale_throwaways(max_age: std::time::Duration) -> (usize, u64) {
    let dir = paths::profiles_dir();
    if !dir.exists() { return (0, 0); }
    let now = std::time::SystemTime::now();
    let mut count = 0usize;
    let mut bytes = 0u64;
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("throwaway-") { continue; }
        let path = entry.path();
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        let too_old = modified
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d > max_age)
            .unwrap_or(true);
        if !too_old { continue; }
        // Tally bytes (best-effort) before delete so we can report
        // the freed amount.
        for sub in walkdir::WalkDir::new(&path).into_iter().flatten() {
            if sub.file_type().is_file() {
                bytes += sub.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            count += 1;
        }
    }
    (count, bytes)
}

pub fn create(name: &str) -> Result<Profile> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(anyhow!("invalid profile name: {name}"));
    }
    let dir = paths::profile_dir(name);
    std::fs::create_dir_all(&dir)?;
    Ok(Profile { name: name.into(), dir })
}

pub fn delete(name: &str) -> Result<()> {
    let dir = paths::profile_dir(name);
    if !dir.exists() { return Err(anyhow!("no such profile: {name}")); }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

pub async fn handle(cmd: ProfileCmd) -> Result<()> {
    paths::ensure_dirs()?;
    match cmd {
        ProfileCmd::New    { name } => { create(&name)?; println!("created profile {name}"); Ok(()) }
        ProfileCmd::Delete { name } => { delete(&name)?; println!("deleted profile {name}"); Ok(()) }
        ProfileCmd::List   => {
            for p in list()? { println!("{}\t{}", p.name, p.dir.display()); }
            Ok(())
        }
        ProfileCmd::Reset  { name, scope } => {
            let scope = reset::ResetScope::parse(&scope)?;
            reset::reset_profile(&paths::profile_dir(&name), scope)?;
            println!("reset {name} ({:?})", scope);
            Ok(())
        }
        ProfileCmd::Seed   { name, version } => {
            seed::seed_lists(&name, &version).await?;
            println!("seeded lists for profile {name} using {version}");
            Ok(())
        }
    }
}
