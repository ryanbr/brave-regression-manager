use egui::{Color32, RichText, Ui};

use chrono::Datelike;

use crate::config::BraveLogLevel;
use crate::verdict::{self, Verdict};
use crate::versions;

use super::state::{AppState, ReleaseRow};

const RELEASE_COUNT_OPTIONS: &[u32] = &[
    50, 100, 150, 200, 250, 300, 350, 400, 450, 500,
    600, 700, 800, 900, 1000, 1250, 1500, 2000,
];

/// Brave's brave-browser repo started shipping Nightly tags in 2021 — there
/// is nothing usable older than that, so cap the date pickers there.
const DATE_MIN_YEAR:  i32 = 2021;
const DATE_MIN_MONTH: u32 = 1;
const DATE_MIN_DAY:   u32 = 1;
fn min_allowed_date() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(DATE_MIN_YEAR, DATE_MIN_MONTH, DATE_MIN_DAY).unwrap()
}
fn clamp_date(d: chrono::NaiveDate) -> chrono::NaiveDate {
    let today = chrono::Utc::now().date_naive();
    d.max(min_allowed_date()).min(today)
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Brave Versions");

    // ── Row 1: primary actions + summary (wraps on narrow windows) ────────
    // Bump every text style up by 3px in this row so the action buttons
    // (Refresh installed / Fetch GitHub releases) read clearly larger
    // than the rest of the page. style_mut() COW-clones the parent's
    // style so the change only applies to this scope.
    ui.horizontal_wrapped(|ui| {
        for (_, font_id) in ui.style_mut().text_styles.iter_mut() {
            font_id.size += 3.0;
        }
        if ui.button("Refresh installed").clicked() {
            state.installed = versions::list_installed().unwrap_or_default();
        }
        let fetching = state.fetching_releases;
        if ui.add_enabled(!fetching, egui::Button::new(
            if fetching { "Fetching…" } else { "Fetch GitHub releases" }
        )).clicked() {
            spawn_fetch(state);
        }
        ui.separator();
        let installable = state.available.iter().filter(|r| r.host_asset.is_some()).count();
        let in_range = state.available.iter()
            .filter(|r| date_in_range(&r.published_at, state.date_from, state.date_to))
            .count();
        super::app::weak_label(ui, format!(
            "installed: {}    tags: {} ({installable} installable, {in_range} in range)",
            state.installed.len(), state.available.len()
        ));
        if let Some(t) = state.available_fetched_at {
            super::app::weak_label(ui, format!("· cache: {}", t.format("%Y-%m-%d %H:%M")));
        }
    });

    // ── Row 2: filters (hide + date range) ────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        for (_, font_id) in ui.style_mut().text_styles.iter_mut() {
            font_id.size += 3.0;
        }
        let mut hide = state.hide_no_installer;
        if ui.checkbox(&mut hide, "Hide releases with no installer").changed() {
            state.hide_no_installer = hide;
            state.config_dirty = true;
            crate::console::info(&state.console, "config",
                format!("hide_no_installer: {}", if hide { "on" } else { "off" }));
        }

        ui.separator();
        ui.label("from:");
        let today = chrono::Utc::now().date_naive();
        // Year + month dropdowns whose option lists are constrained to
        // `2021..=current_year` and Jan..Dec respectively. The user
        // *cannot* select pre-2021 because those years aren't in the
        // dropdown at all.
        let prev_from = state.date_from;
        let prev_to   = state.date_to;
        ym_combo(ui, "date_from", &mut state.date_from, today,
                 EndOfMonth::Start, &mut state.config_dirty);
        ui.label("to:");
        ym_combo(ui, "date_to", &mut state.date_to, today,
                 EndOfMonth::End, &mut state.config_dirty);

        if ui.small_button("Clear").clicked()
            && (state.date_from.is_some() || state.date_to.is_some())
        {
            state.date_from = None;
            state.date_to   = None;
            state.config_dirty = true;
            crate::console::info(&state.console, "filter",
                "date filter cleared");
        }
        // Preset clicks are an explicit single-action intent — auto-fetch
        // is fine here. The from/to combos are NOT auto-fetched: picking
        // just the year would otherwise immediately fire a fetch back to
        // January of that year before the user got to the month combo.
        // The user can still trigger a fetch via the explicit "Fetch
        // GitHub releases" button after editing the combos.
        let mut preset_clicked: Option<&str> = None;
        for (label, days) in [("7d", 7i64), ("30d", 30), ("60d", 60), ("90d", 90), ("120d", 120), ("150d", 150)] {
            if ui.small_button(label).clicked() {
                state.date_to   = Some(today);
                state.date_from = Some(clamp_date(today - chrono::Duration::days(days)));
                state.config_dirty = true;
                preset_clicked = Some(label);
            }
        }
        // Echo any date-filter change to the Console — preset name when
        // a quick-button was used, "custom" otherwise (year/month combo
        // edit). Useful when a fetch is misbehaving and you want to
        // confirm what filter the GUI thinks is active.
        let from_changed = state.date_from != prev_from;
        let to_changed   = state.date_to   != prev_to;
        if from_changed || to_changed {
            let fmt_d = |d: Option<chrono::NaiveDate>|
                d.map(|d| d.to_string()).unwrap_or_else(|| "(none)".to_string());
            let src = preset_clicked.map(|p| format!("preset {p}"))
                .unwrap_or_else(|| "custom".to_string());
            crate::console::info(&state.console, "filter", format!(
                "date range {} ({} -> {} | {} -> {})",
                src,
                fmt_d(prev_from), fmt_d(state.date_from),
                fmt_d(prev_to),   fmt_d(state.date_to)));
        }

        // Auto-refetch only fires for preset clicks. Refetch when the
        // requested window asks for releases *older* than anything
        // currently cached, OR when the cache is stale (>10 min — Brave
        // nightlies land multiple times a day).
        let filter_active = state.date_from.is_some() || state.date_to.is_some();
        if preset_clicked.is_some() && filter_active
            && !state.available.is_empty() && !state.fetching_releases
        {
            let oldest_cached = state.available.iter()
                .filter_map(|r| r.published_at.get(..10))
                .filter_map(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .min();
            let needs_older = match (state.date_from, oldest_cached) {
                (Some(want), Some(have)) => want < have,
                (Some(_),    None)       => true,
                _                        => false,
            };
            let cache_stale = state.available_fetched_at
                .map(|t| (chrono::Utc::now() - t).num_minutes() >= 10)
                .unwrap_or(true);
            if needs_older || cache_stale {
                spawn_fetch(state);
            }
        }
    });

    // ── Settings (collapsible) — release count, log level, freeze, token ──
    egui::CollapsingHeader::new("Settings")
        .id_source("versions_settings")
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("settings_grid").num_columns(2)
                .spacing([12.0, 6.0]).show(ui, |ui|
            {
                ui.label("Releases to fetch:");
                let mut new_count = state.release_count;
                egui::ComboBox::from_id_source("release_count")
                    .width(80.0)
                    .selected_text(format!("{}", state.release_count))
                    .show_ui(ui, |ui| {
                        for &n in RELEASE_COUNT_OPTIONS {
                            if ui.selectable_label(state.release_count == n, format!("{n}")).clicked() {
                                new_count = n;
                            }
                        }
                    });
                if new_count != state.release_count {
                    let prev = state.release_count;
                    state.release_count = new_count;
                    state.config_dirty = true;
                    crate::console::info(&state.console, "config",
                        format!("release_count: {prev} -> {new_count}"));
                    if !state.available.is_empty() && !state.fetching_releases {
                        spawn_fetch(state);
                    }
                }
                ui.end_row();

                ui.label("Theme:");
                let dark = !state.theme.eq_ignore_ascii_case("light");
                // Show the icon for the *target* mode so the button doubles
                // as a "click here to switch" indicator. ☀ / ☾ are BMP
                // codepoints — egui's bundled NotoEmoji subset covers them.
                let (icon, hover) = if dark
                    { ("☀ Light",  "Currently dark mode — click to switch to light") }
                    else { ("☾ Dark", "Currently light mode — click to switch to dark") };
                if ui.button(icon).on_hover_text(hover).clicked() {
                    state.theme = if dark { "light".into() } else { "dark".into() };
                    state.config_dirty = true;
                    super::app::apply_theme(ui.ctx(), &state.theme);
                    crate::console::info(&state.console, "config",
                        format!("theme set to {}", state.theme));
                }
                ui.end_row();

                ui.label("Brave logging:");
                let mut new_lvl = state.brave_log_level;
                egui::ComboBox::from_id_source("brave_log_level")
                    .selected_text(state.brave_log_level.label())
                    .show_ui(ui, |ui| {
                        for lvl in BraveLogLevel::ALL {
                            if ui.selectable_label(state.brave_log_level == lvl, lvl.label()).clicked() {
                                new_lvl = lvl;
                            }
                        }
                    });
                if new_lvl != state.brave_log_level {
                    state.brave_log_level = new_lvl;
                    state.config_dirty = true;
                    crate::console::info(&state.console, "config",
                        format!("brave logging set to {}", new_lvl.label()));
                }
                ui.end_row();

                ui.label("Freeze components:");
                let mut freeze = state.freeze_components;
                if ui.checkbox(&mut freeze, "").on_hover_text(
                    "When ON: Brave launches with --disable-component-update + poison-URL \
                     component server, so adblock lists stay pinned to whatever's on disk.\n\n\
                     When OFF: Brave can fetch fresh component updates from Brave's update server.\n\n\
                     The Seed lists button on Tab 2 always lets components fetch regardless."
                ).changed() {
                    state.freeze_components = freeze;
                    state.config_dirty = true;
                    crate::console::info(&state.console, "config",
                        if freeze { "components frozen on next launch" }
                        else      { "components allowed to update on next launch" });
                }
                ui.end_row();

                ui.label("Channels:").on_hover_text(
                    "Which Brave release channels to include in the available list. \
                     At least one must be checked.");
                ui.horizontal(|ui| {
                    let prev = (state.channel_release, state.channel_beta, state.channel_nightly);
                    ui.checkbox(&mut state.channel_release, "Release");
                    ui.checkbox(&mut state.channel_beta,    "Beta");
                    ui.checkbox(&mut state.channel_nightly, "Nightly");
                    // Don't let the user uncheck the last one — re-enable
                    // Nightly if they emptied the set.
                    if !state.channel_release && !state.channel_beta && !state.channel_nightly {
                        state.channel_nightly = true;
                    }
                    let now = (state.channel_release, state.channel_beta, state.channel_nightly);
                    if prev != now {
                        state.config_dirty = true;
                        let chans = {
                            let mut v: Vec<&str> = Vec::new();
                            if now.0 { v.push("Release"); }
                            if now.1 { v.push("Beta"); }
                            if now.2 { v.push("Nightly"); }
                            v.join("+")
                        };
                        crate::console::info(&state.console, "config",
                            format!("channels: {chans}"));
                        if !state.fetching_releases {
                            spawn_fetch(state);
                        }
                    }
                });
                ui.end_row();

                ui.label("Brave install folder:").on_hover_text(
                    "Override the directory Brave installs are extracted into. \
                     Useful for putting the heavy install tree on a different drive \
                     (e.g. C: → D:). Empty keeps the default <data-root>/versions/. \
                     Profiles, downloads cache, and the sqlite db are unaffected.\n\n\
                     CHANGE TAKES EFFECT NEXT LAUNCH — existing on-disk installs \
                     are NOT moved automatically.");
                ui.horizontal(|ui| {
                    let prev = state.versions_dir.clone();
                    let hover = if state.versions_dir.is_empty() {
                        format!("default: {}", crate::paths::versions_dir().display())
                    } else {
                        state.versions_dir.clone()
                    };
                    if ui.button("Browse…").on_hover_text(hover).clicked()
                    {
                        let mut dlg = rfd::FileDialog::new()
                            .set_title("Pick Brave install folder");
                        if !state.versions_dir.is_empty() {
                            dlg = dlg.set_directory(&state.versions_dir);
                        }
                        if let Some(picked) = dlg.pick_folder() {
                            state.versions_dir = picked.display().to_string();
                        }
                    }
                    if !state.versions_dir.is_empty()
                        && ui.button("Use Default").on_hover_text(
                            "Drop the override and use the standard \
                             <data-root>/versions/ directory on next launch").clicked()
                    {
                        state.versions_dir.clear();
                    }
                    if prev != state.versions_dir {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            if state.versions_dir.is_empty() {
                                format!("brave install folder: cleared \
                                         (using default {} on next launch)",
                                        crate::paths::versions_dir().display())
                            } else {
                                format!("brave install folder: {} \
                                         (takes effect on next launch)",
                                        state.versions_dir)
                            });
                    }
                });
                ui.end_row();

                ui.label("Default profile folder:").on_hover_text(
                    "When enabled, this folder is used as `--user-data-dir` for any \
                     installed version that doesn't have its own per-row override. \
                     When disabled, versions fall back to the app's standard profile dir.");
                ui.horizontal(|ui| {
                    let prev_enabled = state.default_profile_dir_enabled;
                    let prev_path    = state.default_profile_dir.clone();
                    ui.checkbox(&mut state.default_profile_dir_enabled, "Enabled");
                    let hover = if state.default_profile_dir.is_empty() {
                        "Pick the default user-data-dir".to_string()
                    } else { state.default_profile_dir.clone() };
                    if ui.add_enabled(state.default_profile_dir_enabled,
                                      egui::Button::new("Browse…"))
                        .on_hover_text(hover)
                        .clicked()
                    {
                        let mut dlg = rfd::FileDialog::new()
                            .set_title("Pick default user-data-dir");
                        if !state.default_profile_dir.is_empty() {
                            dlg = dlg.set_directory(&state.default_profile_dir);
                        }
                        if let Some(picked) = dlg.pick_folder() {
                            state.default_profile_dir = picked.display().to_string();
                        }
                    }
                    if !state.default_profile_dir.is_empty()
                        && ui.small_button("×").on_hover_text("Clear path").clicked()
                    {
                        state.default_profile_dir.clear();
                    }
                    if prev_enabled != state.default_profile_dir_enabled
                        || prev_path != state.default_profile_dir
                    {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            if state.default_profile_dir_enabled && !state.default_profile_dir.is_empty() {
                                format!("default profile folder → {}", state.default_profile_dir)
                            } else if state.default_profile_dir_enabled {
                                "default profile folder enabled (no path picked yet)".to_string()
                            } else {
                                "default profile folder disabled".to_string()
                            });
                    }
                });
                ui.end_row();

                ui.label("Default arguments:").on_hover_text(
                    "Default extra Brave command-line args appended to every \
                     launch when this version's row has no per-version override. \
                     Whitespace-separated; per-row args (set on the Installed row) \
                     take precedence over this default.");
                ui.horizontal(|ui| {
                    let prev_enabled = state.default_args_enabled;
                    let prev_args    = state.default_args.clone();
                    ui.checkbox(&mut state.default_args_enabled, "Enabled");
                    let resp = ui.add_enabled(state.default_args_enabled,
                        egui::TextEdit::singleline(&mut state.default_args)
                            .desired_width(280.0)
                            .hint_text("e.g. --js-flags=--stack-trace-limit=50 --enable-features=…"));
                    let _ = resp;
                    if !state.default_args.is_empty()
                        && ui.small_button("×").on_hover_text("Clear default args").clicked()
                    {
                        state.default_args.clear();
                    }
                    if prev_enabled != state.default_args_enabled
                        || prev_args != state.default_args
                    {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            if state.default_args_enabled && !state.default_args.is_empty() {
                                format!("default args → {}", state.default_args)
                            } else if state.default_args_enabled {
                                "default args enabled (empty — nothing to apply)".to_string()
                            } else {
                                "default args disabled".to_string()
                            });
                    }
                });
                ui.end_row();

                ui.label("Clean profile per launch:").on_hover_text(
                    "Use a fresh, unique --user-data-dir on every Launch / \
                     Apply & Launch instead of reusing the selected profile. \
                     Useful when bisecting regressions where the existing \
                     profile state may itself be the culprit. Per-row \
                     'Profile…' overrides still take precedence.");
                ui.horizontal(|ui| {
                    let prev = state.clean_profile_per_launch;
                    ui.checkbox(&mut state.clean_profile_per_launch, "Enabled");
                    if prev != state.clean_profile_per_launch {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            if state.clean_profile_per_launch {
                                "clean profile per launch: enabled".to_string()
                            } else {
                                "clean profile per launch: disabled".to_string()
                            });
                    }
                });
                ui.end_row();

                ui.label("Launch as administrator:").on_hover_text(
                    "Route every Launch through a privilege-escalation \
                     wrapper:\n\
                     • Windows: powershell Start-Process -Verb RunAs (UAC)\n\
                     • macOS: osascript … with administrator privileges\n\
                     • Linux: pkexec (polkit graphical auth)\n\
                     Linux launches automatically add --no-sandbox \
                     (Chromium refuses to run as root otherwise).\n\n\
                     Caveats: stderr pipe and per-row Stop force-kill \
                     don't apply — the Child handle is the launcher, \
                     not Brave. Use only for debugging permission \
                     issues; running browsers as root/admin is risky.");
                ui.horizontal(|ui| {
                    let prev = state.launch_as_admin;
                    ui.checkbox(&mut state.launch_as_admin, "Enabled");
                    if prev != state.launch_as_admin {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            if state.launch_as_admin {
                                "launch as admin: enabled".to_string()
                            } else {
                                "launch as admin: disabled".to_string()
                            });
                    }
                });
                ui.end_row();

                ui.label("Incremental release cache:").on_hover_text(
                    "Persist every release we've ever fetched into sqlite \
                     and break out of pagination as soon as we re-encounter \
                     a known tag. After the first deep walk, subsequent \
                     fetches only paginate the few pages newer than the \
                     latest cached tag — much friendlier to GitHub's rate \
                     limit when bisecting against older releases. Off by \
                     default; safe to enable / disable at any time.");
                ui.horizontal(|ui| {
                    let prev = state.incremental_release_cache;
                    ui.checkbox(&mut state.incremental_release_cache, "Enabled");
                    if prev != state.incremental_release_cache {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            if state.incremental_release_cache {
                                "incremental release cache: enabled".to_string()
                            } else {
                                "incremental release cache: disabled".to_string()
                            });
                    }
                });
                ui.end_row();

                ui.label("GitHub token:").on_hover_text(
                    "Optional — bumps anonymous 60 req/hr to 5,000 req/hr. \
                     https://github.com/settings/tokens (no scopes needed).");
                let mut tok = state.github_token.clone();
                if ui.add(egui::TextEdit::singleline(&mut tok)
                    .password(true).desired_width(220.0)).changed()
                {
                    state.github_token = tok;
                    state.config_dirty = true;
                    // Token value is intentionally never logged — only
                    // the cleared/set state and length are surfaced.
                    let s = if state.github_token.is_empty() { "cleared".to_string() }
                            else { format!("set ({} chars)", state.github_token.len()) };
                    crate::console::info(&state.console, "config",
                        format!("github_token: {s}"));
                }
                ui.end_row();
            });
            super::app::weak_label(ui, format!("Date range minimum: {} (Brave Nightly history starts here)",
                            min_allowed_date()));
        });

    ui.separator();

    // Installed list defaults to ~7 rows tall; the user can drag the
    // divider below it to resize. Session state — see
    // `state.installed_panel_height`. Available list fills whatever
    // remains so we don't get a big blank area below it.
    let row_h = ui.spacing().interact_size.y + 2.0;
    let installed_h = state.installed_panel_height.unwrap_or(row_h * 7.0);

    let heading_size = egui::TextStyle::Body.resolve(ui.style()).size + 2.0;
    ui.label(RichText::new("Installed versions").strong().size(heading_size));

    // Pre-compute installed tags newest-first so each row can offer a
    // "compare vs the next-newer installed version" link to brave-core.
    let mut sorted_tags: Vec<String> = state.installed.iter().map(|v| v.tag.clone()).collect();
    sort_tags_newest_first(&mut sorted_tags);

    // Find the closest GOOD/BAD pair (regardless of direction) so we can
    // surface a "commits between these tags" affordance. `older` is the
    // tag with the lower semver, `newer` is the higher — that's the
    // direction GitHub's `compare/A...B` expects to enumerate commits.
    //
    // Compare only within the same channel: a Beta GOOD vs a Nightly BAD
    // would point at a brave-core range that mixes commits from two
    // different release branches, which isn't a meaningful regression
    // window. Each row is tagged with its channel from the available
    // cache (or "?" when unknown — those still pair with each other but
    // never cross with a known channel).
    let channel_of = |tag: &str| -> String {
        state.available.iter().find(|r| r.tag == tag)
            .map(|r| r.channel.clone()).unwrap_or_default()
    };
    let mut goods: Vec<(usize, String, String)> = Vec::new(); // (idx, tag, channel)
    let mut bads:  Vec<(usize, String, String)> = Vec::new();
    for (i, tag) in sorted_tags.iter().enumerate() {
        let ch = channel_of(tag);
        match verdict::version_verdict(tag).unwrap_or(Verdict::Unknown) {
            Verdict::Good => goods.push((i, tag.clone(), ch)),
            Verdict::Bad  => bads.push((i, tag.clone(), ch)),
            // BUGGY / UNSURE / UNTESTED / Unknown don't anchor a bracket.
            // Only firm GOOD ↔ BAD pairs trigger the compare panel.
            _ => {}
        }
    }
    // Per-channel brackets: pick the closest GOOD↔BAD pair *within each
    // channel* so Beta and Nightly (and Release) can each show their own
    // compare panel side-by-side.
    let mut channels: Vec<String> = goods.iter().chain(bads.iter())
        .map(|(_, _, ch)| ch.clone()).collect();
    channels.sort();
    channels.dedup();
    let mut brackets: Vec<(String, String, String, String, String)> = Vec::new();
    // entries: (channel, older, newer, good, bad)
    for ch in &channels {
        let mut best_dist = usize::MAX;
        let mut chosen: Option<(String, String, String, String)> = None;
        for (gi, gt, gch) in &goods {
            if gch != ch { continue; }
            for (bi, bt, bch) in &bads {
                if bch != ch { continue; }
                let d = gi.abs_diff(*bi);
                if d < best_dist {
                    best_dist = d;
                    let (older, newer) = if gi > bi { (gt.clone(), bt.clone()) }
                                         else      { (bt.clone(), gt.clone()) };
                    chosen = Some((older, newer, gt.clone(), bt.clone()));
                }
            }
        }
        if let Some((older, newer, good, bad)) = chosen {
            brackets.push((ch.clone(), older, newer, good, bad));
        }
    }

    // auto_shrink([false, true]) so the panel collapses to the actual row
    // count when there are fewer than 7 installed — no big empty band.
    egui::ScrollArea::vertical().id_source("installed").max_height(installed_h)
        .auto_shrink([false, true]).show(ui, |ui|
    {
        // Bump every text style up by 1px inside the installed panel only —
        // gives the row labels / monospace tags a touch more legibility
        // without growing the rest of the tab.
        for (_, font_id) in ui.style_mut().text_styles.iter_mut() {
            font_id.size += 1.0;
        }
        let installed = state.installed.clone();
        if installed.is_empty() {
            super::app::weak_label(ui, "(none yet — install a tag below)");
        }
        // Fixed widths so the leading cells (bullet, tag, path, copy)
        // line up vertically across rows. The trailing widgets (Launch,
        // Profile, verdict, args, Open, Del) still vary because Stop /
        // × only appear under certain conditions.
        const I_DOT:  f32 =  18.0;
        // Tag column tightened to 80 — `v1.91.119` is ~70 px so 100
        // was leaving ~30 px of empty band between tag and path.
        const I_TAG:  f32 =  80.0;
        // Path column at 300 — fits the typical truncated path
        // (`…\AppData\Local\brave-regress\versions\v1.91.119`) with a
        // small breathing margin. Longer paths still truncate
        // (`Label::truncate(true)`), and the row is tight enough that
        // Open + Del stay on-row even when the Stop button appears.
        const I_PATH: f32 = 315.0;
        for v in &installed {
            ui.horizontal(|ui| {
                let verdict = verdict::version_verdict(&v.tag).unwrap_or(Verdict::Unknown);
                let dot_color = verdict_color(verdict);
                let fixed = |ui: &mut Ui, w: f32, draw: &mut dyn FnMut(&mut Ui)| {
                    ui.scope(|ui| {
                        ui.set_min_width(w);
                        ui.set_max_width(w);
                        draw(ui);
                    });
                };
                fixed(ui, I_DOT, &mut |ui| {
                    ui.colored_label(dot_color, "•");
                });
                fixed(ui, I_TAG, &mut |ui| {
                    ui.label(RichText::new(&v.tag).monospace().strong().color(dot_color));
                });
                let full_path = v.folder.display().to_string();
                let short_path = truncate_path(&full_path, 48);
                fixed(ui, I_PATH, &mut |ui| {
                    ui.add(egui::Label::new(&short_path).truncate(true))
                        .on_hover_text(&full_path);
                });
                if ui.small_button("Copy")
                    .on_hover_text(format!("Copy path:\n{full_path}"))
                    .clicked()
                {
                    ui.ctx().copy_text(full_path.clone());
                    state.status_msg = format!("copied: {full_path}");
                }

                if ui.button("Launch").clicked() {
                    let profile = state.selected_profile.clone().unwrap_or_else(|| "default".to_string());
                    let row_args = verdict::launch_args(&v.tag);
                    let effective_args = if !row_args.trim().is_empty() {
                        row_args
                    } else if state.default_args_enabled && !state.default_args.trim().is_empty() {
                        state.default_args.clone()
                    } else {
                        String::new()
                    };
                    let extra_args = verdict::parse_launch_args(&effective_args);
                    // Resolve the user-data-dir source AND log which
                    // precedence tier won — makes it obvious in the
                    // Console why a custom profile did/didn't apply.
                    let (custom, src) = {
                        let per_row = verdict::user_data_dir(&v.tag);
                        if !per_row.is_empty() {
                            (Some(std::path::PathBuf::from(per_row)), "per-row override")
                        } else if state.clean_profile_per_launch {
                            (Some(throwaway_profile_dir(&v.tag)), "clean-profile-per-launch")
                        } else if state.default_profile_dir_enabled
                            && !state.default_profile_dir.is_empty()
                        {
                            (Some(std::path::PathBuf::from(&state.default_profile_dir)),
                             "Settings default profile folder")
                        } else {
                            (None, "standard app profile")
                        }
                    };
                    if let Some(p) = &custom {
                        let exists = p.exists();
                        let local_state = p.join("Local State").exists();
                        let singleton = p.join("SingletonLock").exists();
                        crate::console::info(&state.console, "profile", format!(
                            "source={src}  path={}  dir_exists={exists}  \
                             looks_like_chromium_profile={local_state}  \
                             singleton_lock_present={singleton}",
                            p.display()));
                        if singleton {
                            crate::console::warn(&state.console, "profile",
                                "SingletonLock found — Brave (or its updater) \
                                 may already be running against this profile. \
                                 Will be removed pre-launch, but a LIVE process \
                                 holding the lock will cause Brave to exit \
                                 within a few seconds (single-instance handoff).");
                        }
                        if !local_state && exists {
                            crate::console::warn(&state.console, "profile",
                                "no 'Local State' file in this folder — pointed \
                                 at the wrong directory? Chromium expects the \
                                 user-data-dir ROOT (containing Local State + \
                                 Default/), not a sub-profile folder.");
                        }
                        // Schema/version mismatch + sub-profile inventory
                        // — read Local State once and report both.
                        if local_state {
                            describe_local_state(&state.console, p, &v.tag);
                        }
                    } else {
                        crate::console::info(&state.console, "profile",
                            format!("source={src} (no override)"));
                    }
                    let effective_user_data = custom.clone()
                        .unwrap_or_else(|| crate::paths::profile_dir(&profile));
                    match versions::launch::launch_with_console(&v.tag, &profile, state.console.clone(), state.brave_log_level, state.freeze_components, extra_args, custom, state.launch_as_admin) {
                        Ok(child) => {
                            let msg = format!("launched {} (profile={})", v.tag,
                                effective_user_data.display());
                            crate::console::info(&state.console, "launch", &msg);
                            state.running.insert(v.tag.clone(), super::state::RunningBrave {
                                tag: v.tag.clone(),
                                profile: profile.clone(),
                                child,
                                user_data_dir: effective_user_data,
                                spawned_at: std::time::Instant::now(),
                            });
                            state.status_msg = msg;
                        }
                        Err(e) => {
                            let raw = format!("{e:#}");
                            let msg = match launch_failure_hint(&raw) {
                                Some(h) => format!("launch failed: {raw}\nhint: {h}"),
                                None    => format!("launch failed: {raw}"),
                            };
                            crate::console::error(&state.console, "launch", &msg);
                            state.status_msg = msg;
                        }
                    }
                }
                if state.running.contains_key(&v.tag) && ui.button("Stop")
                    .on_hover_text("Force-kill Brave and every helper/renderer it spawned")
                    .clicked()
                {
                    if let Some(mut r) = state.running.remove(&v.tag) {
                        let pid = r.child.id();
                        // Hard kill the entire process tree first — this
                        // catches orphaned Helper / Renderer / GPU children
                        // that Child::kill alone would leave running.
                        versions::launch::force_kill_tree(pid);
                        // Then reap our direct child handle so we don't
                        // leak a zombie pid.
                        let _ = r.child.kill();
                        let _ = r.child.wait();
                        state.status_msg = format!("force-killed {} (pid {pid})", v.tag);
                        crate::console::info(&state.console, "launch",
                            format!("force-killed {} (pid {pid})", v.tag));
                    }
                }

                // Per-version custom user-data-dir. Empty stored value =
                // default (the app's profile dir for `selected_profile`).
                let cur = verdict::user_data_dir(&v.tag);
                let (btn_label, hover) = if cur.is_empty() {
                    ("Profile…".to_string(),
                     "Pick a custom Chrome user-data-dir for this version. \
                      Empty = use the app's default profile.".to_string())
                } else {
                    let short = std::path::Path::new(&cur)
                        .file_name().and_then(|s| s.to_str())
                        .unwrap_or(cur.as_str()).to_string();
                    (format!("Profile: {short}"),
                     format!("Custom user-data-dir for this version:\n{cur}\n\nClick to change."))
                };
                if ui.button(btn_label).on_hover_text(hover).clicked() {
                    let mut dlg = rfd::FileDialog::new()
                        .set_title(format!("Pick user-data-dir for {}", v.tag));
                    if !cur.is_empty() {
                        dlg = dlg.set_directory(&cur);
                    }
                    if let Some(picked) = dlg.pick_folder() {
                        let p = picked.display().to_string();
                        if let Err(e) = verdict::set_user_data_dir(&v.tag, &p) {
                            state.status_msg = format!("save profile path failed: {e}");
                        } else {
                            crate::console::info(&state.console, "config",
                                format!("custom user-data-dir for {}: {p}", v.tag));
                        }
                    }
                }
                if !cur.is_empty()
                    && ui.small_button("×").on_hover_text("Clear custom profile path").clicked()
                {
                    let _ = verdict::set_user_data_dir(&v.tag, "");
                    crate::console::info(&state.console, "config",
                        format!("cleared custom user-data-dir for {}", v.tag));
                }

                // Verdict as a single combo box instead of three buttons —
                // saves ~3 button widths per row and keeps the dot-color
                // indicator on the left in sync with the chosen value.
                let current_verdict = verdict;
                let mut new_verdict = current_verdict;
                egui::ComboBox::from_id_source(format!("verdict-{}", v.tag))
                    .width(82.0)
                    .selected_text(RichText::new(verdict_label(current_verdict))
                        .color(verdict_color(current_verdict)).strong())
                    .show_ui(ui, |ui| {
                        for v in [
                            Verdict::Good,
                            Verdict::Bad,
                            Verdict::Buggy,
                            Verdict::Unsure,
                            Verdict::Untested,
                            Verdict::Unknown,
                        ] {
                            // Color each option's label so the dropdown
                            // mirrors the row's dot-colour palette.
                            let txt = RichText::new(verdict_label(v))
                                .color(verdict_color(v)).strong();
                            ui.selectable_value(&mut new_verdict, v, txt);
                        }
                    });
                if new_verdict != current_verdict {
                    let s = match new_verdict {
                        Verdict::Good     => "good",
                        Verdict::Bad      => "bad",
                        Verdict::Buggy    => "buggy",
                        Verdict::Unsure   => "unsure",
                        Verdict::Untested => "untested",
                        Verdict::Unknown  => "clear",
                    };
                    let _ = verdict::mark("version", &v.tag, s, None);
                }

                // Compact icon-only buttons for Open / Uninstall — tooltips
                // explain on hover so the row stays readable.
                // Per-version extra Brave launch args. Loaded from sqlite
                // on first render, edited in an in-memory buffer, persisted
                // when the field loses focus.
                let buf = state.launch_args_buf.entry(v.tag.clone())
                    .or_insert_with(|| verdict::launch_args(&v.tag));
                let resp = ui.add(
                    egui::TextEdit::singleline(buf)
                        .desired_width(140.0)
                        .hint_text("extra args (e.g. --js-flags=…)")
                );
                if resp.lost_focus() {
                    let _ = verdict::set_launch_args(&v.tag, buf);
                    crate::console::info(&state.console, "config",
                        if buf.trim().is_empty() {
                            format!("cleared launch args for {}", v.tag)
                        } else {
                            format!("saved launch args for {}: {buf}", v.tag)
                        });
                }

                if ui.button("Open").on_hover_text("Open install folder").clicked() {
                    open_in_explorer(&v.folder);
                }
                if ui.button("Del").on_hover_text("Uninstall (remove version folder)").clicked() {
                    let folder = v.folder.clone();
                    let tag = v.tag.clone();
                    crate::console::info(&state.console, "uninstall",
                        format!("removing {tag} → {}", folder.display()));
                    let started = std::time::Instant::now();
                    match std::fs::remove_dir_all(&folder) {
                        Ok(()) => {
                            state.installed = versions::list_installed().unwrap_or_default();
                            let secs = started.elapsed().as_secs_f64();
                            crate::console::info(&state.console, "uninstall",
                                format!("uninstalled {tag} in {secs:.1}s"));
                            state.status_msg = format!("uninstalled {tag}");
                        }
                        Err(e) => {
                            crate::console::error(&state.console, "uninstall",
                                format!("{tag}: {e:#}  (folder still on disk; \
                                 close any open files / processes and retry)"));
                            state.status_msg = format!("uninstall failed: {e}");
                        }
                    }
                }
            });
        }
    });

    // ── Commits between bracketed tags (brave-core) ─────────────────────
    // Draggable horizontal divider — drag up/down to resize the
    // Installed-versions panel vs the Commits-in-bracket panel below.
    // 6px tall hit zone with a centered hairline so it reads as a
    // resize handle, not just empty space.
    let handle_resp = ui.allocate_response(
        egui::vec2(ui.available_width(), 6.0),
        egui::Sense::drag()
    );
    let stroke = if handle_resp.hovered() || handle_resp.dragged() {
        ui.visuals().widgets.active.bg_stroke
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    let mid_y = handle_resp.rect.center().y;
    ui.painter().line_segment(
        [egui::pos2(handle_resp.rect.left(),  mid_y),
         egui::pos2(handle_resp.rect.right(), mid_y)],
        stroke);
    if handle_resp.hovered() || handle_resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if handle_resp.dragged() {
        let cur = state.installed_panel_height.unwrap_or(installed_h);
        let new_h = (cur + handle_resp.drag_delta().y).clamp(row_h * 2.0, row_h * 25.0);
        state.installed_panel_height = Some(new_h);
    }
    render_compare_section(ui, state, brackets.clone());

    ui.separator();
    let chans = {
        let mut v: Vec<&str> = Vec::new();
        if state.channel_release { v.push("Release"); }
        if state.channel_beta    { v.push("Beta"); }
        if state.channel_nightly { v.push("Nightly"); }
        if v.is_empty() { "Nightly".to_string() } else { v.join(" + ") }
    };
    let avail_heading_size = egui::TextStyle::Body.resolve(ui.style()).size + 2.0;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Available on GitHub ({chans})"))
            .strong().size(avail_heading_size));
        // Right-aligned Clear menu — drops down with two destructive
        // actions: wipe every verdict, or wipe every comment. Each
        // targets a distinct sqlite table so the user can clear one
        // without touching the other.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui|
        {
            ui.menu_button("Clear v", |ui| {
                if ui.button("Verdicts")
                    .on_hover_text(
                        "Wipe every per-tag verdict (GOOD / BAD / BUGGY / \
                         UNSURE / NEW). Notes, launch args, and per-tag \
                         profile dirs are not affected.")
                    .clicked()
                {
                    match verdict::clear_all_version_verdicts() {
                        Ok(n) => {
                            crate::console::info(&state.console, "verdict",
                                format!("cleared {n} verdict row(s)"));
                            state.status_msg = format!("cleared {n} verdict(s)");
                        }
                        Err(e) => {
                            crate::console::error(&state.console, "verdict",
                                format!("clear failed: {e:#}"));
                            state.status_msg = format!("clear failed: {e}");
                        }
                    }
                    ui.close_menu();
                }
                if ui.button("Comments")
                    .on_hover_text(
                        "Wipe every per-tag note. Verdicts, launch args, \
                         and per-tag profile dirs are not affected.")
                    .clicked()
                {
                    match verdict::clear_all_notes() {
                        Ok(n) => {
                            crate::console::info(&state.console, "verdict",
                                format!("cleared {n} note row(s)"));
                            state.status_msg = format!("cleared {n} note(s)");
                        }
                        Err(e) => {
                            crate::console::error(&state.console, "verdict",
                                format!("clear notes failed: {e:#}"));
                            state.status_msg = format!("clear failed: {e}");
                        }
                    }
                    ui.close_menu();
                }
                if ui.button("Remove Cached files")
                    .on_hover_text(
                        "Delete every downloaded installer asset under \
                         cache/downloads/. Already-installed Brave versions \
                         are not affected — only the on-disk archives that \
                         the [cached] / Install (cached) shortcut uses.")
                    .clicked()
                {
                    match remove_cached_downloads() {
                        Ok((n, bytes)) => {
                            // Refresh `cached` flags on the in-memory rows
                            // so the [cached] pill / "Install (cached)"
                            // label disappear next frame.
                            for r in std::sync::Arc::make_mut(&mut state.available).iter_mut() {
                                r.refresh_cached();
                            }
                            let mb = bytes as f64 / 1_048_576.0;
                            crate::console::info(&state.console, "cache",
                                format!("removed {n} file(s), freed {mb:.1} MiB"));
                            state.status_msg = format!(
                                "removed {n} cached file(s) ({mb:.1} MiB)");
                        }
                        Err(e) => {
                            crate::console::error(&state.console, "cache",
                                format!("remove cached files failed: {e:#}"));
                            state.status_msg = format!("remove failed: {e}");
                        }
                    }
                    ui.close_menu();
                }
            });
        });
    });

    // ── Add release by tag — single-call manual fetch ───────────────────
    // For pulling a specific older release (e.g. v1.85.99) without
    // walking pagination back to it. One API call to releases/tags/<tag>;
    // the row is upserted into state.available + the persistent cache.
    ui.horizontal(|ui| {
        super::app::weak_label(ui, "Add release by tag:");
        ui.add(egui::TextEdit::singleline(&mut state.add_by_tag_buf)
            .desired_width(140.0)
            .hint_text("v1.85.99"));
        let raw = state.add_by_tag_buf.trim().to_string();
        let can_add = !raw.is_empty() && !state.adding_by_tag;
        let label = if state.adding_by_tag { "Adding…" } else { "Add" };
        if ui.add_enabled(can_add, egui::Button::new(label))
            .on_hover_text(
                "Pull this exact tag's metadata from GitHub in a single \
                 API call (no pagination). The result is added to the \
                 Available list and persisted to the sqlite cache so it \
                 survives across sessions.")
            .clicked()
        {
            // Brave tags are `vMAJOR.MINOR.PATCH`. Normalise the user's
            // input: strip any leading `v`/`V` they may have typed, then
            // prefix exactly one lowercase `v`. Handles `v1.91.119`,
            // `V1.91.119`, and bare `1.91.119` identically.
            let bare = raw.trim_start_matches(['v', 'V']);
            let tag = format!("v{bare}");
            spawn_add_by_tag(state, tag);
        }
    });

    // Bulk-load every per-tag verdict and note in two sqlite queries
    // before render — the row loop and the sort comparator both look
    // these up per-row, so without this we'd pay O(n) reads per render
    // + O(n log n) reads per sort, every frame. With ~4000 cached
    // releases that worked out to ~50k mutex+query operations per
    // 60fps frame; this collapses it to two queries per frame.
    let verdicts_by_tag = verdict::all_version_verdicts();
    let notes_by_tag    = verdict::all_notes();
    // Fill remaining vertical space so a tall window doesn't show a big
    // empty band below this panel.
    egui::ScrollArea::vertical().id_source("avail")
        .auto_shrink([false; 2]).show(ui, |ui|
    {
        // Bump every text style up by 1px inside the Available list. Use
        // `style_mut()` so Arc::make_mut COW-clones the shared Style and
        // the change actually applies to subsequent child UIs (rows).
        for (_, font_id) in ui.style_mut().text_styles.iter_mut() {
            font_id.size += 1.0;
        }
        let rows = state.available.clone();
        let installing_now = state.installing.clone();
        if rows.is_empty() && !state.fetching_releases {
            if state.loading_startup_cache {
                super::app::weak_label(ui, "(loading cache from disk…)");
            } else {
                super::app::weak_label(ui, "(click \"Fetch GitHub releases\" to populate)");
            }
        }
        // Compute how many rows actually clear the active filters and the
        // oldest cached release date — used for the helpful empty-results
        // message below when filters hide everything.
        let mut shown = 0usize;
        let mut oldest: Option<&str> = None;
        // Client-side channel filter — needed because incremental cache
        // mode pulls all channels from GitHub regardless of the user's
        // checkbox selection. Manually-added tags (via the Add-by-tag
        // flow) are exempted: when the user explicitly pulled v1.85.99
        // they expect to see it even if only Nightly is ticked. Capture
        // flags + the manual-set as locals so the helper doesn't keep
        // an immutable borrow on `state` across the row loop's mutable
        // uses.
        let (ch_release, ch_beta, ch_nightly) =
            (state.channel_release, state.channel_beta, state.channel_nightly);
        let manual_tags = state.manual_release_tags.clone();
        let pass_channel = move |r: &super::state::ReleaseRow| -> bool {
            if manual_tags.contains(&r.tag) { return true; }
            match r.channel.as_str() {
                "Release" => ch_release,
                "Beta"    => ch_beta,
                "Nightly" => ch_nightly,
                _ => true, // unknown channel — don't hide
            }
        };
        for r in rows.iter() {
            if let Some(o) = oldest {
                if r.published_at.as_str() < o { oldest = Some(&r.published_at); }
            } else {
                oldest = Some(&r.published_at);
            }
            let pass_installer = !(state.hide_no_installer && r.host_asset.is_none());
            let pass_date      = date_in_range(&r.published_at, state.date_from, state.date_to);
            if pass_installer && pass_date && pass_channel(r) { shown += 1; }
        }
        if !rows.is_empty() && shown == 0 {
            let oldest_short = oldest.map(short_date).unwrap_or_default();
            let date_filter_active = state.date_from.is_some() || state.date_to.is_some();
            ui.horizontal(|ui| {
                if date_filter_active {
                    ui.colored_label(Color32::from_rgb(220, 180, 60), format!(
                        "0 of {} releases match the date filter. Cache only goes back to {}.",
                        rows.len(), oldest_short));
                    // Actionable button — kicks off a fetch back to the
                    // user's requested date_from in one click. Beats the
                    // old "go to Settings → bump count → re-fetch" prose.
                    if !state.fetching_releases
                        && ui.button("Fetch back to date range").clicked()
                    {
                        spawn_fetch(state);
                    }
                } else {
                    ui.colored_label(Color32::from_rgb(220, 180, 60), format!(
                        "0 of {} releases pass the current filters.", rows.len()));
                }
            });
        }
        // Fixed column widths so each row aligns vertically — looks much
        // tidier than ui.horizontal where every cell sizes itself. Header
        // uses the same widths so columns line up under their titles.
        const COL_TAG:      f32 = 110.0;
        const COL_DATE:     f32 =  90.0;
        const COL_CHANNEL:  f32 =  76.0;
        const COL_VERDICT:  f32 =  72.0;
        const COL_NOTE:     f32 =  44.0;
        // Status/action is fixed-width so the trailing Comments cell
        // shares a common left edge across rows. Tight enough that
        // "installed" rows don't leave a huge empty band; the asset
        // filename uses Label::truncate(true) to clip-with-ellipsis
        // so longer names don't push past this cap (full name still
        // available on hover).
        const COL_STATUS:   f32 = 260.0;

        // Header row (only when there's data to show). Each title is
        // clickable: first click sorts by that column, repeat clicks
        // toggle ascending / descending. The active column shows ▲/▼.
        if shown > 0 {
            ui.horizontal(|ui| {
                let mut header = |ui: &mut Ui, w: f32, text: &str,
                                  col: super::state::AvailSortColumn|
                {
                    ui.scope(|ui| {
                        ui.set_min_width(w);
                        ui.set_max_width(w);
                        let active = state.avail_sort_by == col;
                        let arrow = if !active { "" }
                            else if state.avail_sort_asc { " ^" } else { " v" };
                        let color = if active { Color32::from_rgb(220, 200, 100) }
                                    else      { Color32::from_gray(160) };
                        let label = egui::Label::new(
                            RichText::new(format!("{text}{arrow}")).strong().color(color)
                        ).sense(egui::Sense::click());
                        if ui.add(label)
                            .on_hover_text(if active {
                                format!("Click to {} order", if state.avail_sort_asc { "descend" } else { "ascend" })
                            } else {
                                format!("Sort by {text}")
                            })
                            .clicked()
                        {
                            if active {
                                state.avail_sort_asc = !state.avail_sort_asc;
                            } else {
                                state.avail_sort_by  = col;
                                // Default direction per column: dates and
                                // verdicts feel right newest/strongest-first
                                // (descending), text fields ascend by default.
                                state.avail_sort_asc = matches!(col,
                                    super::state::AvailSortColumn::Tag
                                  | super::state::AvailSortColumn::Channel
                                  | super::state::AvailSortColumn::Note);
                            }
                        }
                    });
                };
                use super::state::AvailSortColumn as C;
                header(ui, COL_TAG,     "Tag",     C::Tag);
                header(ui, COL_DATE,    "Date",    C::Date);
                header(ui, COL_CHANNEL, "Channel", C::Channel);
                header(ui, COL_VERDICT, "Verdict", C::Verdict);
                header(ui, COL_NOTE,    "Note",    C::Note);
                ui.scope(|ui| {
                    ui.set_min_width(COL_STATUS);
                    ui.set_max_width(COL_STATUS);
                    ui.label(RichText::new("Status / action").strong()
                        .color(Color32::from_gray(160)));
                });
                ui.label(RichText::new("Comments").strong()
                    .color(Color32::from_gray(160)));
            });
            ui.separator();
        }

        // Apply the active sort to a fresh row order. Sorting happens on
        // the rendered slice only — the cached `state.available` keeps
        // GitHub's published order so a re-fetch isn't needed. Then
        // promote manually-added tags to the top so they're easy to
        // find regardless of the user's current sort key, with a
        // separator drawn between the manual block and the rest.
        // Index-based sort so we don't have to deep-clone the row Vec
        // out of the Arc snapshot. Sorting 4000 usizes is essentially
        // free vs cloning 4000 ReleaseRow structs (each with several
        // Strings inside).
        let mut order: Vec<usize> = (0..rows.len()).collect();
        sort_available_indices(&mut order, &rows, state.avail_sort_by,
            state.avail_sort_asc, &verdicts_by_tag, &notes_by_tag);
        order.sort_by_key(|&i| !state.manual_release_tags.contains(&rows[i].tag));

        // Manually-added tags also bypass the date filter — if the user
        // explicitly asked for v1.46.66 they shouldn't have to widen
        // their date range to see it. Channel filter is already bypassed
        // by `pass_channel`'s manual-tag check.
        let mut last_was_manual = false;
        for &row_idx in &order {
            let r = &rows[row_idx];
            let is_manual = state.manual_release_tags.contains(&r.tag);
            if state.hide_no_installer && r.host_asset.is_none() { continue; }
            if !is_manual && !date_in_range(&r.published_at, state.date_from, state.date_to) { continue; }
            if !pass_channel(r) { continue; }
            // Draw a separator the moment we transition from the manual
            // block to the regular fetched block.
            if last_was_manual && !is_manual {
                ui.separator();
            }
            last_was_manual = is_manual;
            ui.horizontal(|ui| {
                // Reserve a fixed-width cell, then place the widget inside.
                // `scope` lets us set a min_size without bleeding into the
                // next cell.
                let fixed_cell = |ui: &mut Ui, w: f32, draw: &mut dyn FnMut(&mut Ui)| {
                    ui.scope(|ui| {
                        ui.set_min_width(w);
                        ui.set_max_width(w);
                        draw(ui);
                    });
                };

                fixed_cell(ui, COL_TAG, &mut |ui| {
                    // Cyan-ish for manually-added tags so they pop out
                    // of the list. Picked to sit clear of every other
                    // tag/text colour we use (verdict greens/reds, the
                    // green asset-name label, the blue [cached] pill,
                    // the channel pill colours).
                    if is_manual {
                        ui.label(RichText::new(&r.tag).monospace().strong()
                            .color(Color32::from_rgb(120, 220, 230)));
                    } else {
                        ui.monospace(&r.tag);
                    }
                });
                fixed_cell(ui, COL_DATE, &mut |ui| {
                    ui.label(short_date(&r.published_at));
                });
                fixed_cell(ui, COL_CHANNEL, &mut |ui| {
                    let (chan_label, chan_color) = match r.channel.as_str() {
                        "Release" => ("Release", Color32::from_rgb( 80, 170, 240)),
                        "Beta"    => ("Beta",    Color32::from_rgb(220, 170,  60)),
                        "Nightly" => ("Nightly", Color32::from_rgb(160, 120, 220)),
                        _         => ("?",       Color32::from_rgb(150, 150, 150)),
                    };
                    ui.colored_label(chan_color, format!("[{chan_label}]"));
                });

                fixed_cell(ui, COL_VERDICT, &mut |ui| {
                    let row_verdict = verdicts_by_tag.get(&r.tag).copied().unwrap_or(Verdict::Unknown);
                    if row_verdict != Verdict::Unknown {
                        ui.colored_label(verdict_color(row_verdict),
                            RichText::new(format!("[{}]", verdict_label(row_verdict))).strong());
                    }
                });

                // Note cell — inline so it can mutate state when clicked.
                let cur_note = notes_by_tag.get(&r.tag).cloned().unwrap_or_default();
                ui.scope(|ui| {
                    ui.set_min_width(COL_NOTE);
                    ui.set_max_width(COL_NOTE);
                    let (note_label, note_color, hover) = if cur_note.is_empty() {
                        ("+", Color32::from_gray(110), "Add a note for this tag".to_string())
                    } else {
                        ("note", Color32::from_rgb(140, 180, 220), cur_note.clone())
                    };
                    if ui.add(egui::Label::new(
                            RichText::new(note_label).monospace().color(note_color))
                            .sense(egui::Sense::click()))
                        .on_hover_text(hover)
                        .clicked()
                    {
                        state.editing_note_tag = Some(r.tag.clone());
                        state.editing_note_buf = cur_note.clone();
                    }
                });

                let installed = versions::is_installed(&r.tag);
                let busy = installing_now.contains(&r.tag);

                // Status/action cell. For manually-added rows, the
                // Remove button is rendered inside the same fixed-width
                // slot as Install so it doesn't push into the Comments
                // column.
                ui.scope(|ui| {
                    ui.set_min_width(COL_STATUS);
                    ui.set_max_width(COL_STATUS);
                    ui.horizontal(|ui| {
                        render_status_cell(ui, state, r, installed, busy);
                        if is_manual && ui.button("Remove")
                            .on_hover_text(
                                "Remove this manually-added tag from the \
                                 Available list AND uninstall the on-disk \
                                 version (if any). Verdicts and notes are \
                                 preserved in sqlite — re-adding the tag \
                                 later will pick them back up.")
                            .clicked()
                        {
                            let tag = r.tag.clone();
                            let dir = crate::paths::version_dir(&tag);
                            let was_installed = dir.exists();
                            let was_running   = state.running.contains_key(&tag);
                            // Pre-action echo so the user sees what
                            // they're about to do (and we can later
                            // diagnose a failure mid-sequence).
                            crate::console::info(&state.console, "manual", format!(
                                "removing manual tag {tag} \
                                 (installed={was_installed}, running={was_running})"));

                            if let Some(mut running) = state.running.remove(&tag) {
                                let pid = running.child.id();
                                versions::launch::force_kill_tree(pid);
                                let _ = running.child.kill();
                                let _ = running.child.wait();
                                crate::console::info(&state.console, "manual",
                                    format!("  • killed running Brave (pid {pid})"));
                            }

                            let mut uninstall_note = String::new();
                            if was_installed {
                                match std::fs::remove_dir_all(&dir) {
                                    Ok(()) => {
                                        uninstall_note = " + uninstalled".to_string();
                                        state.installed = versions::list_installed()
                                            .unwrap_or_default();
                                        crate::console::info(&state.console, "manual",
                                            format!("  • uninstalled {}", dir.display()));
                                    }
                                    Err(e) => {
                                        uninstall_note = format!(" (uninstall failed: {e})");
                                        crate::console::error(&state.console, "uninstall",
                                            format!("{tag}: {e:#}"));
                                    }
                                }
                            }
                            state.manual_release_tags.remove(&tag);
                            std::sync::Arc::make_mut(&mut state.available)
                                .retain(|x| x.tag != tag);
                            if let Err(e) = verdict::unmark_manual_release(&tag) {
                                crate::console::error(&state.console, "manual",
                                    format!("  • sqlite cleanup failed: {e:#}"));
                                state.status_msg = format!("remove failed: {e}");
                            } else {
                                crate::console::info(&state.console, "manual",
                                    "  • dropped from manual_release_tags + release_cache");
                                crate::console::info(&state.console, "manual",
                                    format!("removed manual tag {tag}{uninstall_note}"));
                                state.status_msg = format!("removed {tag}{uninstall_note}");
                            }
                        }
                    });
                });

                // Comments cell — full note body shown to the right of the
                // Install column. Force a left-justified layout so the
                // text hugs the left edge of its slot regardless of the
                // parent's default alignment. Truncated to one line of
                // 60 chars; full body in the hover tooltip.
                if !cur_note.is_empty() {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let one_line = cur_note.lines().next().unwrap_or("");
                        let trimmed = if one_line.chars().count() > 60 {
                            let head: String = one_line.chars().take(60).collect();
                            format!("{head}…")
                        } else {
                            one_line.to_string()
                        };
                        // Theme-aware: soft amber on dark for low contrast
                        // against the dark panel; a deeper olive on light
                        // mode where the dark text on cream is legible.
                        let note_color = if ui.ctx().style().visuals.dark_mode {
                            Color32::from_rgb(200, 200, 160)
                        } else {
                            Color32::from_rgb( 90,  80,  10)
                        };
                        if ui.add(egui::Label::new(
                                RichText::new(trimmed).color(note_color))
                                .sense(egui::Sense::click()))
                            .on_hover_text(&cur_note)
                            .clicked()
                        {
                            state.editing_note_tag = Some(r.tag.clone());
                            state.editing_note_buf = cur_note.clone();
                        }
                    });
                }
            });
        }
    });

    render_note_editor(ui, state);
}

