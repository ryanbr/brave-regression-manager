use crate::runner::RunResult;
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub struct CellDiff {
    pub url:                String,
    pub newly_blocked:      BTreeSet<String>,
    pub newly_allowed:      BTreeSet<String>,
    pub new_console_errors: Vec<String>,
}

pub fn diff(base: &RunResult, candidate: &RunResult) -> CellDiff {
    let newly_blocked = candidate.blocked_hosts.difference(&base.blocked_hosts).cloned().collect();
    let newly_allowed = base.blocked_hosts.difference(&candidate.blocked_hosts).cloned().collect();
    let mut new_errors: Vec<String> = candidate.console_errors.to_vec();
    new_errors.retain(|e| !base.console_errors.contains(e));
    CellDiff { url: candidate.url.clone(), newly_blocked, newly_allowed, new_console_errors: new_errors }
}
