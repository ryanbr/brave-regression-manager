use anyhow::Result;
use tokio::runtime::Handle;

mod app;
mod console_panel;
mod tab_versions;
mod tab_lists;
mod list_editor;
mod state;

pub fn launch(handle: Handle) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([900.0, 560.0]),
        // Don't persist window position/size — eframe's saved state can
        // come back as an awkward shape (e.g. very tall and narrow) from a
        // prior resize, which the layout can't recover from.
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "Brave Regression Manager",
        options,
        Box::new(move |cc| Box::new(app::App::new(cc, handle.clone()))),
    ).map_err(|e| anyhow::anyhow!("eframe: {e}"))
}