/// Renders the Status / action cell for one Available row. Pulled out of
/// the row closure so the row can wrap it in a fixed-width scope and the
/// trailing Comments cell still lines up under its header.
fn render_status_cell(
    ui: &mut Ui,
    state: &mut AppState,
    r: &super::state::ReleaseRow,
    installed: bool,
    busy: bool,
) {
    let installing_now = state.installing.clone();
    ui.horizontal(|ui| {
        match (&r.host_asset, installed, busy) {
                    (_, true, _) => { ui.label("installed"); }
                    (None, _, _) => {
                        ui.colored_label(Color32::from_rgb(180, 130, 60),
                            format!("(skip) {}", r.skip_reason));
                        ui.add_enabled(false, egui::Button::new("Install"));
                    }
                    (Some(name), false, true) => {
                        ui.add(egui::Label::new(
                            RichText::new(name).color(Color32::from_rgb(60, 200, 90)))
                            .truncate(true))
                            .on_hover_text(format!("Asset: {name}"));
                        let progress = super::state::progress_for(&state.slots, &r.tag);
                        if let Some(p) = progress {
                            let txt = format_progress_text(&p);
                            ui.add(egui::ProgressBar::new(p.fraction())
                                   .desired_width(180.0).show_percentage().text(txt));
                        } else {
                            ui.label("installing…");
                        }
                    }
                    (Some(name), false, false) => {
                        ui.add(egui::Label::new(
                            RichText::new(name).color(Color32::from_rgb(60, 200, 90)))
                            .truncate(true))
                            .on_hover_text(format!("Asset: {name}"));
                        if r.cached {
                            ui.colored_label(Color32::from_rgb(140, 180, 220), "[cached]");
                        }
                        let btn_label = if r.cached { "Install (cached)" } else { "Install" };
                        let arch_mismatch = is_opposite_arch_asset(name);
                        let already_installing = installing_now.contains(&r.tag);
                        let cap_reached = installing_now.len()
                            >= super::state::MAX_CONCURRENT_INSTALLS;
                        let btn_resp = ui.add_enabled(
                            !already_installing && !cap_reached && !arch_mismatch,
                            egui::Button::new(btn_label));
                        let btn_resp = if arch_mismatch {
                            btn_resp.on_disabled_hover_text(
                                "Cached asset URL is the wrong architecture for \
                                 this host. Click 'Fetch GitHub releases' to \
                                 refresh the cache, then re-install.")
                        } else if cap_reached && !already_installing {
                            btn_resp.on_disabled_hover_text(format!(
                                "Already installing {} version(s) — wait for one to \
                                 finish before starting another.",
                                super::state::MAX_CONCURRENT_INSTALLS))
                        } else { btn_resp };
                        if btn_resp.clicked() {
                            state.installing.insert(r.tag.clone());
                            state.installing_started.insert(r.tag.clone(),
                                std::time::Instant::now());
                            state.status_msg = format!("installing {}…", r.tag);
                            // Pre-install summary — confirms which asset is
                            // being pulled + the URL (copy-pasteable for
                            // manual retry if the in-app fetch fails).
                            let mb = r.asset_size.unwrap_or(0) as f64 / 1_048_576.0;
                            crate::console::info(&state.console, "install", format!(
                                "{}: {} ({:.1} MiB, cached={}) — {}",
                                r.tag, name, mb, r.cached,
                                r.asset_url.as_deref().unwrap_or("(no url)")));
                            let slot     = state.slots.install_done.clone();
                            let progress = state.slots.install_progress.clone();
                            // Per-tag map: clear THIS tag's entry so a
                            // stale completed-state doesn't briefly
                            // flash before the new download writes its
                            // first sample. Other tags' entries stay.
                            progress.lock().unwrap().remove(&r.tag);
                            let tag2     = r.tag.clone();
                            let name2    = name.clone();
                            let url      = r.asset_url.clone();
                            let size     = r.asset_size;
                            let cons     = state.console.clone();
                            state.rt.spawn(async move {
                                let result = match (url, size) {
                                    (Some(u), Some(s)) =>
                                        versions::install::install_tag_with_asset_console(
                                            &tag2, &name2, &u, s, Some(progress), Some(cons)).await,
                                    _ =>
                                        versions::install::install_tag_with_progress(
                                            &tag2, Some(progress)).await,
                                };
                                let result = result.map(|p| p.display().to_string())
                                                   .map_err(|e| format!("{e:#}"));
                                slot.lock().unwrap().push((tag2, result));
                            });
                        }
                        if r.cached
                            && ui.button("?").on_hover_text("Diagnose downloaded installer").clicked()
                        {
                            let asset_name = name.clone();
                            let console = state.console.clone();
                            let cache = crate::paths::downloads_dir().join(&asset_name);
                            crate::console::info(&console, "diagnose",
                                format!("running diagnostic on {}", cache.display()));
                            std::thread::spawn(move || {
                                match versions::diagnose::diagnose_installer(&cache) {
                                    Ok(report) => {
                                        for line in report.lines() {
                                            crate::console::info(&console, "diagnose", line.to_string());
                                        }
                                    }
                                    Err(e) => crate::console::error(&console, "diagnose",
                                        format!("{e:#}")),
                                }
                            });
                        }
                    }
                }
            });
}

