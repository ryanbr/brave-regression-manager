use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use crate::lists::discover::EnabledList;
use crate::versions::install::{DownloadProgress, ProgressSink};
use crate::versions::InstalledVersion;

/// Cap on how many parallel install tasks the GUI lets the user fire
/// off. Three is enough to overlap network + extract waits without
/// thrashing the disk or saturating GitHub's per-connection rate.
pub const MAX_CONCURRENT_INSTALLS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab { Versions, Lists, Console }

/// Which column is driving the Available-list sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailSortColumn {
    Tag,
    Date,
    Channel,
    Verdict,
    Note,
}

/// One-shot slot a background task writes when it finishes; the GUI polls.
pub type AsyncSlot<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Per-channel compare-commit result keyed by the channel string —
/// stored as `Vec<(channel, result)>` so multiple per-channel loads can
/// land in the same frame without clobbering one another.
pub type CompareQueue = Arc<Mutex<Vec<(String, Result<crate::versions::github::CompareResult, String>)>>>;

/// One-shot tag-metadata fetch results, keyed by tag. Multiple in-flight
/// fetches can resolve into the same frame, so we collect results in a
/// `Vec` rather than overwriting a single slot.
pub type TagMetaQueue = Arc<Mutex<Vec<(String, Result<(), String>)>>>;

/// Install completion queue: `(tag, Result<install_path, error>)`.
/// Vec rather than a single slot so up to MAX_CONCURRENT_INSTALLS
/// parallel installs can complete into the same frame.
pub type InstallQueue = Arc<Mutex<Vec<(String, Result<String, String>)>>>;

/// Async results that arrive from background tokio tasks.
#[derive(Debug, Default, Clone)]
pub struct AsyncSlots {
    pub available:        AsyncSlot<Vec<ReleaseRow>>,
    /// Mid-flight partial fetch results. The streaming GitHub fetcher
    /// writes every page's cumulative output here so the GUI can render
    /// progressively instead of waiting for the full set.
    pub partial_releases: Arc<Mutex<Option<Vec<ReleaseRow>>>>,
    /// Queue of completed installs (see `InstallQueue`).
    pub install_done:     InstallQueue,
    pub install_progress: ProgressSink,           // updated live during download
    pub seed_done:        AsyncSlot<()>,
    pub apply_done:       AsyncSlot<()>,
    /// Compare results queue, keyed by the channel the bracket belongs to.
    pub compare_done: CompareQueue,
    /// One-shot per-tag metadata fetch results — used by the Chromium
    /// override row to populate fields for tags outside the current
    /// fetch window.
    pub tag_metadata_done: TagMetaQueue,
    /// One-shot "Add release by tag" result — single ReleaseRow added
    /// to state.available + sqlite when the user manually requests a
    /// specific tag from the GitHub API.
    pub add_by_tag_done: AsyncSlot<ReleaseRow>,
    /// Result of a one-shot fetch of Brave's regional adblock-list
    /// catalog from `brave/adblock-resources` on GitHub. Lands in
    /// state.regional_catalog when the spawn completes.
    pub regional_catalog_done:
        AsyncSlot<crate::lists::catalog::CatalogCache>,
    /// Result of the background startup-cache load (releases.json +
    /// sqlite incremental merge). Populated once shortly after the
    /// first frame so the window paints immediately and the heavy
    /// JSON parse + merge happens on a worker thread.
    pub startup_cache_done: AsyncSlot<(Vec<ReleaseRow>, Option<chrono::DateTime<chrono::Utc>>)>,
}

/// In-flight download snapshot for a specific tag's install. Returns
/// `None` when that tag isn't currently downloading. Per-tag lookup
/// avoids the flicker that the old single-Option sink produced when
/// multiple parallel installs took turns writing it.
pub fn progress_for(slots: &AsyncSlots, tag: &str) -> Option<DownloadProgress> {
    slots.install_progress.lock().unwrap().get(tag).cloned()
}

