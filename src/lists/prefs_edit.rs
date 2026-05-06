//! Atomic read/edit/write of `<user_data>/Local State` (NOT
//! `Default/Preferences` — Brave keeps adblock list state in the
//! browser-wide Local State file, even though most other prefs live
//! per-profile).
//!
//! Pref keys (all under Local State):
//!   brave.ad_block.regional_filters   dict UUID -> { "enabled": bool }
//!   brave.ad_block.list_subscriptions dict URL  -> { "enabled": bool, "title": str, ... }
//!   brave.ad_block.custom_filters     string
//!
//! The dicts only exist once the user has deviated from catalog
//! defaults — but Brave honours pre-written entries. Confirmed by
//! reading boce's source: it loads + writes regional_filters from
//! `<user_data>/Local State`, never from `Default/Preferences`. An
//! earlier draft of this file targeted `Default/Preferences` and
//! Brave silently dropped every edit on the floor.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

pub const PREF_REGIONAL: &str = "brave.ad_block.regional_filters";
pub const PREF_SUBSCRIPTIONS: &str = "brave.ad_block.list_subscriptions";
pub const PREF_CUSTOM_FILTERS: &str = "brave.ad_block.custom_filters";

/// One row from `list_subscriptions` for the GUI.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub url:     String,
    pub enabled: bool,
    pub title:   Option<String>,
}

