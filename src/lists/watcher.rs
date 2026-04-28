use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

pub struct ListWatcher {
    _watcher: RecommendedWatcher,
    pub rx:   Receiver<notify::Result<notify::Event>>,
    pub root: PathBuf,
}

/// Watch the profile dir for component-folder mutations (new sibling versions,
/// list.txt rewrites). The GUI consumes events from `rx` and decides whether to
/// quarantine, prompt, or ignore.
pub fn watch_profile(profile_dir: &Path) -> Result<ListWatcher> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(tx)?;
    watcher.watch(profile_dir, RecursiveMode::Recursive)?;
    Ok(ListWatcher { _watcher: watcher, rx, root: profile_dir.to_path_buf() })
}
