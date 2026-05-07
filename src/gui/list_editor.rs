//! Plain-text editor for adblock list files. Each list is loaded into a
//! string buffer; the user edits it directly in a multi-line text area.
//! The original file content is preserved in `<list>-org` the first time
//! the user saves an edit, so they can always revert.
//!
//! Built-in egui shortcuts inside the text area:
//!   Ctrl+A   select all
//!   Ctrl+C   copy
//!   Ctrl+X   cut
//!   Ctrl+V   paste
//!   Ctrl+Z   undo
//!   Ctrl+Y   redo
//!
//! On top of those we add: Find / Find Next (button + F3 shortcut).

use egui::Ui;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::lists::discover::EnabledList;

thread_local! {
    static EDITORS: RefCell<HashMap<PathBuf, Buffer>> = RefCell::new(HashMap::new());
}

struct Buffer {
    /// Mutable edit buffer.
    text: String,
    /// Hash of `text` at the time we last loaded from / saved to disk.
    saved_text: String,
    /// Find-state: query string + byte offset to resume searching from on
    /// the next "Find Next".
    find_query:   String,
    next_search_byte: usize,
    last_find_status: String,
    /// Snapshot of `text` at the moment we last pushed to the undo stack.
    /// When the live `text` diverges from this we push another snapshot.
    last_snapshot: String,
    /// When the previous snapshot was committed to the undo stack.
    /// Used to debounce: typing in a single ~500ms burst gets one
    /// undo unit instead of one-per-keystroke (which on a 5MB
    /// buffer was cloning ~10MB into the undo stack per character
    /// — a real perf killer).
    last_snapshot_at: Option<std::time::Instant>,
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    /// After a successful Find, we record the 0-based line index of the
    /// match. The next render uses this to scroll the editor so the match
    /// is visible on screen, then clears the field.
    pending_scroll_line: Option<usize>,
    /// Char range of the most recent match. We paint a yellow rectangle
    /// over the editor at this range each frame until the buffer is
    /// edited (which would invalidate the offsets).
    highlight_chars: Option<(usize, usize)>,
    /// True while the diff popup window is open. Cleared when the user
    /// closes it.
    show_diff: bool,
    /// Cached unified-diff text. Recomputed each time the diff button is
    /// clicked.
    diff_content: String,
}

/// Hard cap on the number of undo entries — protects against an
/// editor session with millions of tiny edits. Combined with the
/// 500ms snapshot debounce that's a lot of edit history.
const UNDO_LIMIT: usize = 500;
/// Hard cap on the total undo-stack RAM footprint, summed across
/// every snapshot. Each snapshot is a full clone of the buffer
/// (egui's TextEdit owns a String — we don't have a rope), so on
/// a 5MB list one snapshot is 5MB. With UNDO_LIMIT = 500 alone
/// the worst case is 2.5GB of pinned RAM — at which point the OS
/// is swapping and the editor feels unusably sluggish for
/// reasons unrelated to egui itself. We trim oldest snapshots
/// until the total fits this budget.
const UNDO_BYTES_LIMIT: usize = 64 * 1024 * 1024;

pub struct ListEditorState;