/// Display row for the GUI's available-releases panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseRow {
    pub tag: String,
    pub published_at: String,
    pub host_asset:  Option<String>,   // None => no installer for current platform
    pub asset_url:   Option<String>,   // direct download URL for the picked asset
    pub asset_size:  Option<u64>,
    pub skip_reason: String,           // empty when host_asset is Some
    /// True when the asset is already downloaded to the cache directory at
    /// the expected size — install can skip the download and go straight to
    /// extract. Computed at fetch time and refreshed after each install.
    #[serde(default)]
    pub cached:      bool,
    /// "Release" / "Beta" / "Nightly" — derived from the release's assets at
    /// fetch time so the GUI can label rows without re-inspecting them.
    #[serde(default)]
    pub channel:     String,
    /// The pinned Chromium version parsed out of the GitHub release title
    /// (e.g. `Release v1.89.145 (Chromium 147.0.7727.137)` → `147.0.7727.137`).
    /// `None` when the title didn't match the expected pattern. Used to
    /// build a `chromium/chromium/compare/<a>...<b>` link for the
    /// per-channel compare panel.
    #[serde(default)]
    pub chromium_version: Option<String>,
}

/// Snapshot of `<data-root>/cache/downloads/` — `(file_name -> size_in_bytes)`.
/// Built once via `read_downloads_index` and reused to refresh `cached`
/// flags for every ReleaseRow without paying a syscall per row. With
/// ~4000 rows the old per-row `fs::metadata` was N stat() calls; this
/// is one read_dir.
pub type DownloadsIndex = std::collections::HashMap<String, u64>;

/// Single read_dir of the downloads cache, returning a name -> size
/// map. Empty / unreadable dir → empty map (every row flips to
/// `cached=false` which is the safe default).
pub fn read_downloads_index() -> DownloadsIndex {
    let mut out = DownloadsIndex::new();
    let dir = crate::paths::downloads_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let (Some(name), Ok(meta)) = (
                entry.file_name().to_str().map(str::to_string),
                entry.metadata(),
            ) {
                if meta.is_file() { out.insert(name, meta.len()); }
            }
        }
    }
    out
}

impl ReleaseRow {
    /// Re-stat the cache directory to refresh `cached` for this row.
    /// Single-row variant — pays an `fs::metadata` syscall. Use
    /// `refresh_cached_with` when refreshing many rows in a row, since
    /// that lets all rows share one `read_dir`.
    pub fn refresh_cached(&mut self) {
        self.cached = match (&self.host_asset, self.asset_size) {
            (Some(name), Some(size)) => {
                let p = crate::paths::downloads_dir().join(name);
                std::fs::metadata(&p).map(|m| m.len() == size).unwrap_or(false)
            }
            _ => false,
        };
    }
    /// Bulk-friendly variant — looks the (name, size) pair up in a
    /// pre-built `DownloadsIndex` instead of doing its own `metadata`
    /// syscall. Caller is responsible for building the index once
    /// before iterating.
    pub fn refresh_cached_with(&mut self, idx: &DownloadsIndex) {
        self.cached = match (&self.host_asset, self.asset_size) {
            (Some(name), Some(size)) => {
                idx.get(name).map(|&s| s == size).unwrap_or(false)
            }
            _ => false,
        };
    }

    /// Best-effort channel inference from the row's host asset name and
    /// tag, used to back-fill rows loaded from older caches that didn't
    /// persist a channel string. Brave's portable `.zip` filenames carry
    /// no channel marker, so unmarked rows stay `?` until the next fetch
    /// re-derives the channel from the full asset list.
    pub fn ensure_channel(&mut self) {
        if !self.channel.is_empty() { return; }
        let probe = format!("{} {}",
            self.host_asset.as_deref().unwrap_or(""), self.tag).to_lowercase();
        self.channel = if probe.contains("nightly") { "Nightly".into() }
            else if probe.contains("beta")          { "Beta".into() }
            else                                    { "?".into() };
    }
}

/// On-disk cache for the available-releases listing so the in-memory
/// `state.available` survives a relaunch — installs can then go direct to
/// S3 from cached URLs without re-querying the GitHub API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCache {
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub rows:       Vec<ReleaseRow>,
}

impl ReleaseCache {
    pub fn load() -> Option<Self> {
        let p = crate::paths::releases_cache_path();
        let s = std::fs::read_to_string(&p).ok()?;
        serde_json::from_str(&s).ok()
    }
    pub fn save(rows: &[ReleaseRow]) -> std::io::Result<()> {
        let p = crate::paths::releases_cache_path();
        if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
        let payload = ReleaseCache {
            fetched_at: chrono::Utc::now(),
            rows: rows.to_vec(),
        };
        let json = serde_json::to_string_pretty(&payload)
            .map_err(std::io::Error::other)?;
        // Atomic-ish write so a crash mid-save doesn't corrupt the cache.
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }
}

pub struct AppState {
    pub tab: Tab,

