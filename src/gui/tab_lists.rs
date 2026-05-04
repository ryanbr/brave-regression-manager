use egui::{RichText, Ui};

use crate::lists;
use crate::paths;
use crate::profile;
use crate::versions;

use super::list_editor::ListEditorState;
use super::state::AppState;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Adblock Lists");

    ui.horizontal(|ui| {
        ui.label("Brave version:");
        let installed = state.installed.clone();
        let mut sel_tag = state.selected_tag.clone().unwrap_or_default();
        egui::ComboBox::from_id_source("ver")
            .selected_text(if sel_tag.is_empty() { "—".into() } else { sel_tag.clone() })
            .show_ui(ui, |ui| {
                for v in &installed {
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
        if !sel.is_empty() { state.selected_profile = Some(sel); }

        if ui.button("+ New profile").clicked() {
            let name = format!("profile-{}", chrono::Utc::now().timestamp());
            if let Err(e) = profile::create(&name) {
                state.status_msg = format!("new profile failed: {e}");
            } else {
                state.profiles = profile::list().unwrap_or_default()
                    .into_iter().map(|p| p.name).collect();
                state.selected_profile = Some(name);
            }
        }

        if ui.button("Reset…").clicked() {
            if let Some(p) = &state.selected_profile {
                let dir = paths::profile_dir(p);
                let _ = profile::reset::reset_profile(&dir, profile::reset::ResetScope::Full);
                state.status_msg = format!("reset {p}");
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
                let extra_args = crate::verdict::parse_launch_args(&effective_args);
                let custom = {
                    let per_row = crate::verdict::user_data_dir(&tag);
                    if !per_row.is_empty() {
                        Some(std::path::PathBuf::from(per_row))
                    } else if state.clean_profile_per_launch {
                        Some(super::tab_versions::throwaway_profile_dir(&tag))
                    } else if state.default_profile_dir_enabled
                        && !state.default_profile_dir.is_empty()
                    {
                        Some(std::path::PathBuf::from(&state.default_profile_dir))
                    } else {
                        None
                    }
                };
                match versions::launch::launch_with_console(&tag, &prof, state.console.clone(), state.brave_log_level, state.freeze_components, extra_args, custom, state.launch_as_admin) {
                    Ok(child) => {
                        crate::console::info(&state.console, "launch",
                            format!("relaunched {tag} (profile={prof})"));
                        state.running.insert(tag.clone(), super::state::RunningBrave {
                            tag, profile: prof.clone(), child,
                            user_data_dir: paths::profile_dir(&prof),
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
            if let Some(prof) = &state.selected_profile {
                let dir = paths::profile_dir(prof);
                match lists::discover::enabled_lists(&dir) {
                    Ok(lists) => {
                        crate::console::info(&state.console, "rescan",
                            format!("found {} list(s) under {}", lists.len(), dir.display()));
                        if lists.is_empty() {
                            // Dump the top-level component-id-shaped folders
                            // so the user can see whether Brave wrote anything
                            // at all. Most common cause of empty rescan: the
                            // profile was never seeded, or freeze_components
                            // is on so Brave never fetched components.
                            let dump = lists::discover::dump_component_dirs(&dir);
                            if dump.is_empty() {
                                crate::console::warn(&state.console, "rescan",
                                    "no component folders present — \
                                     run 'Seed lists' (with Freeze components OFF) first");
                            } else {
                                crate::console::warn(&state.console, "rescan",
                                    format!("component folders present but no list.txt found in any: {dump}"));
                            }
                        }
                        state.lists_for_profile = lists;
                    }
                    Err(e) => {
                        crate::console::error(&state.console, "rescan",
                            format!("{e:#}"));
                    }
                }
            } else {
                crate::console::warn(&state.console, "rescan", "no profile selected");
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
                state.rt.spawn(async move {
                    let result = profile::seed::seed_lists(&prof, &tag).await
                        .map_err(|e| e.to_string());
                    *slot.lock().unwrap() = Some(result);
                });
            }
        }
    });

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
            let row_text = egui::RichText::new(format!("• {display_name}\n   {meta}"));
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
                            ListEditorState::save_current(&list, &mut state.status_msg);
                        }
                        if ui.button("Restore original").clicked() {
                            ListEditorState::restore_current(&list, &mut state.status_msg);
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
                        let applying = state.applying;
                        if ui.add_enabled(!applying, egui::Button::new(
                            if applying { "Applying…" } else { "Apply & Launch Brave" }
                        )).clicked() {
                            if let (Some(tag), Some(prof)) = (state.selected_tag.clone(), state.selected_profile.clone()) {
                                ListEditorState::save_current(&list, &mut state.status_msg);
                                state.applying = true;
                                let slot = state.slots.apply_done.clone();
                                state.rt.spawn(async move {
                                    let result = lists::apply::apply_and_relaunch(&prof, &tag).await
                                        .map_err(|e| e.to_string());
                                    *slot.lock().unwrap() = Some(result);
                                });
                            }
                        }
                    });
                });

                // Editor fills the remaining space above the action panel.
                ListEditorState::ensure_for(&list, ui, &state.console);
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