/// Floating popup for editing the freeform note attached to a tag.
/// Opened by clicking the `+` / `note` cell in the Available list. Stays
/// modal-feeling but is just an `egui::Window` — Save persists to sqlite,
/// Cancel/Escape/× close without saving.
fn render_note_editor(ui: &mut Ui, state: &mut AppState) {
    let Some(tag) = state.editing_note_tag.clone() else { return };
    let mut open = true;
    let mut close_after = false;
    egui::Window::new(format!("Note · {tag}"))
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .open(&mut open)
        .show(ui.ctx(), |ui|
    {
        ui.add(egui::TextEdit::multiline(&mut state.editing_note_buf)
            .desired_rows(6).desired_width(400.0)
            .hint_text("Free-form notes for this tag — saved when you click Save."));
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                let trimmed = state.editing_note_buf.trim().to_string();
                if let Err(e) = verdict::set_note(&tag, &trimmed) {
                    state.status_msg = format!("save note failed: {e}");
                } else {
                    crate::console::info(&state.console, "note",
                        if trimmed.is_empty() {
                            format!("cleared note for {tag}")
                        } else {
                            format!("saved note for {tag}")
                        });
                }
                close_after = true;
            }
            if ui.button("Cancel").clicked() { close_after = true; }
            if !verdict::note(&tag).is_empty()
                && ui.button("Delete").clicked()
            {
                let _ = verdict::set_note(&tag, "");
                crate::console::info(&state.console, "note",
                    format!("cleared note for {tag}"));
                close_after = true;
            }
        });
    });
    if !open || close_after {
        state.editing_note_tag = None;
        state.editing_note_buf.clear();
    }
}