impl ListEditorState {
    pub fn ensure_for(
        list: &EnabledList,
        ui: &mut Ui,
        console: &crate::console::Handle,
        preferred_external_editor: &str,
    ) {
        let editor_id = egui::Id::new(("list_editor", &list.path));

        EDITORS.with(|e| {
            let mut map = e.borrow_mut();
            if !map.contains_key(&list.path) {
                let text = std::fs::read_to_string(&list.path).unwrap_or_default();
                map.insert(list.path.clone(), Buffer {
                    saved_text:       text.clone(),
                    last_snapshot:    text.clone(),
                    last_snapshot_at: None,
                    text,
                    find_query:       String::new(),
                    next_search_byte: 0,
                    last_find_status: String::new(),
                    undo_stack:       Vec::new(),
                    redo_stack:       Vec::new(),
                    pending_scroll_line: None,
                    highlight_chars: None,
                    show_diff: false,
                    diff_content: String::new(),
                });
            }
            let st = map.get_mut(&list.path).unwrap();
            // Push a snapshot if the buffer changed since last frame. Any
            // typing / button-driven edit lands here exactly once per change.
            maybe_snapshot(st);

            // (Header row with path/modified/backup-name was removed —
            // long paths overlapped the window and the same info is shown
            // in the action row at the bottom of the tab.)
            //
            // The previous `let _dirty = st.text != st.saved_text;` was
            // doing an O(n) memcmp per frame on a multi-MB buffer for a
            // value nothing read; removed.

            // Find row + edit-action buttons.
            //
            // Capture Enter / F3 *before* any TextEdit renders this frame.
            // egui's singleline TextEdit consumes the Enter event during
            // its own event-handling pass (it removes Key::Enter from
            // `i.events` and sets lost_focus=true), so checking
            // `i.key_pressed(Enter)` AFTER the singleline ran would
            // return false — Find would silently not fire.
            let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let f3            = ui.input(|i| i.key_pressed(egui::Key::F3));
            let mut do_find = f3;

            // Forward Enter to Find Next when:
            //   • the editor is focused (so the highlight is visible)
            //   • a query is set (so there's something to look for)
            // We strip the Enter event from the input queue further down,
            // so the editor never inserts a newline. Effect: after the
            // first Find pulls focus to the editor, repeated Enters keep
            // cycling through matches with the highlight always visible.
            let editor_focused = ui.memory(|m| m.has_focus(editor_id));
            if enter_pressed && editor_focused && !st.find_query.trim().is_empty() {
                do_find = true;
                crate::console::info(console, "find",
                    format!("editor-Enter forwarded to find_next (query=\"{}\", focus=editor)",
                            st.find_query.trim()));
            } else if enter_pressed && editor_focused {
                // Diagnostic: Enter came in while focus was on editor but
                // no find query — Enter goes through as a newline. Useful
                // to see when the user expects find but query is empty.
                crate::console::info(console, "find",
                    "editor-Enter ignored (no find query) — passing through as newline");
            }
            if f3 {
                crate::console::info(console, "find", "F3 pressed → find_next");
            }
            let mut do_cut       = false;
            let mut do_copy      = false;
            let mut do_paste     = false;
            let mut do_select_all = false;
            ui.horizontal(|ui| {
                ui.label("Find:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut st.find_query)
                        .desired_width(220.0)
                        .hint_text("search text…")
                );
                if resp.lost_focus() && enter_pressed {
                    do_find = true;
                    // Keep focus on the Find field so subsequent Enters
                    // keep cycling matches.
                    ui.memory_mut(|m| m.request_focus(resp.id));
                }
                if ui.button("Find Next").on_hover_text("F3").clicked() {
                    do_find = true;
                }
                ui.separator();
                if ui.button("Cut").clicked()        { do_cut = true; }
                if ui.button("Copy").clicked()       { do_copy = true; }
                if ui.button("Paste").clicked()      { do_paste = true; }
                if ui.button("Select All").clicked() { do_select_all = true; }
                ui.separator();
                let can_undo = !st.undo_stack.is_empty();
                let can_redo = !st.redo_stack.is_empty();
                if ui.add_enabled(can_undo, egui::Button::new("Undo"))
                    .on_hover_text(format!("Undo ({} step(s) available)", st.undo_stack.len()))
                    .clicked() { do_undo(st, ui.ctx(), editor_id, console); }
                if ui.add_enabled(can_redo, egui::Button::new("Redo"))
                    .on_hover_text(format!("Redo ({} step(s) available)", st.redo_stack.len()))
                    .clicked() { do_redo(st, ui.ctx(), editor_id, console); }
                if !st.last_find_status.is_empty() {
                    super::app::weak_label(ui, st.last_find_status.clone());
                }
            });

            // (The large-file warning + Open externally button used
            // to live here. Both moved to the editor's bottom action
            // row in tab_lists.rs so the affordance is always
            // visible — no surprise threshold, no duplicate
            // placement. `open_external` is now pub(super) so the
            // bottom row can call it directly.)
            // Suppress unused-variable warning for the threaded
            // editor preference — kept on the signature for future
            // per-buffer needs even though this function no longer
            // consumes it directly.
            let _ = preferred_external_editor;

            if do_find {
                let stripped = ui.ctx().input_mut(|i| {
                    let before = i.events.len();
                    i.events.retain(|e| !matches!(
                        e,
                        egui::Event::Key { key: egui::Key::Enter, pressed: true, .. }
                            | egui::Event::Key { key: egui::Key::F3, pressed: true, .. }
                    ));
                    before - i.events.len()
                });
                if stripped > 0 {
                    crate::console::info(console, "find",
                        format!("stripped {stripped} Enter/F3 event(s) so editor won't see them"));
                }
                find_next(st, ui.ctx(), editor_id, console);
            }
            if do_cut        { do_clipboard(st, ui.ctx(), editor_id, ClipboardOp::Cut, console); }
            if do_copy       { do_clipboard(st, ui.ctx(), editor_id, ClipboardOp::Copy, console); }
            if do_paste      { do_clipboard(st, ui.ctx(), editor_id, ClipboardOp::Paste, console); }
            if do_select_all { do_clipboard(st, ui.ctx(), editor_id, ClipboardOp::SelectAll, console); }

            // Multi-line text area + right-click context menu.
            //
            // We snapshot the previous frame's selection BEFORE rendering
            // this frame's TextEdit. Right-click moves the cursor as part
            // of TextEdit's event handling, which clears the selection
            // before the context_menu callback can read it. The snapshot
            // captures the user's intended selection from the prior frame
            // so Cut / Copy still operate on what was highlighted.
            let pre_selection: Option<(usize, usize)> =
                egui::TextEdit::load_state(ui.ctx(), editor_id)
                    .and_then(|s| s.cursor.char_range())
                    .filter(|r| r.primary.index != r.secondary.index)
                    .map(|r| {
                        let a = r.primary.index;
                        let b = r.secondary.index;
                        (a.min(b), a.max(b))
                    });

            // Wrap in a ScrollArea. We don't use `vertical_scroll_offset`
            // here — that's only applied on the FIRST creation of the
            // ScrollArea per id_source, and egui caches the scroll
            // position thereafter. Subsequent Find Next clicks would be
            // silently ignored. Instead we call `ui.scroll_to_rect`
            // inside the show closure (below), which forces a scroll
            // every time we ask for one.
            let scroll = egui::ScrollArea::both()
                .id_source(("list_scroll", &list.path))
                .auto_shrink([false; 2]);
            scroll.show(ui, |ui| {
                // Use TextEdit::show() (not ui.add_sized) so we get the
                // TextEditOutput which exposes the galley + galley_pos —
                // we need those to compute the screen rect for the find
                // match and paint a custom yellow highlight on top.
                let output = egui::TextEdit::multiline(&mut st.text)
                    .id(editor_id)
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(20)
                    .show(ui);

                output.response.context_menu(|ui| {
                    text_context_menu(ui, st, editor_id, pre_selection, console);
                });

                // Force-scroll the parent ScrollArea so the match is
                // centered. We call `scroll_to_rect` *inside* the show
                // closure each time `pending_scroll_line` is set, which
                // overrides egui's cached scroll position for this frame.
                if let Some(line) = st.pending_scroll_line.take() {
                    if let Some((a, b)) = st.highlight_chars {
                        // egui's CCursor clamps internally if the
                        // index overshoots the buffer; we used to do
                        // an explicit `a.min(text.chars().count())`
                        // here as defensive programming, but that
                        // walked the whole 5MB UTF-8 string per
                        // frame for no win. Trust egui's clamp.
                        let start_cur = output.galley.from_ccursor(egui::text::CCursor::new(a));
                        let end_cur   = output.galley.from_ccursor(egui::text::CCursor::new(b));
                        let start_rect = output.galley.pos_from_cursor(&start_cur);
                        let end_rect   = output.galley.pos_from_cursor(&end_cur);
                        let off = output.galley_pos.to_vec2();
                        let screen_rect = egui::Rect::from_min_max(
                            start_rect.min + off,
                            end_rect.max + off,
                        );
                        ui.scroll_to_rect(screen_rect, Some(egui::Align::Center));
                        crate::console::info(console, "find",
                            format!("scroll: line={line} centered match (rect.y={:.0})",
                                    screen_rect.center().y));
                    }
                }

                // Paint the find-highlight rectangle (or per-row rectangles
                // for multi-line matches) on top of the editor.
                if let Some((a, b)) = st.highlight_chars {
                    // Same as above — drop the per-frame chars().count()
                    // clamp; egui's CCursor handles out-of-range internally.
                    let start_cur = output.galley.from_ccursor(egui::text::CCursor::new(a));
                    let end_cur   = output.galley.from_ccursor(egui::text::CCursor::new(b));
                    let start_rect = output.galley.pos_from_cursor(&start_cur);
                    let end_rect   = output.galley.pos_from_cursor(&end_cur);
                    let off = output.galley_pos.to_vec2();
                    let highlight_color = egui::Color32::from_rgba_unmultiplied(255, 215, 0, 110);

                    if (start_rect.top() - end_rect.top()).abs() < 0.5 {
                        // Single-line match: one rectangle from start.left to end.right.
                        let r = egui::Rect::from_min_max(
                            start_rect.min + off,
                            end_rect.max + off,
                        );
                        ui.painter().rect_filled(r, 1.0, highlight_color);
                    } else {
                        // Multi-line match: paint the first line from start
                        // to its row end, the last line from row start to
                        // end, and full rows in between. Galley rows give
                        // us the row rects.
                        let line_h = (start_rect.bottom() - start_rect.top()).max(1.0);
                        let mut y = start_rect.top();
                        // First (partial) row
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(
                                start_rect.min + off,
                                egui::pos2(end_rect.right().max(start_rect.right()),
                                            start_rect.bottom()) + off,
                            ),
                            1.0, highlight_color);
                        // Middle rows (full width-ish)
                        y += line_h;
                        while y + line_h <= end_rect.top() + 0.5 {
                            let row_rect = egui::Rect::from_min_size(
                                egui::pos2(0.0, y) + off,
                                egui::vec2(end_rect.right().max(start_rect.right()), line_h),
                            );
                            ui.painter().rect_filled(row_rect, 1.0, highlight_color);
                            y += line_h;
                        }
                        // Last (partial) row
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, end_rect.top()) + off,
                                end_rect.max + off,
                            ),
                            1.0, highlight_color);
                    }
                }
            });

            // Diff popup window — shows the unified diff between the
            // current edit buffer and the `-org` backup (or on-disk file).
            // Toggled via the Diff button in the toolbar above.
            if st.show_diff {
                render_diff_window(ui.ctx(), st, list);
            }
        });
    }

    pub fn save_current(list: &EnabledList, status: &mut String) {
        EDITORS.with(|e| {
            let map = e.borrow();
            let st = match map.get(&list.path) {
                Some(s) => s,
                None    => { *status = "no buffer to save".into(); return; }
            };
            let backup = backup_path(&list.path);
            if !backup.exists() {
                if let Err(err) = std::fs::copy(&list.path, &backup) {
                    *status = format!("backup failed: {err}");
                    return;
                }
            }
            let tmp = list.path.with_extension(
                list.path.extension().and_then(|s| s.to_str()).map(|e| format!("{e}.new"))
                    .unwrap_or_else(|| "new".to_string()));
            if let Err(err) = std::fs::write(&tmp, &st.text) {
                *status = format!("write failed: {err}");
                return;
            }
            if let Err(err) = std::fs::rename(&tmp, &list.path) {
                *status = format!("rename failed: {err}");
                return;
            }
            drop(map);
            EDITORS.with(|e| {
                if let Some(s) = e.borrow_mut().get_mut(&list.path) {
                    s.saved_text = s.text.clone();
                }
            });
            *status = format!("saved {} (backup: {})",
                              list.path.display(),
                              backup.file_name().map(|s| s.to_string_lossy().into_owned())
                                  .unwrap_or_default());
        });
    }

    /// Compute a unified diff between the current edit buffer and the
    /// `-org` backup (or the on-disk file if no backup exists yet) and
    /// open the diff window. Called from the action row in tab_lists.rs.
    pub fn show_diff_for(list: &EnabledList, console: &crate::console::Handle, status: &mut String) {
        EDITORS.with(|e| {
            let mut map = e.borrow_mut();
            if let Some(st) = map.get_mut(&list.path) {
                let backup = backup_path(&list.path);
                let original = if backup.exists() {
                    std::fs::read_to_string(&backup).unwrap_or_default()
                } else {
                    std::fs::read_to_string(&list.path).unwrap_or_default()
                };
                st.diff_content = compute_diff(&original, &st.text);
                let n_lines = st.diff_content.lines().count();
                st.show_diff = true;
                let against = if backup.exists() { "-org backup" } else { "on-disk file" };
                crate::console::info(console, "diff",
                    format!("computed diff: {n_lines} lines (against {against})"));
                *status = format!("diff against {against}: {n_lines} diff lines");
            } else {
                *status = "open the list in the editor first".into();
            }
        });
    }

    pub fn restore_current(list: &EnabledList, status: &mut String) {
        let backup = backup_path(&list.path);
        if !backup.exists() {
            *status = format!("no backup at {} — nothing to restore", backup.display());
            return;
        }
        if let Err(err) = std::fs::copy(&backup, &list.path) {
            *status = format!("restore failed: {err}");
            return;
        }
        EDITORS.with(|e| { e.borrow_mut().remove(&list.path); });
        *status = format!("restored {} from {}", list.path.display(), backup.display());
    }
}

