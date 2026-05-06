use eframe::CreationContext;
use egui::Context;
use tokio::runtime::Handle;

use crate::config::Config;
use crate::paths;

/// Apply egui's built-in dark/light visuals based on the saved theme name.
/// The light theme is softened from pure-white toward a muted off-white so
/// it doesn't glare against dark surrounding chrome.
pub fn apply_theme(ctx: &Context, theme: &str) {
    if theme.eq_ignore_ascii_case("light") {
        let mut v = egui::Visuals::light();
        // Panels / window backgrounds — was effectively rgb(248,248,248).
        v.panel_fill        = egui::Color32::from_gray(224);
        v.window_fill       = egui::Color32::from_gray(224);
        // Keep TextEdit / "extreme" backgrounds slightly brighter than the
        // panel so input fields stand out.
        v.extreme_bg_color  = egui::Color32::from_gray(236);
        // Used for alternating-row striping and similar subtle banding.
        v.faint_bg_color    = egui::Color32::from_gray(214);
        // Noninteractive widget backgrounds (separators, frames) follow
        // the panel tone so they don't show as bright bands.
        v.widgets.noninteractive.bg_fill      = egui::Color32::from_gray(224);
        v.widgets.noninteractive.weak_bg_fill = egui::Color32::from_gray(224);
        ctx.set_visuals(v);
    } else {
        ctx.set_visuals(egui::Visuals::dark());
    }
}

/// Render dim/secondary text. egui's default `ui.weak()` blends text
/// against the window fill, which on the light theme produces a near-
/// invisible pale-grey on white. This helper uses a darker grey in light
/// mode (legible on white) while preserving the default fade in dark mode.
pub fn weak_label(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    let visuals = &ui.ctx().style().visuals;
    let color = if visuals.dark_mode {
        visuals.weak_text_color()
    } else {
        // Solid mid-grey — readable on white without screaming for attention.
        egui::Color32::from_gray(95)
    };
    ui.colored_label(color, text.into())
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    if s.is_empty() { return None; }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn clamp_loaded_date(d: chrono::NaiveDate) -> chrono::NaiveDate {
    let min   = chrono::NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
    let today = chrono::Utc::now().date_naive();
    d.max(min).min(today)
}

use super::state::{AppState, ReleaseCache, Tab};
use super::{console_panel, tab_lists, tab_versions};

use crate::console;

pub struct App {
    state: AppState,
    /// Set the first time `update` runs. We use it to push a one-shot
    /// `InnerSize` viewport command that forces our default window size
    /// regardless of any leftover state in the OS / window manager.
    initial_size_applied: bool,
}

/// Pattern-match GitHub-fetch failure messages and return a short
/// actionable hint when we recognise one. Same shape as the install /
/// launch hint helpers — purely additive context, the raw error
/// stays visible either way.
fn fetch_failure_hint(raw: &str) -> Option<&'static str> {
    let lc = raw.to_lowercase();
    if lc.contains("403") || lc.contains("rate limit") {
        return Some("GitHub rate limit hit. Paste a personal access \
                     token in Settings → GitHub token (no scopes \
                     needed) to bump the anonymous 60 req/hr cap to \
                     5000 req/hr.");
    }
    if lc.contains("404") {
        return Some("repository or release path not found — likely a \
                     transient GitHub issue, retry shortly.");
    }
    if lc.contains("503") || lc.contains("502") || lc.contains("504") {
        return Some("transient GitHub outage. Retry shortly.");
    }
    if lc.contains("dns")
        || lc.contains("connection refused")
        || lc.contains("network is unreachable")
        || lc.contains("os error 11001") /* WSAHOST_NOT_FOUND */
    {
        return Some("network unreachable. Check connectivity and any \
                     firewall / VPN that might be blocking api.github.com.");
    }
    if lc.contains("certificate") || lc.contains("tls") {
        return Some("TLS handshake failed. Check the system clock \
                     (out-of-sync clocks reject GitHub's cert) and \
                     any corporate-proxy MITM cert handling.");
    }
    None
}

