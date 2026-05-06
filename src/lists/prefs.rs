//! Read (and eventually write) Brave's adblock list preferences from
//! `<profile>/Default/Preferences`. The keys have shifted across Brave
//! versions; we match against several known shapes and fall back
//! gracefully. **Read-only for now** — write support comes later, with
//! backup-before-write and a refusal to operate while Brave is running.

use std::path::Path;

/// One adblock filter list as Brave records it in Preferences.
/// `enabled` is the whole point; `id` is the Brave component UUID
/// (the same string Brave uses internally), `title` is the
/// human-readable label when Preferences carries one.
#[derive(Debug, Clone)]
pub struct PrefList {
    pub id:      String,
    pub title:   Option<String>,
    pub enabled: bool,
    /// Source bucket the entry was read from — useful for debugging
    /// schema-shape changes ("regional_filters" / "list_catalog" /
    /// "list_subscriptions" / etc).
    pub source:  &'static str,
}

/// Bag of every adblock-related setting we could parse, plus a record
/// of which fallback path was hit so the GUI can complain when nothing
/// matched.
#[derive(Debug, Clone, Default)]
pub struct PrefsReadResult {
    pub lists:           Vec<PrefList>,
    /// Custom subscription URLs — `brave.ad_block.list_subscriptions`
    /// in modern Brave. Stored separately because they don't have a
    /// stable UUID, just a URL.
    pub custom_subs:     Vec<String>,
    /// Sub-keys we successfully resolved, in the order we tried them.
    pub matched_paths:   Vec<&'static str>,
    /// Sub-keys we tried but didn't find — useful when the user runs
    /// against an unusually old or unusually new Brave and our matcher
    /// table is out of date.
    pub missed_paths:    Vec<&'static str>,
    /// Top-level key names we observed under `brave.ad_block` and
    /// `brave.shields` — diagnostic dump for when the schema doesn't
    /// match any known shape, so we can extend the matcher.
    pub probe_keys:      Vec<(String, Vec<String>)>,
}

/// Walk the user-data-dir (one level deep at the root, two levels
/// under Default/) and return relative file paths sorted by mtime
/// descending. Useful when none of the JSON probes turn up the
/// adblock list state — gives the user a diagnostic dump of every
/// file Brave wrote so we can spot non-obvious storage locations
/// (LevelDB dirs, hash-named JSONs, custom DB files, etc).
pub fn list_pref_candidate_files(profile_dir: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out: Vec<(String, u64, std::time::SystemTime)> = Vec::new();
    let interesting_exts = ["json", "ldb", "log", "leveldb", "sqlite", "db", "txt", "dat", "pref"];
    for entry in walkdir::WalkDir::new(profile_dir).max_depth(3).into_iter().flatten() {
        let p = entry.path();
        if !p.is_file() { continue; }
        let name_l = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
        let ext_match = interesting_exts.iter().any(|e| name_l.ends_with(e));
        let name_match = name_l.contains("filter") || name_l.contains("adblock")
            || name_l.contains("shields") || name_l.contains("regional")
            || name_l.contains("subscription") || name_l == "preferences"
            || name_l == "local state" || name_l == "secure preferences";
        if !(ext_match || name_match) { continue; }
        let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
        let rel = p.strip_prefix(profile_dir).unwrap_or(p)
            .to_string_lossy().into_owned();
        out.push((rel, meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

/// Read every JSON pref store Brave keeps in the user-data-dir
/// (`Default/Preferences`, `Default/Secure Preferences`, and the
/// user-data-dir-root `Local State`) and aggregate hits across all
/// three. Modern Brave moved bits of the adblock-list state out of
/// `Default/Preferences` so the older single-file probe wasn't
/// finding anything.
pub fn read_profile_prefs(profile_dir: &Path) -> anyhow::Result<PrefsReadResult> {
    let candidates = [
        ("Preferences",        profile_dir.join("Default/Preferences")),
        ("Secure Preferences", profile_dir.join("Default/Secure Preferences")),
        ("Local State",        profile_dir.join("Local State")),
    ];
    let mut combined = PrefsReadResult::default();
    let mut any_file_found = false;
    for (label, path) in candidates {
        if !path.exists() { continue; }
        any_file_found = true;
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                combined.missed_paths.push(Box::leak(
                    format!("{label}: read err {e}").into_boxed_str()));
                continue;
            }
        };
        let json: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                combined.missed_paths.push(Box::leak(
                    format!("{label}: json err {e}").into_boxed_str()));
                continue;
            }
        };
        let mut part = parse_prefs(&json);
        // Tag every probed key with the file it came from so the user
        // can tell which pref store the entry lives in.
        for (parent, _) in &mut part.probe_keys {
            *parent = format!("[{label}] {parent}");
        }
        combined.lists.append(&mut part.lists);
        combined.custom_subs.append(&mut part.custom_subs);
        combined.matched_paths.extend(part.matched_paths);
        combined.missed_paths.extend(part.missed_paths);
        combined.probe_keys.append(&mut part.probe_keys);
    }
    if !any_file_found {
        // Caller treats a fully-empty result as "Brave hasn't run".
    }
    Ok(combined)
}