/// Renders one "Commits in range" panel per channel that has a
/// GOOD↔BAD pair installed. Each panel hits
/// `brave/brave-core/compare/<older>...<newer>` independently so a Beta
/// regression and a Nightly regression can be inspected side-by-side.
fn render_compare_section(
    ui: &mut Ui,
    state: &mut AppState,
    brackets: Vec<(String, String, String, String, String)>, // (channel, older, newer, good, bad)
) {
    // Drop loaded commits whose channel either no longer has a bracket
    // or whose bracket endpoints have changed (verdict edits, uninstalls).
    let valid: std::collections::HashMap<String, (String, String)> = brackets.iter()
        .map(|(ch, o, n, _, _)| (ch.clone(), (o.clone(), n.clone()))).collect();
    state.compare_results.retain(|ch, cr| {
        valid.get(ch).map(|(o, n)| &cr.base == o && &cr.head == n).unwrap_or(false)
    });
    state.compare_errors.retain(|ch, _| valid.contains_key(ch));

    let cmp_heading_size = egui::TextStyle::Body.resolve(ui.style()).size + 2.0;
    ui.label(RichText::new("Commits in bracket (brave-core)")
        .strong().size(cmp_heading_size));

    if brackets.is_empty() {
        super::app::weak_label(ui,
            "(mark a version GOOD and another BAD in the same channel to enable the compare panel)");
        return;
    }

    // Match the +3 bump used on the Brave Versions action rows so the
    // Load / Open on GitHub / Chromium buttons inside each bracket panel
    // read at the same scale. Scoped via allocate_ui so the styling
    // doesn't bleed into siblings rendered after this section.
    // Build a tag -> (chromium_version, published_at) index ONCE
    // before iterating channels. Without this each render_compare_one
    // call did O(n) iter().find() lookups for both endpoints —
    // N≈4000 × 2 × 3 channels = ~24k linear scans per frame the panel
    // is visible. Owned strings (not &str) so the map doesn't borrow
    // from state, freeing state for the &mut pass into the closure.
    let row_by_tag: std::collections::HashMap<String, (Option<String>, String)> =
        state.available.iter()
            .map(|r| (r.tag.clone(),
                     (r.chromium_version.clone(), r.published_at.clone())))
            .collect();
    ui.allocate_ui(ui.available_size(), |ui| {
        for (_, font_id) in ui.style_mut().text_styles.iter_mut() {
            font_id.size += 3.0;
        }
        for (channel, older, newer, good, bad) in &brackets {
            render_compare_one(ui, state, channel, older, newer, good, bad, &row_by_tag);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn render_compare_one(
    ui: &mut Ui,
    state: &mut AppState,
    channel: &str,
    older: &str,
    newer: &str,
    good: &str,
    bad: &str,
    row_by_tag: &std::collections::HashMap<String, (Option<String>, String)>,
) {
    let loading = state.compare_loading.contains(channel);
    let has_result = state.compare_results.contains_key(channel);
    // Auto-parsed pinned Chromium versions + dates from the bracket
    // endpoints — computed in the outer scope so the override row below
    // can reuse them as seeds and as the "reset" target. Falls back to
    // the sqlite tag_metadata cache when a bracket tag isn't in the
    // currently-loaded available window (e.g. an older installed tag).
    let lookup_chr = |tag: &str| -> Option<String> {
        row_by_tag.get(tag)
            .and_then(|(chr, _)| chr.clone())
            .or_else(|| verdict::tag_metadata(tag).0)
    };
    let lookup_date = |tag: &str| -> Option<String> {
        row_by_tag.get(tag)
            .map(|(_, pa)| pa.get(..10).unwrap_or(pa).to_string())
            .or_else(|| verdict::tag_metadata(tag).1
                .map(|s| s.get(..10).unwrap_or(&s).to_string()))
    };
    let older_chr = lookup_chr(older);
    let newer_chr = lookup_chr(newer);
    let older_date = lookup_date(older);
    let newer_date = lookup_date(newer);
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("[{channel}]")).strong().monospace());
            ui.colored_label(Color32::from_rgb(220, 180, 60),
                format!("GOOD {good} ↔ BAD {bad}  (range {older}...{newer})"));
            let label = if loading        { "Loading…".to_string() }
                else if has_result        { "Reload".to_string() }
                else                      { format!("Load {older}...{newer}") };
            if ui.add_enabled(!loading, egui::Button::new(label)).clicked() {
                spawn_compare(state, channel.to_string(),
                              older.to_string(), newer.to_string());
            }
            if ui.button("Open on GitHub").on_hover_text(format!(
                "https://github.com/brave/brave-core/compare/{older}...{newer}")).clicked()
            {
                let url = format!("https://github.com/brave/brave-core/compare/{older}...{newer}");
                crate::console::info(&state.console, "compare", &url);
                open_url(&url);
            }
            // Chromium changes — opens GitHub directly. Tag-compare when
            // both pinned Chromium versions are known (best for Stable /
            // Beta whose Chromium pins are usually tagged); date-bounded
            // commits/main as a fallback (Nightly's tip-of-tree pins
            // often aren't tagged).
            let chromium_url = match (&older_chr, &newer_chr) {
                (Some(a), Some(b)) => Some(
                    format!("https://github.com/chromium/chromium/compare/{a}...{b}")),
                _ => match (&older_date, &newer_date) {
                    (Some(a), Some(b)) => Some(
                        // ±2 day padding: Chromium commits land days before
                        // Brave Nightly pins them, and there's a tail of
                        // late-arriving fixes after the pin too.
                        format!("https://github.com/chromium/chromium/commits/main\
                                 ?since={a}&until={b}")),
                    _ => None,
                },
            };
            if let Some(url) = chromium_url {
                let hover = match (&older_chr, &newer_chr) {
                    (Some(a), Some(b)) => format!(
                        "Chromium tag compare:\nchromium/chromium/compare/{a}...{b}\
                         \n\nNote: Nightly pins are often untagged → may 404."),
                    _ => format!(
                        "Chromium changes by date (approximate):\n{url}\
                         \n\nUsed when one or both pinned Chromium versions \
                         aren't recorded yet — re-fetch GitHub releases to \
                         enable exact tag-compare."),
                };
                if ui.button("Chromium").on_hover_text(hover).clicked() {
                    crate::console::info(&state.console, "compare", &url);
                    open_url(&url);
                }
            }
            if has_result && ui.small_button("×")
                .on_hover_text("Clear loaded commits").clicked()
            {
                state.compare_results.remove(channel);
                state.compare_errors.remove(channel);
            }
        });

        // ── Chromium tag override row (Design A) ───────────────────────
        // Two text fields seeded with the auto-parsed pins, plus an
        // "Open compare" button so the user can nudge either side to a
        // nearby tagged Chromium milestone when Brave Nightly's exact
        // pin isn't tagged on chromium/chromium.
        let auto_older = older_chr.clone().unwrap_or_default();
        let auto_newer = newer_chr.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            super::app::weak_label(ui, "Chromium tags:");
            let key = (channel.to_string(), older.to_string(), newer.to_string());
            let entry = state.chromium_overrides.entry(key.clone())
                .or_insert_with(|| (auto_older.clone(), auto_newer.clone()));
            let cur_a = entry.0.clone();
            let cur_b = entry.1.clone();
            ui.add(egui::TextEdit::singleline(&mut entry.0)
                .desired_width(120.0)
                .hint_text("147.0.7727.130"));
            super::app::weak_label(ui, "…");
            ui.add(egui::TextEdit::singleline(&mut entry.1)
                .desired_width(120.0)
                .hint_text("147.0.7727.137"));
            let can_compare = !entry.0.trim().is_empty() && !entry.1.trim().is_empty();
            if ui.add_enabled(can_compare, egui::Button::new("Open compare"))
                .on_hover_text(format!(
                    "https://github.com/chromium/chromium/compare/{}...{}",
                    entry.0.trim(), entry.1.trim()))
                .clicked()
            {
                let url = format!(
                    "https://github.com/chromium/chromium/compare/{}...{}",
                    entry.0.trim(), entry.1.trim());
                crate::console::info(&state.console, "compare", &url);
                open_url(&url);
            }
            // Right-aligned hint showing what the auto-parser pulled, so
            // the user can spot when they've drifted away from the pinned
            // versions.
            let drifted = cur_a != auto_older || cur_b != auto_newer;
            if drifted {
                if ui.small_button("reset")
                    .on_hover_text(format!("Restore pinned: {auto_older} -> {auto_newer}"))
                    .clicked()
                {
                    state.chromium_overrides.insert(key,
                        (auto_older.clone(), auto_newer.clone()));
                }
            } else {
                let pinned_text = match (auto_older.is_empty(), auto_newer.is_empty()) {
                    (false, false) => format!("pinned: {auto_older} -> {auto_newer}"),
                    _              => "pinned: (unknown)".to_string(),
                };
                super::app::weak_label(ui, pinned_text);
            }
            // Per-tag metadata fetch — populates the pinned Chromium
            // version for an installed bracket endpoint that isn't in
            // the currently-loaded available window. One API call per
            // missing tag; results upserted to sqlite.
            let missing: Vec<&str> = [
                (older, &auto_older), (newer, &auto_newer)
            ].iter()
                .filter_map(|(tag, val)| if val.is_empty() { Some(*tag) } else { None })
                .collect();
            if !missing.is_empty() {
                let any_in_flight = missing.iter().any(|t|
                    state.tag_fetch_pending.contains(*t));
                let label = if any_in_flight { "Fetching…" }
                            else             { "Fetch tag info" };
                if ui.add_enabled(!any_in_flight, egui::Button::new(label))
                    .on_hover_text(format!(
                        "Fetch GitHub release metadata for: {}\n\nUses one API call \
                         per missing tag — useful when the tag is older than the \
                         current Available fetch window.",
                        missing.join(", ")))
                    .clicked()
                {
                    for tag in &missing {
                        spawn_tag_metadata_fetch(state, (*tag).to_string());
                    }
                }
            }
        });

        if let Some(err) = state.compare_errors.get(channel) {
            ui.colored_label(Color32::from_rgb(220, 80, 80),
                format!("compare failed: {err}"));
        }

        let Some(cr) = state.compare_results.get(channel).cloned() else { return; };
        ui.horizontal(|ui| {
            super::app::weak_label(ui, format!(
                "{} {}..{}  ·  showing {} of {}{}",
                if cr.commits.is_empty() { "no commits" } else { "" },
                cr.base, cr.head, cr.commits.len(), cr.total,
                if cr.truncated { " (capped at 250 by GitHub — open on GitHub for full list)" } else { "" }
            ));
        });
        let row_h = ui.spacing().interact_size.y + 2.0;
        egui::ScrollArea::vertical().id_source(("compare_commits", channel))
            .max_height(row_h * 8.0)
            .auto_shrink([false, true]).show(ui, |ui|
        {
            for c in &cr.commits {
                ui.horizontal(|ui| {
                    if ui.add(egui::Label::new(
                            RichText::new(&c.short).monospace()
                                .color(Color32::from_rgb(140, 180, 220)))
                            .sense(egui::Sense::click()))
                        .on_hover_text(format!("Open commit on GitHub:\n{}", c.html_url))
                        .clicked()
                    {
                        open_url(&c.html_url);
                    }
                    let date_short = c.date.split('T').next().unwrap_or(&c.date);
                    super::app::weak_label(ui, date_short.to_string());
                    super::app::weak_label(ui, c.author.to_string());
                    ui.label(&c.subject);
                });
            }
        });
    });
}