/// Pattern-match install-failure messages and return a short
/// actionable hint when we recognise one. Returns None otherwise —
/// the raw error stays visible either way.
fn install_failure_hint(raw: &str) -> Option<&'static str> {
    let lc = raw.to_lowercase();
    if lc.contains("403") {
        return Some("GitHub rate limit. Paste a personal access token \
                     in Settings → GitHub token to bump the anonymous \
                     60 req/hr cap to 5000 req/hr.");
    }
    if lc.contains("404") {
        return Some("asset URL stale (release was edited / asset \
                     re-uploaded). Click 'Fetch GitHub releases' to \
                     refresh the cached URL, then re-install.");
    }
    if lc.contains("503") || lc.contains("502") || lc.contains("504") {
        return Some("transient GitHub / S3 outage. Retry shortly — \
                     downloads resume from the .part file via HTTP Range.");
    }
    if lc.contains("os error 28") || lc.contains("no space left") {
        return Some("disk full. Free up space under <data-root>/cache/ \
                     or <data-root>/versions/ (Clear → Remove Cached \
                     files wipes downloaded archives).");
    }
    if lc.contains("os error 5") || lc.contains("permission denied") {
        return Some("permission denied writing to the install or cache \
                     dir. Check that <data-root> isn't read-only and \
                     that an antivirus isn't holding open files.");
    }
    if lc.contains("brave binary not found") {
        return Some("the asset extracted but didn't yield a brave \
                     executable. The asset may be a debug-symbols \
                     archive or a wrong-arch package — re-fetch GitHub \
                     releases and try again.");
    }
    None
}

