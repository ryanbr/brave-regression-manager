use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Inner component metadata block used inside Brave's catalog entries.
/// We only care about `component_id` for resolving the on-disk dir.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogComponent {
    #[serde(default)] pub component_id: String,
    #[serde(default)] pub base64_public_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogEntry {
    pub uuid:         String,
    pub title:        String,
    /// Modern catalog wraps the component_id under
    /// `list_text_component.component_id`. We expose it as a flat
    /// `component_id` for the GUI, copied from the inner block on
    /// load via `flatten_components`.
    #[serde(default)] pub component_id: String,
    #[serde(default)] pub list_text_component: CatalogComponent,
    /// Source URL — modern catalog uses `sources: [{ "url": "..." }]`.
    /// Flattened on load.
    #[serde(default)] pub url: String,
    #[serde(default)] pub sources: Vec<CatalogSource>,
    /// Two-letter ISO language codes — Brave uses these to auto-enable
    /// regional lists matching the user's UI language.
    #[serde(default)] pub langs: Vec<String>,
    /// "Standard" / "PlainList" / "Hosts" — Brave's filter format hint.
    #[serde(default)] pub format: String,
    /// True when Brave enables this list by default for matching
    /// languages.
    #[serde(default)] pub default_enabled: bool,
    /// `hidden=true` lists are internals (e.g. the "default" first-
    /// party adblock filters) not surfaced in Brave's UI. We still
    /// show them in our GUI but tag the row visually.
    #[serde(default)] pub hidden: bool,
    #[serde(default)] pub desc: String,
    #[serde(default)] pub support_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogSource {
    #[serde(default)] pub url: String,
    #[serde(default)] pub title: String,
}

impl CatalogEntry {
    /// Lift `list_text_component.component_id` and the first
    /// `sources[].url` into the flat fields the GUI uses, so callers
    /// don't have to deal with the nested catalog shape.
    pub fn flatten(&mut self) {
        if self.component_id.is_empty() && !self.list_text_component.component_id.is_empty() {
            self.component_id = self.list_text_component.component_id.clone();
        }
        if self.url.is_empty() {
            if let Some(s) = self.sources.first() {
                self.url = s.url.clone();
            }
        }
    }
}

pub type Catalog = HashMap<String, CatalogEntry>;

/// Parse Brave's regional adblock catalog. Brave ships either `list_catalog.json`
/// or a similarly-named manifest under the catalog component folder.
pub fn load(component_version_dir: &Path) -> Result<Catalog> {
    for name in ["list_catalog.json", "regional_catalog.json", "catalog.json"] {
        let p = component_version_dir.join(name);
        if p.exists() {
            let s = std::fs::read_to_string(&p)?;
            let entries: Vec<CatalogEntry> = serde_json::from_str(&s).unwrap_or_default();
            return Ok(entries.into_iter().map(|e| (e.uuid.clone(), e)).collect());
        }
    }
    Ok(HashMap::new())
}

/// On-disk cache of the upstream catalog. Survives across sessions so
/// the GUI can show the catalog instantly on startup; refreshed on
/// demand from Brave's `brave/adblock-resources` GitHub repo.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogCache {
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub source_url: String,
    pub entries:    Vec<CatalogEntry>,
}

impl CatalogCache {
    pub fn path() -> std::path::PathBuf {
        crate::paths::data_root().join("cache/regional_catalog.json")
    }
    pub fn load_from_disk() -> Option<Self> {
        let s = std::fs::read_to_string(Self::path()).ok()?;
        serde_json::from_str(&s).ok()
    }
    pub fn save_to_disk(&self) -> Result<()> {
        let p = Self::path();
        if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Copy a component directory tree from one profile to another —
/// used to seed a throwaway dir's component layout from the user's
/// regular profile so list edits made before Brave has run in the
/// throwaway have somewhere to land. Walks the entire UUID-named
/// component tree (`<src>/<component_id>/<version>/...`) into
/// `<dst>/<component_id>/<version>/...`.
pub fn mirror_component_dir(
    src_profile: &Path,
    dst_profile: &Path,
    component_id: &str,
) -> Result<()> {
    let src_root = src_profile.join(component_id);
    if !src_root.is_dir() {
        return Err(anyhow!(
            "no source component dir at {} — selected profile has no \
             record of this component yet (run Brave once with the \
             selected profile first)", src_root.display()));
    }
    let dst_root = dst_profile.join(component_id);
    std::fs::create_dir_all(&dst_root)?;
    for entry in walkdir::WalkDir::new(&src_root) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(&src_root).unwrap_or(entry.path());
        let dst = dst_root.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dst)?;
        } else if entry.file_type().is_file() {
            if let Some(p) = dst.parent() { std::fs::create_dir_all(p)?; }
            std::fs::copy(entry.path(), &dst)?;
        }
    }
    Ok(())
}

