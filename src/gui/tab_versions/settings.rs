//! The collapsible Settings panel rendered in the Brave Versions tab,
//! the Adblock Lists tab, or both — controlled by
//! `state.settings_location`. Extracted from the (very large)
//! `tab_versions.rs` for sanity. All settings still write straight into
//! the shared `AppState`; there's no local state here.

use egui::Ui;
use crate::config::BraveLogLevel;
// The settings panel reaches into a few siblings of its parent
// `tab_versions` module — `gui::state` for AppState/types,
// `gui::app` for the theme + weak-label helpers — and a couple of
// sibling helpers in `tab_versions` itself.
use super::super::state::AppState;
use super::super::app;
use super::{min_allowed_date, spawn_fetch, remove_cached_downloads};
// `RELEASE_COUNT_OPTIONS` lives in the parent module's main file.
use super::RELEASE_COUNT_OPTIONS;

pub(crate) fn render_settings_panel(ui: &mut Ui, state: &mut AppState, id_suffix: &str) {
    // The outer CollapsingHeader was useful when this panel sat
    // inline at the top of the Versions / Lists tabs (didn't want
    // it to dominate the screen by default). Now that the panel
    // owns its own dedicated tab, the header was just an extra
    // click between the user and the content — content opens
    // immediately. The grid id is kept stable so egui memory
    // (column widths, etc.) survives the header's removal.
    egui::Grid::new(format!("settings_grid_{id_suffix}")).num_columns(2)
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
                    app::apply_theme(ui.ctx(), &state.theme);
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

                ui.label("Block Drive launcher:");
                let mut block = state.block_drive_launcher;
                if ui.checkbox(&mut block, "").on_hover_text(
                    "When ON: 'Application Launcher for Drive' (Chrome Web Store id \
                     lmjegmlicamnimmfhcmpkclmigmmcbeh) is added to \
                     extensions.install.deny_list in <user-data-dir>/Default/Preferences \
                     before every Brave launch. Brave refuses to load it on startup.\n\n\
                     When OFF: stock Brave behaviour. The id is NOT removed from the \
                     deny_list — toggle this off and re-launch, then edit Preferences \
                     manually if you want it back.\n\n\
                     The deny_list is merged with any existing entries; we never remove \
                     ids placed there by other tools."
                ).changed() {
                    state.block_drive_launcher = block;
                    state.config_dirty = true;
                    crate::console::info(&state.console, "config",
                        if block { "block_drive_launcher on next launch: ON" }
                        else     { "block_drive_launcher on next launch: OFF" });
                }
                ui.end_row();

                ui.label("Preferred external editor:")
                    .on_hover_text(
                        "Path to your text editor. The list editor's \
                         'Open in External editor' button (always shown \
                         in the bottom action row) hands the on-disk \
                         list.txt to this program. Empty falls back to \
                         the OS default handler.");
                ui.horizontal(|ui| {
                    let prev = state.preferred_external_editor.clone();
                    let resp = ui.add(egui::TextEdit::singleline(
                        &mut state.preferred_external_editor)
                        .desired_width(280.0)
                        .hint_text("e.g. C:\\Program Files\\Notepad++\\notepad++.exe"));
                    if ui.button("Browse…").on_hover_text(
                        "Pick the editor's executable. Avoids \
                         shell-quoting issues with paths containing spaces.")
                        .clicked()
                    {
                        if let Some(picked) = rfd::FileDialog::new()
                            .set_title("Pick text editor executable")
                            .pick_file()
                        {
                            state.preferred_external_editor = picked.display().to_string();
                        }
                    }
                    if !state.preferred_external_editor.is_empty()
                        && ui.small_button("x")
                            .on_hover_text("Clear (use OS default)")
                            .clicked()
                    {
                        state.preferred_external_editor.clear();
                    }
                    // Persist on Browse / clear (value diverges
                    // immediately) OR on textfield edit (only when
                    // focus is lost so we don't write per keystroke).
                    let textfield_change = resp.lost_focus()
                        && state.preferred_external_editor != prev;
                    let other_change = !resp.has_focus()
                        && state.preferred_external_editor != prev;
                    if textfield_change || other_change {
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config", format!(
                            "preferred_external_editor: {}",
                            if state.preferred_external_editor.is_empty() {
                                "(OS default)".into()
                            } else {
                                state.preferred_external_editor.clone()
                            }));
                    }
                });
                ui.end_row();

                ui.label("Auto-open URL on launch:");
                ui.horizontal(|ui| {
                    let prev_enabled = state.auto_open_url_enabled;
                    let prev_url     = state.auto_open_url.clone();
                    ui.checkbox(&mut state.auto_open_url_enabled, "")
                        .on_hover_text(
                            "When ON, the URL on the right is appended as a \
                             positional argument to every Brave launch. \
                             Chromium opens whatever's not a --flag as a tab \
                             on startup, so the page loads in the new window.\n\n\
                             Leave the URL field blank to disable without \
                             un-ticking.");
                    let resp = ui.add(egui::TextEdit::singleline(&mut state.auto_open_url)
                        .desired_width(280.0)
                        .hint_text("https://example.com/test-page"));
                    let changed = state.auto_open_url_enabled != prev_enabled
                               || (resp.lost_focus() && state.auto_open_url != prev_url);
                    if changed {
                        state.config_dirty = true;
                        let on = state.auto_open_url_enabled
                              && !state.auto_open_url.trim().is_empty();
                        crate::console::info(&state.console, "config", format!(
                            "auto_open_url on next launch: {}",
                            if on { state.auto_open_url.as_str() } else { "off" }));
                    }
                });
                ui.end_row();

                ui.label("Suppress P3A banner:");
                let mut p3a = state.suppress_p3a_banner;
                if ui.checkbox(&mut p3a, "").on_hover_text(
                    "When ON, the following keys are written to \
                     <user-data-dir>/Local State before every Brave launch:\n\
                       - brave.p3a.notice_acknowledged = true\n\
                       - brave.p3a.enabled             = false\n\
                       - brave.stats.reporting_enabled = false\n\
                     Hides Brave's first-run P3A telemetry consent banner \
                     and stops the P3A + DAU stats subsystems from pinging \
                     home. Atomic + verify; no-op when state is already \
                     correct (no per-launch backup file).\n\n\
                     When OFF: stock Brave behaviour. Existing values are \
                     NOT reset — turn on briefly to dismiss, then off if \
                     you want full Brave behaviour after."
                ).changed() {
                    state.suppress_p3a_banner = p3a;
                    state.config_dirty = true;
                    crate::console::info(&state.console, "config",
                        if p3a { "suppress_p3a_banner on next launch: ON" }
                        else   { "suppress_p3a_banner on next launch: OFF" });
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
                        // Toggling the parent off resets the session
                        // memo so the next time it's flipped back on,
                        // every tag starts with a brand-new throwaway.
                        if !state.clean_profile_per_launch {
                            state.session_throwaway_dirs.clear();
                        }
                        crate::console::info(&state.console, "config",
                            if state.clean_profile_per_launch {
                                "clean profile per launch: enabled".to_string()
                            } else {
                                "clean profile per launch: disabled".to_string()
                            });
                    }
                });
                ui.end_row();

                // Sub-toggle on its own grid row so it can't hide off
                // the right edge of the parent's column. Greyed out
                // when the parent isn't on (no behaviour to control).
                ui.label("  ↳ Reuse across relaunches:").on_hover_text(
                    "When on, the first launch of each tag in this \
                     session creates a throwaway dir and remembers \
                     it; subsequent relaunches re-use it so settings, \
                     lists, and cookies persist across relaunches. \
                     Restarting the app rotates to a fresh throwaway. \
                     Has no effect unless 'Clean profile per launch' \
                     is also enabled.");
                ui.horizontal(|ui| {
                    let prev_reuse = state.reuse_clean_profile;
                    ui.add_enabled(
                        state.clean_profile_per_launch,
                        egui::Checkbox::new(&mut state.reuse_clean_profile,
                            "Enabled"));
                    if prev_reuse != state.reuse_clean_profile {
                        state.config_dirty = true;
                        // Flipping reuse off discards any memoized
                        // session paths so the next launch is fresh.
                        if !state.reuse_clean_profile {
                            state.session_throwaway_dirs.clear();
                        }
                        crate::console::info(&state.console, "config", format!(
                            "reuse clean profile across relaunches: {}",
                            if state.reuse_clean_profile { "enabled" } else { "disabled" }));
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

                // Discoverable mirror of the Available header's
                // Clear -> Remove Cached files action — same behaviour
                // (wipes everything under <data-root>/cache/downloads/),
                // exposed here so the user doesn't have to scroll down
                // to find it.
                ui.label("Cached files:").on_hover_text(
                    "Delete every downloaded installer asset under \
                     cache/downloads/. Already-installed Brave versions \
                     are NOT touched — only the on-disk archives that \
                     drive the [cached] / Install (cached) shortcut.");
                ui.horizontal(|ui| {
                    if ui.button("Delete cached files").clicked() {
                        match remove_cached_downloads() {
                            Ok((n, bytes)) => {
                                // Refresh `cached` flags on every Available
                                // row so the [cached] pill / Install (cached)
                                // labels disappear next frame.
                                let dl_idx = super::super::state::read_downloads_index();
                                for r in std::sync::Arc::make_mut(&mut state.available).iter_mut() {
                                    r.refresh_cached_with(&dl_idx);
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
                    }
                    if ui.button("Clear args history")
                        .on_hover_text(
                            "Wipe the dropdown of recently-used custom \
                             launch args (the v menu next to each \
                             Installed row's args field). Per-tag args \
                             you've typed into individual rows are NOT \
                             touched — only the global recent-args list \
                             that the dropdown pulls from.")
                        .clicked()
                    {
                        // Snapshot the entries before we DELETE so the
                        // console line names every string being removed
                        // — useful when you want a paper trail of "what
                        // was in there" before nuking.
                        let snapshot = crate::verdict::recent_launch_args(usize::MAX)
                            .unwrap_or_default();
                        for (i, s) in snapshot.iter().enumerate() {
                            crate::console::info(&state.console, "config",
                                format!("  [{}] removing args entry: {s}", i + 1));
                        }
                        match crate::verdict::clear_launch_args_history() {
                            Ok(n) => {
                                state.launch_args_history_cache = None;
                                crate::console::info(&state.console, "config",
                                    format!("cleared {n} args history entr(ies)"));
                                state.status_msg = format!(
                                    "cleared {n} args history entr(ies)");
                            }
                            Err(e) => {
                                crate::console::error(&state.console, "config",
                                    format!("clear args history failed: {e:#}"));
                                state.status_msg = format!("clear failed: {e}");
                            }
                        }
                    }
                });

                ui.end_row();

                // Where to render this panel — the same widgets can
                // appear in the Brave Versions tab, the Adblock Lists
                // tab, or both. State change persists to config.toml
                // and the GUI reflects it next paint.
                ui.label("Settings location:").on_hover_text(
                    "Where to expose the Settings panel:\n\
                     • Brave Versions — only on the first tab (legacy)\n\
                     • Adblock Lists — only on the second tab\n\
                     • Both — render the same panel in both tabs (no \
                     state divergence; same fields).");
                ui.horizontal(|ui| {
                    let prev = state.settings_location.clone();
                    let mut sel = state.settings_location.clone();
                    egui::ComboBox::from_id_source(
                        format!("settings_location_combo_{id_suffix}"))
                        .width(160.0)
                        .selected_text(match sel.as_str() {
                            "lists"    => "Adblock Lists",
                            "both"     => "Both",
                            _          => "Brave Versions",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut sel, "versions".into(), "Brave Versions");
                            ui.selectable_value(&mut sel, "lists".into(),    "Adblock Lists");
                            ui.selectable_value(&mut sel, "both".into(),     "Both");
                        });
                    if sel != prev {
                        state.settings_location = sel.clone();
                        state.config_dirty = true;
                        crate::console::info(&state.console, "config",
                            format!("settings_location: {sel}"));
                    }
                });
                ui.end_row();
    });
    app::weak_label(ui, format!("Date range minimum: {} (Brave Nightly history starts here)",
                    min_allowed_date()));
}
