use anyhow::Result;
use std::collections::HashSet;

use crate::config::Config;
use crate::paths;
use crate::verdict::{self, Verdict};

#[derive(Debug, Default)]
pub struct PruneReport {
    pub kept:    Vec<String>,
    pub pruned:  Vec<String>,
    pub bytes_freed: u64,
    pub dry_run: bool,
}

pub async fn prune_cli(keep: Option<usize>, dry_run: bool, protect_marked: bool) -> Result<()> {
    paths::ensure_dirs()?;
    let cfg = Config::load_or_default(&paths::config_path())?;
    let keep = keep.unwrap_or(cfg.retention.keep_versions);

    let report = prune(keep, protect_marked, dry_run)?;
    println!("kept {} versions, pruned {} ({} bytes freed){}",
             report.kept.len(),
             report.pruned.len(),
             report.bytes_freed,
             if dry_run { " [dry-run]" } else { "" });
    for t in &report.pruned { println!("  pruned: {t}"); }
    Ok(())
}

pub fn prune(keep: usize, protect_marked: bool, dry_run: bool) -> Result<PruneReport> {
    let mut report = PruneReport { dry_run, ..Default::default() };
    let mut installed = super::list_installed()?;
    // Newest tags last alphabetically isn't reliable across schemes;
    // we trust the install order (lexicographic after `v` prefix is good enough for `vX.Y.Z`).
    installed.sort_by(|a, b| b.tag.cmp(&a.tag));

    let protected: HashSet<String> = if protect_marked {
        verdict::list_version_verdicts()?
            .into_iter()
            .filter(|v| !matches!(v.verdict, Verdict::Unknown))
            .map(|v| v.tag)
            .collect()
    } else { HashSet::new() };

    for (idx, v) in installed.iter().enumerate() {
        let keep_window = idx < keep;
        let is_protected = protected.contains(&v.tag);
        if keep_window || is_protected {
            report.kept.push(v.tag.clone());
            continue;
        }
        let bytes = dir_size(&v.folder).unwrap_or(0);
        if !dry_run {
            std::fs::remove_dir_all(&v.folder)?;
        }
        report.pruned.push(v.tag.clone());
        report.bytes_freed += bytes;
    }
    Ok(report)
}

fn dir_size(p: &std::path::Path) -> Result<u64> {
    let mut total = 0;
    for e in walkdir::WalkDir::new(p) {
        let e = e?;
        if e.file_type().is_file() { total += e.metadata()?.len(); }
    }
    Ok(total)
}