/// One-shot fetch of a full release by tag — single API call, no
/// pagination. Result lands in `slots.add_by_tag_done` for app.rs to
/// merge into state.available; also upserted to sqlite when
/// incremental cache mode is on so the row sticks across sessions.
fn spawn_add_by_tag(state: &mut AppState, tag: String) {
    state.adding_by_tag = true;
    state.status_msg = format!("fetching release by tag: {tag}…");
    crate::console::info(&state.console, "github",
        format!("add-by-tag: fetching releases/tags/{tag} (single API call)"));
    let token = state.github_token.clone();
    let slot  = state.slots.add_by_tag_done.clone();
    let incremental = state.incremental_release_cache;
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        let result = versions::github::fetch_release_by_tag(&tag, tok).await
            .map_err(|e| format!("{e:#}"))
            .map(|r| {
                let skip_reason = r.skip_reason();
                let channel = match versions::github::detect_release_channel(&r) {
                    versions::github::Channel::Release => "Release",
                    versions::github::Channel::Beta    => "Beta",
                    versions::github::Channel::Nightly => "Nightly",
                }.to_string();
                let (asset_url, asset_size) = r.assets.iter()
                    .find(|a| Some(&a.name) == r.host_asset.as_ref())
                    .map(|a| (Some(a.browser_download_url.clone()), Some(a.size)))
                    .unwrap_or((None, None));
                let chromium_version = parse_chromium_version(&r.name);
                let _ = verdict::upsert_tag_metadata(
                    &r.tag, chromium_version.as_deref(),
                    Some(&r.published_at), Some(&channel));
                let mut row = ReleaseRow {
                    tag: r.tag, published_at: r.published_at,
                    host_asset: r.host_asset, asset_url, asset_size,
                    skip_reason, cached: false, channel, chromium_version,
                };
                row.refresh_cached();
                if incremental {
                    if let Ok(json) = serde_json::to_string(&row) {
                        let _ = verdict::upsert_release_cache_row(&row.tag, &json);
                    }
                }
                row
            });
        *slot.lock().unwrap() = Some(result);
    });
}