impl App {
    pub fn new(_cc: &CreationContext<'_>, rt: Handle) -> Self {
        let _ = paths::ensure_dirs();
        let cfg = Config::load_or_default(&paths::config_path()).unwrap_or_default();

        let mut state = AppState::new(rt);
        state.release_count     = cfg.gui.release_count.clamp(50, 2000);
        state.hide_no_installer = cfg.gui.hide_no_installer;
        // Clamp persisted dates to the [2021-01-01, today] window so an
        // out-of-range value (saved by an older build, hand-edited config)
        // can't make the date picker silently no-op.
        state.date_from = parse_date(&cfg.gui.date_from).map(clamp_loaded_date);
        state.date_to   = parse_date(&cfg.gui.date_to).map(clamp_loaded_date);
        state.brave_log_level = cfg.gui.brave_log_level;
        state.github_token    = cfg.gui.github_token.clone();
        state.freeze_components = cfg.gui.freeze_components;
        state.theme = cfg.gui.theme.clone();
        state.channel_release = cfg.gui.channel_release;
        state.channel_beta    = cfg.gui.channel_beta;
        state.channel_nightly = cfg.gui.channel_nightly;
        state.default_profile_dir_enabled = cfg.gui.default_profile_dir_enabled;
        state.default_profile_dir         = cfg.gui.default_profile_dir.clone();
        state.default_args_enabled        = cfg.gui.default_args_enabled;
        state.default_args                = cfg.gui.default_args.clone();
        state.clean_profile_per_launch    = cfg.gui.clean_profile_per_launch;
        state.reuse_clean_profile         = cfg.gui.reuse_clean_profile;
        // incremental_release_cache used to be a Settings toggle —
        // now always-on; the field stays in config.toml for backward
        // compat with already-written configs but is ignored.
        let _ = cfg.gui.incremental_release_cache;
        state.launch_as_admin             = cfg.gui.launch_as_admin;
        state.versions_dir                = cfg.gui.versions_dir.clone();
        state.settings_location           = cfg.gui.settings_location.clone();
        // Tolerate hand-edited config typos by snapping to a default.
        if !matches!(state.settings_location.as_str(), "versions" | "lists" | "both") {
            state.settings_location = "versions".into();
        }
        // Wire the override into paths::versions_dir() *before* any
        // disk work happens (cache load reads version_dir() to refresh
        // `cached` flags). Empty value keeps the default.
        if !state.versions_dir.is_empty() {
            crate::paths::set_versions_dir_override(
                std::path::PathBuf::from(&state.versions_dir));
        }
        // Guard against an all-off persisted state — default back to Nightly.
        if !state.channel_release && !state.channel_beta && !state.channel_nightly {
            state.channel_nightly = true;
        }
        state.installed = crate::versions::list_installed().unwrap_or_default();
        state.manual_release_tags = crate::verdict::manual_release_tags();
        // Pull the regional-adblock-list catalog off disk if we have
        // a cached copy. Fresh fetch happens on user demand via the
        // Adblock Lists tab's catalog panel.
        state.regional_catalog =
            crate::lists::catalog::CatalogCache::load_from_disk();

        // Single-line settings summary at startup — confirms what got
        // loaded so the user can sanity-check the persisted config.
        // GitHub token is masked: we report present/absent + length
        // (never the value) so the line is safe to share when
        // troubleshooting.
        let chans = {
            let mut v: Vec<&str> = Vec::new();
            if state.channel_release { v.push("Release"); }
            if state.channel_beta    { v.push("Beta"); }
            if state.channel_nightly { v.push("Nightly"); }
            v.join("+")
        };
        let token_str = if state.github_token.is_empty() { "absent".to_string() }
            else { format!("present ({} chars)", state.github_token.len()) };
        let date_filter = format!("{}..{}",
            state.date_from.map(|d| d.to_string()).unwrap_or_else(|| "(none)".into()),
            state.date_to  .map(|d| d.to_string()).unwrap_or_else(|| "(none)".into()));
        let prof_dir = if state.default_profile_dir_enabled
            && !state.default_profile_dir.is_empty()
        { format!("on ({})", state.default_profile_dir) }
        else if state.default_profile_dir_enabled { "on (empty)".to_string() }
        else { "off".to_string() };
        let def_args = if state.default_args_enabled
            && !state.default_args.is_empty()
        { format!("on ({})", state.default_args) }
        else if state.default_args_enabled { "on (empty)".to_string() }
        else { "off".to_string() };
        let versions_dir_str = if state.versions_dir.is_empty() {
            format!("default ({})", crate::paths::versions_dir().display())
        } else { state.versions_dir.clone() };
        console::info(&state.console, "settings", format!(
            "theme={}  channels={}  release_count={}  date={}  \
             log_level={:?}  freeze_components={}  \
             versions_dir={}  default_profile_folder={}  default_args={}  \
             clean_profile_per_launch={}  reuse_clean_profile={}  \
             launch_as_admin={}  github_token={}  settings_location={}",
            state.theme, chans, state.release_count, date_filter,
            state.brave_log_level, state.freeze_components,
            versions_dir_str, prof_dir, def_args,
            state.clean_profile_per_launch, state.reuse_clean_profile,
            state.launch_as_admin, token_str, state.settings_location));
        // Defer the heavy startup work — releases.json read + JSON
        // parse + (incremental) sqlite merge — to a background tokio
        // task so the window paints immediately. Drain block in
        // drain_async_results below picks up the result and populates
        // state.available on the next frame.
        state.loading_startup_cache = true;
        let slot = state.slots.startup_cache_done.clone();
        let console_for_purge = state.console.clone();
        state.rt.spawn(async move {
            // Best-effort purge of throwaway-* profile dirs older than
            // 24 h before the cache load. They're single-use, never
            // picked manually, and accumulate fast when Clean profile
            // per launch is enabled. Logs a one-line summary when at
            // least one was removed.
            let (purged, freed) = crate::profile::purge_stale_throwaways(
                std::time::Duration::from_secs(24 * 60 * 60));
            if purged > 0 {
                let mb = freed as f64 / 1_048_576.0;
                crate::console::info(&console_for_purge, "profile",
                    format!("purged {purged} stale throwaway profile(s), \
                             freed {mb:.1} MiB"));
            }
            let mut payload: (Vec<super::state::ReleaseRow>,
                              Option<chrono::DateTime<chrono::Utc>>)
                = (Vec::new(), None);
            if let Some(cache) = ReleaseCache::load() {
                let dl_idx = super::state::read_downloads_index();
                let mut rows = cache.rows;
                for r in &mut rows { r.refresh_cached_with(&dl_idx); r.ensure_channel(); }
                // Always-on incremental cache — union with everything
                // sqlite has ever seen so the GUI starts up with the
                // full known history (releases.json holds the last
                // fetch's window only).
                use std::collections::HashMap;
                let mut by_tag: HashMap<String, super::state::ReleaseRow> =
                    rows.into_iter().map(|r| (r.tag.clone(), r)).collect();
                for json in crate::verdict::all_release_cache_rows() {
                    if let Ok(mut r) = serde_json::from_str::<super::state::ReleaseRow>(&json) {
                        r.refresh_cached_with(&dl_idx);
                        r.ensure_channel();
                        by_tag.entry(r.tag.clone()).or_insert(r);
                    }
                }
                rows = by_tag.into_values().collect();
                rows.sort_by(|a, b| b.published_at.cmp(&a.published_at));
                payload = (rows, Some(cache.fetched_at));
            }
            *slot.lock().unwrap() = Some(Ok(payload));
        });
        state.profiles  = crate::profile::list().unwrap_or_default()
            .into_iter().map(|p| p.name).collect();
        Self { state, initial_size_applied: false }
    }

