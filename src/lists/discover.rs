use anyhow::Result;
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ListKind { Default, Regional }

#[derive(Debug, Clone)]
pub struct EnabledList {
    pub component_id: String,
    pub uuid:         Option<String>,   // regional list UUID, if any
    pub name:         String,
    pub version:      String,
    pub path:         PathBuf,
    pub sha256:       String,
    pub line_count:   usize,
    pub kind:         ListKind,
}

/// IDs are stable across Brave versions.
const DEFAULT_COMPONENT_ID:  &str = "cffkpbalmllkdoenhmdmpbkajipdjfam";
/// Brave's regional list catalog component has shifted IDs across
/// versions:
///   gkboaolpopklhgplhaaiboijnklogmbc — modern (matches boce)
///   gccbbnhkhcdjncjfbknbnepflcabamhf — older / alternate channel
/// Probe both; keep the first that resolves on disk.
const REGIONAL_CATALOG_IDS:  &[&str] = &[
    "gkboaolpopklhgplhaaiboijnklogmbc",
    "gccbbnhkhcdjncjfbknbnepflcabamhf",
];

/// Read enabled adblock lists for a Brave profile.
///
/// Strategy:
///   1. Try the catalog-driven path: read `Default/Preferences` for enabled
///      regional UUIDs, resolve via the regional catalog, locate each
///      component's `list.txt` on disk.
///   2. Fall back to a generic scan: walk the profile for *any* folder
///      whose name looks like a Chromium component-id (32 lowercase chars)
///      containing a versioned `list.txt`. Treats anything found this way
///      as an enabled list. This catches components whose IDs we don't
///      hard-code (Brave occasionally rebrands them) and components added
///      since we last updated the static IDs.
pub fn enabled_lists(profile_dir: &Path) -> Result<Vec<EnabledList>> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Catalog-driven default list.
    if let Some(p) = active_component_path(profile_dir, DEFAULT_COMPONENT_ID) {
        if let Some(list) = build_entry(&p, DEFAULT_COMPONENT_ID, None,
                                        "Brave default lists", ListKind::Default) {
            seen.insert(list.component_id.clone());
            out.push(list);
        }
    }

    // Catalog-driven regional lists.
    let enabled_uuids = read_enabled_regional_uuids(profile_dir).unwrap_or_default();
    let catalog = REGIONAL_CATALOG_IDS.iter()
        .find_map(|cid| active_component_path(profile_dir, cid))
        .map(|p| super::catalog::load(&p).unwrap_or_default())
        .unwrap_or_default();
    for uuid in enabled_uuids {
        let entry = catalog.get(&uuid);
        let component_id = entry.map(|e| e.component_id.clone()).unwrap_or_default();
        if component_id.is_empty() { continue; }
        let title = entry.map(|e| e.title.clone()).unwrap_or_else(|| uuid.clone());
        if let Some(p) = active_component_path(profile_dir, &component_id) {
            if let Some(list) = build_entry(&p, &component_id, Some(uuid), &title, ListKind::Regional) {
                seen.insert(list.component_id.clone());
                out.push(list);
            }
        }
    }

    // Fallback generic scan: any component-id-shaped folder with list.txt
    // we haven't already picked up. Use the manifest's `name` field if
    // present, otherwise the component id.
    for root in [profile_dir.to_path_buf(), profile_dir.join("Default")] {
        if !root.is_dir() { continue; }
        let entries = match std::fs::read_dir(&root) { Ok(r) => r, Err(_) => continue };
        for e in entries.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_component_id(&name) { continue; }
            if seen.contains(&name) { continue; }
            // The regional catalog component holds an index of regional lists,
            // not a filter list itself — skip it so it doesn't show up as an
            // empty "0 lines" entry in the GUI.
            if REGIONAL_CATALOG_IDS.contains(&name.as_str()) { continue; }
            if let Some(p) = pick_highest_version_dir(&e.path()) {
                // Only treat folders that contain an actual `list.txt` as
                // filter lists. Components with only `list_catalog.json`
                // are catalogs, not lists.
                if !p.join("list.txt").exists() { continue; }
                let title = read_manifest_name(&p).unwrap_or_else(|| name.clone());
                if let Some(list) = build_entry(&p, &name, None, &title, ListKind::Regional) {
                    seen.insert(list.component_id.clone());
                    out.push(list);
                }
            }
        }
    }
    Ok(out)
}

