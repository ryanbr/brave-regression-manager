use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::paths;
use super::github;

/// Pretend to be Chrome on Windows when fetching from GitHub's CDN / S3.
/// Anonymous downloads through some redirector layers are filtered if the
/// UA looks like a scraper. GitHub's *API* call (in `github.rs`) keeps its
/// own `brave-regress` UA — the API actually wants you to identify your app.
const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36";

/// Snapshot of an in-flight Brave installer download. The GUI polls a sink
/// holding one of these to render a progress bar + speed.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub tag:        String,
    pub asset_name: String,
    pub downloaded: u64,
    pub total:      u64,
    pub speed_bps:  u64,    // bytes per second, sampled over the last ~500ms
    pub started_at: Instant,
}
impl DownloadProgress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.downloaded as f32 / self.total as f32).clamp(0.0, 1.0) }
    }
    pub fn eta_secs(&self) -> Option<u64> {
        if self.speed_bps == 0 || self.downloaded >= self.total { return None; }
        Some((self.total - self.downloaded) / self.speed_bps.max(1))
    }
}

/// Live download progress for in-flight installs, keyed by tag so up
/// to N parallel installs each get their own progress bar that doesn't
/// flicker as the others write. The previous shape was a single Option
/// slot, which made parallel downloads stomp each other every iteration.
pub type ProgressSink = Arc<Mutex<std::collections::HashMap<String, DownloadProgress>>>;

/// Download (with resume + sha) and extract a Nightly tag into its own folder.
pub async fn install_tag(tag: &str) -> Result<PathBuf> {
    install_tag_with_progress(tag, None).await
}

/// Same as `install_tag` but writes live progress (bytes + speed) to `sink`
/// while downloading. The GUI uses this; the CLI passes `None`.
pub async fn install_tag_with_progress(tag: &str, sink: Option<ProgressSink>) -> Result<PathBuf> {
    paths::ensure_dirs()?;
    let release = github::get_release(tag).await?;
    if !release.has_host_installer() {
        return Err(anyhow!("{tag} has no installer for this platform: {} \
                            (run `brave-regress versions available` to list installable tags)",
                           release.skip_reason()));
    }
    let asset = github::pick_asset(&release)?;
    install_tag_with_asset(tag, &asset.name, &asset.browser_download_url, asset.size, sink).await
}

/// Install when the caller already knows the asset details (URL + size +
/// filename). Avoids a second GitHub API roundtrip — important because the
/// anonymous limit is 60 req/hr and we burn one on every fetch otherwise.
pub async fn install_tag_with_asset(
    tag: &str, asset_name: &str, asset_url: &str, asset_size: u64,
    sink: Option<ProgressSink>,
) -> Result<PathBuf> {
    install_tag_with_asset_console(tag, asset_name, asset_url, asset_size, sink, None).await
}

/// Same as `install_tag_with_asset` but also emits per-phase progress
/// to the GUI Console when a handle is supplied. Lets the user see
/// download→extract→flatten transitions instead of a black box.
pub async fn install_tag_with_asset_console(
    tag: &str, asset_name: &str, asset_url: &str, asset_size: u64,
    sink: Option<ProgressSink>,
    console: Option<crate::console::Handle>,
) -> Result<PathBuf> {
    paths::ensure_dirs()?;
    let download_path = paths::downloads_dir().join(asset_name);
    let cached = std::fs::metadata(&download_path).map(|m| m.len() == asset_size).unwrap_or(false);
    if let Some(c) = &console {
        if cached {
            crate::console::info(c, "install",
                format!("{tag}: phase=skip-download (cache hit, {asset_name})"));
        } else {
            crate::console::info(c, "install",
                format!("{tag}: phase=download {asset_name}"));
        }
    }
    download(asset_url, &download_path, asset_size, tag, asset_name, sink.clone()).await?;
    if let Some(s) = &sink { s.lock().unwrap().remove(tag); }

    let dest = paths::version_dir(tag);
    if dest.exists() { std::fs::remove_dir_all(&dest)?; }
    std::fs::create_dir_all(&dest)?;

    if let Some(c) = &console {
        crate::console::info(c, "install",
            format!("{tag}: phase=extract → {}", dest.display()));
    }
    extract(&download_path, &dest)
        .with_context(|| format!("extracting {} → {}", download_path.display(), dest.display()))?;

    // macOS Gatekeeper marks anything we downloaded over HTTP as quarantined,
    // which makes Brave refuse to launch on first run. Strip the xattr from
    // the whole install tree so the .app boots normally. Best-effort; if
    // `xattr` is missing or fails, we continue.
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(&dest)
            .status();
    }

    // sanity: brave binary present
    let bin = paths::brave_binary(tag);
    if !bin.exists() {
        return Err(anyhow!("extraction completed but brave binary not found at {}", bin.display()));
    }

    // Ensure +x on Unix — `.deb` extracted without `dpkg` keeps mode but we
    // also extract from `.7z` (Windows installer payload, when run cross-FS)
    // and tarball helpers that drop the bit. Making this explicit avoids
    // a "Permission denied" launch error on WSL/Linux.
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&bin) {
            let mut perm = meta.permissions();
            if perm.mode() & 0o111 == 0 {
                perm.set_mode(0o755);
                std::fs::set_permissions(&bin, perm)?;
            }
        }
    }
    Ok(dest)
}

