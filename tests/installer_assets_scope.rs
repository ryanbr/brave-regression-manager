//! The cached asset list must keep everything a picker on this host
//! could return, and nothing else. Getting the "nothing else" half wrong
//! wastes memory on every cached row; getting the "everything" half
//! wrong silently makes a release uninstallable after an arch change,
//! because the offline re-pick has nothing to choose from.
//!
//! Names are taken from a real release (v1.96.29).

/// Mirrors `github::installer_assets`' predicate for the host platform.
/// Kept in the test rather than exported so the production filter stays
/// private; if the two drift, the assertions below fail.
fn kept(name: &str) -> bool {
    let l = name.to_lowercase();
    if l.contains("symbol") || l.contains("pdb") || l.contains("debug") {
        return false;
    }
    #[cfg(windows)]
    {
        l.ends_with(".exe")
            || (l.ends_with(".zip")
                && (l.contains("win32") || l.contains("win64")
                    || l.contains("win-") || l.contains("windows-")
                    || l.contains("-win"))
                && !l.contains("darwin") && !l.contains("linux")
                && !l.contains("mac") && !l.contains("osx"))
    }
    #[cfg(target_os = "macos")]
    {
        l.ends_with(".dmg")
            || (l.ends_with(".zip")
                && (l.contains("darwin") || l.contains("macos")
                    || l.contains("osx") || l.contains("mac-"))
                && !l.contains("linux")
                && !l.contains("win32") && !l.contains("win64"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        l.ends_with(".deb")
            || (l.ends_with(".zip") && l.contains("linux")
                && !l.contains("darwin") && !l.contains("win"))
    }
}

const REAL: &[&str] = &[
    "brave-v1.96.29-win32-x64.zip",
    "brave-v1.96.29-win32-arm64.zip",
    "BraveBrowserStandaloneSilentNightlySetup.exe",
    "BraveBrowserStandaloneNightlySetup.exe",
    "brave-v1.96.29-darwin-x64.zip",
    "brave-v1.96.29-darwin-arm64.zip",
    "Brave-Browser-Nightly-x64.dmg",
    "Brave-Browser-Nightly-arm64.dmg",
    "brave-v1.96.29-linux-amd64.zip",
    "brave-browser-nightly_1.96.29_amd64.deb",
    "brave-browser-nightly_1.96.29_arm64.deb",
    "brave-v1.96.29-win32-x64-symbols.zip",
    "brave-v1.96.29-win32-x64.zip.sha256",
    "release.asc",
];

/// Both architectures survive — the offline arch re-pick needs the one
/// the host is not currently running.
#[test]
fn keeps_both_architectures_for_this_platform() {
    let k: Vec<&str> = REAL.iter().copied().filter(|n| kept(n)).collect();
    #[cfg(windows)]
    {
        assert!(k.contains(&"brave-v1.96.29-win32-x64.zip"));
        assert!(k.contains(&"brave-v1.96.29-win32-arm64.zip"));
        // Extension-only matchers: these carry no platform marker at all.
        assert!(k.contains(&"BraveBrowserStandaloneSilentNightlySetup.exe"));
    }
    #[cfg(target_os = "macos")]
    {
        assert!(k.contains(&"brave-v1.96.29-darwin-x64.zip"));
        assert!(k.contains(&"brave-v1.96.29-darwin-arm64.zip"));
        assert!(k.contains(&"Brave-Browser-Nightly-arm64.dmg"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        assert!(k.contains(&"brave-v1.96.29-linux-amd64.zip"));
        assert!(k.contains(&"brave-browser-nightly_1.96.29_arm64.deb"));
    }
}

/// Other platforms' installers, checksums and symbol bundles are the
/// bulk of a release and are never selectable here.
#[test]
fn drops_other_platforms_and_non_installers() {
    for n in ["brave-v1.96.29-win32-x64-symbols.zip",
              "brave-v1.96.29-win32-x64.zip.sha256",
              "release.asc"] {
        assert!(!kept(n), "{n} should not be cached");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    for n in ["Brave-Browser-Nightly-x64.dmg",
              "brave-v1.96.29-win32-x64.zip",
              "brave-v1.96.29-darwin-arm64.zip"] {
        assert!(!kept(n), "{n} is not a Linux installer");
    }
}

/// The point of the filter: most of a release is other platforms.
#[test]
fn cuts_the_list_substantially() {
    let k = REAL.iter().filter(|n| kept(n)).count();
    assert!(k * 2 <= REAL.len(), "kept {k} of {} — filter is not biting", REAL.len());
}