/// Pure parsing — separated from disk I/O so we can unit-test it
/// against captured Preferences fixtures without touching the FS.
pub fn parse_prefs(json: &serde_json::Value) -> PrefsReadResult {
    let mut out = PrefsReadResult::default();

    // Tried in priority order. The first hit at each shape kind is
    // accepted. Newer Brave (>= 1.55ish) keeps regional filter state
    // under `brave.ad_block.regional_filters`; some older builds had
    // `brave.ad_block.list_catalog`; even older used
    // `brave.shields.regional_filters`.
    let regional_keys = [
        ("brave.ad_block.regional_filters",  "regional_filters"),
        ("brave.ad_block.list_catalog",      "list_catalog"),
        ("brave.shields.regional_filters",   "shields.regional_filters"),
        ("brave_shields.regional_filters",   "brave_shields.regional_filters"),
        ("brave_shields.regional_lists",     "brave_shields.regional_lists"),
        ("brave_shields.fp_lists",           "brave_shields.fp_lists"),
    ];
    let mut got_regional = false;
    for (jp, label) in regional_keys {
        if let Some(obj) = pointer(json, jp).and_then(|v| v.as_object()) {
            for (uuid, entry) in obj {
                let enabled = entry.get("enabled")
                    .and_then(|v| v.as_bool()).unwrap_or(false);
                let title = entry.get("title").and_then(|v| v.as_str()).map(str::to_string);
                out.lists.push(PrefList {
                    id: uuid.clone(),
                    title,
                    enabled,
                    source: label,
                });
            }
            out.matched_paths.push(label);
            got_regional = true;
            break;
        } else {
            out.missed_paths.push(label);
        }
    }
    let _ = got_regional; // intentionally unused — paths slice records it

    // Custom subscriptions — URL-keyed, schema is more stable.
    let sub_keys = [
        ("brave.ad_block.list_subscriptions",  "list_subscriptions"),
        ("brave.shields.subscriptions",        "shields.subscriptions"),
        ("brave_shields.list_subscriptions",   "brave_shields.list_subscriptions"),
        ("brave_shields.subscriptions",        "brave_shields.subscriptions"),
    ];
    for (jp, label) in sub_keys {
        if let Some(obj) = pointer(json, jp).and_then(|v| v.as_object()) {
            for (url, _entry) in obj {
                out.custom_subs.push(url.clone());
            }
            out.matched_paths.push(label);
            break;
        } else {
            out.missed_paths.push(label);
        }
    }

    // Schema-discovery probe: enumerate sub-keys under common parent
    // namespaces so the user can see which keys actually exist when
    // none of our matchers fire. This is read-only and cheap.
    for parent in [
        "",                                   // top-level keys
        "brave",
        "brave.ad_block",
        "brave.shields",
        "brave.shields.advanced_view",
        // Modern Brave puts adblock list state at the TOP level under
        // `brave_shields` (separate from `brave.shields`). Probe its
        // immediate sub-keys + a few likely list-bucket names.
        "brave_shields",
        "brave_shields.regional_filters",
        "brave_shields.regional_lists",
        "brave_shields.regional",
        "brave_shields.fp_lists",
        "brave_shields.list_subscriptions",
        "brave_shields.subscriptions",
        "brave_shields.ad_block",
        "extensions",                         // Brave shields are partly extensions
        "extensions.settings",
        "components",                         // adblock components manifest
        "component_updater",
    ] {
        let v = if parent.is_empty() { Some(json) } else { pointer(json, parent) };
        if let Some(obj) = v.and_then(|v| v.as_object()) {
            let keys: Vec<String> = obj.keys().take(40).cloned().collect();
            if !keys.is_empty() {
                let label = if parent.is_empty() { "<root>" } else { parent };
                out.probe_keys.push((label.to_string(), keys));
            }
        }
    }

    out
}

/// `serde_json::Value::pointer` uses RFC-6901 syntax (slash-separated)
/// but Brave keys are dotted. Translate.
fn pointer<'a>(v: &'a serde_json::Value, dotted: &str) -> Option<&'a serde_json::Value> {
    let p: String = std::iter::once(String::new())
        .chain(dotted.split('.').map(|s| s.to_string()))
        .collect::<Vec<_>>().join("/");
    v.pointer(&p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_regional_filters() {
        let j: serde_json::Value = serde_json::from_str(r#"{
            "brave": { "ad_block": { "regional_filters": {
                "12345": { "enabled": true,  "title": "EasyList" },
                "67890": { "enabled": false, "title": "Fanboy" }
            }}}
        }"#).unwrap();
        let r = parse_prefs(&j);
        assert_eq!(r.lists.len(), 2);
        assert!(r.matched_paths.contains(&"regional_filters"));
    }
    #[test]
    fn empty_prefs_returns_no_lists() {
        let j: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
        let r = parse_prefs(&j);
        assert!(r.lists.is_empty());
        assert!(!r.missed_paths.is_empty());
    }
}
