use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

use super::{discover, merge, pin};

#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub component_id: String,
    pub from_version: String,
    pub to_version:   String,
    pub from_path:    PathBuf,
    pub to_path:      PathBuf,
}

/// Pre-launch guard: reconcile pins against on-disk state.
/// Quarantines any unauthorized newer sibling, returning the list of
/// updates the caller may want to surface for user review.
pub fn pre_launch_guard(profile_dir: &Path) -> Result<Vec<PendingUpdate>> {
    let mut pending = Vec::new();
    for (component_id, status) in pin::verify_all(profile_dir)? {
        if let pin::PinStatus::NewerSiblingAppeared { found } = status {
            let pinned = pin::load(profile_dir)?.pins.get(&component_id).cloned()
                .ok_or_else(|| anyhow!("missing pin for {component_id}"))?;
            let from_path = found.with_file_name(&pinned.version);
            let to_version = found.file_name().unwrap_or_default().to_string_lossy().into_owned();

            // Quarantine the newer sibling immediately.
            let target = found.with_file_name(format!("{to_version}.disabled"));
            let _ = std::fs::rename(&found, &target);

            pending.push(PendingUpdate {
                component_id,
                from_version: pinned.version,
                to_version,
                from_path,
                to_path: target,
            });
        }
    }
    Ok(pending)
}

pub fn pending_updates(profile_dir: &Path) -> Result<Vec<PendingUpdate>> {
    pre_launch_guard(profile_dir)
}

/// Accept all pending updates: bring `.disabled` siblings back, optionally
/// 3-way merge user edits onto the new content.
pub fn accept_all(profile_dir: &Path, do_merge: bool) -> Result<()> {
    let pending = pending_updates(profile_dir)?;
    let pins = pin::load(profile_dir)?;
    for u in pending {
        // Restore the new version folder (drop `.disabled`).
        let restored = u.to_path.with_file_name(u.to_version.clone());
        if !restored.exists() {
            std::fs::rename(&u.to_path, &restored)?;
        }
        // 3-way merge if requested and we still have the old version on disk.
        if do_merge {
            if let Some(pin) = pins.pins.get(&u.component_id) {
                let _ = merge::three_way(&u.from_path, &restored, &pin.sha256_at_pin);
            }
        }
        // Re-pin to the new version.
        if let Some(list) = discover::enabled_lists(profile_dir)?
            .into_iter().find(|l| l.component_id == u.component_id) {
            let mut pin_file = pin::load(profile_dir)?;
            pin_file.pins.insert(u.component_id.clone(), pin::Pin {
                component_id:  u.component_id,
                version:       list.version,
                sha256_at_pin: list.sha256,
                pinned_at:     chrono::Utc::now(),
            });
            pin::save(profile_dir, &pin_file)?;
        }
    }
    Ok(())
}
