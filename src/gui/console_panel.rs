use egui::{Color32, RichText, Ui};

use crate::console::{self, Level};

use super::state::AppState;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.heading("Console");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Clear").clicked() {
                if let Ok(mut g) = state.console.lock() { g.clear(); }
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