    // Tab 1: versions
    /// Installed Brave versions. `Arc` so per-frame snapshots
    /// (catalog dropdown, render path) take an O(1) clone instead
    /// of duplicating the whole Vec — same pattern as
    /// `regional_catalog_entries`. Mutate via `Arc::make_mut` /
    /// re-assignment with a fresh Arc.
    pub installed: std::sync::Arc<Vec<InstalledVersion>>,
    /// `Arc<Vec<ReleaseRow>>` rather than a bare `Vec` so the Available
    /// render loop can grab a cheap O(1) refcount-bump snapshot each
    /// frame instead of paying ~20–40k heap allocations to deep-clone
    /// every row. Mutations (push / retain / refresh_cached) go through
    /// `Arc::make_mut` which COW-clones the inner Vec exactly once,
    /// only when needed.
    pub available: std::sync::Arc<Vec<ReleaseRow>>,
    /// In-memory edit buffer for per-version launch args, keyed by tag.
    /// Loaded lazily on first render of each row. Persisted to sqlite on
    /// blur via `verdict::set_launch_args`.
    pub launch_args_buf: HashMap<String, String>,
    pub available_fetched_at: Option<chrono::DateTime<chrono::Utc>>,
    pub hide_no_installer: bool,
    pub release_count: u32,
    pub date_from: Option<chrono::NaiveDate>,
    pub date_to:   Option<chrono::NaiveDate>,
    pub brave_log_level: crate::config::BraveLogLevel,
    pub github_token: String,
    pub freeze_components: bool,
    /// Mirror of `cfg.gui.block_drive_launcher`. When on, the
    /// "Application Launcher for Drive" extension's id is added
    /// to `extensions.install.deny_list` in Default/Preferences
    /// before every launch.
    pub block_drive_launcher: bool,
    pub theme: String,
    pub channel_release: bool,
    pub channel_beta:    bool,
    pub channel_nightly: bool,
    pub default_profile_dir_enabled: bool,
    pub default_profile_dir:         String,
    pub default_args_enabled: bool,
    pub default_args:         String,
    pub clean_profile_per_launch: bool,
    pub reuse_clean_profile: bool,
    /// Session-only memo: when **reuse_clean_profile** is on, the
    /// throwaway dir generated for each tag's first launch is kept
    /// here and reused for every subsequent relaunch of that tag.
    /// Cleared on app restart so new sessions always start fresh.
    pub session_throwaway_dirs: HashMap<String, std::path::PathBuf>,
    /// Cached `(sample_time, target_hash, conflict_present)` from
    /// the panel render path so we don't pay a process-list scan
    /// per frame at 60fps. `target_hash` is a u64 digest of the
    /// target set, so cache validity check is alloc-free per frame
    /// (vs the prior Vec<String> key which built fresh strings
    /// every paint).
    pub brave_running_cache: Option<(std::time::Instant, u64, bool)>,
    /// In-memory record of every list edit the user has made this
    /// session: catalog UUID → enabled. Re-applied to whichever
    /// user-data-dir Brave is about to launch with, so a Brave
    /// "first launch race" (component-updater hasn't loaded the
    /// catalog yet, so the UUID is unrecognised at startup) self-
    /// heals on the next relaunch. Cleared on app restart — the
    /// user re-applies if they want them remembered.
    pub regional_overrides: HashMap<String, bool>,
    /// Cached `verdict::recent_launch_args(50)` result. The
    /// per-row Installed dropdown calls this every paint per row;
    /// caching takes the disk hit from O(rows*frames) to O(1) per
    /// session, with an invalidate on every add/forget/clear so
    /// the dropdown reflects mutations the very next paint.
    pub launch_args_history_cache: Option<std::sync::Arc<Vec<String>>>,
    /// Snapshot of `regional_filters[uuid].enabled` from the
    /// selected profile's Local State. Refreshed on profile change
    /// and after each edit. Used by the catalog grid to show the
    /// effective on/off state per row (when a UUID isn't in the
    /// map, the catalog's `default_enabled` is the effective state).
    pub regional_state_view: HashMap<String, bool>,
    /// Subscription edits this session: URL -> Set(enabled) | Remove.
    /// Replayed before every launch alongside `regional_overrides`.
    pub subscription_overrides: HashMap<String, crate::lists::prefs_edit::SubAction>,
    /// Loaded snapshot of `list_subscriptions` for the selected
    /// profile, refreshed when the profile changes. Used by the
    /// subscriptions panel grid.
    pub subscriptions_view: Vec<crate::lists::prefs_edit::Subscription>,
    /// Buffer for the "+ Add subscription" URL input.
    pub subscription_add_buffer: String,
    /// Custom filter rules: editor buffer + the text we last loaded
    /// from disk (for dirty detection and reset).
    pub custom_filters_original: String,
    pub custom_filters_buffer:   String,
    /// Set when the user has hit Save on custom filters this
    /// session — replayed before every launch.
    pub custom_filters_override: Option<String>,
    pub launch_as_admin: bool,
    pub versions_dir: String,
    /// "versions" / "lists" / "both" — where the Settings panel
    /// renders. Mirrors `cfg.gui.settings_location`.
    pub settings_location: String,
    pub fetching_releases: bool,
    /// Wall-clock at fetch spawn — the success/error drain formats a
    /// "in N.Ns" suffix on the completion line so the user can see how
    /// long the GitHub walk took. Cleared in the drain.
    pub fetching_started: Option<std::time::Instant>,
    /// Tags that have an install task currently in flight. Up to
    /// `MAX_CONCURRENT_INSTALLS` may run at once; the Install button
    /// disables itself once that cap is reached.
    pub installing: HashSet<String>,
    /// Per-tag spawn time for the in-flight installs — the completion
    /// drain reads this to format a "in N.Ns" duration in the
    /// post-install line.
    pub installing_started: HashMap<String, std::time::Instant>,
    /// True while the deferred startup cache load is still in flight.
    /// Suppresses the "(click Fetch GitHub releases to populate)"
    /// empty-state message during the brief window between window-show
    /// and the cache landing in `state.available`.
    pub loading_startup_cache: bool,
    pub selected_tag: Option<String>,

