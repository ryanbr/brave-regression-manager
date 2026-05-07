use egui::{RichText, Ui};

use crate::lists;
use crate::paths;
use crate::profile;
use crate::versions;

use super::list_editor::ListEditorState;
use super::state::AppState;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Adblock Lists");

    // Settings panel moved to the top-bar Settings popup —
    // see `App::render_settings_window`.

    ui.horizontal(|ui| {
        ui.label("Brave version:");
        let installed = state.installed.clone();
        let mut sel_tag = state.selected_tag.clone().unwrap_or_default();
        egui::ComboBox::from_id_source("ver")
            .selected_text(if sel_tag.is_empty() { "—".into() } else { sel_tag.clone() })
            .show_ui(ui, |ui| {
                for v in installed.iter() {
                    if ui.selectable_label(state.selected_tag.as_deref() == Some(&v.tag), &v.tag).clicked() {
                        sel_tag = v.tag.clone();
                    }
                }
            });
        if !sel_tag.is_empty() { state.selected_tag = Some(sel_tag); }

        ui.label("Profile:");
        let profiles = state.profiles.clone();
        let mut sel = state.selected_profile.clone().unwrap_or_default();
        egui::ComboBox::from_id_source("prof")
            .selected_text(if sel.is_empty() { "—".into() } else { sel.clone() })
            .show_ui(ui, |ui| {
                for p in &profiles {
                    if ui.selectable_label(state.selected_profile.as_deref() == Some(p), p).clicked() {
                        sel = p.clone();
                    }
                }
            });
        if !sel.is_empty()
            && state.selected_profile.as_deref() != Some(sel.as_str())
        {
            crate::console::info(&state.console, "profile",
                format!("selected profile: '{sel}'"));
            state.selected_profile = Some(sel.clone());
            // Profiles each have their own list.txt content per
            // component, so the ✓ "On disk" column in the catalog
            // panel needs to follow the selection. Re-discover the
            // seeded lists for the new profile so it reflects the
            // right state without waiting for a manual Re-scan.
            // Discover walks both the source profile dir AND the
            // active throwaway (when clean_profile_per_launch +
            // reuse is on) — Brave's actually been pulling
            // components into the throwaway.
            let (combined, _targets) = discover_combined_lists(state);
            set_lists_for_profile(state, combined);
            // Single Local State read populates all three filter
            // views (regional flags, subscriptions, custom_filters).
            reload_all_filter_views(state);
        }

        if ui.button("+ New profile").clicked() {
            let name = format!("profile-{}", chrono::Utc::now().timestamp());
            match profile::create(&name) {
                Err(e) => {
                    crate::console::error(&state.console, "profile",
                        format!("new profile '{name}' failed: {e:#}"));
                    state.status_msg = format!("new profile failed: {e}");
                }
                Ok(_) => {
                    state.profiles = profile::list().unwrap_or_default()
                        .into_iter().map(|p| p.name).collect();
                    state.selected_profile = Some(name.clone());
                    crate::console::info(&state.console, "profile",
                        format!("created profile '{name}' at {}",
                            paths::profile_dir(&name).display()));
                }
            }
        }

        if ui.button("Reset…").clicked() {
            if let Some(p) = &state.selected_profile {
                let dir = paths::profile_dir(p);
                let p = p.clone();
                crate::console::info(&state.console, "profile",
                    format!("resetting profile '{p}' at {}", dir.display()));
                match profile::reset::reset_profile(&dir, profile::reset::ResetScope::Full) {
                    Ok(()) => {
                        crate::console::info(&state.console, "profile",
                            format!("reset {p}: ok"));
                        state.status_msg = format!("reset {p}");
                    }
                    Err(e) => {
                        crate::console::error(&state.console, "profile",
                            format!("reset {p} failed: {e:#}"));
                        state.status_msg = format!("reset {p} failed: {e}");
                    }
                }
            } else {
                crate::console::warn(&state.console, "profile",
                    "no profile selected — pick one before Reset");
            }
        }
    });

    ui.horizontal(|ui| {
        let running = state.selected_profile.as_ref()
            .and_then(|p| state.running.values().find(|r| &r.profile == p))
            .is_some();
        ui.label(if running { "Brave status: Running" } else { "Brave status: Stopped" });

        if ui.button("Relaunch Brave").clicked() {
            if let (Some(tag), Some(prof)) = (state.selected_tag.clone(), state.selected_profile.clone()) {
                let to_stop: Vec<String> = state.running.iter()
                    .filter(|(_, r)| r.profile == prof).map(|(t,_)| t.clone()).collect();
                for t in to_stop {
                    if let Some(mut r) = state.running.remove(&t) {
                        // Tree-kill so orphaned Brave helpers from the
                        // previous run don't survive into the relaunch.
                        versions::launch::force_kill_tree(r.child.id());
                        let _ = r.child.kill();
                        let _ = r.child.wait();
                    }
                }
                let row_args = crate::verdict::launch_args(&tag);
                let effective_args = if !row_args.trim().is_empty() {
                    row_args
                } else if state.default_args_enabled && !state.default_args.trim().is_empty() {
                    state.default_args.clone()
                } else {
                    String::new()
                };
                let mut extra_args = crate::verdict::parse_launch_args(&effective_args);
                if state.auto_open_url_enabled
                    && !state.auto_open_url.trim().is_empty()
                {
                    extra_args.push(state.auto_open_url.trim().to_string());
                }
                // Resolve the user-data-dir AND tell the user which
                // precedence tier won — same diagnostic line as the
                // Installed Versions Launch button so reuse / clean-
                // per-launch / default behaviour is visible.
                let (custom, src) = {
                    let per_row = crate::verdict::user_data_dir(&tag);
                    if !per_row.is_empty() {
                        (Some(std::path::PathBuf::from(per_row)), "per-row override".to_string())
                    } else if state.clean_profile_per_launch {
                        let s = if state.reuse_clean_profile {
                            "clean-profile-per-launch (reused)".to_string()
                        } else {
                            "clean-profile-per-launch (fresh)".to_string()
                        };
                        (Some(super::tab_versions::clean_profile_dir_for(state, &tag)), s)
                    } else if state.default_profile_dir_enabled
                        && !state.default_profile_dir.is_empty()
                    {
                        (Some(std::path::PathBuf::from(&state.default_profile_dir)),
                         "Settings default profile folder".to_string())
                    } else {
                        (None, "standard app profile".to_string())
                    }
                };
                if let Some(p) = custom.as_ref() {
                    crate::console::info(&state.console, "profile", format!(
                        "source={src}  path={}", p.display()));
                } else {
                    crate::console::info(&state.console, "profile",
                        format!("source={src} (no override; using paths::profile_dir({prof}))"));
                }
                let effective_user_data = custom.clone()
                    .unwrap_or_else(|| paths::profile_dir(&prof));
                replay_overrides_into(state, &effective_user_data);
                match versions::launch::launch_with_console(&tag, &prof, state.console.clone(), state.brave_log_level, state.freeze_components, extra_args, custom, state.launch_as_admin) {
                    Ok(child) => {
                        crate::console::info(&state.console, "launch",
                            format!("relaunched {tag} (profile={})",
                                effective_user_data.display()));
                        state.running.insert(tag.clone(), super::state::RunningBrave {
                            tag, profile: prof.clone(), child,
                            user_data_dir: effective_user_data,
                            spawned_at: std::time::Instant::now(),
                        });
                        state.status_msg = "relaunched".into();
                    }
                    Err(e) => {
                        let raw = format!("{e:#}");
                        let msg = match super::tab_versions::launch_failure_hint(&raw) {
                            Some(h) => format!("launch failed: {raw}\nhint: {h}"),
                            None    => format!("launch failed: {raw}"),
                        };
                        crate::console::error(&state.console, "launch", &msg);
                        state.status_msg = msg;
                    }
                }
            }
        }

        if ui.button("Re-scan").clicked() {
            if state.selected_profile.is_some() {
                let (combined, targets) = discover_combined_lists(state);
                let dir_summary: Vec<String> = targets.iter()
                    .map(|p| p.display().to_string()).collect();
                crate::console::info(&state.console, "rescan",
                    format!("found {} list(s) across {} dir(s): {}",
                        combined.len(), targets.len(),
                        dir_summary.join(" ; ")));
                if combined.is_empty() {
                    // No components anywhere — usually because Brave
                    // hasn't run yet (or freeze_components is on so
                    // Brave never fetched anything).
                    for dir in &targets {
                        let dump = lists::discover::dump_component_dirs(dir);
                        if dump.is_empty() {
                            crate::console::warn(&state.console, "rescan",
                                format!("no component folders under {} — \
                                 run Brave once (Freeze components OFF) so \
                                 the components are pulled", dir.display()));
                        } else {
                            crate::console::warn(&state.console, "rescan",
                                format!("{}: component folders present but no \
                                 list.txt: {dump}", dir.display()));
                        }
                    }
                }
                set_lists_for_profile(state, combined);
            } else {
                crate::console::warn(&state.console, "rescan", "no profile selected");
            }
        }

        // Read-only inspect of the current adblock prefs in the
        // selected profile's Default/Preferences. Dumps every parsed
        // list (UUID + title + enabled) to the Console — useful as a
        // diagnostic before we wire write support.
        if ui.button("Inspect list prefs")
            .on_hover_text(
                "Read <profile>/Default/Preferences and dump which adblock \
                 filter lists are enabled, plus any custom-subscription URLs. \
                 Read-only — Brave can be running.")
            .clicked()
        {
            if let Some(prof) = &state.selected_profile {
                let dir = paths::profile_dir(prof);
                match crate::lists::prefs::read_profile_prefs(&dir) {
                    Err(e) => crate::console::error(&state.console, "list-pref",
                        format!("{prof}: {e:#}")),
                    Ok(r) => {
                        if r.lists.is_empty() && r.custom_subs.is_empty()
                            && r.matched_paths.is_empty()
                        {
                            crate::console::warn(&state.console, "list-pref",
                                format!("{prof}: no adblock keys recognised — \
                                 schema for this Brave version isn't in our \
                                 matcher table. Tried: {}",
                                 r.missed_paths.join(", ")));
                            // Dump what IS under the parent namespaces so
                            // we can extend the matcher without guessing.
                            if r.probe_keys.is_empty() {
                                crate::console::warn(&state.console, "list-pref",
                                    "no `brave.*` namespaces present at all — \
                                     Preferences exists but Brave hasn't \
                                     written its prefs yet (run Brave once \
                                     against this profile and re-inspect).");
                            } else {
                                for (parent, keys) in &r.probe_keys {
                                    crate::console::info(&state.console, "list-pref",
                                        format!("probe under '{parent}': {}",
                                            keys.join(", ")));
                                }
                            }
                            // Filesystem-level probe: show the user
                            // every adblock-shaped file Brave wrote, so
                            // when the JSON probes turn up nothing we
                            // can spot a LevelDB / SQLite / hash-named
                            // JSON store the matcher doesn't know about.
                            let dir = paths::profile_dir(prof);
                            let files = crate::lists::prefs::list_pref_candidate_files(&dir);
                            crate::console::info(&state.console, "list-pref",
                                format!("filesystem probe: {} candidate file(s) under {}",
                                    files.len(), dir.display()));
                            for (rel, size, mtime) in files.iter().take(40) {
                                let mtime_s = mtime.duration_since(std::time::UNIX_EPOCH)
                                    .ok()
                                    .and_then(|d| chrono::DateTime::<chrono::Utc>
                                        ::from_timestamp(d.as_secs() as i64, 0))
                                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_default();
                                crate::console::info(&state.console, "list-pref",
                                    format!("  {size:>10} B  {mtime_s}  {rel}"));
                            }
                        } else {
                            crate::console::info(&state.console, "list-pref",
                                format!("{prof}: matched {}; {} list entr(ies), \
                                 {} custom subscription(s)",
                                 r.matched_paths.join("+"),
                                 r.lists.len(), r.custom_subs.len()));
                            for l in &r.lists {
                                let title = l.title.as_deref().unwrap_or("?");
                                crate::console::info(&state.console, "list-pref",
                                    format!("  {} [{}] '{}' enabled={}",
                                        if l.enabled { "+" } else { "-" },
                                        &l.id[..l.id.len().min(8)],
                                        title, l.enabled));
                            }
                            for url in &r.custom_subs {
                                crate::console::info(&state.console, "list-pref",
                                    format!("  + custom subscription: {url}"));
                            }
                            if !r.missed_paths.is_empty() {
                                crate::console::info(&state.console, "list-pref",
                                    format!("(also tried {} but not present)",
                                        r.missed_paths.join(", ")));
                            }
                        }
                    }
                }
            } else {
                crate::console::warn(&state.console, "list-pref",
                    "no profile selected");
            }
        }

        let seeding = state.seeding;
        if ui.add_enabled(!seeding, egui::Button::new(
            if seeding { "Seeding…" } else { "Seed lists" }
        )).clicked() {
            if let (Some(tag), Some(prof)) = (state.selected_tag.clone(), state.selected_profile.clone()) {
                state.seeding = true;
                state.status_msg = "seeding…".into();
                let slot = state.slots.seed_done.clone();
                let cons = state.console.clone();
                state.rt.spawn(async move {
                    let result = profile::seed::seed_lists_with_console(
                        &prof, &tag, Some(cons)).await
                        .map_err(|e| e.to_string());
                    *slot.lock().unwrap() = Some(result);
                });
            }
        }
    });

    // The catalog editor only makes sense once both a Brave version
    // AND a profile are picked — Enable/Disable need a target dir,
    // and the on-disk ✓ column needs a profile to scan. Show a
    // weak placeholder hint until both are selected.
    ui.separator();
    if state.selected_tag.is_some() && state.selected_profile.is_some() {
        render_regional_catalog_panel(ui, state);
    } else {
        super::app::weak_label(ui,
            "Edit adblock lists: pick a Brave version and a profile above to enable.");
    }
    ui.separator();

    egui::SidePanel::left("lists_left")
        // Now that list names are cleaned up they fit in ~260 px, so the
        // editor on the right gets the rest of the window. Hard min_width
        // so egui's cached Memory state can't shrink the panel below that.
        .default_width(240.0).resizable(true)
        .min_width(240.0).max_width(480.0)
        .show_inside(ui, |ui|
    {
        ui.label(RichText::new("Lists in profile").strong());
        let lists = state.lists_for_profile.clone();
        for (i, l) in lists.iter().enumerate() {
            // Two-line layout: cleaned-up name on top, lightweight metadata
            // (line count + version) on a second indented line. Stops the
            // awful word-wrap on long names like
            // "Brave Ad Block Updater (Brave First Party Adblock Filters (plaintext))".
            let display_name = clean_list_name(&l.name);
            let meta = format!("{} lines  ·  v{}", l.line_count, l.version);
            // Flag empty list.txt — usually leftover from an older
            // brave-regress that truncated list.txt to empty on
            // Disable. Brave won't re-fetch a component whose
            // manifest version matches, so the list is effectively
            // dead until the version dir is wiped + Brave relaunches.
            let row_text = if l.line_count == 0 {
                egui::RichText::new(format!(
                    "- {display_name}\n   {meta}  (EMPTY!)"))
                    .color(egui::Color32::from_rgb(220, 140, 100))
            } else {
                egui::RichText::new(format!("- {display_name}\n   {meta}"))
            };
            if ui.selectable_label(state.selected_list == Some(i), row_text).clicked() {
                state.selected_list = Some(i);
            }
        }
        if ui.button("Show file paths").clicked() {
            for l in &lists { state.status_msg = format!("{}: {}", l.name, l.path.display()); }
        }
    });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        if let Some(idx) = state.selected_list {
            if let Some(list) = state.lists_for_profile.get(idx).cloned() {
                ui.label(RichText::new(format!("{} — {} lines", list.name, list.line_count)).strong());
                ui.label(format!("path: {}", list.path.display()));
                ui.label(format!("sha256: {}", &list.sha256[..16]));
                ui.separator();

                // Anchor the action row at the bottom of the editor area
                // first — TopBottomPanel reserves its own space, so the
                // editor below sizes to the remaining height and can't
                // overflow into the action row.
                egui::TopBottomPanel::bottom(egui::Id::new(("editor_actions", &list.path)))
                    .resizable(false)
                    // Just enough vertical padding (4px) so the buttons
                    // aren't clipped, and 4px horizontal so they don't
                    // touch the editor's edge. No separator line.
                    .frame(egui::Frame::none()
                        .inner_margin(egui::Margin::symmetric(4.0, 4.0)))
                    .show_separator_line(false)
                    .show_inside(ui, |ui|
                {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Save").clicked() {
                            crate::console::info(&state.console, "list",
                                format!("saving '{}' -> {}", list.name, list.path.display()));
                            ListEditorState::save_current(&list, &mut state.status_msg);
                            // status_msg is the source of truth for
                            // success/failure; mirror it to the
                            // Console so the action leaves a record.
                            crate::console::info(&state.console, "list",
                                format!("save result: {}", state.status_msg));
                        }
                        ui.separator();
                        // Always-available external-editor handoff —
                        // useful at any size, mandatory at multi-MB
                        // where egui's per-paint galley re-layout
                        // makes typing in-app unbearable.
                        if ui.button("Open in External editor")
                            .on_hover_text(
                                "Hand the on-disk list.txt to the editor in \
                                 Settings → Preferred external editor (or \
                                 the OS default when unset). Save there + \
                                 Re-scan picks up your edits.")
                            .clicked()
                        {
                            crate::console::info(&state.console, "edit",
                                format!("opening externally: {}", list.path.display()));
                            super::list_editor::open_external(
                                &list.path,
                                &state.console,
                                &state.preferred_external_editor);
                        }
                        ui.separator();
                        if ui.button("Restore original").clicked() {
                            crate::console::info(&state.console, "list",
                                format!("restoring '{}' from -org backup ({} )",
                                    list.name, list.path.display()));
                            ListEditorState::restore_current(&list, &mut state.status_msg);
                            crate::console::info(&state.console, "list",
                                format!("restore result: {}", state.status_msg));
                        }
                        if ui.button("Show diff")
                            .on_hover_text(
                                "Show diff against original file — compare the current \
                                 edit buffer against the -org backup (or the on-disk \
                                 file if no backup yet). Opens a popup window.")
                            .clicked()
                        {
                            ListEditorState::show_diff_for(&list, &state.console, &mut state.status_msg);
                        }
                        // Wipe the component dir so Brave's component-
                        // updater treats the list as missing and pulls
                        // a fresh copy on next launch. Used to recover
                        // from 0-byte list.txt files left over from the
                        // old truncate-on-disable code path.
                        if ui.button("Force re-download")
                            .on_hover_text(
                                "Delete the component dir so Brave's \
                                 component-updater treats it as missing \
                                 and re-downloads on the next launch. \
                                 Use to recover EMPTY lists. Refuses if \
                                 Brave is running on this user-data-dir.")
                            .clicked()
                        {
                            force_redownload(state, &list);
                        }
                        let applying = state.applying;
                        if ui.add_enabled(!applying, egui::Button::new(
                            if applying { "Applying…" } else { "Apply & Launch Brave" }
                        )).clicked() {
                            if let (Some(tag), Some(prof)) = (state.selected_tag.clone(), state.selected_profile.clone()) {
                                crate::console::info(&state.console, "apply",
                                    format!("applying edited list '{}' and relaunching {tag} \
                                             on profile '{prof}'", list.name));
                                ListEditorState::save_current(&list, &mut state.status_msg);
                                state.applying = true;
                                let slot = state.slots.apply_done.clone();
                                state.rt.spawn(async move {
                                    let result = lists::apply::apply_and_relaunch(&prof, &tag).await
                                        .map_err(|e| e.to_string());
                                    *slot.lock().unwrap() = Some(result);
                                });
                            } else {
                                crate::console::warn(&state.console, "apply",
                                    "Apply & Launch needs both a Brave version and a profile selected");
                            }
                        }
                    });
                });

                // Editor fills the remaining space above the action panel.
                ListEditorState::ensure_for(&list, ui, &state.console,
                    &state.preferred_external_editor);
            }
        } else {
            ui.label("Select a list on the left to edit.");
        }
    });
}

