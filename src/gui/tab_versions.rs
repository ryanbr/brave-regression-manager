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
    ui.horizontal_wrapped(|ui| {
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
        let mut hide = state.hide_no_installer;
        if ui.checkbox(&mut hide, "Hide releases with no installer for this platform").changed() {
            state.hide_no_installer = hide;
            state.config_dirty = true;
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
        }
        for (label, days) in [("7d", 7i64), ("30d", 30), ("60d", 60), ("90d", 90), ("120d", 120), ("150d", 150)] {
            if ui.small_button(label).clicked() {
                state.date_to   = Some(today);
                state.date_from = Some(clamp_date(today - chrono::Duration::days(days)));
                state.config_dirty = true;
            }
        }

        // Auto-refetch on any date filter change so the new `stop_at`
        // (date_from) is honoured by the fetcher — pulls in extra pages
        // when the user widens the window backward, and stops earlier
        // when they narrow it. Skip when a fetch is already in flight or
        // when we have nothing cached yet (let the user click Fetch
        // explicitly the first time).
        let date_changed  = state.date_from != prev_from || state.date_to != prev_to;
        let filter_active = state.date_from.is_some() || state.date_to.is_some();
        if date_changed && filter_active
            && !state.available.is_empty() && !state.fetching_releases
        {
            spawn_fetch(state);
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
                    state.release_count = new_count;
                    state.config_dirty = true;
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
                        if !state.fetching_releases {
                            spawn_fetch(state);
                        }
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
                    let label = if state.default_profile_dir.is_empty() {
                        "Browse…".to_string()
                    } else {
                        let short = std::path::Path::new(&state.default_profile_dir)
                            .file_name().and_then(|s| s.to_str())
                            .unwrap_or(state.default_profile_dir.as_str())
                            .to_string();
                        format!("{short}")
                    };
                    if ui.add_enabled(state.default_profile_dir_enabled,
                                      egui::Button::new(label))
                        .on_hover_text(if state.default_profile_dir.is_empty() {
                            "Pick the default user-data-dir".to_string()
                        } else { state.default_profile_dir.clone() })
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

                ui.label("GitHub token:").on_hover_text(
                    "Optional — bumps anonymous 60 req/hr to 5,000 req/hr. \
                     https://github.com/settings/tokens (no scopes needed).");
                let mut tok = state.github_token.clone();
                if ui.add(egui::TextEdit::singleline(&mut tok)
                    .password(true).desired_width(220.0)).changed()
                {
                    state.github_token = tok;
                    state.config_dirty = true;
                }
                ui.end_row();
            });
            super::app::weak_label(ui, format!("Date range minimum: {} (Brave Nightly history starts here)",
                            min_allowed_date()));
        });

    ui.separator();

    // Installed list stays compact (7 rows). Available list fills the
    // remainder of the window so we don't get a big blank area below it.
    let row_h = ui.spacing().interact_size.y + 2.0;
    let installed_h = row_h * 7.0;

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
    let mut goods: Vec<(usize, String)> = Vec::new();
    let mut bads:  Vec<(usize, String)> = Vec::new();
    for (i, tag) in sorted_tags.iter().enumerate() {
        match verdict::version_verdict(tag).unwrap_or(Verdict::Unknown) {
            Verdict::Good => goods.push((i, tag.clone())),
            Verdict::Bad  => bads.push((i, tag.clone())),
            // BUGGY / UNSURE / UNTESTED / Unknown don't anchor a bracket.
            // Only firm GOOD ↔ BAD pairs trigger the compare panel.
            _ => {}
        }
    }
    let mut bracket: Option<(String, String, String, String)> = None; // (older, newer, good, bad)
    let mut best_dist = usize::MAX;
    for (gi, gt) in &goods {
        for (bi, bt) in &bads {
            let d = if gi > bi { gi - bi } else { bi - gi };
            if d < best_dist {
                best_dist = d;
                let (older, newer) = if gi > bi { (gt.clone(), bt.clone()) } else { (bt.clone(), gt.clone()) };
                bracket = Some((older, newer, gt.clone(), bt.clone()));
            }
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
        for v in &installed {
            ui.horizontal(|ui| {
                let verdict = verdict::version_verdict(&v.tag).unwrap_or(Verdict::Unknown);
                let dot_color = verdict_color(verdict);
                // Use a basic asterisk-style bullet that egui's default font
                // definitely supports — `●` (U+25CF) was rendering as a tofu square.
                ui.colored_label(dot_color, "•");
                // Color + bold the version number by its verdict so the row's
                // status is readable at a glance even if you ignore the bullet.
                ui.label(RichText::new(&v.tag).monospace().strong().color(dot_color));
                ui.label(v.folder.display().to_string());

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
                    let custom = {
                        let per_row = verdict::user_data_dir(&v.tag);
                        if !per_row.is_empty() {
                            Some(std::path::PathBuf::from(per_row))
                        } else if state.default_profile_dir_enabled
                            && !state.default_profile_dir.is_empty()
                        {
                            Some(std::path::PathBuf::from(&state.default_profile_dir))
                        } else {
                            None
                        }
                    };
                    let effective_user_data = custom.clone()
                        .unwrap_or_else(|| crate::paths::profile_dir(&profile));
                    match versions::launch::launch_with_console(&v.tag, &profile, state.console.clone(), state.brave_log_level, state.freeze_components, extra_args, custom) {
                        Ok(child) => {
                            let msg = format!("launched {} (profile={})", v.tag,
                                effective_user_data.display());
                            crate::console::info(&state.console, "launch", &msg);
                            state.running.insert(v.tag.clone(), super::state::RunningBrave {
                                tag: v.tag.clone(),
                                profile: profile.clone(),
                                child,
                                user_data_dir: effective_user_data,
                            });
                            state.status_msg = msg;
                        }
                        Err(e) => {
                            let msg = format!("launch failed: {e:#}");
                            crate::console::error(&state.console, "launch", &msg);
                            state.status_msg = msg;
                        }
                    }
                }
                if state.running.contains_key(&v.tag) && ui.button("Stop").clicked() {
                    if let Some(mut r) = state.running.remove(&v.tag) {
                        let _ = r.child.kill();
                        let _ = r.child.wait();
                        state.status_msg = format!("stopped {}", v.tag);
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
                        .set_title(&format!("Pick user-data-dir for {}", v.tag));
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
                    .selected_text(verdict_label(current_verdict))
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
                            let txt = RichText::new(verdict_label(v)).color(verdict_color(v));
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
                        .desired_width(180.0)
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
                    if let Err(e) = std::fs::remove_dir_all(&v.folder) {
                        state.status_msg = format!("uninstall failed: {e}");
                    } else {
                        state.installed = versions::list_installed().unwrap_or_default();
                        state.status_msg = format!("uninstalled {}", v.tag);
                    }
                }
            });
        }
    });

    // ── Commits between bracketed tags (brave-core) ─────────────────────
    ui.separator();
    render_compare_section(ui, state, bracket.clone());

    ui.separator();
    let chans = {
        let mut v: Vec<&str> = Vec::new();
        if state.channel_release { v.push("Release"); }
        if state.channel_beta    { v.push("Beta"); }
        if state.channel_nightly { v.push("Nightly"); }
        if v.is_empty() { "Nightly".to_string() } else { v.join(" + ") }
    };
    let avail_heading_size = egui::TextStyle::Body.resolve(ui.style()).size + 2.0;
    ui.label(RichText::new(format!("Available on GitHub ({chans})"))
        .strong().size(avail_heading_size));

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
            super::app::weak_label(ui, "(click \"Fetch GitHub releases\" to populate)");
        }
        // Compute how many rows actually clear the active filters and the
        // oldest cached release date — used for the helpful empty-results
        // message below when filters hide everything.
        let mut shown = 0usize;
        let mut oldest: Option<&str> = None;
        for r in &rows {
            if let Some(o) = oldest {
                if r.published_at.as_str() < o { oldest = Some(&r.published_at); }
            } else {
                oldest = Some(&r.published_at);
            }
            let pass_installer = !(state.hide_no_installer && r.host_asset.is_none());
            let pass_date      = date_in_range(&r.published_at, state.date_from, state.date_to);
            if pass_installer && pass_date { shown += 1; }
        }
        if !rows.is_empty() && shown == 0 {
            let oldest_short = oldest.map(short_date).unwrap_or_default();
            let msg = if state.date_from.is_some() || state.date_to.is_some() {
                format!(
                    "0 of {} releases match the date filter. Cache only goes back to {}. \
                     Increase 'Releases to fetch' in Settings and re-fetch to load older tags.",
                    rows.len(), oldest_short)
            } else {
                format!("0 of {} releases pass the current filters.", rows.len())
            };
            ui.colored_label(Color32::from_rgb(220, 180, 60), msg);
        }
        for r in &rows {
            if state.hide_no_installer && r.host_asset.is_none() { continue; }
            if !date_in_range(&r.published_at, state.date_from, state.date_to) { continue; }
            ui.horizontal(|ui| {
                ui.monospace(&r.tag);
                ui.label(short_date(&r.published_at));
                let (chan_label, chan_color) = match r.channel.as_str() {
                    "Release" => ("Release", Color32::from_rgb( 80, 170, 240)),
                    "Beta"    => ("Beta",    Color32::from_rgb(220, 170,  60)),
                    "Nightly" => ("Nightly", Color32::from_rgb(160, 120, 220)),
                    _         => ("?",       Color32::from_rgb(150, 150, 150)),
                };
                ui.colored_label(chan_color, format!("[{chan_label}]"));

                // Note affordance: small clickable label that opens an
                // edit popup. `+` (dim) when no note exists; `note` (blue)
                // with the body as tooltip when one does.
                let cur_note = verdict::note(&r.tag);
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
                    state.editing_note_buf = cur_note;
                }

                let installed = versions::is_installed(&r.tag);
                let busy = installing_now.as_deref() == Some(r.tag.as_str());

                match (&r.host_asset, installed, busy) {
                    (_, true, _) => { ui.label("installed"); }
                    (None, _, _) => {
                        ui.colored_label(Color32::from_rgb(180, 130, 60),
                            format!("(skip) {}", r.skip_reason));
                        ui.add_enabled(false, egui::Button::new("Install"));
                    }
                    (Some(name), false, true) => {
                        ui.colored_label(Color32::from_rgb(60, 200, 90), name.to_string());
                        // Live progress bar with bytes / total / speed / ETA.
                        let progress = super::state::current_progress(&state.slots);
                        if let Some(p) = progress.filter(|p| p.tag == r.tag) {
                            let txt = format_progress_text(&p);
                            ui.add(egui::ProgressBar::new(p.fraction())
                                   .desired_width(260.0).show_percentage().text(txt));
                        } else {
                            ui.label("installing…");
                        }
                    }
                    (Some(name), false, false) => {
                        ui.colored_label(Color32::from_rgb(60, 200, 90), name.to_string());
                        if r.cached {
                            ui.colored_label(Color32::from_rgb(140, 180, 220), "[cached]");
                        }
                        let btn_label = if r.cached { "Install (cached)" } else { "Install" };
                        if ui.add_enabled(installing_now.is_none(),
                                          egui::Button::new(btn_label)).clicked() {
                            state.installing = Some(r.tag.clone());
                            state.status_msg = format!("installing {}…", r.tag);
                            let slot     = state.slots.install_done.clone();
                            let progress = state.slots.install_progress.clone();
                            *progress.lock().unwrap() = None;
                            let tag2     = r.tag.clone();
                            let name2    = name.clone();
                            let url      = r.asset_url.clone();
                            let size     = r.asset_size;
                            state.rt.spawn(async move {
                                let result = match (url, size) {
                                    // Fast path: we already have the URL + size from
                                    // the listing — skip the second GitHub call so we
                                    // don't burn another anonymous-rate-limit slot.
                                    (Some(u), Some(s)) =>
                                        versions::install::install_tag_with_asset(
                                            &tag2, &name2, &u, s, Some(progress)).await,
                                    _ =>
                                        versions::install::install_tag_with_progress(
                                            &tag2, Some(progress)).await,
                                };
                                let result = result.map(|p| p.display().to_string())
                                                   .map_err(|e| format!("{e:#}"));
                                *slot.lock().unwrap() = Some(result);
                            });
                        }
                        // Only show diagnose-button when the asset is actually
                        // on disk. When not cached we just don't render the
                        // button at all (no disabled placeholder).
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
    });

    render_note_editor(ui, state);
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

/// Renders the "Commits in range" panel under the Installed list. Shows
/// a Load button that fetches `brave/brave-core/compare/<older>...<newer>`
/// in the background; once the result lands, lists each commit with a
/// click-to-open link to its GitHub page.
fn render_compare_section(
    ui: &mut Ui,
    state: &mut AppState,
    bracket: Option<(String, String, String, String)>, // (older, newer, good, bad)
) {
    // Drop stale commits when the active bracket no longer matches what
    // we previously loaded (verdicts changed, a version was uninstalled,
    // bracket disappeared entirely, etc.).
    let stale = match (&bracket, &state.compare_result) {
        (Some((o, n, _, _)), Some(cr)) => &cr.base != o || &cr.head != n,
        (None,               Some(_))  => true,
        _                              => false,
    };
    if stale {
        state.compare_result = None;
        state.compare_error  = None;
    }

    let cmp_heading_size = egui::TextStyle::Body.resolve(ui.style()).size + 2.0;
    ui.label(RichText::new("Commits in bracket (brave-core)")
        .strong().size(cmp_heading_size));
    match &bracket {
        Some((older, newer, good, bad)) => {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(220, 180, 60),
                    format!("GOOD {good} ↔ BAD {bad}  (range {older}...{newer})"));
                let cur_matches = state.compare_result.as_ref()
                    .map(|c| c.base == *older && c.head == *newer).unwrap_or(false);
                let label = if state.compare_loading { "Loading…".to_string() }
                    else if cur_matches               { "Reload".to_string() }
                    else                              { format!("Load {older}...{newer}") };
                if ui.add_enabled(!state.compare_loading, egui::Button::new(label)).clicked() {
                    spawn_compare(state, older.clone(), newer.clone());
                }
                if ui.button("Open on GitHub").on_hover_text(format!(
                    "https://github.com/brave/brave-core/compare/{older}...{newer}")).clicked()
                {
                    let url = format!("https://github.com/brave/brave-core/compare/{older}...{newer}");
                    crate::console::info(&state.console, "compare", &url);
                    open_url(&url);
                }
                if state.compare_result.is_some() && ui.small_button("×")
                    .on_hover_text("Clear loaded commits").clicked()
                {
                    state.compare_result = None;
                    state.compare_error  = None;
                }
            });
        }
        None => {
            super::app::weak_label(ui,
                "(mark a version GOOD and another BAD to enable the compare panel)");
        }
    }

    if let Some(err) = &state.compare_error {
        ui.colored_label(Color32::from_rgb(220, 80, 80),
            format!("compare failed: {err}"));
    }

    let Some(cr) = state.compare_result.clone() else { return; };
    ui.horizontal(|ui| {
        super::app::weak_label(ui, format!(
            "{} {}..{}  ·  showing {} of {}{}",
            if cr.commits.is_empty() { "no commits" } else { "" },
            cr.base, cr.head, cr.commits.len(), cr.total,
            if cr.truncated { " (capped at 250 by GitHub — open on GitHub for full list)" } else { "" }
        ));
    });
    let row_h = ui.spacing().interact_size.y + 2.0;
    egui::ScrollArea::vertical().id_source("compare_commits")
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
                super::app::weak_label(ui, format!("{date_short}"));
                super::app::weak_label(ui, format!("{}", c.author));
                ui.label(&c.subject);
            });
        }
    });
}