    fn maybe_persist_settings(&mut self) {
        if !self.state.config_dirty { return; }
        let mut cfg = Config::load_or_default(&paths::config_path()).unwrap_or_default();
        cfg.gui.release_count     = self.state.release_count;
        cfg.gui.hide_no_installer = self.state.hide_no_installer;
        cfg.gui.date_from = self.state.date_from.map(|d| d.to_string()).unwrap_or_default();
        cfg.gui.date_to   = self.state.date_to.map(|d| d.to_string()).unwrap_or_default();
        cfg.gui.brave_log_level = self.state.brave_log_level;
        cfg.gui.github_token    = self.state.github_token.clone();
        cfg.gui.freeze_components = self.state.freeze_components;
        cfg.gui.theme = self.state.theme.clone();
        cfg.gui.channel_release = self.state.channel_release;
        cfg.gui.channel_beta    = self.state.channel_beta;
        cfg.gui.channel_nightly = self.state.channel_nightly;
        cfg.gui.default_profile_dir_enabled = self.state.default_profile_dir_enabled;
        cfg.gui.default_profile_dir         = self.state.default_profile_dir.clone();
        cfg.gui.default_args_enabled        = self.state.default_args_enabled;
        cfg.gui.default_args                = self.state.default_args.clone();
        cfg.gui.clean_profile_per_launch    = self.state.clean_profile_per_launch;
        cfg.gui.reuse_clean_profile         = self.state.reuse_clean_profile;
        // incremental_release_cache is always-on now; keep writing
        // `true` so older builds reading the same config still see
        // the expected default.
        cfg.gui.incremental_release_cache   = true;
        cfg.gui.launch_as_admin             = self.state.launch_as_admin;
        cfg.gui.versions_dir                = self.state.versions_dir.clone();
        cfg.gui.settings_location           = self.state.settings_location.clone();
        if let Err(e) = cfg.save(&paths::config_path()) {
            self.state.status_msg = format!("settings save failed: {e}");
        }
        self.state.config_dirty = false;
    }