/// Trim the verbose Brave manifest names down to the meaningful piece.
/// Brave's adblock components register their `name` as
/// `"Brave Ad Block Updater (Brave First Party Adblock Filters (plaintext))"`
/// — repetitive and word-wraps in our left panel. Strip the wrapping
/// `Brave Ad Block Updater (` prefix and matching `)` suffix when present;
/// also strip the trailing `(plaintext)` annotation.
fn clean_list_name(raw: &str) -> String {
    let s = raw.trim();
    // Pull out the inner parenthesised name when wrapped by "Brave Ad Block Updater ( … )".
    let inner = if let Some(rest) = s.strip_prefix("Brave Ad Block Updater (") {
        rest.strip_suffix(')').unwrap_or(rest)
    } else { s };
    // Drop a trailing " (plaintext)" / " (DAT)" / " (json)" file-format note.
    let inner = ["(plaintext)", "(DAT)", "(dat)", "(json)"].iter()
        .fold(inner.to_string(), |acc, suffix| {
            acc.trim_end_matches(suffix).trim_end().to_string()
        });
    if inner.is_empty() { raw.to_string() } else { inner }
}

/// Collapsing panel that shows Brave's regional adblock-list catalog
/// (fetched from `brave/adblock-resources` on GitHub) with a flag
/// per row indicating whether the list's component is present on
/// disk in the currently-selected profile. Read-only for now —
/// toggle actions land in a follow-up.
fn render_regional_catalog_panel(ui: &mut Ui, state: &mut AppState) {
    egui::CollapsingHeader::new("Edit adblock lists")
        .id_source("regional_catalog")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let loading = state.regional_catalog_loading;
                let label = if loading { "Fetching…" }
                            else if state.regional_catalog.is_some() { "Re-fetch catalog" }
                            else { "Fetch catalog" };
                if ui.add_enabled(!loading, egui::Button::new(label))
                    .on_hover_text(
                        "Pull the regional filter-list catalog from disk \
                         (Brave's local component file) or — if not yet \
                         present — from raw.githubusercontent.com/brave/\
                         adblock-resources. Cached to \
                         <data-root>/cache/regional_catalog.json.")
                    .clicked()
                {
                    spawn_fetch_regional_catalog(state);
                }
                if ui.button("Refresh from disk")
                    .on_hover_text(
                        "Re-read everything from disk in one click: \
                         (1) regional_filters state (the State column), \
                         (2) list_subscriptions, \
                         (3) custom_filters, \
                         (4) Lists-in-profile sidebar (component dirs). \
                         Use after Brave has shut down so the GUI \
                         reflects whatever Brave wrote back.")
                    .clicked()
                {
                    refresh_all_views(state);
                }
                if ui.button("Clean stale empty lists")
                    .on_hover_text(
                        "Delete component dirs whose list.txt is 0 \
                         bytes — leftovers from the old code path \
                         that wrote enable/disable directly to \
                         list.txt. Brave's component-updater will \
                         re-fetch on next launch. Refuses while \
                         Brave is running on this user-data-dir.")
                    .clicked()
                {
                    cleanup_empty_lists(state);
                }
                if let Some(c) = &state.regional_catalog {
                    super::app::weak_label(ui, format!(
                        "{} list(s)  ·  fetched {}",
                        c.entries.len(),
                        c.fetched_at.format("%Y-%m-%d %H:%M")));
                } else if !loading {
                    super::app::weak_label(ui,
                        "(no cache yet — click Fetch catalog)");
                }
            });
            // Tell the user explicitly which dirs Enable/Disable will
            // write to. When Clean-profile-per-launch is on we also
            // mirror edits into the active throwaway dir for the
            // selected tag, so the buttons "just work" regardless of
            // which dir Brave actually loads.
            let targets = list_edit_targets_view(state);
            // Hoisted so the per-row grid below shares a single
            // (cached) process-list scan instead of one per row.
            let brave_running = if targets.is_empty() {
                false
            } else {
                cached_brave_running_for_targets(state, &targets)
            };
            if !targets.is_empty() {
                ui.horizontal(|ui| {
                    super::app::weak_label(ui, format!(
                        "edits target {} dir(s):", targets.len()));
                });
                for t in &targets {
                    super::app::weak_label(ui, format!("  • {}", t.display()));
                }
                if state.clean_profile_per_launch
                    && state.session_throwaway_dirs.is_empty()
                {
                    ui.colored_label(egui::Color32::from_rgb(220, 180, 60),
                        "⚠ Clean profile per launch is on but no throwaway \
                         exists yet for this tag — launch Brave once first \
                         so a throwaway is created, then edit lists here.");
                }
                // Use the same target set as the action handler — a
                // user's separately-installed Brave running on its
                // own profile dir doesn't gate edits to ours.
                if brave_running {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110),
                        "⚠ Brave is running on this profile — Enable/Disable \
                         disabled. Close Brave (this profile only — other \
                         Brave installs are fine). If you write while it's \
                         live, the next shutdown overwrites the edit from \
                         Brave's in-memory pref state.");
                }
            }

            // O(1) Arc clone instead of a 59-element Vec<CatalogEntry>
            // realloc per frame. The Arc is refreshed in lockstep
            // with `regional_catalog`, so render always sees the
            // current entries without taking a borrow on state for
            // the whole grid scope (which would conflict with the
            // &mut state Enable/Disable spawners need).
            let entries = state.regional_catalog_entries.clone();
            if !entries.is_empty() {
                // Cached Arc<HashSet> — no per-frame rebuild from
                // lists_for_profile; refreshed in lockstep with
                // it via `set_lists_for_profile`. Arc clone here
                // is one atomic increment, vs the previous
                // .iter().map().collect() which reallocated every
                // member string per paint.
                let installed_ids = state.installed_component_ids.clone();
                let row_h = ui.spacing().interact_size.y + 2.0;
                egui::ScrollArea::vertical().id_source("catalog_scroll")
                    .max_height(row_h * 12.0)
                    .auto_shrink([false, true]).show(ui, |ui|
                {
                    egui::Grid::new("regional_catalog_grid")
                        .num_columns(6)
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui|
                    {
                        ui.label(RichText::new("State").strong())
                            .on_hover_text(
                                "Effective on/off as Brave will see it: \
                                 = explicit override in Local State if any, \
                                 else the catalog's default_enabled. \
                                 Suffix '*' = explicit override, no suffix = default.");
                        ui.label(RichText::new("Title").strong());
                        ui.label(RichText::new("Langs").strong());
                        ui.label(RichText::new("On disk").strong())
                            .on_hover_text(
                                "+ = component file present under this profile's \
                                 user-data-dir; - = not yet downloaded by Brave.");
                        ui.label(RichText::new("Action").strong());
                        ui.label(RichText::new("Source").strong());
                        ui.end_row();
                        for e in entries.iter() {
                            let on_disk = installed_ids.contains(&e.component_id);
                            // Effective state: explicit override > catalog default.
                            let override_val = state.regional_state_view.get(&e.uuid).copied();
                            let effective = override_val.unwrap_or(e.default_enabled);
                            let state_label = match (effective, override_val.is_some()) {
                                (true,  true)  => RichText::new("ON  *")
                                    .color(egui::Color32::from_rgb(120, 200, 120)).strong(),
                                (true,  false) => RichText::new("ON")
                                    .color(egui::Color32::from_rgb(120, 200, 120)),
                                (false, true)  => RichText::new("OFF *")
                                    .color(egui::Color32::from_rgb(200, 120, 120)).strong(),
                                (false, false) => RichText::new("OFF")
                                    .color(egui::Color32::from_rgb(160, 160, 160)),
                            };
                            ui.label(state_label).on_hover_text(format!(
                                "default_enabled={} · override={}",
                                e.default_enabled,
                                override_val.map_or("(none)".into(), |b| b.to_string())));
                            ui.label(&e.title)
                                .on_hover_text(format!("UUID: {}\ncomponent_id: {}",
                                    e.uuid, e.component_id));
                            ui.label(e.langs.join(", "));
                            ui.label(if on_disk { "+" } else { "-" });
                            // Single contextual toggle button — flips the
                            // effective state. Label reflects what'll
                            // happen on click.
                            let have_profile = state.selected_profile.is_some();
                            let can_edit = have_profile && !brave_running;
                            let (label, will_set) = if effective {
                                ("Disable", false)
                            } else {
                                ("Enable", true)
                            };
                            if ui.add_enabled(
                                can_edit,
                                egui::Button::new(label))
                                .on_hover_text(format!(
                                    "Currently {}. Click to write \
                                     regional_filters[{}].enabled={} to \
                                     <user-data-dir>/Local State. \
                                     Atomic + verify; refuses if Brave is running.",
                                    if effective { "ON" } else { "OFF" },
                                    &e.uuid[..e.uuid.len().min(8)],
                                    will_set))
                                .clicked()
                            {
                                spawn_set_list_enabled(state, e.clone(), will_set);
                            }
                            // Source URL — click to copy.
                            let short: String = e.url.chars().take(48).collect();
                            let resp = ui.add(egui::Label::new(
                                    RichText::new(&short).monospace()
                                        .color(egui::Color32::from_rgb(140, 180, 220)))
                                    .sense(egui::Sense::click()));
                            if resp.on_hover_text(&e.url).clicked() {
                                ui.ctx().copy_text(e.url.clone());
                                state.status_msg = format!("copied: {}", e.url);
                            }
                            ui.end_row();
                        }
                    });
                });
            }
            ui.separator();
            render_subscriptions_panel(ui, state, brave_running);
            ui.separator();
            render_custom_filters_panel(ui, state, brave_running);
        });
}

