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
            // Window 60px taller than the original so the
            // Available-on-GitHub list fits a few additional
            // release rows at default sizing.
            .with_inner_size([1000.0, 780.0])
            .with_min_inner_size([900.0, 620.0]),
        // Don't persist window position/size — eframe's saved state can
        // come back as an awkward shape (e.g. very tall and narrow) from a
        // prior resize, which the layout can't recover from.
        persist_window: false,
        ..Default::default()
    };
    // Version comes from Cargo.toml's `version` field — `cargo set-version`
    // bumps it, the release workflow tags `v$version` on git, so the title
    // bar always matches the GitHub release the user downloaded.
    const TITLE: &str = concat!(
        "Brave Regression Manager v", env!("CARGO_PKG_VERSION")
    );
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| Box::new(app::App::new(cc, handle.clone()))),
    ).map_err(|e| anyhow::anyhow!("eframe: {e}"))
}
