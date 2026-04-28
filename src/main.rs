// Hide the console window on release Windows builds — keep it on debug
// builds so tracing output and panics are visible while developing.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// Several modules ship as stubs (runner, diff, report, offline, watcher,
// merge, …) that hold the API surface for code we haven't wired in yet.
// Silence the dead-code chatter while that scaffolding is in place.
#![allow(dead_code)]

use anyhow::Result;
use clap::Parser;

mod cli;
mod config;
mod console;
mod paths;
mod versions;
mod profile;
mod lists;
mod verdict;
mod bisect;
mod runner;
mod diff;
mod report;
mod offline;
mod gui;
mod wsl;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .init();

    // Build the runtime explicitly. The GUI runs on the main thread (required
    // by winit/eframe, esp. on macOS) and submits async work to this runtime
    // via the Handle. Async CLI commands use `rt.block_on` directly.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let handle = rt.handle().clone();

    let args = cli::Cli::parse();
    cli::run(args, handle, &rt)
}
