use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::runtime::{Handle, Runtime};

#[derive(Parser, Debug)]
#[command(name = "brave-regress", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    Gui,
    #[command(subcommand)] Versions(VersionsCmd),
    #[command(subcommand)] Profile(ProfileCmd),
    #[command(subcommand)] Lists(ListsCmd),
    Mark { kind: String, target: String, verdict: String, #[arg(long)] note: Option<String> },
    #[command(subcommand)] Bisect(BisectCmd),
    Prune {
        #[arg(long)] keep: Option<usize>,
        #[arg(long)] dry_run: bool,
        #[arg(long)] no_protect_marked: bool,
    },
    /// Inspect a downloaded installer .exe — PE walk, overlay analysis,
    /// magic-byte scan for NSIS / 7z / MSI / Inno / WiX / CAB / LZMA.
    Diagnose { path: std::path::PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum VersionsCmd {
    /// List Brave Nightly releases. By default hides releases with no host installer.
    Available { #[arg(long)] all: bool },
    Installed,
    Install { tag: String },
    Uninstall { tag: String },
    Launch { tag: String, #[arg(long)] profile: String },
}

#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    New    { name: String },
    Delete { name: String },
    List,
    Reset  { name: String, #[arg(long, default_value = "full")] scope: String },
    Seed   { name: String, #[arg(long)] version: String },
}

#[derive(Subcommand, Debug)]
pub enum ListsCmd {
    Show   { profile: String },
    Apply  { profile: String, #[arg(long)] version: String },
    Pin    { profile: String, #[arg(long)] verify: bool, #[arg(long)] unpin: bool },
    Update { profile: String, #[arg(long, default_value = "review")] action: String },
}

#[derive(Subcommand, Debug)]
pub enum BisectCmd {
    Versions { #[arg(long)] good: String, #[arg(long)] bad: String,
               #[arg(long)] url: String, #[arg(long)] check: Option<String> },
    Rules    { #[arg(long)] version: String, #[arg(long)] list: String,
               #[arg(long)] url: String, #[arg(long)] expect: String },
}

pub fn run(args: Cli, handle: Handle, rt: &Runtime) -> Result<()> {
    match args.cmd.unwrap_or(Cmd::Gui) {
        Cmd::Gui                  => crate::gui::launch(handle),
        Cmd::Versions(c)          => rt.block_on(crate::versions::handle(c)),
        Cmd::Profile(c)           => rt.block_on(crate::profile::handle(c)),
        Cmd::Lists(c)             => rt.block_on(crate::lists::handle(c)),
        Cmd::Mark { kind, target, verdict, note }
                                  => crate::verdict::mark(&kind, &target, &verdict, note.as_deref()),
        Cmd::Bisect(c)            => rt.block_on(crate::bisect::handle(c)),
        Cmd::Prune { keep, dry_run, no_protect_marked }
                                  => rt.block_on(crate::versions::retention::prune_cli(keep, dry_run, !no_protect_marked)),
        Cmd::Diagnose { path }    => {
            let report = crate::versions::diagnose::diagnose_installer(&path)?;
            println!("{report}");
            Ok(())
        }
    }
}
