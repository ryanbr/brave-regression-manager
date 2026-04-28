use anyhow::Result;
use std::path::Path;

#[derive(Debug, Default)]
pub struct MergeReport {
    pub conflicts:        usize,
    pub kept_user_removals: usize,
    pub took_upstream_lines: usize,
}

/// 3-way merge user-edited list against new upstream.
/// Stub: writes upstream as-is for now and reports zero conflicts.
/// Real implementation will use `imara-diff` (line-3-way).
pub fn three_way(_user_edited: &Path, _upstream_new: &Path, _base_sha: &str) -> Result<MergeReport> {
    Ok(MergeReport::default())
}