/// Fire a one-shot GitHub fetch for `tag`'s release metadata, parse the
/// pinned Chromium version + channel out of it, upsert into the sqlite
/// `tag_metadata` cache. The bracket panel re-renders next frame and
/// picks up the populated values via the cache fallback.
fn spawn_tag_metadata_fetch(state: &mut AppState, tag: String) {
    if state.tag_fetch_pending.contains(&tag) { return; }
    state.tag_fetch_pending.insert(tag.clone());
    state.status_msg = format!("fetching tag info: {tag}…");
    let token = state.github_token.clone();
    let slot  = state.slots.tag_metadata_done.clone();
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        let result = versions::github::fetch_release_metadata(&tag, tok).await
            .map_err(|e| format!("{e:#}"))
            .and_then(|(name, published_at, prerelease)| {
                let chromium = parse_chromium_version(&name);
                // Channel guess: prefix-match the title; fallback to
                // prerelease flag (Stable=false; Beta/Nightly=true,
                // best-guess Nightly when ambiguous).
                let channel = {
                    let lc = name.trim_start().to_lowercase();
                    if lc.starts_with("nightly ") || lc.starts_with("nightly v") { "Nightly" }
                    else if lc.starts_with("beta ") || lc.starts_with("beta v") { "Beta" }
                    else if lc.starts_with("release ") || lc.starts_with("release v") { "Release" }
                    else if prerelease { "Nightly" } else { "Release" }
                };
                crate::verdict::upsert_tag_metadata(
                    &tag,
                    chromium.as_deref(),
                    Some(&published_at),
                    Some(channel),
                ).map_err(|e| format!("sqlite upsert: {e}"))
            });
        slot.lock().unwrap().push((tag, result));
    });
}

