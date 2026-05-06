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

/// `<user_data>/Default/Preferences` — per-profile prefs. Used for
/// the extension blocklist (which Brave reads per-profile, not
/// browser-wide), among other things we don't currently touch.
pub fn default_prefs_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("Default").join("Preferences")
}

/// Pre-write the `Local State` keys Brave reads to decide whether
/// to surface the first-run P3A telemetry banner. With these set,
/// the banner stays hidden and the P3A subsystem doesn't ping
/// home. Idempotent — re-runs on already-correct files are a
/// no-op via the same short-circuit pattern as
/// `ensure_extension_blocklist`.
pub fn ensure_p3a_dismissed(profile_dir: &Path) -> Result<Option<PathBuf>> {
    // Resolve to a Value tree, set 3 keys, atomic-write back.
    // Local State (the file we already operate on for adblock) is
    // also where Brave keeps `brave.p3a.*` and
    // `brave.stats.reporting_enabled`.
    let mut root = read_or_empty(profile_dir)?;
    let already_correct = root.pointer("/brave/p3a/notice_acknowledged")
            .and_then(|v| v.as_bool()) == Some(true)
        && root.pointer("/brave/p3a/enabled")
            .and_then(|v| v.as_bool()) == Some(false)
        && root.pointer("/brave/stats/reporting_enabled")
            .and_then(|v| v.as_bool()) == Some(false);
    if already_correct { return Ok(None); }
    set_path(&mut root, "brave.p3a.notice_acknowledged", Value::Bool(true))?;
    set_path(&mut root, "brave.p3a.enabled",             Value::Bool(false))?;
    set_path(&mut root, "brave.stats.reporting_enabled", Value::Bool(false))?;
    let backup = write_atomic(profile_dir, &root)?;
    // Verify all three round-tripped before returning success.
    let disk = read_or_empty(profile_dir)?;
    let ack = disk.pointer("/brave/p3a/notice_acknowledged").and_then(|v| v.as_bool());
    let p3a = disk.pointer("/brave/p3a/enabled").and_then(|v| v.as_bool());
    let stt = disk.pointer("/brave/stats/reporting_enabled").and_then(|v| v.as_bool());
    if ack != Some(true) || p3a != Some(false) || stt != Some(false) {
        return Err(anyhow!(
            "verify failed at {}: p3a state ack={ack:?} enabled={p3a:?} \
             stats_reporting={stt:?}", prefs_path(profile_dir).display()));
    }
    Ok(backup)
}

/// Read+modify+atomic-write `Default/Preferences` to make Brave
/// behave as if the given extensions were never there:
///
///   1. Add to `extensions.install.deny_list`     — refuse to load
///   2. Add to `extensions.external_uninstalls`   — suppress "Action
///      required" / "Previously installed external extension"
///      notifications; tells Chromium the user has uninstalled
///      this external extension and not to re-add it
///   3. Remove `extensions.settings.<id>`         — clears the
///      tombstone Brave keeps for previously-seen extensions
///   4. Remove `extensions.external_extensions.<id>` (if present)
///
/// Without (2)+(3) the deny_list alone leaves a record Brave shows
/// in chrome://extensions as a "disabled by policy" card with an
/// Action-required badge.
///
/// Existing entries in deny_list / external_uninstalls (added by
/// other code or by Brave itself) are preserved; we just merge +
/// dedupe. Verifies the round-trip after rename.
pub fn ensure_extension_blocklist(
    profile_dir: &Path,
    blocked_ids: &[&str],
) -> Result<Option<PathBuf>> {
    if blocked_ids.is_empty() { return Ok(None); }
    let path = default_prefs_path(profile_dir);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut root: Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", path.display()))?
    } else {
        Value::Object(Default::default())
    };
    // Short-circuit: if every requested id is already in deny_list
    // AND in external_uninstalls AND absent from extensions.settings
    // AND absent from extensions.external_extensions, the on-disk
    // state already reflects what we want — skip the rewrite (and
    // dodge the per-launch backup file + mtime bump).
    {
        let arr_set = |p: &str| -> std::collections::HashSet<&str> {
            root.pointer(p).and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default()
        };
        let deny    = arr_set("/extensions/install/deny_list");
        let uninst  = arr_set("/extensions/external_uninstalls");
        let in_settings = root.pointer("/extensions/settings")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().any(|k| blocked_ids.iter().any(|id| k == *id)))
            .unwrap_or(false);
        let in_ext = root.pointer("/extensions/external_extensions")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().any(|k| blocked_ids.iter().any(|id| k == *id)))
            .unwrap_or(false);
        let all_present = blocked_ids.iter().all(|id|
            deny.contains(*id) && uninst.contains(*id));
        if all_present && !in_settings && !in_ext {
            return Ok(None);
        }
    }
    // (1) deny_list — refuse to load
    {
        let mut merged: std::collections::BTreeSet<String> =
            root.pointer("/extensions/install/deny_list")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
        for id in blocked_ids { merged.insert((*id).to_string()); }
        let arr: Vec<Value> = merged.iter().map(|s| Value::String(s.clone())).collect();
        set_path(&mut root, "extensions.install.deny_list", Value::Array(arr))?;
    }
    // (2) external_uninstalls — silence "Action required" for
    //     external/preloaded extensions Brave bundles by default
    {
        let mut merged: std::collections::BTreeSet<String> =
            root.pointer("/extensions/external_uninstalls")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
        for id in blocked_ids { merged.insert((*id).to_string()); }
        let arr: Vec<Value> = merged.iter().map(|s| Value::String(s.clone())).collect();
        set_path(&mut root, "extensions.external_uninstalls", Value::Array(arr))?;
    }
    // (3) + (4) drop the per-id tombstones so Brave doesn't list
    // it on chrome://extensions at all.
    if let Some(settings) = root.pointer_mut("/extensions/settings")
        .and_then(|v| v.as_object_mut())
    {
        for id in blocked_ids { settings.remove(*id); }
    }
    if let Some(ext) = root.pointer_mut("/extensions/external_extensions")
        .and_then(|v| v.as_object_mut())
    {
        for id in blocked_ids { ext.remove(*id); }
    }
    // Atomic tmp+rename
    let backup = if path.is_file() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let bk = path.with_file_name(format!("Preferences.bak-{stamp}"));
        std::fs::copy(&path, &bk).ok();
        Some(bk)
    } else { None };
    let tmp = path.with_extension("brave-regress.tmp");
    let body = serde_json::to_string(&root)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    // Verify every id is present in deny_list AND external_uninstalls
    // AND absent from extensions.settings.
    let raw = std::fs::read_to_string(&path)?;
    let disk: Value = serde_json::from_str(&raw)?;
    let arr_to_set = |p: &str| -> std::collections::HashSet<String> {
        disk.pointer(p)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let in_deny = arr_to_set("/extensions/install/deny_list");
    let in_uninst = arr_to_set("/extensions/external_uninstalls");
    let still_in_settings = disk.pointer("/extensions/settings")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect::<std::collections::HashSet<_>>())
        .unwrap_or_default();
    for id in blocked_ids {
        if !in_deny.contains(*id) {
            return Err(anyhow!("verify: deny_list missing {id} at {}", path.display()));
        }
        if !in_uninst.contains(*id) {
            return Err(anyhow!("verify: external_uninstalls missing {id} at {}", path.display()));
        }
        if still_in_settings.contains(*id) {
            return Err(anyhow!("verify: extensions.settings still has {id} at {}", path.display()));
        }
    }
    Ok(backup)
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