/// Find the next occurrence of `st.find_query` (case-insensitive) starting
/// at `st.next_search_byte`. If found, select that range in the TextEdit
/// and advance the search cursor. Wraps to the start when reaching EOF.
fn find_next(st: &mut Buffer, ctx: &egui::Context, id: egui::Id, console: &crate::console::Handle) {
    let query = st.find_query.trim();
    crate::console::info(console, "find",
        format!("find_next: query=\"{}\" buf_chars={} resume_byte={}",
                query, st.text.chars().count(), st.next_search_byte));
    if query.is_empty() {
        st.last_find_status = "(empty query)".into();
        crate::console::warn(console, "find", "empty query");
        return;
    }
    let needle = query.to_ascii_lowercase();

    fn find_from(haystack_lower: &str, needle: &str, start: usize) -> Option<usize> {
        if start > haystack_lower.len() { return None; }
        haystack_lower[start..].find(needle).map(|p| p + start)
    }

    let lower = st.text.to_ascii_lowercase();
    let mut at = find_from(&lower, &needle, st.next_search_byte);
    let mut wrapped = false;
    if at.is_none() && st.next_search_byte > 0 {
        // Wrap around.
        at = find_from(&lower, &needle, 0);
        wrapped = true;
    }
    let Some(byte_pos) = at else {
        st.last_find_status = format!("\"{}\" not found", query);
        crate::console::warn(console, "find",
            format!("not found: \"{query}\" (resume_byte was {})", st.next_search_byte));
        return;
    };

    // Convert byte offsets → character indices for egui's CCursor (which
    // counts unicode characters, not bytes).
    let char_start = byte_to_char_index(&st.text, byte_pos);
    let char_end   = byte_to_char_index(&st.text, byte_pos + query.len());

    // Set the TextEdit selection so the match is highlighted.
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let range = egui::text::CCursorRange::two(
            egui::text::CCursor::new(char_start),
            egui::text::CCursor::new(char_end),
        );
        state.cursor.set_char_range(Some(range));
        state.store(ctx, id);
    }
    // Move the resume cursor past this match.
    st.next_search_byte = byte_pos + query.len();
    st.last_find_status = if wrapped {
        format!("found at char {} (wrapped)", char_start)
    } else {
        format!("found at char {}", char_start)
    };
    // Compute the 0-based line index of the match so the next render can
    // scroll the editor's ScrollArea so this line is visible.
    let match_line = st.text[..byte_pos].matches('\n').count();
    st.pending_scroll_line = Some(match_line);
    st.highlight_chars = Some((char_start, char_end));
    crate::console::info(console, "find",
        format!("hit: byte={byte_pos} char_range={char_start}..{char_end} \
                 line={match_line} next_resume_byte={} wrapped={wrapped}",
                 byte_pos + query.len()));
    // Pull focus to the editor so the highlight is immediately visible.
    // Subsequent Enter presses inside the editor are caught by the
    // "Enter forwards to find_next" path in `ensure_for` (below) so the
    // user can keep pressing Enter to cycle through matches without
    // typing newlines into the buffer.
    ctx.memory_mut(|m| m.request_focus(id));
    crate::console::info(console, "find",
        format!("focus→editor (id={id:?}) so highlight is visible"));
}

