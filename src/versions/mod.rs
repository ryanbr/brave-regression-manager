use anyhow::{anyhow, Result};
use std::path::PathBuf;

use crate::cli::VersionsCmd;
use crate::paths;

pub mod detect;
pub mod diagnose;
pub mod github;
pub mod install;
pub mod launch;
pub mod retention;

#[derive(Debug, Clone)]
pub struct InstalledVersion {
    pub tag: String,
    pub folder: PathBuf,
    pub binary: PathBuf,
}

pub fn list_installed() -> Result<Vec<InstalledVersion>> {
    let dir = paths::versions_dir();
    if !dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() { continue; }
        let tag = entry.file_name().to_string_lossy().into_owned();
        let binary = paths::brave_binary(&tag);
        if binary.exists() {
            out.push(InstalledVersion { tag, folder: entry.path(), binary });
        }
    }
    out.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(out)
}

pub fn is_installed(tag: &str) -> bool {
    paths::brave_binary(tag).exists()
}

pub async fn handle(cmd: VersionsCmd) -> Result<()> {
    paths::ensure_dirs()?;
    match cmd {
        VersionsCmd::Available { all } => {
            // CLI default mirrors the GUI default of 100.
            for r in github::list_nightly_releases(100).await? {
                if !all && !r.has_host_installer() { continue; }
                let status = match &r.host_asset {
                    Some(name) => format!("✓ {name}"),
                    None       => format!("⊘ {}", r.skip_reason()),
                };
                println!("{}\t{}\t{}", r.tag, r.published_at, status);
            }
            Ok(())
        }
        VersionsCmd::Installed => {
            for v in list_installed()? { println!("{}\t{}", v.tag, v.folder.display()); }
            Ok(())
        }
        VersionsCmd::Install { tag } => {
            if let Err(e) = install::install_tag(&tag).await {
                // `{e:#}` walks the anyhow cause chain so the user sees the
                // underlying reason, not just the wrap message.
                return Err(anyhow::anyhow!("install {tag}: {e:#}"));
            }
            println!("installed {tag} → {}", paths::version_dir(&tag).display());
            Ok(())
        }
        VersionsCmd::Uninstall { tag } => {
            let dir = paths::version_dir(&tag);
            if !dir.exists() { return Err(anyhow!("not installed: {tag}")); }
            std::fs::remove_dir_all(&dir)?;
            println!("uninstalled {tag}");
            Ok(())
        }
        VersionsCmd::Launch { tag, profile } => {
            let _child = launch::launch(&tag, &profile)?;
            println!("launched {tag} (profile={profile})");
            Ok(())
        }
    }
}
