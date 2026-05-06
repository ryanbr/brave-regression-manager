//! OS-level Brave-running check, scoped to **our edit targets**.
//!
//! Editing Preferences is only unsafe when a Brave process has the
//! same dir loaded — that one's shutdown will overwrite our edit
//! from its in-memory pref state. A user's separately-installed
//! Brave running against `%LOCALAPPDATA%/BraveSoftware/...` is
//! irrelevant to a brave-regress managed profile and shouldn't gate
//! the edit.
//!
//! We scan the process list, find Brave processes, and compare each
//! one's `--user-data-dir` arg (case-insensitive on Windows) against
//! the target dirs we're about to write. Only matches block.

use std::path::Path;

use sysinfo::{ProcessRefreshKind, RefreshKind, System};

const BRAVE_EXE_NAMES: &[&str] = &[
    "brave.exe",
    "brave",
    "brave-browser",
    "Brave Browser",
    "Brave-Browser",
];

fn fresh_system() -> System {
    System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    )
}

/// One running Brave with the user-data-dir we extracted from its
/// argv. The PID is mostly for the error message — humans skim it,
/// it lands in the GUI.
#[derive(Debug, Clone)]
pub struct BraveProc {
    pub pid:           u32,
    pub name:          String,
    pub user_data_dir: Option<String>,
}

/// Every Brave process on the host. The caller decides which ones
/// matter.
pub fn list_brave_procs() -> Vec<BraveProc> {
    let sys = fresh_system();
    let mut out = Vec::new();
    for (pid, p) in sys.processes() {
        let name = p.name().to_string_lossy().into_owned();
        if !name_matches_brave(&name) { continue; }
        let user_data_dir = p.cmd().iter()
            .find_map(|arg| extract_user_data_dir(&arg.to_string_lossy()));
        out.push(BraveProc {
            pid: pid.as_u32(),
            name,
            user_data_dir,
        });
    }
    out
}

/// True if any Brave is running with `--user-data-dir` matching one
/// of the given target dirs. Brave that's running against an
/// unrelated dir (a user's regular Brave install on a different
/// profile path) is ignored — those don't endanger our edit.
///
/// A Brave process whose argv we couldn't read (sysinfo returns
/// nothing for `cmd()`) is *not* treated as a conflict, since
/// blocking on a Brave that probably isn't even on our profile is
/// worse UX than the small risk of letting a write through. In
/// practice the user's session reliably exposes argv for processes
/// they launched themselves, so this only kicks in for cross-session
/// Brave instances which are by definition not on our profiles.
pub fn brave_running_for_targets(targets: &[std::path::PathBuf]) -> Vec<BraveProc> {
    let target_lc: Vec<String> = targets.iter()
        .map(|p| normalise_path(p))
        .collect();
    list_brave_procs()
        .into_iter()
        .filter(|p| match &p.user_data_dir {
            Some(udd) => {
                let udd_lc = normalise_str(udd);
                target_lc.iter().any(|t| paths_match(&udd_lc, t))
            }
            None => false,
        })
        .collect()
}

fn paths_match(a: &str, b: &str) -> bool {
    // Exact match OR one is a prefix of the other (covers
    // "Default" subdirs and trailing-separator differences).
    a == b
        || a.starts_with(b) && a[b.len()..].starts_with(['/', '\\'])
        || b.starts_with(a) && b[a.len()..].starts_with(['/', '\\'])
}

fn normalise_path(p: &Path) -> String {
    normalise_str(&p.to_string_lossy())
}

fn normalise_str(s: &str) -> String {
    s.trim_end_matches(['/', '\\']).to_lowercase()
}

/// Pull `<value>` out of a `--user-data-dir=<value>` argv element.
/// Returns None for anything else (including the rarer
/// `--user-data-dir <value>` two-token form, since sysinfo gives us
/// `cmd()` as the original tokens — we'd need to scan adjacent args
/// to handle that, which Brave's launchers don't actually use).
fn extract_user_data_dir(arg: &str) -> Option<String> {
    arg.strip_prefix("--user-data-dir=")
        .map(|v| v.trim_matches('"').to_string())
}

fn name_matches_brave(name: &str) -> bool {
    let n = name.to_lowercase();
    BRAVE_EXE_NAMES.iter()
        .any(|b| n == b.to_lowercase() || n.starts_with(&b.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_user_data_dir() {
        assert_eq!(extract_user_data_dir("--user-data-dir=C:\\foo\\bar"),
                   Some("C:\\foo\\bar".to_string()));
        assert_eq!(extract_user_data_dir("--user-data-dir=\"C:\\foo bar\""),
                   Some("C:\\foo bar".to_string()));
        assert_eq!(extract_user_data_dir("--no-first-run"), None);
    }
    #[test]
    fn paths_match_prefix_and_exact() {
        assert!(paths_match("c:\\foo\\bar", "c:\\foo\\bar"));
        assert!(paths_match("c:\\foo\\bar\\default", "c:\\foo\\bar"));
        assert!(paths_match("c:\\foo\\bar", "c:\\foo\\bar\\default"));
        assert!(!paths_match("c:\\foo\\bar", "c:\\foo\\baz"));
        assert!(!paths_match("c:\\foo\\barbaz", "c:\\foo\\bar"));
    }
}