fn spawn_compare(state: &mut AppState, channel: String, older: String, newer: String) {
    state.compare_loading.insert(channel.clone());
    state.compare_errors.remove(&channel);
    state.status_msg = format!("loading commits {older}...{newer}… [{channel}]");
    let token = state.github_token.clone();
    let slot  = state.slots.compare_done.clone();
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        let result = versions::github::compare_commits(&older, &newer, tok).await
            .map_err(|e| format!("{e:#}"));
        slot.lock().unwrap().push((channel, result));
    });
}

pub(super) fn spawn_fetch(state: &mut AppState) {
    // When a custom date filter is active, bump the effective count so the
    // fetch reaches farther back in history (Brave averages ~3 nightly
    // tags/day; 600 ≈ ~7 months of coverage). Without a filter we honour
    // the user's "Releases to fetch" setting (default 50).
    let date_filter_active = state.date_from.is_some() || state.date_to.is_some();
    let count = if date_filter_active {
        state.release_count.max(600)
    } else {
        state.release_count
    };
    state.fetching_releases = true;
    state.fetching_started = Some(std::time::Instant::now());
    state.status_msg = if state.date_from.is_some() {
        format!("fetching tags back to {}…",
                state.date_from.map(|d| d.to_string()).unwrap_or_default())
    } else if count != state.release_count {
        format!("fetching {count} tags… (expanded for date filter)")
    } else {
        format!("fetching {count} tags…")
    };
    // Pre-fetch summary — confirms what we're about to walk and
    // whether the request is going through anonymous (60 req/hr) or
    // token-authenticated (5000 req/hr) GitHub API quota. Helps
    // when troubleshooting slow / rate-limited fetches.
    let chans_str = {
        let mut v: Vec<&str> = Vec::new();
        if state.channel_release { v.push("Release"); }
        if state.channel_beta    { v.push("Beta"); }
        if state.channel_nightly { v.push("Nightly"); }
        if v.is_empty() { "Nightly".to_string() } else { v.join("+") }
    };
    let auth_str = if !state.github_token.is_empty() { "token (5000/hr)" }
                   else                              { "anonymous (60/hr)" };
    let stop_str = state.date_from.map(|d| format!("stop_at={d}"))
        .unwrap_or_else(|| "no stop_at".to_string());
    let inc_str = if state.incremental_release_cache { "incremental=on" }
                  else                                { "incremental=off" };
    crate::console::info(&state.console, "github", format!(
        "fetch start: count<={count}  channels={chans_str}  {stop_str}  \
         {inc_str}  auth={auth_str}"));
    // Snapshot the oldest cached release date so the async task can
    // decide whether incremental's known-tag short-circuit is safe to
    // apply (it isn't when the user is asking for something deeper
    // than the cache currently covers).
    let oldest_cached: Option<chrono::NaiveDate> = state.available.iter()
        .filter_map(|r| r.published_at.get(..10))
        .filter_map(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .min();
    let slot          = state.slots.available.clone();
    let partial_slot  = state.slots.partial_releases.clone();
    let token         = state.github_token.clone();
    // When the user has set a `from` date, pass it as `stop_at` so the
    // fetcher halts once it has reached that date — saves API calls when
    // the user only cares about a recent date window.
    let stop_at       = state.date_from;
    // Incremental mode: fetch ALL channels (filter applied client-side
    // only) so the cache always grows uniformly and switching channels
    // later doesn't require a re-fetch. Off mode keeps the GUI's
    // current channel filter as the server-side filter.
    let incremental = state.incremental_release_cache;
    let filter = if incremental {
        versions::github::ChannelFilter { release: true, beta: true, nightly: true }
    } else {
        versions::github::ChannelFilter {
            release: state.channel_release,
            beta:    state.channel_beta,
            nightly: state.channel_nightly,
        }
    };
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        // Helper: convert a Vec<github::Release> → Vec<ReleaseRow> for the GUI.
        // Also persists each row to sqlite `release_cache` when incremental
        // mode is on, so future fetches can short-circuit on known tags.
        fn to_rows(rs: Vec<versions::github::Release>, incremental: bool) -> Vec<ReleaseRow> {
            rs.into_iter().map(|r| {
                let skip_reason = r.skip_reason();
                let channel = match versions::github::detect_release_channel(&r) {
                    versions::github::Channel::Release => "Release",
                    versions::github::Channel::Beta    => "Beta",
                    versions::github::Channel::Nightly => "Nightly",
                }.to_string();
                let (asset_url, asset_size) = r.assets.iter()
                    .find(|a| Some(&a.name) == r.host_asset.as_ref())
                    .map(|a| (Some(a.browser_download_url.clone()), Some(a.size)))
                    .unwrap_or((None, None));
                let chromium_version = parse_chromium_version(&r.name);
                let _ = verdict::upsert_tag_metadata(
                    &r.tag,
                    chromium_version.as_deref(),
                    Some(&r.published_at),
                    Some(&channel),
                );
                let mut row = ReleaseRow {
                    tag: r.tag,
                    published_at: r.published_at,
                    host_asset: r.host_asset,
                    asset_url, asset_size,
                    skip_reason,
                    cached: false,
                    channel,
                    chromium_version,
                };
                row.refresh_cached();
                if incremental {
                    if let Ok(json) = serde_json::to_string(&row) {
                        let _ = verdict::upsert_release_cache_row(&row.tag, &json);
                    }
                }
                row
            }).collect()
        }
        // Stream each page of results into the partial slot. The GUI's
        // drain loop picks them up between frames and re-renders.
        //
        // Honor the known-tag short-circuit only when the user isn't
        // explicitly asking for a date deeper than the cache. If
        // stop_at is older than the oldest cached release, breaking
        // out on the first known tag would leave the requested deep
        // range un-fetched — pass None instead so the fetcher walks
        // all the way back to stop_at, then everything new along the
        // way is upserted into sqlite as usual.
        let need_deeper_walk = match (stop_at, oldest_cached) {
            (Some(want), Some(have)) => want < have,
            (Some(_),    None)       => true,
            _                        => false,
        };
        let use_known = incremental && !need_deeper_walk;
        let known = if use_known { verdict::known_release_cache_tags() }
                    else         { Default::default() };
        let result = if use_known {
            versions::github::list_nightly_releases_streaming_incremental(
                count, tok, stop_at, filter, &known,
                |partial| {
                    let rows = to_rows(partial, incremental);
                    *partial_slot.lock().unwrap() = Some(rows);
                }).await
                .map(|rs| to_rows(rs, incremental))
                .map_err(|e| e.to_string())
        } else {
            versions::github::list_nightly_releases_streaming(
                count, tok, stop_at, filter,
                |partial| {
                    let rows = to_rows(partial, incremental);
                    *partial_slot.lock().unwrap() = Some(rows);
                }).await
                .map(|rs| to_rows(rs, incremental))
                .map_err(|e| e.to_string())
        };
        *slot.lock().unwrap() = Some(result);
    });
}

/// Display label for a verdict — appears in the row dot, the row's tag
/// text, and the per-row combo dropdown. Kept short so the combo doesn't
/// push the Open / Del buttons off the row.
fn verdict_label(v: Verdict) -> &'static str {
    match v {
        Verdict::Good     => "GOOD",
        Verdict::Bad      => "BAD",
        Verdict::Buggy    => "BUGGY",
        Verdict::Unsure   => "UNSURE",
        Verdict::Untested => "NEW",   // short for "untested / not yet run"
        Verdict::Unknown  => "Clear",
    }
}

/// Display colour for a verdict. One source of truth — both the row dot
/// and the dropdown's per-option label use this so they stay in sync.
fn verdict_color(v: Verdict) -> Color32 {
    match v {
        Verdict::Good     => Color32::from_rgb( 60, 200,  90),  // green
        Verdict::Bad      => Color32::from_rgb(220,  70,  70),  // red
        Verdict::Buggy    => Color32::from_rgb(220, 130,  60),  // orange
        Verdict::Unsure   => Color32::from_rgb(220, 200,  60),  // yellow
        Verdict::Untested => Color32::from_rgb(130, 160, 200),  // blue-grey
        Verdict::Unknown  => Color32::from_rgb(150, 150, 150),  // neutral grey
    }
}

fn short_date(iso: &str) -> String {
    iso.split('T').next().unwrap_or(iso).to_string()
}

fn format_progress_text(p: &crate::versions::install::DownloadProgress) -> String {
    let dl    = format_bytes(p.downloaded);
    let total = format_bytes(p.total);
    let speed = if p.speed_bps == 0 { "—".to_string() }
                else { format!("{}/s", format_bytes(p.speed_bps)) };
    let eta = match p.eta_secs() {
        Some(s) if s < 3600 => format!(" · {}:{:02} left", s / 60, s % 60),
        Some(s)             => format!(" · {}:{:02}:{:02} left", s / 3600, (s / 60) % 60, s % 60),
        None                => String::new(),
    };
    format!("{dl} / {total} · {speed}{eta}")
}

fn format_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let v = b as f64;
    if v >= GB      { format!("{:.2} GB", v / GB) }
    else if v >= MB { format!("{:.1} MB", v / MB) }
    else if v >= KB { format!("{:.0} KB", v / KB) }
    else            { format!("{b} B") }
}

/// Return true when `published_at` (RFC3339) falls within the inclusive
/// `[from, to]` window. Either bound being `None` disables that side.
/// Whether `ym_combo`'s `(year, month)` selection should resolve to the
/// first day of that month (Start, used for the lower bound) or the last
/// day (End, used for the upper bound).
#[derive(Copy, Clone, PartialEq)]
enum EndOfMonth { Start, End }

const MONTH_NAMES: [&str; 12] = [
    "Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec",
];

/// Year + Month dropdowns with a small clear button. The year list is
/// strictly `[DATE_MIN_YEAR, today.year()]` — there is no UI path to
/// reach a year outside that range. Selecting a (year, month) writes the
/// resolved date into `value` (1st of month for Start, last day for End).
/// The "×" button clears `value` back to None.
fn ym_combo(
    ui: &mut egui::Ui,
    id_source: &str,
    value: &mut Option<chrono::NaiveDate>,
    today: chrono::NaiveDate,
    eom: EndOfMonth,
    config_dirty: &mut bool,
) {
    let max_year = today.year();
    // Default-display value when none is set. Show today's year/month so
    // the dropdown previews are meaningful; we don't write anything to
    // `value` until the user actually picks something.
    let default_date = match eom {
        EndOfMonth::Start => chrono::NaiveDate::from_ymd_opt(DATE_MIN_YEAR, 1, 1).unwrap(),
        EndOfMonth::End   => today,
    };
    let effective = value.unwrap_or(default_date);
    let initial_year  = effective.year().clamp(DATE_MIN_YEAR, max_year);
    let initial_month = effective.month();
    let mut year  = initial_year;
    let mut month = initial_month;

    egui::ComboBox::from_id_source((id_source, "y")).width(64.0)
        .selected_text(year.to_string())
        .show_ui(ui, |ui| {
            for y in DATE_MIN_YEAR..=max_year {
                ui.selectable_value(&mut year, y, y.to_string());
            }
        });

    egui::ComboBox::from_id_source((id_source, "m")).width(60.0)
        .selected_text(MONTH_NAMES[(month - 1) as usize])
        .show_ui(ui, |ui| {
            for (i, name) in MONTH_NAMES.iter().enumerate() {
                ui.selectable_value(&mut month, (i as u32) + 1, *name);
            }
        });

    if ui.small_button("×").on_hover_text("Clear").clicked() {
        if value.is_some() { *value = None; *config_dirty = true; }
        return;
    }

    // Only commit a new value when the user actually changed year or month.
    // Otherwise, opening the app with `value = None` would silently set
    // it to today on first render.
    if year != initial_year || month != initial_month || value.is_none() && (year, month) != (default_date.year(), default_date.month()) {
        let day = match eom {
            EndOfMonth::Start => 1,
            EndOfMonth::End   => days_in_month(year, month),
        };
        let new_date = chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap_or(default_date)
            .max(chrono::NaiveDate::from_ymd_opt(DATE_MIN_YEAR, 1, 1).unwrap())
            .min(today);
        if Some(new_date) != *value {
            *value = Some(new_date);
            *config_dirty = true;
        }
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11              => 30,
        2 if is_leap(year) => 29,
        2                  => 28,
        _ => 30,
    }
}
fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn date_in_range(published_at: &str,
                 from: Option<chrono::NaiveDate>,
                 to:   Option<chrono::NaiveDate>) -> bool {
    if from.is_none() && to.is_none() { return true; }
    let date_str = published_at.split('T').next().unwrap_or("");
    let d = match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return true, // unparseable → don't hide
    };
    if let Some(f) = from { if d < f { return false; } }
    if let Some(t) = to   { if d > t { return false; } }
    true
}