    /// Poll every tracked child non-blockingly. When Brave exits on its own
    /// (user closed the window, crash, etc.), drop the entry so the GUI
    /// stops showing "Stop" / "running" for it.
    /// Diagnostic dump of a Brave profile's adblock-related state —
    /// emitted when a launched Brave exits so the user can see whether
    /// their custom filter list survived the close.
    fn probe_profile_persistence(&self, tag: &str, dir: &std::path::Path) {
        let prefs = dir.join("Default/Preferences");
        if !prefs.exists() {
            console::warn(&self.state.console, "profile", format!(
                "{tag}: Default/Preferences missing under {} — Brave didn't \
                 fully initialise this profile, or wrote it elsewhere",
                dir.display()));
            return;
        }
        let body = match std::fs::read_to_string(&prefs) {
            Ok(s) => s,
            Err(e) => {
                console::warn(&self.state.console, "profile",
                    format!("{tag}: couldn't read Default/Preferences: {e:#}"));
                return;
            }
        };
        let size = body.len();
        // Look for tell-tale strings that indicate the user touched
        // brave://settings/shields/filters during the session. Brave
        // stores custom-filter URLs in `brave.shields.fp_*` /
        // `brave.shields.regional_filters` / `brave.ad_block_*`
        // depending on version — match loosely.
        let has_custom_filters = body.contains("custom_filters")
            || body.contains("regional_filters")
            || body.contains("custom_subscriptions");
        let last_modified = std::fs::metadata(&prefs).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .map(|s| chrono::DateTime::<chrono::Utc>::from_timestamp(s as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_default())
            .unwrap_or_default();
        console::info(&self.state.console, "profile", format!(
            "{tag}: Default/Preferences {} bytes, mtime={}, \
             custom_filters_keys={}",
            size, last_modified, has_custom_filters));
    }

    fn reap_running(&mut self) {
        // Capture (tag, exit_status, time-since-spawn) so the Console
        // line can include both the duration the process was alive and
        // its exit code — useful when a launch behaves oddly without
        // having to guess from a bare "exited" message.
        let now = std::time::Instant::now();
        let dead: Vec<(String, Option<std::process::ExitStatus>, Option<std::time::Duration>)> =
            self.state.running.iter_mut()
                .filter_map(|(tag, r)| match r.child.try_wait() {
                    Ok(Some(s)) => Some((tag.clone(), Some(s),
                        Some(now.saturating_duration_since(r.spawned_at)))),
                    Ok(None)    => None,
                    Err(_)      => Some((tag.clone(), None, None)),
                })
                .collect();
        for (tag, status, age) in dead {
            if let Some(r) = self.state.running.remove(&tag) {
                let code = status.and_then(|s| s.code())
                    .map(|c| format!(" (exit code {c})"))
                    .unwrap_or_default();
                let dur = age.map(|d| format!(" after {:.1}s", d.as_secs_f64()))
                    .unwrap_or_default();
                console::info(&self.state.console, "launch",
                    format!("{tag} exited{dur}{code}"));
                // Probe the profile dir Brave just closed to confirm
                // whether prefs (incl. custom adblock filter lists)
                // actually flushed to disk.
                self.probe_profile_persistence(&tag, &r.user_data_dir);
            }
        }
    }

    fn drain_async_results(&mut self) {
        // Deferred startup-cache load result — first frame paints
        // immediately, this populates state.available a tick or two
        // later. Also fires a refetch when any cached row has a
        // missing/`?` channel marker (legacy caches from before the
        // channel column existed).
        let cache_taken = self.state.slots.startup_cache_done.lock().unwrap().take();
        if let Some(res) = cache_taken {
            self.state.loading_startup_cache = false;
            if let Ok((rows, fetched_at)) = res {
                let needs_refetch = rows.iter()
                    .any(|r| r.channel == "?" || r.channel.is_empty());
                self.state.available = std::sync::Arc::new(rows);
                self.state.available_fetched_at = fetched_at;
                if needs_refetch { tab_versions::spawn_fetch(&mut self.state); }
            }
        }
        // Mid-flight partial fetch results — stream into the available
        // list as each page lands so the UI shows progress instead of a
        // blank "fetching…" wait.
        if let Some(partial) = self.state.slots.partial_releases.lock().unwrap().take() {
            self.state.available = std::sync::Arc::new(partial);
            // Persisting between every page would thrash the cache file;
            // wait for the final result before saving.
        }
        if let Some(res) = self.state.slots.available.lock().unwrap().take() {
            self.state.fetching_releases = false;
            match res {
                Ok(mut rows) => {
                    // The fetch only returned NEW rows (it broke
                    // pagination as soon as it hit a known tag). Merge
                    // with everything already in the sqlite release_cache
                    // so state.available reflects the full history, not
                    // just the delta. Without this, a post-incremental
                    // render would shrink to "today's few new tags".
                    use std::collections::HashMap;
                    let mut by_tag: HashMap<String, super::state::ReleaseRow> =
                        rows.into_iter().map(|r| (r.tag.clone(), r)).collect();
                    for json in crate::verdict::all_release_cache_rows() {
                        if let Ok(r) = serde_json::from_str::<super::state::ReleaseRow>(&json) {
                            by_tag.entry(r.tag.clone()).or_insert(r);
                        }
                    }
                    rows = by_tag.into_values().collect();
                    rows.sort_by(|a, b| b.published_at.cmp(&a.published_at));
                    let installable = rows.iter().filter(|r| r.host_asset.is_some()).count();
                    let elapsed = self.state.fetching_started.take()
                        .map(|t| format!(" in {:.1}s", t.elapsed().as_secs_f64()))
                        .unwrap_or_default();
                    let msg = format!("fetched {} tags{elapsed} ({installable} installable on this platform)",
                        rows.len());
                    console::info(&self.state.console, "github", &msg);
                    self.state.status_msg = msg;
                    if let Err(e) = ReleaseCache::save(&rows) {
                        console::warn(&self.state.console, "cache",
                            format!("could not persist releases cache: {e}"));
                    }
                    self.state.available = std::sync::Arc::new(rows);
                    self.state.available_fetched_at = Some(chrono::Utc::now());
                }
                Err(e) => {
                    let elapsed = self.state.fetching_started.take()
                        .map(|t| format!(" after {:.1}s", t.elapsed().as_secs_f64()))
                        .unwrap_or_default();
                    let hint = fetch_failure_hint(&e);
                    let msg = match hint {
                        Some(h) => format!("{e}\nhint: {h}"),
                        None    => e.clone(),
                    };
                    console::error(&self.state.console, "github", &msg);
                    self.state.status_msg = format!("github error{elapsed}: {e}");
                }
            }
        }
        let install_drained: Vec<(String, Result<String, String>)> = std::mem::take(
            &mut *self.state.slots.install_done.lock().unwrap());
        for (tag, res) in install_drained {
            let elapsed = self.state.installing_started.remove(&tag)
                .map(|t| format!(" in {:.1}s", t.elapsed().as_secs_f64()))
                .unwrap_or_default();
            self.state.installing.remove(&tag);
            match res {
                Ok(p)  => {
                    let msg = format!("{tag} installed{elapsed} → {p}");
                    console::info(&self.state.console, "install", &msg);
                    self.state.status_msg = msg;
                    self.state.installed = crate::versions::list_installed().unwrap_or_default();
                    let dl_idx = super::state::read_downloads_index();
                    for r in std::sync::Arc::make_mut(&mut self.state.available).iter_mut() {
                        r.refresh_cached_with(&dl_idx);
                    }
                }
                Err(e) => {
                    let hint = install_failure_hint(&e);
                    let msg = match hint {
                        Some(h) => format!("{tag}: {e}\nhint: {h}"),
                        None    => format!("{tag}: {e}"),
                    };
                    console::error(&self.state.console, "install", &msg);
                    self.state.status_msg = format!("{tag} install failed{elapsed}: {e}");
                }
            }
        }
        if let Some(res) = self.state.slots.seed_done.lock().unwrap().take() {
            self.state.seeding = false;
            match res {
                Ok(()) => {
                    console::info(&self.state.console, "seed", "lists seeded");
                    self.state.status_msg = "seeded".into();
                    if let Some(p) = &self.state.selected_profile {
                        self.state.lists_for_profile = crate::lists::discover::enabled_lists(
                            &crate::paths::profile_dir(p)).unwrap_or_default();
                    }
                }
                Err(e) => {
                    console::error(&self.state.console, "seed", &e);
                    self.state.status_msg = format!("seed failed: {e}");
                }
            }
        }
        let drained: Vec<_> = std::mem::take(&mut *self.state.slots.compare_done.lock().unwrap());
        for (channel, res) in drained {
            self.state.compare_loading.remove(&channel);
            match res {
                Ok(cr) => {
                    let count = cr.commits.len();
                    let total = cr.total;
                    console::info(&self.state.console, "compare",
                        format!("[{channel}] loaded {count} commits ({total} total) {}..{}",
                                cr.base, cr.head));
                    self.state.compare_errors.remove(&channel);
                    self.state.compare_results.insert(channel, cr);
                }
                Err(e) => {
                    let line = match fetch_failure_hint(&e) {
                        Some(h) => format!("[{channel}] {e}\nhint: {h}"),
                        None    => format!("[{channel}] {e}"),
                    };
                    console::error(&self.state.console, "compare", &line);
                    self.state.compare_results.remove(&channel);
                    self.state.compare_errors.insert(channel, e);
                }
            }
        }
        let drained: Vec<_> = std::mem::take(
            &mut *self.state.slots.list_action_done.lock().unwrap());
        for (component_id, res) in drained {
            self.state.list_action_pending.remove(&component_id);
            // Refresh the seeded-lists view so the on-disk-checked
            // ✓ column flips to match.
            if let Some(p) = &self.state.selected_profile {
                self.state.lists_for_profile = crate::lists::discover::enabled_lists(
                    &crate::paths::profile_dir(p)).unwrap_or_default();
            }
            match res {
                Ok(summary) => {
                    console::info(&self.state.console, "list-edit", &summary);
                    self.state.status_msg = summary;
                }
                Err(e) => {
                    console::error(&self.state.console, "list-edit", &e);
                    self.state.status_msg = format!("list edit failed: {e}");
                }
            }
        }
        if let Some(res) = self.state.slots.regional_catalog_done.lock().unwrap().take() {
            self.state.regional_catalog_loading = false;
            match res {
                Ok(cache) => {
                    let n = cache.entries.len();
                    if let Err(e) = cache.save_to_disk() {
                        console::warn(&self.state.console, "catalog",
                            format!("could not persist regional catalog: {e:#}"));
                    }
                    console::info(&self.state.console, "catalog",
                        format!("fetched regional catalog: {n} list(s) from {}",
                            cache.source_url));
                    self.state.regional_catalog = Some(cache);
                }
                Err(e) => {
                    let hint = fetch_failure_hint(&e);
                    let line = match hint {
                        Some(h) => format!("{e}\nhint: {h}"),
                        None    => e.clone(),
                    };
                    console::error(&self.state.console, "catalog", &line);
                    self.state.status_msg = format!("catalog fetch failed: {e}");
                }
            }
        }
        if let Some(res) = self.state.slots.add_by_tag_done.lock().unwrap().take() {
            self.state.adding_by_tag = false;
            match res {
                Ok(row) => {
                    let tag = row.tag.clone();
                    let av = std::sync::Arc::make_mut(&mut self.state.available);
                    av.retain(|r| r.tag != tag);
                    av.push(row);
                    av.sort_by(|a, b| b.published_at.cmp(&a.published_at));
                    // Mark this tag as user-added so the per-row channel
                    // filter exempts it — adding a Release tag while the
                    // GUI shows Nightly only should still display it.
                    self.state.manual_release_tags.insert(tag.clone());
                    let _ = crate::verdict::mark_manual_release(&tag);
                    console::info(&self.state.console, "github",
                        format!("added release by tag: {tag}"));
                    self.state.status_msg = format!("added {tag}");
                }
                Err(e) => {
                    let line = match fetch_failure_hint(&e) {
                        Some(h) => format!("add by tag failed: {e}\nhint: {h}"),
                        None    => format!("add by tag failed: {e}"),
                    };
                    console::error(&self.state.console, "github", &line);
                    self.state.status_msg = format!("add by tag failed: {e}");
                }
            }
        }
        let tag_drained: Vec<_> = std::mem::take(
            &mut *self.state.slots.tag_metadata_done.lock().unwrap());
        for (tag, res) in tag_drained {
            self.state.tag_fetch_pending.remove(&tag);
            match res {
                Ok(()) => {
                    console::info(&self.state.console, "tag-meta",
                        format!("fetched metadata for {tag}"));
                }
                Err(e) => {
                    let line = match fetch_failure_hint(&e) {
                        Some(h) => format!("[{tag}] {e}\nhint: {h}"),
                        None    => format!("[{tag}] {e}"),
                    };
                    console::error(&self.state.console, "tag-meta", &line);
                }
            }
        }
        if let Some(res) = self.state.slots.apply_done.lock().unwrap().take() {
            self.state.applying = false;
            match res {
                Ok(()) => {
                    console::info(&self.state.console, "apply", "applied & relaunched");
                    self.state.status_msg = "applied & relaunched".into();
                }
                Err(e) => {
                    console::error(&self.state.console, "apply", &e);
                    self.state.status_msg = format!("apply failed: {e}");
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Hard-reset window size on the very first paint. Without this, a
        // stale window state from the OS / window manager (or a previous
        // build that had eframe persistence enabled) can override our
        // default and produce a mis-sized window.
        if !self.initial_size_applied {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                egui::Vec2::new(1000.0, 720.0)));
            // Apply the persisted theme on first paint (and any time
            // `state.theme` changes via `apply_theme()` below).
            apply_theme(ctx, &self.state.theme);
            self.initial_size_applied = true;
        }

        self.drain_async_results();
        self.reap_running();
        self.maybe_persist_settings();

        // While background work is in flight, keep repainting to surface the
        // result the moment it lands. While downloading specifically, repaint
        // faster so the progress bar / speed look live (every ~100ms).
        if !self.state.installing.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.state.fetching_releases || self.state.seeding || self.state.applying
            || !self.state.compare_loading.is_empty()
            || !self.state.tag_fetch_pending.is_empty()
            || self.state.loading_startup_cache
            || self.state.regional_catalog_loading
            || !self.state.list_action_pending.is_empty()
            || self.state.adding_by_tag
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        } else if !self.state.running.is_empty() {
            // Poll for externally-closed Brave windows ~twice a second so
            // the Stop button disappears without needing user input.
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.state.tab, Tab::Versions, "Brave Versions");
                ui.selectable_value(&mut self.state.tab, Tab::Lists,    "Adblock Lists");
                let console_count = self.state.console.lock().map(|g| g.len()).unwrap_or(0);
                let console_label = if console_count == 0 { "Console".to_string() }
                                    else { format!("Console ({console_count})") };
                ui.selectable_value(&mut self.state.tab, Tab::Console, console_label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(if self.state.running.is_empty() {
                        "Brave: idle".to_string()
                    } else {
                        format!("Brave: {} running", self.state.running.len())
                    });
                });
            });
        });

        // Always-present status bar (toggling its existence between frames
        // caused layout reflow / breakage). Tight frame so it's slim when
        // empty rather than reserving ~20px of egui-default padding.
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::none()
                .inner_margin(egui::Margin::symmetric(6.0, 2.0)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.label(&self.state.status_msg);
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.state.tab {
            Tab::Versions => tab_versions::ui(ui, &mut self.state),
            Tab::Lists    => tab_lists::ui(ui, &mut self.state),
            Tab::Console  => console_panel::ui(ui, &mut self.state),
        });
    }
}
