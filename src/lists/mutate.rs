use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum LineState {
    Removed,
    Commented,
}

/// In-memory mutations applied lazily on save. Untouched lines cost zero memory.
#[derive(Debug, Clone, Default)]
pub struct ListBuffer {
    pub path:      PathBuf,
    pub line_count: usize,
    pub state:     BTreeMap<usize, LineState>,   // 1-based line index
}

impl ListBuffer {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let line_count = bytes.iter().filter(|&&b| b == b'\n').count() + 1;
        Ok(Self { path: path.into(), line_count, state: BTreeMap::new() })
    }

    pub fn toggle_remove(&mut self, line: usize) {
        if matches!(self.state.get(&line), Some(LineState::Removed)) {
            self.state.remove(&line);
        } else {
            self.state.insert(line, LineState::Removed);
        }
    }

    pub fn toggle_comment(&mut self, line: usize) {
        if matches!(self.state.get(&line), Some(LineState::Commented)) {
            self.state.remove(&line);
        } else {
            self.state.insert(line, LineState::Commented);
        }
    }

    pub fn restore(&mut self) { self.state.clear(); }

    /// Materialize buffer to disk atomically. Backs up `<path>.orig` on first save.
    pub fn save(&self) -> Result<()> {
        if !self.path.exists() {
            return Err(anyhow!("list file vanished: {}", self.path.display()));
        }
        let orig = self.path.with_extension(format!("{}.orig",
            self.path.extension().and_then(|s| s.to_str()).unwrap_or("txt")));
        if !orig.exists() { std::fs::copy(&self.path, &orig)?; }

        let src = std::fs::read_to_string(&self.path)?;
        let mut out = String::with_capacity(src.len());
        for (idx, line) in src.split_inclusive('\n').enumerate() {
            let lineno = idx + 1;
            match self.state.get(&lineno) {
                Some(LineState::Removed)    => continue,
                Some(LineState::Commented)  => {
                    out.push('!');
                    out.push_str(line);
                }
                None => out.push_str(line),
            }
        }

        let tmp = self.path.with_extension("new");
        std::fs::write(&tmp, out)?;
        // Atomic replace (Windows: MoveFileEx via std::fs::rename which uses MoveFileExW under the hood).
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}
