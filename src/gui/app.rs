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
        // Guard against an all-off persisted state — default back to Nightly.
        if !state.channel_release && !state.channel_beta && !state.channel_nightly {
            state.channel_nightly = true;
        }
        state.installed = crate::versions::list_installed().unwrap_or_default();
        // Restore the on-disk releases cache so installs can go direct to S3
        // immediately on launch without re-querying the GitHub API.
        if let Some(cache) = ReleaseCache::load() {
            // Recompute `cached` at startup — the .zip / .exe in the
            // downloads dir might have been removed (or, more usefully,
            // appeared) since the cache was written.
            let mut rows = cache.rows;
            for r in &mut rows { r.refresh_cached(); r.ensure_channel(); }
            // If the cache predates channel persistence (or held marker-less
            // zips), some rows now read `?`. Fire a single background re-fetch
            // so the live `detect_release_channel` can fill them in from the
            // full asset list. Skips when everything's already labelled.
            let needs_refetch = rows.iter().any(|r| r.channel == "?" || r.channel.is_empty());
            state.available = rows;
            state.available_fetched_at = Some(cache.fetched_at);
            if needs_refetch { tab_versions::spawn_fetch(&mut state); }
        }
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
        if let Err(e) = cfg.save(&paths::config_path()) {
            self.state.status_msg = format!("settings save failed: {e}");
        }
        self.state.config_dirty = false;
    }

    /// Poll every tracked child non-blockingly. When Brave exits on its own
    /// (user closed the window, crash, etc.), drop the entry so the GUI
    /// stops showing "Stop" / "running" for it.
    fn reap_running(&mut self) {
        let dead: Vec<String> = self.state.running.iter_mut()
            .filter_map(|(tag, r)| match r.child.try_wait() {
                Ok(Some(_status)) => Some(tag.clone()),
                Ok(None)          => None,        // still running
                Err(_)             => Some(tag.clone()), // can't poll → forget it
            })
            .collect();
        for tag in dead {
            if let Some(_r) = self.state.running.remove(&tag) {
                console::info(&self.state.console, "launch",
                    format!("{tag} exited"));
            }
        }
    }

    fn drain_async_results(&mut self) {
        // Mid-flight partial fetch results — stream into the available
        // list as each page lands so the UI shows progress instead of a
        // blank "fetching…" wait.
        if let Some(partial) = self.state.slots.partial_releases.lock().unwrap().take() {
            self.state.available = partial;
            // Persisting between every page would thrash the cache file;
            // wait for the final result before saving.
        }
        if let Some(res) = self.state.slots.available.lock().unwrap().take() {
            self.state.fetching_releases = false;
            match res {
                Ok(rows) => {
                    let installable = rows.iter().filter(|r| r.host_asset.is_some()).count();
                    let msg = format!("fetched {} tags ({installable} installable on this platform)", rows.len());
                    console::info(&self.state.console, "github", &msg);
                    self.state.status_msg = msg;
                    if let Err(e) = ReleaseCache::save(&rows) {
                        console::warn(&self.state.console, "cache",
                            format!("could not persist releases cache: {e}"));
                    }
                    self.state.available = rows;
                    self.state.available_fetched_at = Some(chrono::Utc::now());
                }
                Err(e) => {
                    console::error(&self.state.console, "github", &e);
                    self.state.status_msg = format!("github error: {e}");
                }
            }
        }
        if let Some(res) = self.state.slots.install_done.lock().unwrap().take() {
            self.state.installing = None;
            match res {
                Ok(p)  => {
                    let msg = format!("installed → {p}");
                    console::info(&self.state.console, "install", &msg);
                    self.state.status_msg = msg;
                    self.state.installed = crate::versions::list_installed().unwrap_or_default();
                    // After a successful install the cached file definitely
                    // exists on disk; refresh every row's marker so the GUI
                    // reflects reality.
                    for r in &mut self.state.available { r.refresh_cached(); }
                }
                Err(e) => {
                    console::error(&self.state.console, "install", &e);
                    self.state.status_msg = format!("install failed: {e}");
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
                    console::error(&self.state.console, "compare", format!("[{channel}] {e}"));
                    self.state.compare_results.remove(&channel);
                    self.state.compare_errors.insert(channel, e);
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
                    console::error(&self.state.console, "tag-meta",
                        format!("[{tag}] {e}"));
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
        if self.state.installing.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.state.fetching_releases || self.state.seeding || self.state.applying
            || !self.state.compare_loading.is_empty()
            || !self.state.tag_fetch_pending.is_empty()
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