fn open_url(url: &str) {
    #[cfg(windows)]
    { let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn(); }
    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(url).spawn(); }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if crate::wsl::is_wsl() {
            // explorer.exe accepts http(s) URLs and opens them in the
            // Windows default browser — friendlier than wsl-side xdg-open
            // when the user runs the GUI under WSLg.
            let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
            return;
        }
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Order Available rows by the user-selected column. Tag uses a
/// semver-aware compare so v1.91.10 sorts after v1.91.9; date and
/// channel/note are plain string compares; verdict uses a fixed rank
/// so [BAD] / [BUGGY] / [UNSURE] / [GOOD] / [NEW] / (none) cluster
/// predictably rather than by enum-discriminant order.
/// Sort an `order` index slice in place using the underlying `rows`
/// for comparisons — lets the caller hold the row data behind an
/// `Arc<Vec<…>>` without paying a deep clone every frame just to get
/// a sortable slice.
fn sort_available_indices(
    order: &mut [usize],
    rows: &[super::state::ReleaseRow],
    by: super::state::AvailSortColumn,
    asc: bool,
    verdicts_by_tag: &std::collections::HashMap<String, crate::verdict::Verdict>,
    notes_by_tag: &std::collections::HashMap<String, String>,
) {
    use super::state::AvailSortColumn as C;
    use crate::verdict::Verdict;
    let verdict_rank = |v: Verdict| -> u8 {
        match v {
            Verdict::Bad      => 0,
            Verdict::Buggy    => 1,
            Verdict::Unsure   => 2,
            Verdict::Good     => 3,
            Verdict::Untested => 4,
            Verdict::Unknown  => 5,
        }
    };
    order.sort_by(|&ia, &ib| {
        let a = &rows[ia];
        let b = &rows[ib];
        let ord = match by {
            C::Tag => {
                let pa = semver::Version::parse(a.tag.trim_start_matches('v')).ok();
                let pb = semver::Version::parse(b.tag.trim_start_matches('v')).ok();
                match (pa, pb) {
                    (Some(va), Some(vb)) => va.cmp(&vb),
                    _ => a.tag.cmp(&b.tag),
                }
            }
            C::Date => {
                // Compare only the YYYY-MM-DD prefix that the column
                // actually displays — sorting on the full timestamp would
                // let an older-tag release published later in the day
                // jump above a newer tag with the same visible date.
                let a_day = a.published_at.get(..10).unwrap_or(&a.published_at);
                let b_day = b.published_at.get(..10).unwrap_or(&b.published_at);
                a_day.cmp(b_day)
            }
            C::Channel => a.channel.cmp(&b.channel),
            C::Verdict => {
                let ra = verdict_rank(verdicts_by_tag.get(&a.tag).copied().unwrap_or(Verdict::Unknown));
                let rb = verdict_rank(verdicts_by_tag.get(&b.tag).copied().unwrap_or(Verdict::Unknown));
                ra.cmp(&rb)
            }
            C::Note => {
                // Two-key sort: rows with notes first, then by note body.
                let empty = String::new();
                let na = notes_by_tag.get(&a.tag).unwrap_or(&empty);
                let nb = notes_by_tag.get(&b.tag).unwrap_or(&empty);
                let pa = if na.is_empty() { 1 } else { 0 };
                let pb = if nb.is_empty() { 1 } else { 0 };
                pa.cmp(&pb).then(na.cmp(nb))
            }
        };
        // Tag-asc as the stable secondary key — equal primary keys sort
        // by tag so the row order is deterministic between repaints.
        let ord = ord.then_with(|| a.tag.cmp(&b.tag));
        if asc { ord } else { ord.reverse() }
    });
}

/// Pull the pinned Chromium version (e.g. `147.0.7727.137`) out of a
/// Brave release title. Brave's titles are always shaped
/// `<Channel> v<brave> (Chromium <chromium>)`. Returns `None` for any
/// title that doesn't match — old caches lacking this field also see
/// `None` (pre-existing rows from before this column existed).
fn parse_chromium_version(title: &str) -> Option<String> {
    let start = title.find("Chromium ")? + "Chromium ".len();
    let tail = &title[start..];
    let end = tail.find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(tail.len());
    let version = &tail[..end];
    // Sanity-check: must be a four-segment dotted decimal.
    if version.split('.').count() == 4 && !version.is_empty() {
        Some(version.to_string())
    } else {
        None
    }
}

/// Sort installed tags newest-first using semver where parseable; falls
/// back to lexicographic ordering for any tag that isn't `vMAJOR.MINOR.PATCH`.
fn sort_tags_newest_first(tags: &mut [String]) {
    tags.sort_by(|a, b| {
        let pa = semver::Version::parse(a.trim_start_matches('v')).ok();
        let pb = semver::Version::parse(b.trim_start_matches('v')).ok();
        match (pa, pb) {
            (Some(a), Some(b)) => b.cmp(&a),
            _ => b.cmp(a),
        }
    });
}

/// Shorten a long path for display while keeping the rightmost segments
/// (which carry the most meaningful info — the install dir name).
/// Returns the full path unchanged if it's already at or below `max_chars`,
/// otherwise an ellipsised form like `…/last/two/segments`.
fn truncate_path(full: &str, max_chars: usize) -> String {
    if full.chars().count() <= max_chars {
        return full.to_string();
    }
    let sep = if full.contains('\\') && !full.contains('/') { '\\' } else { '/' };
    let segs: Vec<&str> = full.split(sep).filter(|s| !s.is_empty()).collect();
    // Greedily keep tail segments until we hit the budget.
    let mut acc = String::new();
    for s in segs.iter().rev() {
        let candidate = if acc.is_empty() { s.to_string() }
                        else { format!("{s}{sep}{acc}") };
        if candidate.chars().count() + 2 > max_chars { break; }
        acc = candidate;
    }
    if acc.is_empty() {
        // Single segment longer than the budget — clip from the left.
        let tail: String = full.chars().rev().take(max_chars.saturating_sub(1))
            .collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    } else {
        format!("…{sep}{acc}")
    }
}

/// Build a unique throwaway --user-data-dir path under the standard
/// profiles directory. Stamped with the tag and a UTC unix timestamp
/// so concurrent launches don't collide and so the user can tell
/// disposable profiles apart at a glance. Cleanup is the user's job
/// (the GUI doesn't auto-purge — these can be useful for forensics
/// after a crash).
pub(crate) fn throwaway_profile_dir(tag: &str) -> std::path::PathBuf {
    let stamp = chrono::Utc::now().timestamp();
    let safe_tag: String = tag.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    crate::paths::profiles_dir().join(format!("throwaway-{safe_tag}-{stamp}"))
}

/// Read `<user-data-dir>/Local State` and emit two pieces of context
/// to the Console: a sub-profile inventory plus a version-mismatch
/// warning if the launching Brave version is older than whatever last
/// wrote the profile (older Brave can't safely open newer schemas).
fn describe_local_state(
    console: &crate::console::Handle,
    user_data_dir: &std::path::Path,
    launching_tag: &str,
) {
    let path = user_data_dir.join("Local State");
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let json: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Sub-profile inventory: collect names from profile.info_cache
    // (preferred — that's what the profile picker actually uses) plus
    // last_used. Falls back to dir-scanning when info_cache is empty.
    let last_used = json.pointer("/profile/last_used")
        .and_then(|v| v.as_str())
        .unwrap_or("Default")
        .to_string();
    let mut profiles: Vec<String> = json.pointer("/profile/info_cache")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    if profiles.is_empty() {
        if let Ok(entries) = std::fs::read_dir(user_data_dir) {
            profiles = entries.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n == "Default" || n.starts_with("Profile "))
                .collect();
        }
    }
    profiles.sort();
    crate::console::info(console, "profile", format!(
        "sub-profiles in this user-data-dir: [{}]  last_used={last_used}  \
         (override with --profile-directory=<name> in extra args)",
        profiles.join(", ")));

    // Version-mismatch warning. Local State stores e.g.
    // "1.93.45.0"; the launching tag is "v1.92.15". Strip prefixes
    // and split into integer components for a safe non-semver compare.
    let last_ver = json.pointer("/browser/last_browser_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if last_ver.is_empty() { return; }
    let parts = |s: &str| -> Vec<u32> {
        s.trim_start_matches(['v', 'V']).split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };
    let cur = parts(launching_tag);
    let prev = parts(last_ver);
    if cur.is_empty() || prev.is_empty() { return; }
    if cur < prev {
        crate::console::warn(console, "profile", format!(
            "schema downgrade risk: this profile was last opened by Brave \
             {last_ver}; launching {launching_tag} (older). Newer Brave \
             often migrates Preferences/Local State irreversibly — older \
             Brave may refuse to open the profile (clean exit within a \
             few seconds), or open in a degraded state. Use a Brave \
             version >= {last_ver} to read the profile safely."));
    }
}

/// Pattern-match common launch-failure OS error strings and return a
/// short actionable hint when we recognise one. Returns None when we
/// don't have a known answer — the raw OS message stays visible either
/// way, this is purely additive guidance.
pub(super) fn launch_failure_hint(raw: &str) -> Option<&'static str> {
    let lc = raw.to_lowercase();
    // Windows ERROR_SXS_CANT_GEN_ACTCTX (14001) — old Brave needs an
    // older VC++ Redistributable than the host has installed. Also
    // matches the human-readable "side-by-side configuration" message.
    if lc.contains("os error 14001") || lc.contains("side-by-side") {
        return Some("install the Microsoft Visual C++ Redistributable \
                     (vc_redist.x64.exe, latest 2015–2022) and reboot. \
                     Very old Brave versions may also need the 2013 \
                     redist: https://aka.ms/vs/17/release/vc_redist.x64.exe");
    }
    // Windows ERROR_EXE_MACHINE_TYPE_MISMATCH (216) — wrong-arch PE.
    // The Install button now refuses these but very old already-installed
    // versions on disk can still trip it.
    if lc.contains("os error 216")
        || lc.contains("not compatible with the version of windows")
    {
        return Some("the on-disk brave.exe is the wrong CPU architecture \
                     for this host. Uninstall (Del), then re-install — \
                     the picker now refuses cross-arch zips.");
    }
    // Windows ERROR_FILE_NOT_FOUND (2) — usually means brave.exe wasn't
    // produced by extraction (missing top-level dir name change, etc.).
    if lc.contains("os error 2") && lc.contains("brave") {
        return Some("brave.exe is missing from the install directory. \
                     Try uninstalling and re-installing the version.");
    }
    None
}

/// True when `asset_name` clearly targets the opposite CPU architecture
/// of the running host — used to defend against a stale releases.json
/// cache where the OLD Windows picker selected an arm64 zip on an x64
/// host. Conservative: only flags names with explicit arm/x64 markers.
fn is_opposite_arch_asset(asset_name: &str) -> bool {
    let l = asset_name.to_lowercase();
    let host_arch = std::env::consts::ARCH;
    let host_arm = host_arch == "aarch64";
    let asset_arm = l.contains("arm64") || l.contains("aarch64") || l.contains("-arm");
    let asset_x64 = (l.contains("x64") || l.contains("amd64")) && !asset_arm;
    if host_arm { asset_x64 } else { asset_arm }
}

/// Wipe every file in `cache/downloads/`. Returns `(file_count, bytes)`
/// for the status-bar summary. Subdirectories are recursed; the
/// downloads dir itself is preserved (re-creating it would fight with
/// `paths::ensure_dirs()` on the next install).
fn remove_cached_downloads() -> std::io::Result<(usize, u64)> {
    let dir = crate::paths::downloads_dir();
    if !dir.exists() { return Ok((0, 0)); }
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let p = entry.path();
        let md = entry.metadata()?;
        if md.is_dir() {
            // Walk the subtree to tally bytes before removing.
            for s in walkdir::WalkDir::new(&p).into_iter().flatten() {
                if s.file_type().is_file() {
                    bytes += s.metadata().map(|m| m.len()).unwrap_or(0);
                    count += 1;
                }
            }
            std::fs::remove_dir_all(&p)?;
        } else {
            bytes += md.len();
            count += 1;
            std::fs::remove_file(&p)?;
        }
    }
    Ok((count, bytes))
}

fn open_in_explorer(path: &std::path::Path) {
    #[cfg(windows)]
    { let _ = std::process::Command::new("explorer").arg(path).spawn(); }
    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(path).spawn(); }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if crate::wsl::is_wsl() {
            let win_path = std::process::Command::new("wslpath").arg("-w").arg(path).output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string());
            if let Some(win) = win_path {
                let _ = std::process::Command::new("explorer.exe").arg(win).spawn();
                return;
            }
        }
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}