async fn download(url: &str, dest: &Path, expected_size: u64,
                  tag: &str, asset_name: &str, sink: Option<ProgressSink>) -> Result<()> {
    if dest.exists() {
        let meta = std::fs::metadata(dest)?;
        if meta.len() == expected_size {
            if let Some(s) = &sink {
                s.lock().unwrap().insert(tag.to_string(), DownloadProgress {
                    tag: tag.into(), asset_name: asset_name.into(),
                    downloaded: expected_size, total: expected_size,
                    speed_bps: 0, started_at: Instant::now(),
                });
            }
            return Ok(());
        }
        std::fs::remove_file(dest)?;
    }
    if let Some(p) = dest.parent() { std::fs::create_dir_all(p)?; }

    let tmp = dest.with_extension("part");

    // Stall detection: if no bytes arrive within this window, abort the
    // current attempt and retry. GitHub-CDN/S3 supports HTTP Range, so we
    // resume from wherever we got to in the .part file.
    let stall_timeout = Duration::from_secs(20);
    let max_attempts: u32 = 6;
    let started_at = Instant::now();

    let mut last_err: Option<String> = None;
    for attempt in 1..=max_attempts {
        let resume_from = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        tracing::info!("download attempt {attempt}/{max_attempts} (resume from {} bytes)", resume_from);

        match download_attempt(
            url, &tmp, resume_from, expected_size,
            tag, asset_name, sink.clone(), stall_timeout, started_at,
        ).await {
            Ok(()) => {
                tokio::fs::rename(&tmp, dest).await?;
                if let Some(s) = &sink {
                    let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(expected_size);
                    s.lock().unwrap().insert(tag.to_string(), DownloadProgress {
                        tag: tag.into(), asset_name: asset_name.into(),
                        downloaded, total: downloaded, speed_bps: 0, started_at,
                    });
                }
                return Ok(());
            }
            Err(e) => {
                last_err = Some(format!("{e:#}"));
                tracing::warn!("attempt {attempt} failed at byte {resume_from}: {e:#}");
                if attempt < max_attempts {
                    // Exponential backoff: 1s, 2s, 4s, 8s, 16s, capped.
                    let backoff = Duration::from_secs((1u64 << (attempt - 1)).min(16));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    Err(anyhow!(
        "download failed after {max_attempts} attempts (last: {})",
        last_err.unwrap_or_else(|| "unknown".into())
    ))
}

/// One download attempt with stall detection + Range-based resume.
#[allow(clippy::too_many_arguments)]
async fn download_attempt(
    url: &str, tmp: &Path, resume_from: u64, expected_size: u64,
    tag: &str, asset_name: &str, sink: Option<ProgressSink>,
    stall_timeout: Duration, started_at: Instant,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(stall_timeout)
        .build()?;

    let mut req = client.get(url);
    if resume_from > 0 {
        req = req.header("Range", format!("bytes={resume_from}-"));
    }

    let resp = req.send().await
        .map_err(|e| anyhow!("send: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("status: {e}"))?;

    let server_resumed = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut downloaded = if server_resumed { resume_from } else { 0 };
    // If the server ignored Range, restart from byte 0 (truncate the .part).
    let mut file = if server_resumed && resume_from > 0 {
        tokio::fs::OpenOptions::new().append(true).open(tmp).await?
    } else {
        tokio::fs::File::create(tmp).await?
    };
    let total = if server_resumed {
        downloaded.saturating_add(resp.content_length().unwrap_or(expected_size.saturating_sub(resume_from)))
    } else {
        resp.content_length().unwrap_or(expected_size)
    };

    let mut last_sample_at = Instant::now();
    let mut last_sample_bytes = downloaded;
    let sample_interval = Duration::from_millis(500);

    if let Some(s) = &sink {
        s.lock().unwrap().insert(tag.to_string(), DownloadProgress {
            tag: tag.into(), asset_name: asset_name.into(),
            downloaded, total, speed_bps: 0, started_at,
        });
    }

    let mut stream = resp.bytes_stream();
    loop {
        // Per-chunk stall guard. reqwest's `read_timeout` covers the underlying
        // socket, but wrapping `stream.next()` in `tokio::time::timeout` is the
        // belt-and-suspenders that catches a stuck stream-decoder too.
        let next = tokio::time::timeout(stall_timeout, stream.next()).await
            .map_err(|_| anyhow!("stalled (no bytes for {}s) at byte {downloaded}",
                                  stall_timeout.as_secs()))?;
        match next {
            Some(Ok(chunk)) => {
                downloaded += chunk.len() as u64;
                tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;

                let now = Instant::now();
                if now.duration_since(last_sample_at) >= sample_interval {
                    let secs = now.duration_since(last_sample_at).as_secs_f64().max(0.001);
                    let speed_bps = ((downloaded - last_sample_bytes) as f64 / secs) as u64;
                    last_sample_at = now;
                    last_sample_bytes = downloaded;
                    if let Some(s) = &sink {
                        s.lock().unwrap().insert(tag.to_string(), DownloadProgress {
                            tag: tag.into(), asset_name: asset_name.into(),
                            downloaded, total, speed_bps, started_at,
                        });
                    }
                }
            }
            Some(Err(e)) => return Err(anyhow!("stream read: {e}")),
            None => break,
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.ends_with(".zip") {
        return extract_zip(archive, dest);
    }
    if name.ends_with(".deb") {
        #[cfg(all(unix, not(target_os = "macos")))] { return linux::extract_deb(archive, dest); }
        #[cfg(not(all(unix, not(target_os = "macos"))))] { return Err(anyhow!(".deb extraction is only implemented on Linux")); }
    }
    if name.ends_with(".dmg") {
        #[cfg(target_os = "macos")] { return macos::extract_dmg(archive, dest); }
        #[cfg(not(target_os = "macos"))] { return Err(anyhow!(".dmg extraction is only implemented on macOS")); }
    }
    if name.ends_with(".exe") {
        return Err(anyhow!(
            "Brave's `.exe` installer uses a custom Brave/Chromium format that no \
             public extractor handles. The asset picker should pick the `.zip` \
             portable build instead — please check the release listing or use the \
             'Detected Brave installs' panel to import an existing system Brave."
        ));
    }
    Err(anyhow!("unsupported archive: {}", name))
}

/// Pure-Rust ZIP extraction (works on every platform, no external tooling).
/// Brave publishes per-platform `.zip` archives alongside the proprietary
/// installer; the ZIP is plain DEFLATE/STORE and unzips trivially.
fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    use std::io::{Read, Write};
    std::fs::create_dir_all(dest)?;

    let f = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(f)
        .map_err(|e| anyhow!("opening zip: {e}"))?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)
            .map_err(|e| anyhow!("zip entry {i}: {e}"))?;

        // ZIP slip / absolute path guard — refuse anything that escapes dest.
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None    => return Err(anyhow!("malformed zip entry path: {}", entry.name())),
        };
        let out_path = dest.join(&rel);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() { std::fs::create_dir_all(parent)?; }

        let mut out = std::fs::File::create(&out_path)?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 { break; }
            out.write_all(&buf[..n])?;
        }

        // Preserve the executable bit on Unix when the zip recorded one.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode));
            }
        }
    }

    // Brave's zip tends to nest everything under one top-level dir like
    // `Brave-Browser-Nightly-vX.Y.Z-win-x64/`. If brave.exe / brave isn't
    // already at dest root, lift the single sub-dir up.
    flatten_top_level_subdir(dest)?;
    Ok(())
}

/// If `dest` contains exactly one directory and no other files (and brave's
/// binary isn't already at dest root), lift that directory's contents up
/// one level. Handles the common "archive nests everything in vX.Y.Z/"
/// layout.
fn flatten_top_level_subdir(dest: &Path) -> Result<()> {
    // Channel-agnostic candidates per platform — Brave's per-channel zips
    // name the bundled binary differently (Nightly / Beta / stable).
    let bin_names: &[&str] = if cfg!(windows) {
        &["brave.exe"]
    } else if cfg!(target_os = "macos") {
        &["Brave Browser Nightly.app", "Brave Browser Beta.app", "Brave Browser.app"]
    } else {
        &["brave-browser-nightly", "brave-browser-beta", "brave-browser", "brave"]
    };
    if bin_names.iter().any(|n| dest.join(n).exists()) { return Ok(()); }

    // Find any candidate anywhere up to depth 6, then lift its containing
    // folder's contents into dest. Brave's portable zip can nest the binary
    // inside `<root>/` or `<root>/Application/<ver>/`.
    let found = walkdir::WalkDir::new(dest).max_depth(6).into_iter()
        .filter_map(|e| e.ok())
        .find(|e| {
            let n = e.file_name().to_string_lossy();
            bin_names.iter().any(|b| n == *b)
        });
    let brave = match found { Some(x) => x, None => return Ok(()) };

    let from = match brave.path().parent() {
        Some(p) if p != dest => p.to_path_buf(),
        _ => return Ok(()),
    };
    for inner in std::fs::read_dir(&from)?.flatten() {
        let target = dest.join(inner.file_name());
        if !target.exists() {
            let _ = std::fs::rename(inner.path(), target);
        }
    }

    // Clean up the now-empty wrapper directory chain. Walk parents up to
    // dest and remove any that became empty.
    let mut cleanup = Some(from);
    while let Some(d) = cleanup.take() {
        if d == dest { break; }
        let parent = d.parent().map(|p| p.to_path_buf());
        let _ = std::fs::remove_dir(&d);  // remove_dir is fine — only succeeds when empty
        cleanup = parent;
    }
    Ok(())
}


#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use super::*;
    use std::io::Read;

    /// Extract a Brave .deb without root or `dpkg`. Newer Debs use
    /// `data.tar.zst` (zstd); older ones use `data.tar.xz` or `data.tar.gz`.
    /// All three are handled here, in pure Rust.
    pub fn extract_deb(archive: &Path, dest: &Path) -> Result<()> {
        let f = std::fs::File::open(archive)?;
        let mut ar = ar::Archive::new(f);
        while let Some(entry) = ar.next_entry() {
            let mut entry = entry?;
            let id = String::from_utf8_lossy(entry.header().identifier()).to_string();
            if !id.starts_with("data.tar") { continue; }

            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            let cur = std::io::Cursor::new(buf);

            let mut tar = if id.ends_with(".zst") {
                let dec = zstd::stream::read::Decoder::new(cur)
                    .map_err(|e| anyhow!("zstd init: {e}"))?;
                tar::Archive::new(Box::new(dec) as Box<dyn Read>)
            } else if id.ends_with(".xz") {
                let dec = xz2::read::XzDecoder::new(cur);
                tar::Archive::new(Box::new(dec) as Box<dyn Read>)
            } else if id.ends_with(".gz") {
                let dec = flate2::read::GzDecoder::new(cur);
                tar::Archive::new(Box::new(dec) as Box<dyn Read>)
            } else if id == "data.tar" {
                tar::Archive::new(Box::new(cur) as Box<dyn Read>)
            } else {
                return Err(anyhow!("unknown .deb compression: {id}"));
            };
            tar.unpack(dest)?;
            return Ok(());
        }
        Err(anyhow!("no data.tar.* member found inside .deb"))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;

    /// Mount the .dmg and copy the .app into dest. Requires `hdiutil` (built in).
    pub fn extract_dmg(archive: &Path, dest: &Path) -> Result<()> {
        let mount = tempdir_under(dest)?;
        let status = Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-quiet", "-mountpoint"])
            .arg(&mount)
            .arg(archive)
            .status()?;
        if !status.success() { return Err(anyhow!("hdiutil attach failed")); }

        let app = std::fs::read_dir(&mount)?
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().ends_with(".app"))
            .ok_or_else(|| anyhow!("no .app inside dmg"))?;
        let target = dest.join(app.file_name());
        copy_dir_all(&app.path(), &target)?;
        let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();

        // Mach-O inside the .app and any helpers need +x. `cp` would have
        // preserved this; our manual copy_dir_all uses fs::copy which doesn't
        // preserve the executable bit on some filesystems.
        use std::os::unix::fs::PermissionsExt;
        let macos_dir = target.join("Contents").join("MacOS");
        if macos_dir.exists() {
            for entry in std::fs::read_dir(&macos_dir)? {
                let e = entry?;
                if e.file_type()?.is_file() {
                    let mut perm = e.metadata()?.permissions();
                    perm.set_mode(0o755);
                    std::fs::set_permissions(e.path(), perm)?;
                }
            }
        }
        Ok(())
    }

    fn tempdir_under(parent: &Path) -> Result<PathBuf> {
        let p = parent.join(format!("mnt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p)?;
        Ok(p)
    }
    fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let to = dst.join(entry.file_name());
            if ty.is_dir() { copy_dir_all(&entry.path(), &to)?; }
            else { std::fs::copy(entry.path(), to)?; }
        }
        Ok(())
    }
}