fn spawn_compare(state: &mut AppState, older: String, newer: String) {
    state.compare_loading = true;
    state.compare_error   = None;
    state.status_msg      = format!("loading commits {older}...{newer}…");
    let token = state.github_token.clone();
    let slot  = state.slots.compare_done.clone();
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        let result = versions::github::compare_commits(&older, &newer, tok).await
            .map_err(|e| format!("{e:#}"));
        *slot.lock().unwrap() = Some(result);
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
    state.status_msg = if state.date_from.is_some() {
        format!("fetching tags back to {}…",
                state.date_from.map(|d| d.to_string()).unwrap_or_default())
    } else if count != state.release_count {
        format!("fetching {count} tags… (expanded for date filter)")
    } else {
        format!("fetching {count} tags…")
    };
    let slot          = state.slots.available.clone();
    let partial_slot  = state.slots.partial_releases.clone();
    let token         = state.github_token.clone();
    // When the user has set a `from` date, pass it as `stop_at` so the
    // fetcher halts once it has reached that date — saves API calls when
    // the user only cares about a recent date window.
    let stop_at       = state.date_from;
    let filter        = versions::github::ChannelFilter {
        release: state.channel_release,
        beta:    state.channel_beta,
        nightly: state.channel_nightly,
    };
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        // Helper: convert a Vec<github::Release> → Vec<ReleaseRow> for the GUI.
        fn to_rows(rs: Vec<versions::github::Release>) -> Vec<ReleaseRow> {
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
                let mut row = ReleaseRow {
                    tag: r.tag,
                    published_at: r.published_at,
                    host_asset: r.host_asset,
                    asset_url, asset_size,
                    skip_reason,
                    cached: false,
                    channel,
                };
                row.refresh_cached();
                row
            }).collect()
        }
        // Stream each page of results into the partial slot. The GUI's
        // drain loop picks them up between frames and re-renders.
        let result = versions::github::list_nightly_releases_streaming(count, tok, stop_at, filter, |partial| {
            let rows = to_rows(partial);
            *partial_slot.lock().unwrap() = Some(rows);
        }).await
            .map(to_rows)
            .map_err(|e| e.to_string());
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

/// Sort installed tags newest-first using semver where parseable; falls
/// back to lexicographic ordering for any tag that isn't `vMAJOR.MINOR.PATCH`.
fn sort_tags_newest_first(tags: &mut Vec<String>) {
    tags.sort_by(|a, b| {
        let pa = semver::Version::parse(a.trim_start_matches('v')).ok();
        let pb = semver::Version::parse(b.trim_start_matches('v')).ok();
        match (pa, pb) {
            (Some(a), Some(b)) => b.cmp(&a),
            _ => b.cmp(a),
        }
    });
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
