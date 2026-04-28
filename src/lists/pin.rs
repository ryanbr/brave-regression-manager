use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::discover;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub component_id:   String,
    pub version:        String,
    pub sha256_at_pin:  String,
    pub pinned_at:      DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PinFile {
    pub pins: HashMap<String, Pin>,    // component_id -> Pin
}

#[derive(Debug, Clone)]
pub enum PinStatus {
    Intact,
    NewerSiblingAppeared { found: PathBuf },
    ListMutatedExternally { on_disk_sha: String, expected: String },
    PinMissing,
}

pub fn pin_file_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(".brave-regress").join("pins.json")
}

pub fn load(profile_dir: &Path) -> Result<PinFile> {
    let p = pin_file_path(profile_dir);
    if !p.exists() { return Ok(PinFile::default()); }
    let s = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&s)?)
}

pub fn save(profile_dir: &Path, file: &PinFile) -> Result<()> {
    let p = pin_file_path(profile_dir);
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(p, serde_json::to_string_pretty(file)?)?;
    Ok(())
}

pub fn pin_all(profile_dir: &Path) -> Result<usize> {
    let mut file = load(profile_dir)?;
    let mut count = 0;
    for list in discover::enabled_lists(profile_dir)? {
        // Quarantine newer siblings (we expect `version_dir` is the highest semver).
        quarantine_newer_siblings(profile_dir, &list.component_id, &list.version)?;

        file.pins.insert(list.component_id.clone(), Pin {
            component_id:  list.component_id,
            version:       list.version,
            sha256_at_pin: list.sha256,
            pinned_at:     Utc::now(),
        });
        count += 1;
    }
    save(profile_dir, &file)?;
    Ok(count)
}

pub fn unpin_all(profile_dir: &Path) -> Result<()> {
    // Restore any `.disabled` siblings.
    for entry in walkdir::WalkDir::new(profile_dir).max_depth(4) {
        let e = match entry { Ok(e) => e, Err(_) => continue };
        if !e.file_type().is_dir() { continue; }
        let n = e.file_name().to_string_lossy();
        if let Some(orig) = n.strip_suffix(".disabled") {
            let restored = e.path().with_file_name(orig);
            if !restored.exists() {
                let _ = std::fs::rename(e.path(), restored);
            }
        }
    }
    let p = pin_file_path(profile_dir);
    if p.exists() { std::fs::remove_file(p)?; }
    Ok(())
}

pub fn verify_all(profile_dir: &Path) -> Result<HashMap<String, PinStatus>> {
    let pins = load(profile_dir)?;
    let mut out = HashMap::new();
    for list in discover::enabled_lists(profile_dir)? {
        let pin = match pins.pins.get(&list.component_id) {
            Some(p) => p,
            None    => { out.insert(list.component_id.clone(), PinStatus::PinMissing); continue; }
        };
        // Newer sibling on disk?
        if let Some(found) = newer_sibling(profile_dir, &list.component_id, &pin.version)? {
            out.insert(list.component_id.clone(), PinStatus::NewerSiblingAppeared { found });
            continue;
        }
        if list.sha256 != pin.sha256_at_pin {
            out.insert(list.component_id.clone(),
                PinStatus::ListMutatedExternally { on_disk_sha: list.sha256, expected: pin.sha256_at_pin.clone() });
            continue;
        }
        out.insert(list.component_id.clone(), PinStatus::Intact);
    }
    Ok(out)
}

fn quarantine_newer_siblings(profile_dir: &Path, component_id: &str, keep_version: &str) -> Result<()> {
    for root in [profile_dir.to_path_buf(), profile_dir.join("Default")] {
        let comp = root.join(component_id);
        if !comp.is_dir() { continue; }
        let keep = semver::Version::parse(keep_version).map_err(|e| anyhow!("bad version {keep_version}: {e}"))?;
        for e in std::fs::read_dir(&comp)? {
            let e = e?;
            if !e.file_type()?.is_dir() { continue; }
            let n = e.file_name().to_string_lossy().into_owned();
            if let Ok(v) = semver::Version::parse(&n) {
                if v > keep {
                    let target = e.path().with_file_name(format!("{n}.disabled"));
                    let _ = std::fs::rename(e.path(), target);
                }
            }
        }
    }
    Ok(())
}

fn newer_sibling(profile_dir: &Path, component_id: &str, keep_version: &str) -> Result<Option<PathBuf>> {
    let keep = semver::Version::parse(keep_version)?;
    for root in [profile_dir.to_path_buf(), profile_dir.join("Default")] {
        let comp = root.join(component_id);
        if !comp.is_dir() { continue; }
        for e in std::fs::read_dir(&comp)? {
            let e = e?;
            if !e.file_type()?.is_dir() { continue; }
            let n = e.file_name().to_string_lossy().into_owned();
            if let Ok(v) = semver::Version::parse(&n) {
                if v > keep { return Ok(Some(e.path())); }
            }
        }
    }
    Ok(None)
}
