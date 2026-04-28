use anyhow::Result;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub struct RunResult {
    pub url:           String,
    pub started_at:    i64,
    pub finished_at:   i64,
    pub blocked_hosts: BTreeSet<String>,
    pub allowed_hosts: BTreeSet<String>,
    pub console_errors: Vec<String>,
}

/// Drive Brave via CDP (chromiumoxide) and capture network outcomes for `url`.
/// Skeleton — full implementation lands alongside the GUI run console.
pub async fn run_url(_cdp_url: &str, url: &str) -> Result<RunResult> {
    Ok(RunResult { url: url.into(), ..Default::default() })
}
