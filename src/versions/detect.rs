//! Discover Brave installs already on the system. Brave's recent Windows
//! installer is a custom Omaha-style format that no public extractor handles,
//! so the practical way to get a working Brave for testing is to copy the
//! files out of an existing system install.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DetectedInstall {
    /// Human label like "User Brave Nightly" or "System Brave Nightly".
    pub source:  String,
    /// Version string parsed from the `Application/<ver>/` subfolder, prefixed
    /// with `v`. Empty when we couldn't determine it.
    pub version: String,
    /// Path to brave.exe / brave-browser-nightly / Brave Browser Nightly.
    pub binary:  PathBuf,
    /// The folder we'd copy: contains brave.exe + resources + locales/ etc.
    pub root:    PathBuf,
}

pub fn detect() -> Vec<DetectedInstall> {
    let mut out = Vec::new();

    #[cfg(windows)]
    {
        for (label, base) in candidate_roots_windows() {
            scan_windows(&label, &base, &mut out);
        }
    }

    #[cfg(target_os = "macos")]
    {
        for (label, app) in [
            ("System Brave Nightly", PathBuf::from("/Applications/Brave Browser Nightly.app")),
            ("User Brave Nightly",   dirs::home_dir().map(|h| h.join("Applications/Brave Browser Nightly.app")).unwrap_or_default()),
        ] {
            if app.exists() {
                let binary = app.join("Contents/MacOS/Brave Browser Nightly");
                if binary.exists() {
                    out.push(DetectedInstall {
                        source: label.into(),
                        version: read_macos_version(&app).unwrap_or_default(),
                        binary, root: app,
                    });
                }
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (label, root) in [
            ("System Brave Nightly", PathBuf::from("/opt/brave.com/brave-nightly")),
            ("Snap Brave Nightly",   PathBuf::from("/snap/brave-nightly/current")),
        ] {
            let binary = root.join("brave-browser-nightly");
            if binary.exists() {
                out.push(DetectedInstall {
                    source: label.into(), version: String::new(),
                    binary, root,
                });
            }
        }
    }

    out
}

#[cfg(windows)]
fn candidate_roots_windows() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        v.push(("User Brave Nightly".into(),
                local.join(r"BraveSoftware\Brave-Browser-Nightly")));
    }
    v.push(("System Brave Nightly (Program Files)".into(),
            PathBuf::from(r"C:\Program Files\BraveSoftware\Brave-Browser-Nightly")));
    v.push(("System Brave Nightly (Program Files x86)".into(),
            PathBuf::from(r"C:\Program Files (x86)\BraveSoftware\Brave-Browser-Nightly")));
    v
}

#[cfg(windows)]
fn scan_windows(label: &str, base: &std::path::Path, out: &mut Vec<DetectedInstall>) {
    // Layout: <base>\Application\<version>\brave.exe
    let app = base.join("Application");
    if !app.is_dir() { return; }
    let mut versions: Vec<(semver::Version, PathBuf)> = std::fs::read_dir(&app)
        .into_iter().flatten().flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            semver::Version::parse(&n).ok().map(|v| (v, e.path()))
        })
        .collect();
    // Newest first.
    versions.sort_by(|a, b| b.0.cmp(&a.0));

    for (ver, ver_dir) in versions {
        let bin = ver_dir.join("brave.exe");
        if !bin.exists() { continue; }
        out.push(DetectedInstall {
            source:  label.to_string(),
            version: format!("v{ver}"),
            binary:  bin,
            root:    ver_dir,
        });
    }
}

#[cfg(target_os = "macos")]
fn read_macos_version(app: &std::path::Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let text  = std::fs::read_to_string(&plist).ok()?;
    // Cheap regex-free parse: find the CFBundleShortVersionString key/value pair.
    let key = "<key>CFBundleShortVersionString</key>";
    let idx = text.find(key)?;
    let rest = &text[idx + key.len()..];
    let s = rest.find("<string>")?;
    let e = rest.find("</string>")?;
    Some(format!("v{}", rest[s + 8..e].trim()))
}

/// Recursively copy `src` → `dst`. Mirrors `cp -r`. Used to clone a detected
/// install into our `versions/<tag>/` tree so brave-regress can drive it
/// in isolation.
pub fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() { copy_dir_all(&entry.path(), &to)?; }
        else if ty.is_file() { std::fs::copy(entry.path(), &to)?; }
        // Skip symlinks for cross-platform simplicity.
    }
    Ok(())
}