fn byte_to_char_index(s: &str, byte_idx: usize) -> usize {
    s.char_indices().take_while(|(b, _)| *b < byte_idx).count()
}

fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(s.len())
}

/// Push a snapshot to the undo stack if `text` has diverged from the last
/// snapshot. Called once per frame at the top of `ensure_for`. Cap the
/// stack at `UNDO_LIMIT` entries so very long sessions don't grow without
/// bound. Any new change clears the redo stack.
/// Hand a path to the user's preferred editor (when configured)
/// or the OS default handler. Best-effort — failure is logged but
/// doesn't surface a modal. Falls back to the OS default if the
/// preferred editor exists in config but spawning it fails (bad
/// path, missing exe), so the button always opens *something*.
pub(super) fn open_external(
    path: &std::path::Path,
    console: &crate::console::Handle,
    preferred_editor: &str,
) {
    let preferred = preferred_editor.trim();
    if !preferred.is_empty() {
        match std::process::Command::new(preferred).arg(path).spawn() {
            Ok(_) => {
                crate::console::info(console, "edit",
                    format!("opened {} in preferred editor: {preferred}",
                        path.display()));
                return;
            }
            Err(e) => crate::console::warn(console, "edit", format!(
                "preferred editor '{preferred}' failed to spawn ({e}) — \
                 falling back to OS default")),
        }
    }
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "", &path.display().to_string()])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    match result {
        Ok(_)  => {} // success — OS launcher took over
        Err(e) => crate::console::error(console, "edit",
            format!("open externally failed: {e}")),
    }
}

