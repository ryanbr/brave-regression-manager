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

    egui::ScrollArea::vertical().auto_shrink([false; 2]).stick_to_bottom(true).show(ui, |ui|
    {
        let entries: Vec<console::Entry> = state.console.lock()
            .map(|g| g.entries().cloned().collect())
            .unwrap_or_default();
        if entries.is_empty() {
            super::app::weak_label(ui, "(no console output yet)");
            return;
        }
        for e in &entries {
            let (color, prefix) = match e.level {
                Level::Info  => (Color32::from_rgb(190, 190, 190), "INFO "),
                Level::Warn  => (Color32::from_rgb(220, 180, 60),  "WARN "),
                Level::Error => (Color32::from_rgb(220, 80, 80),   "ERROR"),
                Level::Brave => (Color32::from_rgb(100, 180, 220), "BRAVE"),
            };
            let ts = e.ts.format("%H:%M:%S").to_string();
            let line = format!("{ts}  {prefix}  [{}]  {}", e.source, e.msg);
            ui.label(RichText::new(line).monospace().color(color));
        }
    });
}