/// Subscriptions sub-panel — Brave's `list_subscriptions` dict.
/// Each row is one URL (toggle + remove); below is an input for
/// adding new ones. All edits go through the same atomic+verify
/// path as regional_filters and refuse when Brave is running on
/// our targets.
fn render_subscriptions_panel(ui: &mut Ui, state: &mut AppState, brave_running: bool) {
    egui::CollapsingHeader::new("Custom subscriptions")
        .id_source("custom_subscriptions")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Reload from disk")
                    .on_hover_text("Re-read list_subscriptions from \
                        <user-data-dir>/Local State. Useful after Brave \
                        has run and written its own state.")
                    .clicked()
                {
                    reload_subscriptions(state);
                }
                super::app::weak_label(ui, format!(
                    "{} subscription(s) on disk",
                    state.subscriptions_view.len()));
            });
            if !state.subscriptions_view.is_empty() {
                let row_h = ui.spacing().interact_size.y + 2.0;
                egui::ScrollArea::vertical().id_source("subs_scroll")
                    .max_height(row_h * 8.0)
                    .auto_shrink([false, true]).show(ui, |ui|
                {
                    egui::Grid::new("subs_grid").num_columns(4)
                        .striped(true).spacing([10.0, 4.0])
                        .show(ui, |ui|
                    {
                        ui.label(RichText::new("On").strong());
                        ui.label(RichText::new("Title").strong());
                        ui.label(RichText::new("URL").strong());
                        ui.label(RichText::new("").strong());
                        ui.end_row();
                        let subs = state.subscriptions_view.clone();
                        for s in subs {
                            ui.label(if s.enabled { "+" } else { "-" });
                            ui.label(s.title.as_deref().unwrap_or("—"));
                            ui.add(egui::Label::new(
                                RichText::new(&s.url).monospace()
                                    .color(egui::Color32::from_rgb(140, 180, 220))));
                            ui.horizontal(|ui| {
                                let toggle = if s.enabled { "Disable" } else { "Enable" };
                                if ui.add_enabled(!brave_running,
                                    egui::Button::new(toggle)).clicked()
                                {
                                    spawn_set_subscription_enabled(state, &s.url, !s.enabled);
                                }
                                if ui.add_enabled(!brave_running,
                                    egui::Button::new("Remove")).clicked()
                                {
                                    spawn_remove_subscription(state, &s.url);
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
            }
            ui.horizontal(|ui| {
                ui.label("Add URL:");
                let buf = &mut state.subscription_add_buffer;
                let resp = ui.add(egui::TextEdit::singleline(buf)
                    .desired_width(360.0).hint_text("https://example.com/list.txt"));
                let trigger = (resp.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.add_enabled(!brave_running && !buf.trim().is_empty(),
                        egui::Button::new("Add")).clicked();
                if trigger {
                    let url = buf.trim().to_string();
                    if !url.is_empty() {
                        spawn_set_subscription_enabled(state, &url, true);
                        state.subscription_add_buffer.clear();
                    }
                }
            });
        });
}

/// Custom filter rules sub-panel — `brave.ad_block.custom_filters`
/// in Local State. Multi-line editor; Save writes the whole string,
/// atomic + verify.
fn render_custom_filters_panel(ui: &mut Ui, state: &mut AppState, brave_running: bool) {
    egui::CollapsingHeader::new("Custom filter rules")
        .id_source("custom_filters")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Reload from disk")
                    .on_hover_text("Re-read custom_filters from \
                        <user-data-dir>/Local State.")
                    .clicked()
                {
                    reload_custom_filters(state);
                }
                let dirty = state.custom_filters_buffer != state.custom_filters_original;
                let label = if dirty { "Save (unsaved changes)" } else { "Save" };
                if ui.add_enabled(!brave_running && dirty,
                    egui::Button::new(label)
                        .fill(if dirty { egui::Color32::from_rgb(70, 100, 50) }
                              else { egui::Color32::TRANSPARENT }))
                    .on_hover_text("Write the editor contents to \
                        <user-data-dir>/Local State. Atomic + verify.")
                    .clicked()
                {
                    spawn_save_custom_filters(state);
                }
                if ui.button("Revert").clicked() {
                    state.custom_filters_buffer = state.custom_filters_original.clone();
                }
                super::app::weak_label(ui, format!(
                    "{} bytes saved · {} bytes in editor",
                    state.custom_filters_original.len(),
                    state.custom_filters_buffer.len()));
            });
            ui.add(egui::TextEdit::multiline(&mut state.custom_filters_buffer)
                .desired_width(f32::INFINITY)
                .desired_rows(8)
                .code_editor()
                .hint_text("! Brave custom filter rules (uBO syntax)\n\
                          ||example.com^"));
        });
}

/// Set `lists_for_profile` and refresh the cached
/// `installed_component_ids` set in one go so the catalog grid's
/// O(1) "is this on disk" check stays in sync.
fn set_lists_for_profile(state: &mut AppState, lists: Vec<lists::discover::EnabledList>) {
    state.installed_component_ids = std::sync::Arc::new(
        lists.iter().map(|l| l.component_id.clone()).collect());
    state.lists_for_profile = lists;
}

/// Walk every dir Brave might be loading lists from for this
/// session — the source profile plus the active throwaway when
/// `clean_profile_per_launch` + `reuse_clean_profile` is on — and
/// merge the discovered lists into a single deduped Vec keyed by
/// `component_id`.
///
/// **Throwaway wins on dedupe.** When a component appears in both
/// dirs (because the source has leftover 0-byte list.txt files
/// from older brave-regress builds that truncated lists in-place),
/// the throwaway's freshly-fetched copy is the one Brave actually
/// reads, so we surface it. Without this priority flip, Refresh
/// would parade stale EMPTY rows even after a successful
/// component-updater pull into the throwaway.
fn discover_combined_lists(state: &AppState) -> (Vec<lists::discover::EnabledList>, Vec<std::path::PathBuf>) {
    let source = state.selected_profile.as_ref().map(|p| paths::profile_dir(p));
    let throwaway = if state.clean_profile_per_launch && state.reuse_clean_profile {
        state.selected_tag.as_ref()
            .and_then(|t| state.session_throwaway_dirs.get(t).cloned())
    } else { None };
    // Scan order: throwaway first so its entries claim each
    // component_id slot before source's stale leftovers can.
    let mut targets: Vec<std::path::PathBuf> = Vec::new();
    if let Some(t) = &throwaway { targets.push(t.clone()); }
    if let Some(s) = &source    { targets.push(s.clone()); }
    // Build the catalog ONCE from our cached entries (when present)
    // so per-target enabled_lists doesn't re-parse the on-disk
    // catalog component file for each dir we walk. Falls back to
    // the per-target on-disk read inside `enabled_lists_with_catalog`
    // when we don't have a cached snapshot yet.
    let prebuilt: Option<lists::catalog::Catalog> = if state.regional_catalog_entries.is_empty() {
        None
    } else {
        Some(state.regional_catalog_entries.iter()
            .map(|e| (e.uuid.clone(), e.clone()))
            .collect())
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in &targets {
        let lists = lists::discover::enabled_lists_with_catalog(dir, prebuilt.as_ref())
            .unwrap_or_default();
        for l in lists {
            if seen.insert(l.component_id.clone()) { out.push(l); }
        }
    }
    (out, targets)
}

/// One-click refresh of every disk-backed view in the lists panel.
/// Used by the explicit Refresh button + after Brave shutdown
/// scenarios where state could have drifted from the in-memory
/// snapshot (component-updater pulled new components, Brave wrote
/// back Local State, etc).
/// Delete the component directory tree for a given list across all
/// our edit targets (source profile + active throwaway). Brave's
/// component-updater notices the missing dir on next launch and
/// pulls a fresh copy. Refuses if Brave is running on any target —
/// deleting under a live process is asking for handle-locked errors
/// on Windows and surprise behaviour everywhere else.
///
/// Heavy logging — every candidate path is reported alongside its
/// existence + size before delete, and the on-disk source of truth
/// for `list.path` itself is dumped, so post-delete-but-still-
/// EMPTY mysteries can be traced without screenshotting Explorer.
fn force_redownload(state: &mut AppState, list: &lists::discover::EnabledList) {
    let targets = list_edit_targets_for_action(state);
    if targets.is_empty() {
        crate::console::warn(&state.console, "list-edit",
            "force re-download: no targets resolved (no profile selected?)");
        return;
    }
    let conflicts = crate::lists::process_guard::brave_running_for_targets(&targets);
    if !conflicts.is_empty() {
        let msg = format!("refusing force re-download of '{}': Brave is running",
            list.name);
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
        return;
    }
    crate::console::info(&state.console, "list-edit", format!(
        "force re-download '{}' component_id={} list.path={} \
         (size={} bytes, line_count={}); scanning {} target(s)",
        list.name, list.component_id, list.path.display(),
        std::fs::metadata(&list.path).map(|m| m.len()).unwrap_or(0),
        list.line_count, targets.len()));
    let mut deleted = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let component_id = &list.component_id;
    for dir in &targets {
        for candidate in [dir.join(component_id), dir.join("Default").join(component_id)] {
            let exists = candidate.is_dir();
            let inside = if exists {
                std::fs::read_dir(&candidate).ok()
                    .map(|it| it.filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect::<Vec<_>>().join(","))
                    .unwrap_or_default()
            } else { String::new() };
            crate::console::info(&state.console, "list-edit",
                format!("  candidate {}: exists={exists} contents=[{inside}]",
                    candidate.display()));
            if !exists { continue; }
            match std::fs::remove_dir_all(&candidate) {
                Ok(_) => {
                    deleted += 1;
                    crate::console::info(&state.console, "list-edit",
                        "    -> deleted");
                }
                Err(e) => {
                    let m = format!("{}: {e}", candidate.display());
                    crate::console::error(&state.console, "list-edit",
                        format!("    -> delete failed: {m}"));
                    errors.push(m);
                }
            }
        }
    }
    // Dump the post-delete component-dir inventory so the user can
    // confirm with their eyes that the dir is gone — and spot any
    // OTHER component dirs that might be the actual source of the
    // EMPTY list (e.g., Brave moved the component under a different
    // ID we didn't probe).
    for dir in &targets {
        let dump = lists::discover::dump_component_dirs(dir);
        crate::console::info(&state.console, "list-edit",
            format!("  post-delete inventory under {}: {}",
                dir.display(),
                if dump.is_empty() { "(empty)".into() } else { dump }));
    }
    let summary = if errors.is_empty() {
        format!("force re-download '{}': removed {deleted} component dir(s); \
                 launch Brave to refetch", list.name)
    } else {
        format!("force re-download '{}': removed {deleted} dir(s), errors: {}",
            list.name, errors.join(" ; "))
    };
    if errors.is_empty() {
        crate::console::info(&state.console, "list-edit", &summary);
    } else {
        crate::console::error(&state.console, "list-edit", &summary);
    }
    state.status_msg = summary;
    // Refresh the sidebar so the EMPTY row disappears immediately.
    let (combined, _) = discover_combined_lists(state);
    set_lists_for_profile(state, combined);
    state.selected_list = None;
}

/// Walk every target dir, find any component that has a 0-byte
/// list.txt, and delete the whole component subtree. Brave's
/// component-updater treats the missing dir as "not installed"
/// and pulls a fresh copy on the next launch. Bulk version of
/// `force_redownload` for cleaning up leftovers from the prior
/// truncate-on-disable code path.
fn cleanup_empty_lists(state: &mut AppState) {
    let targets = list_edit_targets_for_action(state);
    if targets.is_empty() {
        crate::console::warn(&state.console, "list-edit",
            "cleanup: no targets resolved (no profile selected?)");
        return;
    }
    let conflicts = crate::lists::process_guard::brave_running_for_targets(&targets);
    if !conflicts.is_empty() {
        let msg = "refusing cleanup: Brave is running on this profile".to_string();
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
        return;
    }
    let mut found = 0usize;
    let mut deleted = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for dir in &targets {
        let lists = lists::discover::enabled_lists(dir).unwrap_or_default();
        for l in lists {
            let size = std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(u64::MAX);
            if size != 0 { continue; }
            found += 1;
            let comp_dir = dir.join(&l.component_id);
            let alt_dir  = dir.join("Default").join(&l.component_id);
            for cand in [comp_dir, alt_dir] {
                if !cand.is_dir() { continue; }
                crate::console::info(&state.console, "list-edit", format!(
                    "cleanup: deleting empty '{}' at {}", l.name, cand.display()));
                match std::fs::remove_dir_all(&cand) {
                    Ok(_)  => deleted += 1,
                    Err(e) => errors.push(format!("{}: {e}", cand.display())),
                }
            }
        }
    }
    let summary = if errors.is_empty() {
        format!("cleanup: found {found} empty list(s), removed {deleted} dir(s); \
                 launch Brave to refetch", )
    } else {
        format!("cleanup: removed {deleted}/{found}, errors: {}", errors.join(" ; "))
    };
    if errors.is_empty() {
        crate::console::info(&state.console, "list-edit", &summary);
    } else {
        crate::console::error(&state.console, "list-edit", &summary);
    }
    state.status_msg = summary;
    let (combined, _) = discover_combined_lists(state);
    set_lists_for_profile(state, combined);
    state.selected_list = None;
}

fn refresh_all_views(state: &mut AppState) {
    if state.selected_profile.is_none() {
        crate::console::warn(&state.console, "refresh", "no profile selected");
        return;
    }
    reload_all_filter_views(state);
    let (combined, targets) = discover_combined_lists(state);
    let dir_summary: Vec<String> = targets.iter()
        .map(|p| p.display().to_string()).collect();
    crate::console::info(&state.console, "refresh", format!(
        "reloaded: {} on-disk list(s), {} regional state entr(ies), \
         {} subscription(s), {} bytes custom_filters; scanned {} dir(s): {}",
        combined.len(),
        state.regional_state_view.len(),
        state.subscriptions_view.len(),
        state.custom_filters_original.len(),
        targets.len(),
        dir_summary.join(" ; ")));
    // Per-list disk diagnostic — surfaces "EMPTY!" rows along with
    // their file path + size so it's obvious which target dir
    // produced them, and whether the file is genuinely 0 bytes vs
    // 0 newlines but content present.
    for l in &combined {
        let size = std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(u64::MAX);
        let tag = if l.line_count == 0 { "EMPTY" } else { "ok" };
        crate::console::info(&state.console, "refresh", format!(
            "  [{tag}] '{}' lines={} size={}B path={} component_id={}",
            l.name, l.line_count, size, l.path.display(), l.component_id));
    }
    // Component-id-shaped dirs that DIDN'T resolve to a list — so
    // when Brave pulled a component but `discover` couldn't promote
    // it (e.g., manifest pulled but list.txt still downloading,
    // or list.txt missing entirely), we can see "the dir is there,
    // it just doesn't have list.txt yet".
    for dir in &targets {
        let dump = lists::discover::dump_component_dirs(dir);
        crate::console::info(&state.console, "refresh", format!(
            "  raw component-id dirs under {}: {}",
            dir.display(),
            if dump.is_empty() { "(none)".into() } else { dump }));
    }
    set_lists_for_profile(state, combined);
    state.status_msg = "refreshed".into();
}

/// Single Local State read populating all three filter-list state
/// views (regional flags, subscriptions, custom filters). The
/// per-bucket reload_* helpers below are thin wrappers over this
/// for the callers that mutate just one bucket and want it
/// re-read in isolation; on profile change / Refresh we use this
/// directly to avoid three round-trips through the same JSON file.
fn reload_all_filter_views(state: &mut AppState) {
    let Some(profile) = state.selected_profile.clone() else { return; };
    let dir = paths::profile_dir(&profile);
    match crate::lists::prefs_edit::read_all_views(&dir) {
        Ok(v) => {
            crate::console::info(&state.console, "list-edit", format!(
                "loaded filter views from {}: {} regional, {} subscription(s), \
                 {} bytes custom_filters",
                dir.display(), v.regional.len(),
                v.subscriptions.len(), v.custom.len()));
            state.regional_state_view     = v.regional;
            state.subscriptions_view      = v.subscriptions;
            state.custom_filters_original = v.custom.clone();
            state.custom_filters_buffer   = v.custom;
        }
        Err(e) => crate::console::error(&state.console, "list-edit",
            format!("read Local State views: {e:#}")),
    }
}

fn reload_regional_state(state: &mut AppState) {
    let Some(profile) = state.selected_profile.clone() else { return; };
    let dir = paths::profile_dir(&profile);
    match crate::lists::prefs_edit::read_regional_filter_states(&dir) {
        Ok(m)  => state.regional_state_view = m,
        Err(e) => crate::console::error(&state.console, "list-edit",
            format!("read regional_filters: {e:#}")),
    }
}

fn reload_subscriptions(state: &mut AppState) {
    let Some(profile) = state.selected_profile.clone() else { return; };
    let dir = paths::profile_dir(&profile);
    match crate::lists::prefs_edit::read_subscriptions(&dir) {
        Ok(v)  => state.subscriptions_view = v,
        Err(e) => crate::console::error(&state.console, "list-edit",
            format!("read subscriptions: {e:#}")),
    }
}

fn reload_custom_filters(state: &mut AppState) {
    let Some(profile) = state.selected_profile.clone() else { return; };
    let dir = paths::profile_dir(&profile);
    match crate::lists::prefs_edit::read_custom_filters(&dir) {
        Ok(text) => {
            state.custom_filters_original = text.clone();
            state.custom_filters_buffer = text;
        }
        Err(e) => crate::console::error(&state.console, "list-edit",
            format!("read custom_filters: {e:#}")),
    }
}

fn spawn_set_subscription_enabled(state: &mut AppState, url: &str, enabled: bool) {
    let targets = list_edit_targets_for_action(state);
    if targets.is_empty() { return; }
    if !crate::lists::process_guard::brave_running_for_targets(&targets).is_empty() {
        let msg = "refusing subscription edit: Brave is running on this profile".to_string();
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
        return;
    }
    state.subscription_overrides.insert(url.to_string(),
        crate::lists::prefs_edit::SubAction::Set(enabled));
    let mut wrote = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for dir in &targets {
        match crate::lists::prefs_edit::edit_subscription_enabled(dir, url, enabled) {
            Ok(_)  => wrote += 1,
            Err(e) => errors.push(format!("{}: {e}", dir.display())),
        }
    }
    let summary = format!(
        "set subscription enabled={enabled} for {url} (wrote {wrote} dir(s){})",
        if errors.is_empty() { String::new() }
        else { format!(", errors: {}", errors.join(" ; ")) });
    if errors.is_empty() {
        crate::console::info(&state.console, "list-edit", &summary);
    } else {
        crate::console::error(&state.console, "list-edit", &summary);
    }
    state.status_msg = summary;
    reload_subscriptions(state);
}

fn spawn_remove_subscription(state: &mut AppState, url: &str) {
    let targets = list_edit_targets_for_action(state);
    if targets.is_empty() { return; }
    if !crate::lists::process_guard::brave_running_for_targets(&targets).is_empty() {
        let msg = "refusing subscription remove: Brave is running on this profile".to_string();
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
        return;
    }
    state.subscription_overrides.insert(url.to_string(),
        crate::lists::prefs_edit::SubAction::Remove);
    let mut wrote = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for dir in &targets {
        match crate::lists::prefs_edit::remove_subscription(dir, url) {
            Ok(_)  => wrote += 1,
            Err(e) => errors.push(format!("{}: {e}", dir.display())),
        }
    }
    let summary = format!(
        "removed subscription {url} (wrote {wrote} dir(s){})",
        if errors.is_empty() { String::new() }
        else { format!(", errors: {}", errors.join(" ; ")) });
    if errors.is_empty() {
        crate::console::info(&state.console, "list-edit", &summary);
    } else {
        crate::console::error(&state.console, "list-edit", &summary);
    }
    state.status_msg = summary;
    reload_subscriptions(state);
}

fn spawn_save_custom_filters(state: &mut AppState) {
    let targets = list_edit_targets_for_action(state);
    if targets.is_empty() { return; }
    if !crate::lists::process_guard::brave_running_for_targets(&targets).is_empty() {
        let msg = "refusing custom_filters save: Brave is running on this profile".to_string();
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
        return;
    }
    let text = state.custom_filters_buffer.clone();
    state.custom_filters_override = Some(text.clone());
    let mut wrote = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for dir in &targets {
        match crate::lists::prefs_edit::edit_custom_filters(dir, &text) {
            Ok(_)  => wrote += 1,
            Err(e) => errors.push(format!("{}: {e}", dir.display())),
        }
    }
    let summary = format!(
        "saved custom_filters ({} bytes) to {} dir(s){}",
        text.len(), wrote,
        if errors.is_empty() { String::new() }
        else { format!(", errors: {}", errors.join(" ; ")) });
    if errors.is_empty() {
        state.custom_filters_original = text;
        crate::console::info(&state.console, "list-edit", &summary);
    } else {
        crate::console::error(&state.console, "list-edit", &summary);
    }
    state.status_msg = summary;
}

fn spawn_fetch_regional_catalog(state: &mut AppState) {
    state.regional_catalog_loading = true;
    state.status_msg = "fetching regional catalog…".into();
    // Try the on-disk catalog component first — Brave's already
    // pulled it locally, no network needed. Fall back to the
    // GitHub raw URL only when the local file is absent (fresh
    // profile that's never launched Brave). Mirrors boce.
    let local = state.selected_profile.as_ref()
        .map(|p| paths::profile_dir(p))
        .and_then(|d| crate::lists::catalog::load_local_catalog(&d));
    if let Some(cache) = local {
        crate::console::info(&state.console, "catalog",
            format!("loaded {} entries from local component file: {}",
                cache.entries.len(), cache.source_url));
        let slot = state.slots.regional_catalog_done.clone();
        *slot.lock().unwrap() = Some(Ok(cache));
        return;
    }
    crate::console::info(&state.console, "catalog",
        "no local catalog component on disk; fetching from \
         raw.githubusercontent.com/brave/adblock-resources");
    let token = state.github_token.clone();
    let slot = state.slots.regional_catalog_done.clone();
    state.rt.spawn(async move {
        let tok = if token.is_empty() { None } else { Some(token.as_str()) };
        let result = crate::lists::catalog::fetch_regional_catalog(tok).await
            .map_err(|e| format!("{e:#}"));
        *slot.lock().unwrap() = Some(result);
    });
}

/// READ-ONLY view of the dirs catalog edits would target — used by
/// the panel's render to show the target list. Does NOT mutate state
/// or create dirs; if the throwaway memo hasn't been seeded yet, the
/// throwaway target is simply absent from the result. The actual
/// pre-create + edit happens in `list_edit_targets_for_action` which
/// is only called from click handlers.
fn list_edit_targets_view(state: &AppState) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = &state.selected_profile {
        out.push(paths::profile_dir(p));
    }
    if state.clean_profile_per_launch && state.reuse_clean_profile {
        if let Some(t) = &state.selected_tag {
            if let Some(dir) = state.session_throwaway_dirs.get(t) {
                out.push(dir.clone());
            }
        }
    }
    out
}

/// Click-time variant — pre-creates and memoizes the throwaway dir
/// when needed, so the edit + the next Relaunch share the same path.
/// Only called from spawn_enable_list / spawn_disable_list.
fn list_edit_targets_for_action(state: &mut AppState) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = state.selected_profile.clone() {
        out.push(paths::profile_dir(&p));
    }
    if state.clean_profile_per_launch && state.reuse_clean_profile {
        if let Some(t) = state.selected_tag.clone() {
            let dir = super::tab_versions::clean_profile_dir_for(state, &t);
            let _ = std::fs::create_dir_all(&dir);
            out.push(dir);
        }
    }
    out
}

/// Application Launcher for Drive — Brave's bundled extension that
/// surfaces in `chrome://extensions`. Adding this id to
/// `extensions.install.deny_list` makes Brave refuse to load it on
/// the next launch.
const APP_LAUNCHER_FOR_DRIVE_ID: &str = "lmjegmlicamnimmfhcmpkclmigmmcbeh";

/// Re-write every recorded list override into the user-data-dir
/// Brave is about to launch with. Called from the Relaunch /
/// Launch click handlers, AFTER any prior Brave was killed and
/// BEFORE the new spawn. Covers regional_filters, list_subscriptions,
/// custom_filters (atomic single Local State write) and the
/// extension blocklist (per-profile Preferences edit).
pub(crate) fn replay_overrides_into(state: &mut AppState, user_data_dir: &std::path::Path) {
    match crate::lists::prefs_edit::replay_all_overrides(
        user_data_dir,
        &state.regional_overrides,
        &state.subscription_overrides,
        state.custom_filters_override.as_deref())
    {
        Ok(0) => {} // nothing pending — quiet
        Ok(n) => crate::console::info(&state.console, "list-edit",
            format!("re-applied {n} list override(s) to {} before launch",
                user_data_dir.display())),
        Err(e) => crate::console::error(&state.console, "list-edit",
            format!("override replay to {} failed: {e:#}",
                user_data_dir.display())),
    }
    if state.block_drive_launcher {
        let ids: &[&str] = &[APP_LAUNCHER_FOR_DRIVE_ID];
        match crate::lists::prefs_edit::ensure_extension_blocklist(
            user_data_dir, ids)
        {
            Ok(_)  => crate::console::info(&state.console, "ext-block",
                format!("ensured deny_list contains {ids:?} in {}/Default/Preferences",
                    user_data_dir.display())),
            Err(e) => crate::console::error(&state.console, "ext-block",
                format!("blocklist write to {} failed: {e:#}",
                    user_data_dir.display())),
        }
    }
    if state.suppress_p3a_banner {
        match crate::lists::prefs_edit::ensure_p3a_dismissed(user_data_dir) {
            Ok(None) => {} // already correct, no log noise
            Ok(Some(_)) => crate::console::info(&state.console, "p3a",
                format!("suppressed P3A banner in {}/Local State",
                    user_data_dir.display())),
            Err(e) => crate::console::error(&state.console, "p3a",
                format!("p3a write to {} failed: {e:#}",
                    user_data_dir.display())),
        }
    }
}

/// Throttled wrapper around `process_guard::brave_running_for_targets`.
/// At 60fps the per-row catalog grid would otherwise pay a process-list
/// scan per row; we cache the result for 1s, which is well below human
/// latency for "did I close Brave yet". Cache keyed on a u64 hash of
/// the target set so the cache check is alloc-free per frame.
fn cached_brave_running_for_targets(
    state: &mut AppState,
    targets: &[std::path::PathBuf],
) -> bool {
    use std::hash::{Hash, Hasher};
    const TTL: std::time::Duration = std::time::Duration::from_secs(1);
    let now = std::time::Instant::now();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for t in targets {
        t.hash(&mut hasher);
    }
    let key = hasher.finish();
    if let Some((at, cached_key, val)) = state.brave_running_cache {
        if now.duration_since(at) < TTL && cached_key == key {
            return val;
        }
    }
    let v = !crate::lists::process_guard::brave_running_for_targets(targets).is_empty();
    state.brave_running_cache = Some((now, key, v));
    v
}

/// Flip `brave.ad_block.regional_filters[uuid].enabled` in Local
/// State for every target dir. Atomic + verify per target. Mirrors
/// the boce offline editor's approach.
fn spawn_set_list_enabled(
    state: &mut AppState,
    entry: crate::lists::catalog::CatalogEntry,
    enabled: bool,
) {
    let targets = list_edit_targets_for_action(state);
    if targets.is_empty() { return; }
    // Only block when a Brave process is running with --user-data-dir
    // matching one of OUR targets — a separately installed Brave
    // running on an unrelated profile dir doesn't endanger our edit.
    let conflicts = crate::lists::process_guard::brave_running_for_targets(&targets);
    if !conflicts.is_empty() {
        let detail: Vec<String> = conflicts.iter().map(|p| {
            let udd = p.user_data_dir.as_deref().unwrap_or("?");
            format!("pid {} {} (--user-data-dir={})", p.pid, p.name, udd)
        }).collect();
        let msg = format!(
            "refusing to edit '{}': Brave is running on this profile ({}). \
             Close Brave first — otherwise the next shutdown will overwrite \
             this edit.",
            entry.title, detail.join(" ; "));
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
        return;
    }
    // Record so we can re-apply before every subsequent launch —
    // dodges the first-launch race where Brave drops our flag
    // because the catalog component isn't on disk yet at startup.
    state.regional_overrides.insert(entry.uuid.clone(), enabled);
    let verb = if enabled { "enabling" } else { "disabling" };
    state.status_msg = format!("{verb} {}", entry.title);
    crate::console::info(&state.console, "list-edit",
        format!("{verb} '{}' (uuid {}): writing regional_filters[{}].enabled={enabled} → {} dir(s)",
            entry.title, entry.uuid, &entry.uuid[..entry.uuid.len().min(8)],
            targets.len()));
    let uuid = entry.uuid.clone();
    let title = entry.title.clone();
    let mut wrote: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for dir in &targets {
        let r = if enabled {
            crate::lists::catalog::enable_list(dir, &uuid)
        } else {
            crate::lists::catalog::disable_list(dir, &uuid)
        };
        match r {
            Ok(p)  => wrote.push(p.display().to_string()),
            Err(e) => failed.push(format!("{}: {e}", dir.display())),
        }
    }
    // For Enable on a non-default list (catalog: no default_enabled),
    // Brave won't auto-fetch the component just from our pref flag —
    // its component-updater only honours OnDemand requests for non-
    // default lists, which the in-Brave UI toggle triggers but our
    // pref-only path doesn't. Mirror the component bytes from the
    // user's system Brave install (matches version + signed content
    // exactly) so the list.txt is on disk before next launch.
    if enabled && !entry.default_enabled && !entry.component_id.is_empty() {
        match crate::lists::system_brave::find_component(&entry.component_id) {
            Some((src_version_dir, src_user_data)) => {
                crate::console::info(&state.console, "list-edit", format!(
                    "found system component for '{title}' at {} (from {})",
                    src_version_dir.display(), src_user_data.display()));
                for dst in &targets {
                    match crate::lists::system_brave::mirror_into(
                        &src_version_dir, dst, &entry.component_id)
                    {
                        Ok(r) => {
                            let tag = if r.skipped { "skip (already at version)" }
                                      else { "copied" };
                            crate::console::info(&state.console, "list-edit", format!(
                                "  -> {tag} '{title}' v{} into {} ({} bytes)",
                                r.version, r.dst_version_dir.display(), r.copied_bytes));
                        }
                        Err(e) => crate::console::error(&state.console, "list-edit",
                            format!("  -> mirror to {} failed: {e:#}", dst.display())),
                    }
                }
            }
            None => crate::console::warn(&state.console, "list-edit", format!(
                "no system Brave install has '{title}' (component {}) — \
                 enable it in your system Brave first (or accept that \
                 Brave-regress's Brave will need to fetch it on its own \
                 timeline, which it may not do for default-disabled lists)",
                entry.component_id)),
        }
    }
    let action = if enabled { "enabled" } else { "disabled" };
    if failed.is_empty() {
        let msg = format!("{action} '{title}' ({} dir(s)): {}",
            wrote.len(), wrote.join(" ; "));
        crate::console::info(&state.console, "list-edit", &msg);
        state.status_msg = msg;
    } else {
        let msg = if wrote.is_empty() {
            format!("{action} '{title}' failed: {}", failed.join(" ; "))
        } else {
            format!("{action} '{title}' partial: wrote {}; failed {}",
                wrote.join(" ; "), failed.join(" ; "))
        };
        crate::console::error(&state.console, "list-edit", &msg);
        state.status_msg = msg;
    }
    // Refresh the on-screen state map so the row's status flips
    // immediately on click — no async indirection now.
    reload_regional_state(state);
}