    /// Sort column + direction for the Available list. Session-only —
    /// not persisted; defaults to Date Descending (newest first) which
    /// matches the previous behaviour of "show GitHub's order verbatim".
    pub avail_sort_by:  AvailSortColumn,
    pub avail_sort_asc: bool,
    pub running: HashMap<String, RunningBrave>,

    /// Persisted preferences should be re-saved on next frame.
    pub config_dirty: bool,

    // Tab 2: lists
    pub profiles: Vec<String>,
    pub selected_profile: Option<String>,
    pub lists_for_profile: Vec<EnabledList>,
    /// Cached HashSet of `lists_for_profile[*].component_id` so the
    /// catalog grid's "On disk" lookup is O(1) per row without
    /// rebuilding the set on every paint. `Arc` so render can take
    /// an O(1) clone without contending for an immutable borrow on
    /// `state` (which would block the &mut state spawners). Refreshed
    /// in lockstep with `lists_for_profile`.
    pub installed_component_ids: std::sync::Arc<std::collections::HashSet<String>>,
    pub selected_list: Option<usize>,
    pub seeding: bool,
    pub applying: bool,
    /// Cached regional adblock-list catalog (Brave's
    /// `adblock-resources` regional.json). Populated either from the
    /// on-disk cache at startup or from a fresh fetch. `None` when
    /// nothing has loaded yet.
    pub regional_catalog: Option<crate::lists::catalog::CatalogCache>,
    /// Render-path snapshot of `regional_catalog.entries` wrapped in
    /// an `Arc` so the catalog grid can take an O(1) clone every
    /// frame (vs the previous `Vec<CatalogEntry>::clone` which
    /// reallocated 59 entries' worth of strings every paint). Kept
    /// in lockstep with `regional_catalog` — refresh both together.
    pub regional_catalog_entries: std::sync::Arc<Vec<crate::lists::catalog::CatalogEntry>>,
    /// True while the catalog fetch is in flight.
    pub regional_catalog_loading: bool,

    /// brave-core commit-compare panels, keyed by channel ("Release" /
    /// "Beta" / "Nightly" / "?"). Each channel's GOOD↔BAD bracket gets
    /// its own loaded-commits state so multiple ranges can be inspected
    /// side-by-side.
    pub compare_loading: HashSet<String>,
    pub compare_results: HashMap<String, crate::versions::github::CompareResult>,
    pub compare_errors:  HashMap<String, String>,