/// Enable a regional list by writing
/// `brave.ad_block.regional_filters[<uuid>].enabled = true` into the
/// profile's `Default/Preferences`. Brave reads this on next launch
/// and applies it; no need to fight component-updater or download
/// the source list ourselves — Brave's own component is responsible
/// for delivering the rules.
///
/// Works on profile dirs that don't exist yet (throwaway seeding):
/// we write a minimal Preferences containing only this key, and
/// Brave fills in defaults on first launch.
pub fn enable_list(profile_dir: &Path, uuid: &str) -> Result<std::path::PathBuf> {
    super::prefs_edit::edit_regional_enabled(profile_dir, uuid, true)?;
    Ok(super::prefs_edit::prefs_path(profile_dir))
}

/// Disable a regional list by writing
/// `regional_filters[<uuid>].enabled = false` into Preferences.
/// Same one-shot atomic edit as `enable_list`.
pub fn disable_list(profile_dir: &Path, uuid: &str) -> Result<std::path::PathBuf> {
    super::prefs_edit::edit_regional_enabled(profile_dir, uuid, false)?;
    Ok(super::prefs_edit::prefs_path(profile_dir))
}

/// Brave delivers the regional list catalog as a component on disk.
/// The component ID has shifted across Brave versions:
///   gkboaolpopklhgplhaaiboijnklogmbc — modern (matches boce)
///   gccbbnhkhcdjncjfbknbnepflcabamhf — older / alternate channel
/// We probe both under the profile dir and return the first hit.
const LOCAL_CATALOG_COMPONENT_IDS: &[&str] = &[
    "gkboaolpopklhgplhaaiboijnklogmbc",
    "gccbbnhkhcdjncjfbknbnepflcabamhf",
];

/// Read the catalog from the on-disk component file Brave's already
/// pulled, avoiding the network entirely. Returns None when the
/// component dir hasn't been populated yet (fresh profile that's
/// never launched Brave). Mirrors boce's catalog source.
pub fn load_local_catalog(profile_dir: &Path) -> Option<CatalogCache> {
    for cid in LOCAL_CATALOG_COMPONENT_IDS {
        let ver_dir = super::discover::active_component_path(profile_dir, cid)?;
        let path = ver_dir.join("list_catalog.json");
        if !path.is_file() { continue; }
        let body = std::fs::read_to_string(&path).ok()?;
        let mut entries: Vec<CatalogEntry> = serde_json::from_str(&body).ok()?;
        for e in &mut entries { e.flatten(); }
        return Some(CatalogCache {
            fetched_at: chrono::Utc::now(),
            source_url: format!("local:{}", path.display()),
            entries,
        });
    }
    None
}

/// Fetch the regional adblock catalog from Brave's `adblock-resources`
/// repo. The file is `filter_lists/regional.json` at master; we go via
/// raw.githubusercontent.com so the request doesn't count against the
/// API's 60 req/hr anonymous rate limit (raw.githubusercontent.com
/// uses a separate, far more generous quota).
pub async fn fetch_regional_catalog(token: Option<&str>) -> Result<CatalogCache> {
    // Brave renamed `regional.json` → `list_catalog.json` at some
    // point. Probe the new name first, fall back to the old one if
    // the GitHub repo gets reorganised again.
    let candidates = [
        "https://raw.githubusercontent.com/brave/adblock-resources/master/filter_lists/list_catalog.json",
        "https://raw.githubusercontent.com/brave/adblock-resources/master/filter_lists/regional.json",
    ];
    let client = reqwest::Client::builder()
        .user_agent("brave-regress")
        .build()?;
    let auth_header = token.filter(|t| !t.is_empty())
        .map(|t| format!("Bearer {t}"));
    let mut last_err: Option<String> = None;
    for url in candidates {
        let mut req = client.get(url);
        if let Some(h) = &auth_header {
            req = req.header("Authorization", h);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            last_err = Some(format!("{url}: HTTP {}", resp.status()));
            continue;
        }
        let body = resp.text().await?;
        let mut entries: Vec<CatalogEntry> = serde_json::from_str(&body)
            .map_err(|e| anyhow!("{url} parse: {e}"))?;
        for e in &mut entries { e.flatten(); }
        return Ok(CatalogCache {
            fetched_at: chrono::Utc::now(),
            source_url: url.to_string(),
            entries,
        });
    }
    Err(anyhow!(last_err.unwrap_or_else(||
        "no catalog URL succeeded".into())))
}