/// Drop entries off the front of an undo / redo stack until both
/// the count cap (`UNDO_LIMIT`) and the byte cap
/// (`UNDO_BYTES_LIMIT`) are satisfied. Cheap path when the stack
/// is small; only walks the stack to sum byte sizes when it's
/// large enough that a count check alone might pass while the
/// byte budget is busted.
fn trim_undo(stack: &mut Vec<String>) {
    while stack.len() > UNDO_LIMIT { stack.remove(0); }
    let mut bytes: usize = stack.iter().map(|s| s.len()).sum();
    while bytes > UNDO_BYTES_LIMIT && !stack.is_empty() {
        bytes -= stack[0].len();
        stack.remove(0);
    }
}

/// Time-debounce window for committing an undo snapshot. A typing
/// burst inside this window collapses into a single undo unit;
/// pause longer than this and the next change becomes a fresh
/// unit. Matches what every text editor does and avoids
/// per-keystroke full-buffer clones on multi-MB list.txt files.
const SNAPSHOT_DEBOUNCE: std::time::Duration =
    std::time::Duration::from_millis(500);

fn maybe_snapshot(st: &mut Buffer) {
    if st.text == st.last_snapshot { return; }
    let now = std::time::Instant::now();
    if let Some(prev) = st.last_snapshot_at {
        if now.duration_since(prev) < SNAPSHOT_DEBOUNCE {
            // Mid-burst — defer the snapshot. last_snapshot stays
            // pointing at the previous commit boundary, so a later
            // call (after the user pauses) will fold every keystroke
            // since then into one undo unit.
            return;
        }
    }
    // mem::replace: move the existing last_snapshot into the
    // undo stack (no clone), then build the new last_snapshot
    // by cloning st.text once. Saves one full-buffer clone per
    // debounced commit — for a 5MB buffer that's ~5MB less alloc
    // every time the debounce fires.
    let prev_snapshot = std::mem::replace(&mut st.last_snapshot, st.text.clone());
    st.undo_stack.push(prev_snapshot);
    trim_undo(&mut st.undo_stack);
    st.last_snapshot_at = Some(now);
    st.redo_stack.clear();
    // Any edit invalidates the find-highlight char offsets — clear it.
    st.highlight_chars = None;
}

