/// Detect whether we're running under WSL (Windows Subsystem for Linux).
/// Linux-only check; everything else returns false.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn is_wsl() -> bool {
    if std::env::var("WSL_DISTRO_NAME").is_ok() { return true; }
    if std::env::var("WSL_INTEROP").is_ok() { return true; }
    if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        let l = s.to_lowercase();
        if l.contains("microsoft") || l.contains("wsl") { return true; }
    }
    false
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn is_wsl() -> bool { false }