/// Read `<user_data>/Local State` into a JSON value. Returns an
/// empty object when the file is absent — callers can build a
/// minimal seed and write it back, which Brave fills in on first
/// launch (used for throwaway dirs that have no Local State yet).
pub fn read_or_empty(profile_dir: &Path) -> Result<Value> {
    let path = prefs_path(profile_dir);
    if !path.is_file() {
        return Ok(Value::Object(Default::default()));
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let v: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(v)
}

/// Atomic write: backup existing → write tmp → rename. Returns the
/// backup path (or None when there was no prior file to back up).
pub fn write_atomic(profile_dir: &Path, value: &Value) -> Result<Option<PathBuf>> {
    let path = prefs_path(profile_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let backup = if path.is_file() {
        Some(backup_file(&path)?)
    } else {
        None
    };
    let tmp = path.with_extension("brave-regress.tmp");
    let body = serde_json::to_string(value).context("serializing Local State")?;
    std::fs::write(&tmp, body)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(backup)
}

fn backup_file(path: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let name = path.file_name()
        .ok_or_else(|| anyhow!("invalid path: {}", path.display()))?
        .to_string_lossy().into_owned();
    let backup = path.with_file_name(format!("{name}.bak-{stamp}"));
    std::fs::copy(path, &backup)
        .with_context(|| format!("copying {} -> {}", path.display(), backup.display()))?;
    Ok(backup)
}

/// Path to the file we mutate. Brave uses `<user_data>/Local State`
/// (one level up from `<user_data>/Default/Preferences`) for the
/// `brave.ad_block.*` keys. `profile_dir` here is the user-data-dir
/// (the dir Brave gets via `--user-data-dir=…`), not the inner
/// `Default/` profile.
pub fn prefs_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("Local State")
}

/// Set `enabled` on the given UUID's entry under
/// `brave.ad_block.regional_filters`, creating the dict path when
/// missing. Pure JSON mutation — caller is responsible for the
/// surrounding read/write.
pub fn set_regional_enabled(root: &mut Value, uuid: &str, enabled: bool) -> Result<()> {
    ensure_object(root, PREF_REGIONAL)?;
    let dict = get_path_mut(root, PREF_REGIONAL)
        .ok_or_else(|| anyhow!("failed to resolve {PREF_REGIONAL}"))?;
    let map = dict.as_object_mut()
        .ok_or_else(|| anyhow!("{PREF_REGIONAL} is not an object"))?;
    let entry = map.entry(uuid.to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let obj = entry.as_object_mut()
        .ok_or_else(|| anyhow!("regional entry for {uuid} is not an object"))?;
    obj.insert("enabled".to_string(), Value::Bool(enabled));
    Ok(())
}

/// One-shot: read prefs, flip `regional_filters[uuid].enabled`,
/// atomic-write back, then read-back-and-verify the field actually
/// holds the value we set. Returns the backup path (None when no
/// prior file existed). The verify step mirrors boce's pattern —
/// catches the case where another writer (Brave on shutdown,
/// concurrent edit) clobbered our write between rename and read.
pub fn edit_regional_enabled(
    profile_dir: &Path,
    uuid: &str,
    enabled: bool,
) -> Result<Option<PathBuf>> {
    let mut root = read_or_empty(profile_dir)?;
    set_regional_enabled(&mut root, uuid, enabled)?;
    let backup = write_atomic(profile_dir, &root)?;
    let disk = read_or_empty(profile_dir)?;
    let got = disk.pointer(&pointer_for(uuid))
        .and_then(|v| v.as_bool());
    if got != Some(enabled) {
        return Err(anyhow!(
            "verify failed at {}: regional_filters[{uuid}].enabled = {got:?}, \
             wanted {enabled} — something else wrote to Local State after us",
            prefs_path(profile_dir).display()));
    }
    Ok(backup)
}

fn pointer_for(uuid: &str) -> String {
    let escaped = uuid.replace('~', "~0").replace('/', "~1");
    format!("/brave/ad_block/regional_filters/{escaped}/enabled")
}

fn sub_pointer_for(url: &str) -> String {
    let escaped = url.replace('~', "~0").replace('/', "~1");
    format!("/brave/ad_block/list_subscriptions/{escaped}/enabled")
}

/// Read the `regional_filters` dict from Local State, returning a
/// UUID → enabled map. Only entries with explicit values are
/// included — UUIDs not in the dict mean "use catalog default", so
/// the caller falls back to `CatalogEntry.default_enabled` for those.
pub fn read_regional_filter_states(
    profile_dir: &Path,
) -> Result<std::collections::HashMap<String, bool>> {
    let root = read_or_empty(profile_dir)?;
    let mut out = std::collections::HashMap::new();
    let Some(map) = root.pointer("/brave/ad_block/regional_filters")
        .and_then(|v| v.as_object()) else { return Ok(out); };
    for (uuid, entry) in map {
        if let Some(en) = entry.get("enabled").and_then(|b| b.as_bool()) {
            out.insert(uuid.clone(), en);
        }
    }
    Ok(out)
}

/// Read the `list_subscriptions` dict from Local State, returning a
/// sorted-by-URL Vec for the GUI. Empty when the dict isn't present.
pub fn read_subscriptions(profile_dir: &Path) -> Result<Vec<Subscription>> {
    let root = read_or_empty(profile_dir)?;
    let Some(map) = root.pointer("/brave/ad_block/list_subscriptions")
        .and_then(|v| v.as_object()) else { return Ok(Vec::new()); };
    let mut out: Vec<Subscription> = map.iter().map(|(url, v)| Subscription {
        url:     url.clone(),
        enabled: v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(false),
        title:   v.get("title").and_then(|t| t.as_str()).map(str::to_string),
    }).collect();
    out.sort_by(|a, b| a.url.cmp(&b.url));
    Ok(out)
}

/// Mutate `list_subscriptions[url].enabled` in-place, creating the
/// entry / parent dicts when missing. Pure JSON mutation.
pub fn set_subscription_enabled(root: &mut Value, url: &str, enabled: bool) -> Result<()> {
    ensure_object(root, PREF_SUBSCRIPTIONS)?;
    let dict = get_path_mut(root, PREF_SUBSCRIPTIONS)
        .ok_or_else(|| anyhow!("failed to resolve {PREF_SUBSCRIPTIONS}"))?;
    let map = dict.as_object_mut()
        .ok_or_else(|| anyhow!("{PREF_SUBSCRIPTIONS} is not an object"))?;
    let entry = map.entry(url.to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let obj = entry.as_object_mut()
        .ok_or_else(|| anyhow!("subscription entry for {url} is not an object"))?;
    obj.insert("enabled".to_string(), Value::Bool(enabled));
    Ok(())
}

/// Remove a subscription entry entirely. No-op when absent.
pub fn remove_subscription_entry(root: &mut Value, url: &str) -> Result<()> {
    let Some(dict) = get_path_mut(root, PREF_SUBSCRIPTIONS) else { return Ok(()); };
    if let Some(map) = dict.as_object_mut() {
        map.remove(url);
    }
    Ok(())
}

/// One-shot: read → set subscription enabled → atomic write → verify.
pub fn edit_subscription_enabled(
    profile_dir: &Path,
    url: &str,
    enabled: bool,
) -> Result<Option<PathBuf>> {
    let mut root = read_or_empty(profile_dir)?;
    set_subscription_enabled(&mut root, url, enabled)?;
    let backup = write_atomic(profile_dir, &root)?;
    let disk = read_or_empty(profile_dir)?;
    let got = disk.pointer(&sub_pointer_for(url)).and_then(|v| v.as_bool());
    if got != Some(enabled) {
        return Err(anyhow!(
            "verify failed at {}: list_subscriptions[{url}].enabled = {got:?}, \
             wanted {enabled}", prefs_path(profile_dir).display()));
    }
    Ok(backup)
}

/// One-shot: read → remove subscription → atomic write → verify it's gone.
pub fn remove_subscription(profile_dir: &Path, url: &str) -> Result<Option<PathBuf>> {
    let mut root = read_or_empty(profile_dir)?;
    remove_subscription_entry(&mut root, url)?;
    let backup = write_atomic(profile_dir, &root)?;
    let disk = read_or_empty(profile_dir)?;
    let still_there = disk.pointer("/brave/ad_block/list_subscriptions")
        .and_then(|v| v.as_object())
        .is_some_and(|m| m.contains_key(url));
    if still_there {
        return Err(anyhow!(
            "verify failed at {}: list_subscriptions still contains {url} after remove",
            prefs_path(profile_dir).display()));
    }
    Ok(backup)
}

/// Read `brave.ad_block.custom_filters` from Local State. Empty
/// string when absent — the empty case is "no custom rules", which
/// is also what Brave shows by default.
pub fn read_custom_filters(profile_dir: &Path) -> Result<String> {
    let root = read_or_empty(profile_dir)?;
    Ok(root.pointer("/brave/ad_block/custom_filters")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// One-shot: write `brave.ad_block.custom_filters` to the given
/// string, atomic + verify.
pub fn edit_custom_filters(profile_dir: &Path, text: &str) -> Result<Option<PathBuf>> {
    let mut root = read_or_empty(profile_dir)?;
    set_path(&mut root, PREF_CUSTOM_FILTERS, Value::String(text.to_string()))?;
    let backup = write_atomic(profile_dir, &root)?;
    let disk = read_or_empty(profile_dir)?;
    let got = disk.pointer("/brave/ad_block/custom_filters")
        .and_then(|v| v.as_str()).unwrap_or("");
    if got != text {
        return Err(anyhow!(
            "verify failed at {}: custom_filters length on disk={} expected={}",
            prefs_path(profile_dir).display(), got.len(), text.len()));
    }
    Ok(backup)
}

fn set_path(root: &mut Value, dotted: &str, val: Value) -> Result<()> {
    let segs: Vec<&str> = dotted.split('.').collect();
    if segs.is_empty() { return Err(anyhow!("empty key")); }
    let mut cur = root;
    for seg in &segs[..segs.len() - 1] {
        if !cur.is_object() {
            return Err(anyhow!("cannot descend into non-object at '{seg}'"));
        }
        let map = cur.as_object_mut().unwrap();
        if !map.contains_key(*seg) {
            map.insert((*seg).to_string(), Value::Object(Default::default()));
        }
        cur = map.get_mut(*seg).unwrap();
    }
    let last = segs.last().unwrap();
    let map = cur.as_object_mut()
        .ok_or_else(|| anyhow!("parent of '{last}' is not an object"))?;
    map.insert((*last).to_string(), val);
    Ok(())
}

/// Apply every (uuid -> enabled) entry to the given profile dir's
/// Local State in a single read-modify-write. Used right before
/// launch to re-assert our edits regardless of what Brave wrote
/// during its previous shutdown — protects against the freshly-
/// created throwaway race + Brave shutdown-time pruning.
///
/// Returns the count of entries applied (zero when overrides is
/// empty — caller shouldn't bother logging in that case).
pub fn replay_regional_overrides(
    profile_dir: &Path,
    overrides: &std::collections::HashMap<String, bool>,
) -> Result<usize> {
    if overrides.is_empty() { return Ok(0); }
    let mut root = read_or_empty(profile_dir)?;
    for (uuid, enabled) in overrides {
        set_regional_enabled(&mut root, uuid, *enabled)?;
    }
    write_atomic(profile_dir, &root)?;
    Ok(overrides.len())
}

/// Per-launch action for one subscription URL. `Set(true/false)`
/// flips `enabled`; `Remove` strips the entry so Brave reverts to
/// the catalog default.
#[derive(Debug, Clone)]
pub enum SubAction { Set(bool), Remove }

/// Replay subscription edits + custom filter rules into Local State
/// in a single read-modify-write. Symmetric counterpart to
/// `replay_regional_overrides`. Returns the count of operations
/// applied (subs + 1 if custom_filters was set).
pub fn replay_subscription_and_custom(
    profile_dir: &Path,
    subscription_ops: &std::collections::HashMap<String, SubAction>,
    custom_filters: Option<&str>,
) -> Result<usize> {
    if subscription_ops.is_empty() && custom_filters.is_none() { return Ok(0); }
    let mut root = read_or_empty(profile_dir)?;
    let mut applied = 0usize;
    for (url, action) in subscription_ops {
        match action {
            SubAction::Set(en) => set_subscription_enabled(&mut root, url, *en)?,
            SubAction::Remove  => remove_subscription_entry(&mut root, url)?,
        }
        applied += 1;
    }
    if let Some(text) = custom_filters {
        set_path(&mut root, PREF_CUSTOM_FILTERS, Value::String(text.to_string()))?;
        applied += 1;
    }
    write_atomic(profile_dir, &root)?;
    Ok(applied)
}

fn get_path_mut<'a>(root: &'a mut Value, dotted: &str) -> Option<&'a mut Value> {
    let mut cur = root;
    for seg in dotted.split('.') {
        cur = cur.as_object_mut()?.get_mut(seg)?;
    }
    Some(cur)
}

fn ensure_object(root: &mut Value, dotted: &str) -> Result<()> {
    let segs: Vec<&str> = dotted.split('.').collect();
    if segs.is_empty() { return Err(anyhow!("empty key")); }
    let mut cur = root;
    for seg in &segs {
        if !cur.is_object() {
            return Err(anyhow!("cannot descend into non-object at '{seg}'"));
        }
        let map = cur.as_object_mut().unwrap();
        if !map.contains_key(*seg) {
            map.insert((*seg).to_string(), Value::Object(Default::default()));
        }
        let child = map.get_mut(*seg).unwrap();
        if !child.is_object() {
            return Err(anyhow!("'{seg}' exists but is not an object"));
        }
        cur = child;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_dict_when_missing() {
        let mut v: Value = serde_json::from_str("{}").unwrap();
        set_regional_enabled(&mut v, "abc", false).unwrap();
        assert_eq!(v.pointer("/brave/ad_block/regional_filters/abc/enabled"),
                   Some(&Value::Bool(false)));
    }
    #[test]
    fn flips_existing() {
        let mut v: Value = serde_json::from_str(
            r#"{"brave":{"ad_block":{"regional_filters":{"abc":{"enabled":true}}}}}"#)
            .unwrap();
        set_regional_enabled(&mut v, "abc", false).unwrap();
        assert_eq!(v.pointer("/brave/ad_block/regional_filters/abc/enabled"),
                   Some(&Value::Bool(false)));
    }
}