fn do_undo(st: &mut Buffer, ctx: &egui::Context, editor_id: egui::Id, console: &crate::console::Handle) {
    use egui::text::{CCursor, CCursorRange};
    if let Some(prev) = st.undo_stack.pop() {
        st.redo_stack.push(st.text.clone());
        trim_undo(&mut st.redo_stack);
        st.text = prev.clone();
        st.last_snapshot = prev;
        // Reset the debounce timer so the next typing burst doesn't
        // collapse into the just-undone unit.
        st.last_snapshot_at = None;
        let total = st.text.chars().count();
        if let Some(mut s) = egui::TextEdit::load_state(ctx, editor_id) {
            s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(total))));
            s.store(ctx, editor_id);
        }
        crate::console::info(console, "edit",
            format!("undo: {} undo / {} redo remain", st.undo_stack.len(), st.redo_stack.len()));
    } else {
        crate::console::warn(console, "edit", "undo: nothing to undo");
    }
}

fn do_redo(st: &mut Buffer, ctx: &egui::Context, editor_id: egui::Id, console: &crate::console::Handle) {
    use egui::text::{CCursor, CCursorRange};
    if let Some(next) = st.redo_stack.pop() {
        st.undo_stack.push(st.text.clone());
        trim_undo(&mut st.undo_stack);
        st.text = next.clone();
        st.last_snapshot = next;
        st.last_snapshot_at = None;
        let total = st.text.chars().count();
        if let Some(mut s) = egui::TextEdit::load_state(ctx, editor_id) {
            s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(total))));
            s.store(ctx, editor_id);
        }
        crate::console::info(console, "edit",
            format!("redo: {} undo / {} redo remain", st.undo_stack.len(), st.redo_stack.len()));
    } else {
        crate::console::warn(console, "edit", "redo: nothing to redo");
    }
}

#[derive(Copy, Clone)]
enum ClipboardOp { Cut, Copy, Paste, SelectAll }