fn is_component_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_lowercase())
}

fn pick_highest_version_dir(component_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(component_dir).ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".disabled") { return None; }
            Version::parse(&n).ok().map(|v| (v, e.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
}

fn read_manifest_name(version_dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(version_dir.join("manifest.json")).ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())
}

/// Diagnostic helper: list every top-level component-id-shaped folder under
/// the profile (and the `Default/` subfolder), with what we find inside.
/// Used to explain "Re-scan found 0 lists" without making the user dig
/// through filesystem state by hand.
pub fn dump_component_dirs(profile_dir: &Path) -> String {
    let mut lines = Vec::new();
    for root in [profile_dir.to_path_buf(), profile_dir.join("Default")] {
        if !root.is_dir() { continue; }
        let entries = match std::fs::read_dir(&root) { Ok(r) => r, Err(_) => continue };
        for e in entries.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_component_id(&name) { continue; }
            let inside: Vec<String> = std::fs::read_dir(e.path()).into_iter().flatten()
                .filter_map(|s| s.ok())
                .map(|s| s.file_name().to_string_lossy().into_owned())
                .collect();
            lines.push(format!("{}/[{}]", name, inside.join(",")));
        }
    }
    lines.join("  ·  ")
}

pub fn active_component_path(profile_dir: &Path, component_id: &str) -> Option<PathBuf> {
    let candidates = [profile_dir.to_path_buf(), profile_dir.join("Default")];
    for root in candidates {
        let comp = root.join(component_id);
        if !comp.is_dir() { continue; }
        let best = std::fs::read_dir(&comp).ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                Version::parse(&n).ok().map(|v| (v, e.path()))
            })
            .max_by(|a, b| a.0.cmp(&b.0));
        if let Some((_, p)) = best { return Some(p); }
    }
    None
}

fn build_entry(version_dir: &Path, component_id: &str, uuid: Option<String>,
               title: &str, kind: ListKind) -> Option<EnabledList> {
    let list_path = first_existing(version_dir, &["list.txt", "list_catalog.json"])?;
    let bytes = std::fs::read(&list_path).ok()?;
    let mut hasher = Sha256::new(); hasher.update(&bytes);
    let sha = hex::encode(hasher.finalize());
    let line_count = bytes.iter().filter(|&&b| b == b'\n').count();
    let version = version_dir.file_name()?.to_string_lossy().into_owned();
    Some(EnabledList {
        component_id: component_id.into(),
        uuid,
        name: title.into(),
        version,
        path: list_path,
        sha256: sha,
        line_count,
        kind,
    })
}

fn first_existing(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    for n in names { let p = dir.join(n); if p.exists() { return Some(p); } }
    None
}

/// Parse `<user-data-dir>/Local State` for `brave.ad_block.regional_filters`,
/// returning the UUIDs marked enabled. Brave keeps this dict in
/// Local State (browser-wide), not in the per-profile `Preferences`
/// file — earlier versions of this code read the wrong file and so
/// the catalog-driven scan turned up empty for everyone.
fn read_enabled_regional_uuids(profile_dir: &Path) -> Result<Vec<String>> {
    let path = profile_dir.join("Local State");
    let s = std::fs::read_to_string(&path)?;
    let v: Value = serde_json::from_str(&s)?;
    let map = v.pointer("/brave/ad_block/regional_filters")
        .and_then(|x| x.as_object());
    let mut out = Vec::new();
    if let Some(m) = map {
        for (uuid, entry) in m {
            let enabled = entry.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
            if enabled { out.push(uuid.clone()); }
        }
    }
    Ok(out)
}
