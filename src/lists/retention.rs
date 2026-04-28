use anyhow::Result;
use semver::Version;
use std::path::Path;

use crate::config::Config;
use crate::paths;

/// Per-component subfolder retention: keep only the highest-N semver directories.
/// Skips folders ending in `.disabled` (those are quarantined updates we may want to keep).
pub fn prune_components(profile_dir: &Path) -> Result<()> {
    let cfg = Config::load_or_default(&paths::config_path())?;
    let keep = cfg.retention.keep_component_versions.max(1);
    for root in [profile_dir.to_path_buf(), profile_dir.join("Default")] {
        if !root.exists() { continue; }
        for entry in std::fs::read_dir(&root)? {
            let e = entry?;
            if !e.file_type()?.is_dir() { continue; }
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_component_id(&name) { continue; }

            let mut versions: Vec<(Version, std::path::PathBuf)> = std::fs::read_dir(e.path())?
                .filter_map(|x| x.ok())
                .filter(|x| x.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|x| {
                    let n = x.file_name().to_string_lossy().into_owned();
                    if n.ends_with(".disabled") { return None; }
                    Version::parse(&n).ok().map(|v| (v, x.path()))
                })
                .collect();
            versions.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, path) in versions.into_iter().skip(keep) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
    Ok(())
}

fn is_component_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_lowercase())
}