/// Shared implementation for the explicit Cut / Copy / Paste / Select All
/// buttons. Reads the TextEdit's persisted cursor range (which IS preserved
/// across button clicks because clicking a button only moves focus, doesn't
/// touch the editor's cursor) and operates on `st.text` plus the OS clipboard.
fn do_clipboard(
    st: &mut Buffer,
    ctx: &egui::Context,
    editor_id: egui::Id,
    op: ClipboardOp,
    console: &crate::console::Handle,
) {
    use egui::text::{CCursor, CCursorRange};

    let state_opt = egui::TextEdit::load_state(ctx, editor_id);
    let sel = state_opt.as_ref()
        .and_then(|s| s.cursor.char_range())
        .filter(|r| r.primary.index != r.secondary.index)
        .map(|r| {
            let p = r.primary.index;
            let s = r.secondary.index;
            (p.min(s), p.max(s))
        });
    let cursor_pos = state_opt.as_ref()
        .and_then(|s| s.cursor.char_range())
        .map(|r| r.primary.index)
        .unwrap_or_else(|| st.text.chars().count());

    let store_cursor = |idx: usize| {
        if let Some(mut s) = egui::TextEdit::load_state(ctx, editor_id) {
            s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
            s.store(ctx, editor_id);
        }
    };

    match op {
        ClipboardOp::Copy => {
            if let Some((a, b)) = sel {
                let text: String = st.text.chars().skip(a).take(b - a).collect();
                let n = text.chars().count();
                ctx.copy_text(text);
                crate::console::info(console, "edit",
                    format!("copy: {n} chars from {a}..{b}"));
            } else {
                crate::console::warn(console, "edit", "copy: no selection");
            }
        }
        ClipboardOp::Cut => {
            if let Some((a, b)) = sel {
                let text: String = st.text.chars().skip(a).take(b - a).collect();
                let n = text.chars().count();
                ctx.copy_text(text);
                let s_b = char_to_byte_index(&st.text, a);
                let e_b = char_to_byte_index(&st.text, b);
                st.text.replace_range(s_b..e_b, "");
                store_cursor(a);
                crate::console::info(console, "edit",
                    format!("cut: {n} chars from {a}..{b}; buffer now {} chars",
                            st.text.chars().count()));
            } else {
                crate::console::warn(console, "edit", "cut: no selection");
            }
        }
        ClipboardOp::Paste => {
            let pasted = arboard::Clipboard::new()
                .ok()
                .and_then(|mut c| c.get_text().ok())
                .unwrap_or_default();
            if pasted.is_empty() {
                crate::console::warn(console, "edit", "paste: clipboard empty (or unreadable)");
                return;
            }
            let pasted_chars = pasted.chars().count();
            if let Some((a, b)) = sel {
                let s_b = char_to_byte_index(&st.text, a);
                let e_b = char_to_byte_index(&st.text, b);
                st.text.replace_range(s_b..e_b, &pasted);
                store_cursor(a + pasted_chars);
                crate::console::info(console, "edit",
                    format!("paste: replaced selection {a}..{b} with {pasted_chars} chars"));
            } else {
                let b = char_to_byte_index(&st.text, cursor_pos);
                st.text.insert_str(b, &pasted);
                store_cursor(cursor_pos + pasted_chars);
                crate::console::info(console, "edit",
                    format!("paste: inserted {pasted_chars} chars at cursor {cursor_pos}"));
            }
        }
        ClipboardOp::SelectAll => {
            let total = st.text.chars().count();
            if let Some(mut s) = egui::TextEdit::load_state(ctx, editor_id) {
                s.cursor.set_char_range(Some(CCursorRange::two(
                    CCursor::new(0), CCursor::new(total))));
                s.store(ctx, editor_id);
            }
            ctx.memory_mut(|m| m.request_focus(editor_id));
            crate::console::info(console, "edit", format!("select all: {total} chars"));
        }
    }
}

/// Right-click menu for the editor: Cut / Copy / Paste / Select All.
/// `pre_selection` is the (start_char, end_char) selection captured from
/// the *previous* frame — used because egui's TextEdit clears its in-frame
/// selection during the right-click event handling, before this callback
/// gets a chance to read it.
fn text_context_menu(
    ui: &mut egui::Ui,
    st: &mut Buffer,
    editor_id: egui::Id,
    pre_selection: Option<(usize, usize)>,
    console: &crate::console::Handle,
) {
    use egui::text::{CCursor, CCursorRange};

    // Restore the pre-right-click selection into the TextEdit state right
    // away. The right-click event already moved TextEdit's cursor to the
    // click point and cleared the live selection; writing the snapshot
    // back ensures Cut/Copy operate on the correct text AND the highlight
    // reappears the moment the menu closes (when focus returns to the
    // editor).
    if let Some((a, b)) = pre_selection {
        if let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
            s.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(a), CCursor::new(b))));
            s.store(ui.ctx(), editor_id);
        }
    }

    let (sel_start_char, sel_end_char, has_selection) = match pre_selection {
        Some((a, b)) => (a, b, true),
        None => match egui::TextEdit::load_state(ui.ctx(), editor_id)
            .and_then(|s| s.cursor.char_range())
            .filter(|r| r.primary.index != r.secondary.index)
        {
            Some(r) => {
                let a = r.primary.index;
                let b = r.secondary.index;
                (a.min(b), a.max(b), true)
            }
            None => (0, 0, false),
        }
    };
    let state_opt = egui::TextEdit::load_state(ui.ctx(), editor_id);
    let selected_text = if has_selection {
        st.text.chars().skip(sel_start_char).take(sel_end_char - sel_start_char)
            .collect::<String>()
    } else { String::new() };

    let store_cursor = |ctx: &egui::Context, idx: usize| {
        if let Some(mut s) = egui::TextEdit::load_state(ctx, editor_id) {
            s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
            s.store(ctx, editor_id);
        }
    };

    if ui.add_enabled(has_selection, egui::Button::new("Cut")).clicked() {
        let n = selected_text.chars().count();
        ui.ctx().copy_text(selected_text.clone());
        let s_b = char_to_byte_index(&st.text, sel_start_char);
        let e_b = char_to_byte_index(&st.text, sel_end_char);
        st.text.replace_range(s_b..e_b, "");
        store_cursor(ui.ctx(), sel_start_char);
        crate::console::info(console, "edit",
            format!("ctx-menu cut: {n} chars from {sel_start_char}..{sel_end_char}"));
        ui.close_menu();
    }
    if ui.add_enabled(has_selection, egui::Button::new("Copy")).clicked() {
        let n = selected_text.chars().count();
        ui.ctx().copy_text(selected_text);
        crate::console::info(console, "edit",
            format!("ctx-menu copy: {n} chars from {sel_start_char}..{sel_end_char}"));
        ui.close_menu();
    }
    if ui.button("Paste").clicked() {
        let pasted = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
            .unwrap_or_default();
        if pasted.is_empty() {
            crate::console::warn(console, "edit", "ctx-menu paste: clipboard empty");
        } else {
            let pasted_chars = pasted.chars().count();
            if has_selection {
                let s_b = char_to_byte_index(&st.text, sel_start_char);
                let e_b = char_to_byte_index(&st.text, sel_end_char);
                st.text.replace_range(s_b..e_b, &pasted);
                store_cursor(ui.ctx(), sel_start_char + pasted_chars);
                crate::console::info(console, "edit",
                    format!("ctx-menu paste: replaced {sel_start_char}..{sel_end_char} with {pasted_chars} chars"));
            } else {
                let cur_char = state_opt.as_ref()
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| r.primary.index)
                    .unwrap_or_else(|| st.text.chars().count());
                let b = char_to_byte_index(&st.text, cur_char);
                st.text.insert_str(b, &pasted);
                store_cursor(ui.ctx(), cur_char + pasted_chars);
                crate::console::info(console, "edit",
                    format!("ctx-menu paste: inserted {pasted_chars} chars at {cur_char}"));
            }
        }
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Select All").clicked() {
        let total = st.text.chars().count();
        if let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
            s.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(0), CCursor::new(total))));
            s.store(ui.ctx(), editor_id);
        }
        crate::console::info(console, "edit",
            format!("ctx-menu select all: {total} chars"));
        ui.close_menu();
    }
}

