use egui::{Color32, RichText, Ui};

use crate::console::{self, Level};

use super::state::AppState;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.heading("Console");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Clear Console")
                .on_hover_text("Drop every Console line. The on-disk \
                                config is untouched — only the in-memory \
                                Console buffer.")
                .clicked()
            {
                if let Ok(mut g) = state.console.lock() { g.clear(); }
                state.console_content_w = 0.0;
            }
            // Copy the entire Console buffer to clipboard, in the
            // same `HH:MM:SS  LEVEL  [source]  msg` shape the panel
            // renders. Useful for sharing a log without screenshots.
            if ui.small_button("Copy to clipboard")
                .on_hover_text(
                    "Copy every Console line to the clipboard, formatted \
                     as it appears here. Use to paste a full log into a \
                     bug report / chat / etc.")
                .clicked()
            {
                let entries: Vec<console::Entry> = state.console.lock()
                    .map(|g| g.entries().cloned().collect())
                    .unwrap_or_default();
                let mut buf = String::new();
                for e in &entries {
                    let prefix = match e.level {
                        Level::Info  => "INFO ",
                        Level::Warn  => "WARN ",
                        Level::Error => "ERROR",
                        Level::Brave => "BRAVE",
                    };
                    let ts = e.ts.format("%H:%M:%S");
                    use std::fmt::Write as _;
                    let _ = writeln!(buf, "{ts}  {prefix}  [{}]  {}",
                        e.source, e.msg);
                }
                let bytes = buf.len();
                ui.ctx().copy_text(buf);
                // Status-bar-only feedback — deliberately no console
                // line, otherwise the act of copying mutates the
                // very buffer the user just copied.
                state.status_msg = format!(
                    "copied {} console line(s) ({bytes} bytes) to clipboard",
                    entries.len());
            }
            let count = state.console.lock().map(|g| g.len()).unwrap_or(0);
            super::app::weak_label(ui, format!("{count} entries"));
        });
    });
    ui.separator();

    // Viewport-rendered. ScrollArea::show_rows tells egui exactly
    // how many rows there are and the uniform row height; egui
    // then asks the closure to paint only the on-screen subset
    // each frame. Without this, every entry was format!()'d and
    // laid out per paint regardless of visibility — at thousands
    // of entries that's hundreds of KB of allocation per frame.
    //
    // The lock is taken ONCE, before the row count is read, and held
    // across the closure. The buffer is a 1000-entry ring that
    // background threads push into (and "Clear Console" empties), so
    // reading `len()` under one lock and the rows under another lets
    // entries shift out from under the indices egui hands back — every
    // visible row would show its neighbour, or the panel would paint
    // blank while the scrollbar still claimed 1000 rows. Lock duration
    // is bounded by the closure (sync rendering); nothing inside it
    // pushes to the Console, so there is no re-entrancy.
    // A poisoned mutex must still say so: before this was a single
    // lock the `unwrap_or(0)` fell through to the "(no console output
    // yet)" label, and a bare `return` here would instead paint an
    // unexplained empty tab. `console::push` swallows poison too, so
    // logging is silently dead for the rest of the session — that is
    // worth a visible marker rather than a blank panel.
    let Ok(g) = state.console.lock() else {
        super::app::weak_label(ui, "(console unavailable — internal lock poisoned)");
        return;
    };
    let total = g.len();
    if total == 0 {
        super::app::weak_label(ui, "(no console output yet)");
        return;
    }
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
    // `show_rows` promises egui that every row is exactly `row_h`
    // tall — it reserves `range.start * row_h` of spacer above and
    // `(total - range.end) * row_h` below, then paints the rest. A
    // wrapped label breaks that promise: it paints two-plus lines into
    // a one-line budget, dragging every row after it out of step with
    // the scrollbar and, under stick_to_bottom, shoving the newest
    // entries below the visible area. Console lines are routinely
    // wider than the window (asset URLs, raw Brave stderr), so extend
    // rather than wrap, and give the area a horizontal scrollbar so
    // the overflow is still reachable.
    //
    // That scrollbar needs a content width that does NOT depend on
    // which rows happen to be on screen. egui derives content size from
    // what the closure actually painted and re-clamps the scroll offset
    // to it every frame, so with `show_rows` the horizontal extent
    // would collapse the moment the viewport moved onto short lines —
    // yanking the view back to column 0 while the user was reading a
    // long URL, and resizing the scrollbar handle each frame. The ring
    // tracks the widest line it has ever held, which gives a stable
    // extent for the cost of one `max` per push.
    let char_w = ui.fonts(|f| f.glyph_width(
        &egui::TextStyle::Monospace.resolve(ui.style()), ' '));
    // "HH:MM:SS" + 2 + "LEVEL" + 2 + "[" + "]" + 2 = 21 fixed chars
    // around the per-entry `source` + `msg` the ring measured.
    const PREFIX_CHARS: usize = 21;
    // The char estimate is the floor; `console_content_w` is the running
    // max of what egui actually laid out, which covers the glyphs the
    // monospace face doesn't own and whose real advance is wider.
    let content_w = ((g.max_line_chars() + PREFIX_CHARS) as f32 * char_w)
        .max(state.console_content_w);
    let out = egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .show_rows(ui, row_h, total, |ui, range| {
            ui.set_min_width(content_w);
            for i in range {
                let Some(e) = g.get(i) else { continue };
                let (color, prefix) = match e.level {
                    Level::Info  => (Color32::from_rgb(190, 190, 190), "INFO "),
                    Level::Warn  => (Color32::from_rgb(220, 180, 60),  "WARN "),
                    Level::Error => (Color32::from_rgb(220, 80, 80),   "ERROR"),
                    Level::Brave => (Color32::from_rgb(100, 180, 220), "BRAVE"),
                };
                let ts = e.ts.format("%H:%M:%S").to_string();
                let line = format!("{ts}  {prefix}  [{}]  {}", e.source, e.msg);
                ui.add(egui::Label::new(
                    RichText::new(line).monospace().color(color))
                    .wrap(false));
            }
        });
    // Monotonic: never let the extent shrink back when a wide row
    // scrolls out of the painted range, which is what makes egui clamp
    // the user's horizontal offset to 0 mid-read.
    drop(g);
    state.console_content_w = state.console_content_w.max(out.content_size.x);
}
