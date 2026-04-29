use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use crate::lists::discover::EnabledList;
use crate::versions::install::{DownloadProgress, ProgressSink};
use crate::versions::InstalledVersion;

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

/// Async results that arrive from background tokio tasks.
#[derive(Debug, Default, Clone)]
pub struct AsyncSlots {
    pub available:        AsyncSlot<Vec<ReleaseRow>>,
    /// Mid-flight partial fetch results. The streaming GitHub fetcher
    /// writes every page's cumulative output here so the GUI can render
    /// progressively instead of waiting for the full set.
    pub partial_releases: Arc<Mutex<Option<Vec<ReleaseRow>>>>,
    pub install_done:     AsyncSlot<String>,
    pub install_progress: ProgressSink,           // updated live during download
    pub seed_done:        AsyncSlot<()>,
    pub apply_done:       AsyncSlot<()>,
    /// Compare results queue, keyed by the channel the bracket belongs to.
    pub compare_done: CompareQueue,
}

/// Latest in-flight download snapshot for the current install (if any).
pub fn current_progress(slots: &AsyncSlots) -> Option<DownloadProgress> {
    slots.install_progress.lock().unwrap().clone()
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

impl ReleaseRow {
    /// Re-stat the cache directory to refresh `cached` for this row.
    pub fn refresh_cached(&mut self) {
        self.cached = match (&self.host_asset, self.asset_size) {
            (Some(name), Some(size)) => {
                let p = crate::paths::downloads_dir().join(name);
                std::fs::metadata(&p).map(|m| m.len() == size).unwrap_or(false)
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
    pub installed: Vec<InstalledVersion>,
    pub available: Vec<ReleaseRow>,
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
    pub theme: String,
    pub channel_release: bool,
    pub channel_beta:    bool,
    pub channel_nightly: bool,
    pub default_profile_dir_enabled: bool,
    pub default_profile_dir:         String,
    pub default_args_enabled: bool,
    pub default_args:         String,
    pub fetching_releases: bool,
    pub installing: Option<String>,
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
    pub selected_list: Option<usize>,
    pub seeding: bool,
    pub applying: bool,

    /// brave-core commit-compare panels, keyed by channel ("Release" /
    /// "Beta" / "Nightly" / "?"). Each channel's GOOD↔BAD bracket gets
    /// its own loaded-commits state so multiple ranges can be inspected
    /// side-by-side.
    pub compare_loading: HashSet<String>,
    pub compare_results: HashMap<String, crate::versions::github::CompareResult>,
    pub compare_errors:  HashMap<String, String>,

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
}

impl AppState {
    pub fn new(rt: Handle) -> Self {
        let console = crate::console::new_handle();
        Self {
            console,
            tab: Tab::Versions,
            installed: vec![],
            available: vec![],
            launch_args_buf: HashMap::new(),
            available_fetched_at: None,
            hide_no_installer: true,
            release_count: 50,
            date_from: None,
            date_to:   None,
            brave_log_level: crate::config::BraveLogLevel::Quiet,
            github_token: String::new(),
            freeze_components: false,
            theme: "dark".into(),
            channel_release: false,
            channel_beta:    false,
            channel_nightly: true,
            default_profile_dir_enabled: false,
            default_profile_dir: String::new(),
            default_args_enabled: false,
            default_args: String::new(),
            fetching_releases: false,
            installing: None,
            selected_tag: None,
            avail_sort_by:  AvailSortColumn::Date,
            avail_sort_asc: false,
            running: HashMap::new(),
            config_dirty: false,
            profiles: vec![],
            selected_profile: None,
            lists_for_profile: vec![],
            selected_list: None,
            seeding: false,
            applying: false,
            compare_loading: HashSet::new(),
            compare_results: HashMap::new(),
            compare_errors:  HashMap::new(),
            editing_note_tag: None,
            editing_note_buf: String::new(),
            status_msg: String::new(),
            rt,
            slots: AsyncSlots::default(),
        }
    }
}