    /// User-editable Chromium version override keyed by
    /// `(channel, older_tag, newer_tag)` so a new bracket (e.g. after a
    /// verdict change) always gets a fresh auto-paste from the parsed
    /// pins instead of carrying stale edits from a different range.
    /// Session-only — not persisted; cleared by the "reset" button.
    pub chromium_overrides: HashMap<(String, String, String), (String, String)>,
    /// Tags whose one-shot metadata fetch is in-flight — used to disable
    /// the per-tag "Fetch tag info" button while the request is running.
    pub tag_fetch_pending: HashSet<String>,
    /// Edit buffer for the "Add release by tag" field. Lets the user
    /// pull a specific older release (e.g. `v1.85.99`) in a single API
    /// call without walking pagination back to it.
    pub add_by_tag_buf: String,
    /// True while an Add-by-tag fetch is in flight.
    pub adding_by_tag: bool,
    /// Tags the user explicitly added via the Add-by-tag flow.
    /// Exempted from the channel display filter so a manually-added
    /// Release/Beta tag still shows when only Nightly is ticked.
    pub manual_release_tags: HashSet<String>,
    /// Height in pixels of the Installed-versions panel — adjusted by
    /// dragging the divider beneath it. Session-only (resets on app
    /// restart). `None` falls back to the default 7-row height.
    pub installed_panel_height: Option<f32>,

    /// Per-tag freeform-note editor. `Some(tag)` while the popup is open;
    /// the buffer holds the in-progress edit so it survives repaints.
    pub editing_note_tag: Option<String>,
    pub editing_note_buf: String,

    pub status_msg: String,

    pub rt:      Handle,
    pub slots:   AsyncSlots,
    pub console: crate::console::Handle,
}

pub struct RunningBrave {
    pub tag:     String,
    pub profile: String,
    pub child:   Child,
    pub user_data_dir: PathBuf,
    /// Wall-clock at spawn time. Used by `reap_running` to detect the
    /// "exited within ~5 s" pattern so we can surface a fast-exit hint.
    pub spawned_at: std::time::Instant,
}

impl AppState {
    pub fn new(rt: Handle) -> Self {
        let console = crate::console::new_handle();
        Self {
            console,
            tab: Tab::Versions,
            installed: std::sync::Arc::new(vec![]),
            available: std::sync::Arc::new(vec![]),
            launch_args_buf: HashMap::new(),
            available_fetched_at: None,
            hide_no_installer: true,
            release_count: 50,
            date_from: None,
            date_to:   None,
            brave_log_level: crate::config::BraveLogLevel::Quiet,
            github_token: String::new(),
            freeze_components: false,
            block_drive_launcher: true,
            theme: "dark".into(),
            channel_release: false,
            channel_beta:    false,
            channel_nightly: true,
            default_profile_dir_enabled: false,
            default_profile_dir: String::new(),
            default_args_enabled: false,
            default_args: String::new(),
            clean_profile_per_launch: false,
            reuse_clean_profile: false,
            session_throwaway_dirs: HashMap::new(),
            brave_running_cache: None,
            regional_overrides: HashMap::new(),
            launch_args_history_cache: None,
            regional_state_view: HashMap::new(),
            subscription_overrides: HashMap::new(),
            subscriptions_view: Vec::new(),
            subscription_add_buffer: String::new(),
            custom_filters_original: String::new(),
            custom_filters_buffer: String::new(),
            custom_filters_override: None,
            launch_as_admin: false,
            versions_dir: String::new(),
            settings_location: "versions".into(),
            fetching_releases: false,
            fetching_started: None,
            installing: HashSet::new(),
            installing_started: HashMap::new(),
            loading_startup_cache: false,
            selected_tag: None,
            avail_sort_by:  AvailSortColumn::Date,
            avail_sort_asc: false,
            running: HashMap::new(),
            config_dirty: false,
            profiles: vec![],
            selected_profile: None,
            lists_for_profile: vec![],
            installed_component_ids: std::sync::Arc::new(std::collections::HashSet::new()),
            selected_list: None,
            seeding: false,
            applying: false,
            regional_catalog: None,
            regional_catalog_entries: std::sync::Arc::new(Vec::new()),
            regional_catalog_loading: false,
            compare_loading: HashSet::new(),
            compare_results: HashMap::new(),
            compare_errors:  HashMap::new(),
            chromium_overrides: HashMap::new(),
            tag_fetch_pending: HashSet::new(),
            add_by_tag_buf: String::new(),
            adding_by_tag: false,
            manual_release_tags: HashSet::new(),
            installed_panel_height: None,
            editing_note_tag: None,
            editing_note_buf: String::new(),
            status_msg: String::new(),
            rt,
            slots: AsyncSlots::default(),
        }
    }
}