/// All three filter-list views from one Local State read — used by
/// the GUI's "load everything for this profile" path so we don't
/// open + parse the same JSON three times in a row.
pub struct AllViews {
    pub regional:      std::collections::HashMap<String, bool>,
    pub subscriptions: Vec<Subscription>,
    pub custom:        String,
}

/// One read of `<user-data-dir>/Local State`, three views populated.
pub fn read_all_views(profile_dir: &Path) -> Result<AllViews> {
    let root = read_or_empty(profile_dir)?;
    Ok(AllViews {
        regional:      regional_states_from(&root),
        subscriptions: subscriptions_from(&root),
        custom:        custom_filters_from(&root),
    })
}

fn regional_states_from(root: &Value) -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::new();
    let Some(map) = root.pointer("/brave/ad_block/regional_filters")
        .and_then(|v| v.as_object()) else { return out; };
    for (uuid, entry) in map {
        if let Some(en) = entry.get("enabled").and_then(|b| b.as_bool()) {
            out.insert(uuid.clone(), en);
        }
    }
    out
}

fn subscriptions_from(root: &Value) -> Vec<Subscription> {
    let Some(map) = root.pointer("/brave/ad_block/list_subscriptions")
        .and_then(|v| v.as_object()) else { return Vec::new(); };
    let mut out: Vec<Subscription> = map.iter().map(|(url, v)| Subscription {
        url:     url.clone(),
        enabled: v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(false),
        title:   v.get("title").and_then(|t| t.as_str()).map(str::to_string),
    }).collect();
    out.sort_by(|a, b| a.url.cmp(&b.url));
    out
}

fn custom_filters_from(root: &Value) -> String {
    root.pointer("/brave/ad_block/custom_filters")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Read the `regional_filters` dict from Local State. Single-view
/// helper retained for the few call sites that don't need the
/// other two views.
pub fn read_regional_filter_states(
    profile_dir: &Path,
) -> Result<std::collections::HashMap<String, bool>> {
    Ok(regional_states_from(&read_or_empty(profile_dir)?))
}

/// Read the `list_subscriptions` dict. Single-view helper.
pub fn read_subscriptions(profile_dir: &Path) -> Result<Vec<Subscription>> {
    Ok(subscriptions_from(&read_or_empty(profile_dir)?))
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
    Ok(custom_filters_from(&read_or_empty(profile_dir)?))
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

/// Per-launch action for one subscription URL. `Set(true/false)`
/// flips `enabled`; `Remove` strips the entry so Brave reverts to
/// the catalog default.
#[derive(Debug, Clone)]
pub enum SubAction { Set(bool), Remove }

/// Single read → all overrides applied → single write. Used right
/// before launch to re-assert every list edit (regional flags,
/// subscription ops, custom_filters) the user made this session,
/// regardless of what Brave wrote during its previous shutdown.
/// Halves the disk round-trips vs the prior split-by-bucket impl
/// and makes the operation atomic — no half-applied state if the
/// custom_filters write blew up after subscriptions had already
/// been written.
///
/// Returns the count of changes applied. Zero when nothing's
/// pending (caller skips logging).
pub fn replay_all_overrides(
    profile_dir: &Path,
    regional: &std::collections::HashMap<String, bool>,
    subscription_ops: &std::collections::HashMap<String, SubAction>,
    custom_filters: Option<&str>,
) -> Result<usize> {
    if regional.is_empty() && subscription_ops.is_empty() && custom_filters.is_none() {
        return Ok(0);
    }
    let mut root = read_or_empty(profile_dir)?;
    let mut applied = 0usize;
    for (uuid, en) in regional {
        set_regional_enabled(&mut root, uuid, *en)?;
        applied += 1;
    }
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