/// `easylist.txt` → `easylist-org.txt`; `data` → `data-org`.
/// Build a unified diff (~3 lines of context) between `original` and
/// `edited` using `imara-diff`'s Histogram algorithm. Returns the diff as
/// a single string with `+` / `-` / ` ` line prefixes plus `@@ … @@` hunk
/// headers, suitable for line-by-line color rendering.
fn compute_diff(original: &str, edited: &str) -> String {
    use imara_diff::intern::InternedInput;
    use imara_diff::{diff, Algorithm, UnifiedDiffBuilder};
    let input = InternedInput::new(original, edited);
    diff(Algorithm::Histogram, &input, UnifiedDiffBuilder::new(&input))
}

/// Pop-out window showing the unified diff. Closes via the standard
/// window X. Color-codes added (green) / removed (red) / hunk-header
/// (blue) / context (grey).
fn render_diff_window(ctx: &egui::Context, st: &mut Buffer, list: &EnabledList) {
    let mut open = st.show_diff;
    let title = format!("Diff — {} (edited vs original)",
                        list.path.file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default());
    egui::Window::new(title)
        .open(&mut open)
        .resizable(true)
        .default_width(900.0)
        .default_height(600.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let added: usize = st.diff_content.lines()
                    .filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
                let removed: usize = st.diff_content.lines()
                    .filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
                ui.colored_label(egui::Color32::from_rgb(80, 200, 100),
                                 format!("+{added} added"));
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80),
                                 format!("-{removed} removed"));
                if added == 0 && removed == 0 {
                    super::app::weak_label(ui, "(no changes)");
                }
            });
            ui.separator();

            egui::ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
                if st.diff_content.is_empty() {
                    super::app::weak_label(ui, "(empty diff — buffer matches original exactly)");
                    return;
                }
                for line in st.diff_content.lines() {
                    let color = if line.starts_with("+++") || line.starts_with("---") {
                        // file-pair header
                        egui::Color32::from_rgb(160, 160, 200)
                    } else if line.starts_with("@@") {
                        // hunk marker
                        egui::Color32::from_rgb(140, 140, 220)
                    } else if line.starts_with('+') {
                        egui::Color32::from_rgb(80, 200, 100)
                    } else if line.starts_with('-') {
                        egui::Color32::from_rgb(220, 80, 80)
                    } else {
                        egui::Color32::from_rgb(180, 180, 180)
                    };
                    ui.label(egui::RichText::new(line).monospace().color(color));
                }
            });
        });
    if !open {
        st.show_diff = false;
        // Free the cached unified diff. For a heavily-edited 5MB
        // buffer this string can run into MBs and there's no
        // reason to pin it once the popup's gone — the next
        // Show diff click recomputes lazily.
        st.diff_content.clear();
        st.diff_content.shrink_to_fit();
    }
}

fn backup_path(p: &Path) -> PathBuf {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext  = p.extension().and_then(|s| s.to_str());
    let new_name = match ext {
        Some(e) => format!("{stem}-org.{e}"),
        None    => format!("{stem}-org"),
    };
    p.with_file_name(new_name)
}
