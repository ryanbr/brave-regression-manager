use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

const OWNER: &str = "brave";
const REPO:  &str = "brave-browser";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub published_at: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
    /// Filename of the asset selected for the current platform, if any.
    /// `None` means this release has no installer for the host (e.g.
    /// mobile-only releases that ship `.apk` / `.aab` only).
    #[serde(default)]
    pub host_asset: Option<String>,
}

/// Brave release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel { Release, Beta, Nightly }

/// Which channels the user wants to see in the available-releases list.
#[derive(Debug, Clone, Copy)]
pub struct ChannelFilter {
    pub release: bool,
    pub beta:    bool,
    pub nightly: bool,
}

impl Default for ChannelFilter {
    fn default() -> Self { Self { release: false, beta: false, nightly: true } }
}

impl ChannelFilter {
    pub fn allows(self, c: Channel) -> bool {
        match c {
            Channel::Release => self.release,
            Channel::Beta    => self.beta,
            Channel::Nightly => self.nightly,
        }
    }
    /// Guard against an all-off filter — the GUI clamps to Nightly when the
    /// user unchecks every box, but be defensive here too.
    pub fn nonempty(self) -> Self {
        if !self.release && !self.beta && !self.nightly {
            Self { release: false, beta: false, nightly: true }
        } else { self }
    }
}

/// Decide a release's channel. Brave's release titles always start with
/// the channel name ("Release v1.X.Y …", "Beta v1.X.Y …", "Nightly
/// v1.X.Y …") so the title is the most authoritative signal — way more
/// reliable than asset filenames, which can include cross-channel
/// debug/symbol/checksum files that produced false-positive Nightly
/// classifications for stable Release tags.
///
/// Fallback chain when the title is empty or doesn't match the
/// expected pattern: scan asset filenames, then the `prerelease` flag.
pub fn detect_release_channel(release: &Release) -> Channel {
    // 1. Title prefix — by far the most reliable. Brave's release UI
    //    enforces the "Release "/"Beta "/"Nightly " prefix.
    let title = release.name.trim_start().to_lowercase();
    if title.starts_with("nightly ") || title.starts_with("nightly v") {
        return Channel::Nightly;
    }
    if title.starts_with("beta ") || title.starts_with("beta v") {
        return Channel::Beta;
    }
    if title.starts_with("release ") || title.starts_with("release v") {
        return Channel::Release;
    }
    // 2. Asset name scan — only relevant when the title was unhelpful.
    //    Match channel markers as whole-ish tokens (separator-bounded)
    //    to avoid false positives like a checksum file named
    //    `…Nightly-symbols.txt` shipped alongside a stable build.
    let asset_marker = |needle: &str| -> bool {
        release.assets.iter().any(|a| {
            let l = a.name.to_lowercase();
            // Look for the marker preceded by '-', '_', '.', '/', or
            // start-of-string AND followed by a separator. Catches
            // brave-browser-nightly_… but not random substrings.
            let mut found = false;
            for (i, _) in l.match_indices(needle) {
                let prev_ok = i == 0
                    || matches!(l.as_bytes()[i - 1], b'-' | b'_' | b'.' | b'/');
                let after = i + needle.len();
                let next_ok = after == l.len()
                    || matches!(l.as_bytes()[after], b'-' | b'_' | b'.' | b'/');
                if prev_ok && next_ok { found = true; break; }
            }
            found
        })
    };
    if asset_marker("nightly") { return Channel::Nightly; }
    if asset_marker("beta")    { return Channel::Beta; }
    // 3. Last-resort: GitHub's prerelease flag. Nightly + Beta are both
    //    flagged prerelease=true; we can't tell them apart from this
    //    signal alone, but defaulting to Nightly here matches what the
    //    fetcher used to do.
    if release.prerelease { Channel::Nightly } else { Channel::Release }
}

impl Release {
    pub fn has_host_installer(&self) -> bool { self.host_asset.is_some() }

    /// Short reason why a release is not installable. Empty when it is.
    pub fn skip_reason(&self) -> String {
        if self.host_asset.is_some() { return String::new(); }
        if self.assets.is_empty() { return "no assets yet".into(); }
        let exts: std::collections::BTreeSet<&str> = self.assets.iter()
            .filter_map(|a| a.name.rsplit_once('.').map(|(_, e)| e))
            .collect();
        // Mobile-only?
        let only_mobile = exts.iter().all(|e| matches!(*e, "apk" | "aab" | "asc" | "sha256"))
            && exts.iter().any(|e| matches!(*e, "apk" | "aab"));
        if only_mobile {
            return format!("mobile-only ({} mobile assets)",
                self.assets.iter().filter(|a| a.name.ends_with(".apk") || a.name.ends_with(".aab")).count());
        }
        let has_desktop = self.assets.iter().any(|a|
            a.name.ends_with(".deb") || a.name.ends_with(".exe") || a.name.ends_with(".dmg")
            || a.name.ends_with(".zip"));
        if has_desktop {
            return format!("no host installer (channel: {:?})", detect_release_channel(self));
        }
        format!("no host installer (extensions: {})",
            exts.into_iter().collect::<Vec<_>>().join(", "))
    }
}

/// CLI shim — keeps the old function signature for callers that don't
/// care about channel filtering (default: Nightly only, matching legacy
/// behaviour).
pub async fn list_nightly_releases(count: u32) -> Result<Vec<Release>> {
    list_releases_streaming(count, None, None, ChannelFilter::default(), None, None, |_| {}).await
}

/// Streaming variant: invokes `on_progress` after every paginated page
/// with the cumulative-so-far list of `Release`s, so callers (the GUI)
/// can render a partial list while later pages are still in flight.
///
/// `stop_at` is an optional lower-bound date (UTC). GitHub returns
/// releases newest-first; once we encounter a release published BEFORE
/// `stop_at`, we know we've covered everything from that date onward and
/// stop fetching further pages — saves API calls and time when the user
/// only needs releases since e.g. January 2025.
pub async fn list_nightly_releases_streaming(
    count: u32,
    token: Option<&str>,
    stop_at: Option<chrono::NaiveDate>,
    filter: ChannelFilter,
    console: Option<&crate::console::Handle>,
    on_progress: impl FnMut(Vec<Release>) + Send,
) -> Result<Vec<Release>> {
    list_releases_streaming(count, token, stop_at, filter, None, console, on_progress).await
}

/// Same as the public entry point but also accepts `known_tags` — when
/// provided, pagination breaks out as soon as we encounter a tag that's
/// already in the set. Used by the incremental cache mode so a refetch
/// only walks new pages instead of re-traversing the entire history.
pub async fn list_nightly_releases_streaming_incremental(
    count: u32,
    token: Option<&str>,
    stop_at: Option<chrono::NaiveDate>,
    filter: ChannelFilter,
    known_tags: &std::collections::HashSet<String>,
    console: Option<&crate::console::Handle>,
    on_progress: impl FnMut(Vec<Release>) + Send,
) -> Result<Vec<Release>> {
    list_releases_streaming(count, token, stop_at, filter, Some(known_tags),
                            console, on_progress).await
}

/// Pick the GitHub token to send, and make sure it can legally go in a
/// header. Returns the token plus a label naming where it came from.
///
/// octocrab does not validate: `build()` assembles the header as
/// `format!("Bearer {}", token).parse().unwrap()` (octocrab 0.39
/// lib.rs:743), so a byte the header grammar rejects **panics** rather
/// than erroring — and the release profile sets `panic = "abort"`, so
/// that takes the whole GUI down with no message at all. A token read
/// from a file by a launcher script keeps its trailing newline and does
/// exactly this. Trim the whitespace that causes it and reject anything
/// still outside the grammar with something the user can act on.
///
/// Empty (after trimming) means absent: sending a bare
/// `Authorization: Bearer` earns a 401 indistinguishable from a revoked
/// token. All four GitHub call paths route through here so they cannot
/// drift apart again.
fn header_token(settings: Option<&str>) -> Result<Option<(String, &'static str)>> {
    let picked = settings
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .map(|t| (t, "Settings token"))
        .or_else(|| std::env::var("GITHUB_TOKEN").ok()
            .filter(|s| !s.trim().is_empty())
            .map(|t| (t, "GITHUB_TOKEN env")));
    let Some((raw, source)) = picked else { return Ok(None) };
    let t = raw.trim();
    // Visible ASCII only — the subset every GitHub PAT format uses, and
    // a strict subset of what a header value permits.
    if let Some(bad) = t.bytes().find(|b| !(0x21..=0x7e).contains(b)) {
        return Err(anyhow!(
            "the GitHub token from {source} contains a character that \
             cannot be sent in an HTTP header (byte 0x{bad:02x}). Re-copy \
             the token — an embedded space or line break is the usual \
             cause."));
    }
    Ok(Some((t.to_string(), source)))
}

async fn list_releases_streaming(
    count: u32,
    token: Option<&str>,
    stop_at: Option<chrono::NaiveDate>,
    filter: ChannelFilter,
    known_tags: Option<&std::collections::HashSet<String>>,
    console: Option<&crate::console::Handle>,
    mut on_progress: impl FnMut(Vec<Release>) + Send,
) -> Result<Vec<Release>> {
    // Diagnostics are opt-in: the GUI fetch passes a handle, the
    // internal one-shot helpers pass None. Until this existed the whole
    // fetch was silent between "Fetching…" and its result, so a failure
    // gave the user nothing to act on. NEVER log the token itself — the
    // masked `present (N chars)` form is the only representation used
    // anywhere in this codebase.
    let log = |msg: String| {
        if let Some(h) = console { crate::console::info(h, "github", msg); }
    };
    let started = std::time::Instant::now();

    let filter = filter.nonempty();
    let mut builder = octocrab::OctocrabBuilder::new();
    // Resolved before the start line below so that line can name the
    // token source even when the token itself turns out to be unusable.
    // On rejection this returns before the "fetch start" line below,
    // which the 401 hint promises will name the token source. The error
    // itself names it, so say so rather than emitting nothing at all.
    let chosen_token = match header_token(token) {
        Ok(t) => t,
        Err(e) => {
            log("fetch start: aborted — the configured token cannot be \
                 sent in a header (see the error below)".to_string());
            return Err(e);
        }
    };
    let auth_label = match &chosen_token {
        Some((t, source)) => format!("{source}, present ({} chars)", t.len()),
        None              => "anonymous (60 req/hr cap)".to_string(),
    };
    let target = count.max(1) as usize;
    // 200 * 100 = 20 000 raw releases — easily covers Brave's whole
    // GitHub release history (≈ 7+ years). Either the target count,
    // the stop_at date, the known-tag short-circuit, or this hard
    // ceiling terminates the loop.
    let max_pages: u32 = 200;
    let mut out: Vec<Release> = Vec::new();
    let mut crossed_stop = false;
    let mut crossed_known = false;

    // When stop_at is set, the user has implicitly asked for "everything
    // back to this date" — so the count cap is moot. Use a much larger
    // effective target so we don't stop short before crossing stop_at.
    let effective_target = if stop_at.is_some() { 20_000 } else { target };

    let mut channels = Vec::new();
    if filter.release { channels.push("Release"); }
    if filter.beta    { channels.push("Beta"); }
    if filter.nightly { channels.push("Nightly"); }
    // Emitted BEFORE the client is built. personal_token() on a PAT
    // pasted with a trailing newline or a stray non-ASCII character
    // fails in build() with InvalidHeaderValue, and logging afterwards
    // meant the one failure that is unambiguously token-shaped produced
    // no auth line at all — while the 401 hint tells the user this line
    // names which token was sent.
    log(format!(
        "fetch start: auth={auth_label}, target={effective_target}, \
         channels=[{}], stop_at={}, incremental={}",
        channels.join(", "),
        stop_at.map(|d| d.to_string()).unwrap_or_else(|| "none".into()),
        known_tags.map(|k| format!("yes ({} known tags)", k.len()))
            .unwrap_or_else(|| "no (full walk)".into())));

    if let Some((t, _)) = chosen_token {
        builder = builder.personal_token(t);
    }
    let octo = builder.build()
        .map_err(|e| anyhow::Error::new(e).context("building the GitHub client"))?;

    // Named so the "why did it stop" line at the end can't drift out of
    // sync with the breaks themselves — including the ceiling, which is
    // derived from max_pages rather than restated.
    let mut stop_reason = format!("page ceiling ({max_pages})");

    for page_num in 1..=max_pages {
        let page = match octo.repos(OWNER, REPO).releases().list()
            .per_page(100).page(page_num).send().await
        {
            Ok(p) => p,
            Err(e) => {
                // Name the page. Failing on page 1 is auth or network;
                // failing on page 7 is usually the rate limit landing
                // mid-walk, and the two want different responses. The
                // anyhow wrapper is what makes the message readable at
                // all: octocrab's Error Displays as the bare word
                // "GitHub" and keeps the API's text in its source.
                //
                // Deliberately not logged here — the caller's result
                // drain prints it with the matching hint appended, and
                // the context below survives into that message. Logging
                // both would show one failure as two.
                return Err(anyhow::Error::new(e)
                    .context(format!("GitHub releases page {page_num}")));
            }
        };
        let page_items = page.items.len();
        let before = out.len();
        if page.items.is_empty() {
            log(format!("page {page_num}: empty — no more releases"));
            stop_reason = "release list exhausted".to_string();
            break;
        }

        for r in page.items {
            // Incremental short-circuit: as soon as we see a tag we
            // already have in the persistent release_cache, everything
            // older is also known — break out without recording further.
            // Releases land monotonically (newest first) so a single hit
            // is enough; no gap detection needed because the cache is
            // append-only.
            if let Some(known) = known_tags {
                if known.contains(&r.tag_name) {
                    log(format!("page {page_num}: hit cached tag {} — \
                                 nothing older to fetch", r.tag_name));
                    crossed_known = true;
                    break;
                }
            }

            let published = r.published_at.map(|d| d.to_rfc3339()).unwrap_or_default();
            if let Some(stop) = stop_at {
                if let Some(date_str) = published.split('T').next() {
                    if let Ok(d) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                        if d < stop { crossed_stop = true; }
                    }
                }
            }

            let assets: Vec<ReleaseAsset> = r.assets.into_iter().map(|a| ReleaseAsset {
                name: a.name,
                size: a.size as u64,
                browser_download_url: a.browser_download_url.to_string(),
            }).collect();
            let mut candidate = Release {
                tag: r.tag_name.clone(),
                name: r.name.clone().unwrap_or_default(),
                published_at: published,
                prerelease: r.prerelease,
                assets,
                host_asset: None,
            };
            let channel = detect_release_channel(&candidate);
            if !filter.allows(channel) { continue; }
            candidate.host_asset = pick_asset_for(&candidate, channel).ok().map(|a| a.name.clone());
            out.push(candidate);
            if out.len() >= effective_target { break; }
        }
        on_progress(out.clone());
        // "added", not "kept after the channel filter": the delta also
        // shrinks when the incremental short-circuit or the target cap
        // breaks out mid-page, and the GUI always passes an
        // all-channels filter — so attributing it to the filter would
        // name the one cause that is usually not responsible. Each of
        // those exits logs its own reason.
        // Every page for the first ten, then every tenth. The ceiling is
        // 200 pages and the console ring holds 1000 entries, so logging
        // unconditionally could spend a fifth of the ring on one deep
        // stop_at walk and evict the install / launch / Brave-stderr
        // history it exists to sit alongside. Early pages are where the
        // diagnostic value is; a long tail of them is just noise.
        if page_num <= 10 || page_num % 10 == 0 {
            log(format!("page {page_num}: {page_items} releases, {} added \
                         (running total {})",
                out.len() - before, out.len()));
        }
        // Stop when we've reached our (uncapped if stop_at) target,
        // crossed the stop_at floor, or hit a known tag (incremental).
        if out.len() >= effective_target { stop_reason = "reached target".to_string(); break; }
        if crossed_stop  { stop_reason = "crossed the stop_at date".to_string(); break; }
        if crossed_known { stop_reason = "reached the cached tags".to_string(); break; }
    }
    // "newly fetched", because on the incremental path the caller then
    // logs the merged cache total under this same source — a warm
    // re-fetch would otherwise read "fetch done: 0 releases" directly
    // above "fetched 812 tags", which is the contradiction this change
    // deleted the old pre-fetch line to avoid.
    log(format!("fetch done: {} newly fetched in {:.1}s ({stop_reason})",
        out.len(), started.elapsed().as_secs_f64()));
    Ok(out)
}

pub async fn get_release(tag: &str) -> Result<Release> {
    // Walk a generous window so we can resolve older tags too. Use an
    // all-channels filter so install-by-tag works regardless of the GUI's
    // current display preference.
    let any = ChannelFilter { release: true, beta: true, nightly: true };
    list_releases_streaming(500, None, None, any, None, None, |_| {}).await?
        .into_iter()
        .find(|r| r.tag == tag)
        .ok_or_else(|| anyhow!("release tag not found: {tag}"))
}

/// Pick the best installer asset for the current platform, auto-detecting
/// the release's channel.
pub fn pick_asset(release: &Release) -> Result<&ReleaseAsset> {
    let channel = detect_release_channel(release);
    pick_asset_for(release, channel)
}

/// Pick the asset for an *install key*: the suffixed form selects the
/// emulated build, the bare form the native one.
///
/// The fallback install path resolves a release by tag and then picks,
/// and picking natively for a `+x86` key put the native build into the
/// emulated build's directory — two directories, the same binary, two
/// independent verdicts, one of them a lie.
pub fn pick_asset_for_install_key<'a>(release: &'a Release, key: &str)
    -> Option<&'a ReleaseAsset>
{
    let channel = detect_release_channel(release);
    if key.ends_with("+x86") {
        pick_x86_asset(&release.assets, channel)
    } else {
        pick_for_host(&release.assets, channel)
    }
}

/// The subset of a release's assets worth caching for a later re-pick.
///
/// A Brave release lists ~135 files — every platform's installers, plus a
/// `.sha256` and `.asc` beside most of them — and the row is serialised
/// into both releases.json and every sqlite release_cache row, on the
/// startup parse path. Only what a picker on THIS host could ever return
/// is useful, so the filter is both by kind and by platform: a Windows
/// machine has no use for a `.dmg`, and can never be asked for one.
///
/// Measured against a real release (v1.96.29): 135 assets, 23 after the
/// kind filter alone, 8 after this one — 4196 to 1472 bytes per row, so
/// roughly 16 MB down to 5.6 MB across a 4000-tag cache, and the same
/// saving again on the JSON parsed at startup.
///
/// The rule is derived from what `pick_for_host` and `pick_x86_for_host`
/// actually match on each platform, so nothing a picker could select is
/// dropped. Both architectures are kept — the arch re-pick needs the
/// other one — and a cache carried to a different OS simply reports
/// `needs_asset_backfill` and refetches.
pub fn installer_assets(assets: &[ReleaseAsset]) -> Vec<ReleaseAsset> {
    assets.iter().filter(|a| {
        let l = a.name.to_lowercase();
        if l.contains("symbol") || l.contains("pdb") || l.contains("debug") {
            return false;
        }
        // Windows: every `.exe` (the Standalone / Silent matchers key on
        // the extension alone — those names carry no "win" marker), plus
        // zips carrying a Windows marker.
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
    }).cloned().collect()
}

/// Re-run the host asset pick against an already-known asset list.
///
/// The picked asset is host-architecture-specific, but a cached
/// `ReleaseRow` records only the *result*. Exposing the pick lets a row
/// cached by a build running on another architecture be corrected
/// offline — no API call — instead of showing an x64 zip forever on an
/// ARM host because the incremental fetch short-circuits before it ever
/// re-picks.
pub fn pick_host_asset(assets: &[ReleaseAsset], channel: Channel)
    -> Option<&ReleaseAsset>
{
    pick_for_host(assets, channel)
}

/// The x86-64 asset for a host that can emulate one, or None.
///
/// Separate from `pick_host_asset` rather than folded into it as a
/// fallback: the two are shown as two rows, installed side by side, and
/// judged independently, so the caller decides which it wants instead of
/// the picker silently substituting one for the other.
pub fn pick_x86_asset(assets: &[ReleaseAsset], channel: Channel)
    -> Option<&ReleaseAsset>
{
    if !host_can_run_x86_under_emulation() { return None; }
    pick_x86_for_host(assets, channel)
}

/// True when this host runs an ARM build but can execute x86-64 too:
/// Windows 11 on ARM emulates it, macOS has Rosetta 2. ARM Linux has
/// neither, so it never offers the row.
pub fn host_can_run_x86_under_emulation() -> bool {
    std::env::consts::ARCH == "aarch64" && cfg!(any(windows, target_os = "macos"))
}

/// Identifies the inputs a cached `host_asset` was chosen under — just
/// the host architecture, now that the x86 row is derived rather than
/// substituted. A row whose stored key differs is re-picked from its
/// stored asset list.
pub fn current_pick_key() -> String {
    std::env::consts::ARCH.to_string()
}

/// Parse the channel label persisted in a cached row.
pub fn channel_from_label(s: &str) -> Channel {
    match s {
        "Beta"    => Channel::Beta,
        "Release" => Channel::Release,
        // "Nightly", "?" and anything unrecognised: Nightly is the
        // permissive case for `name_compatible`, so a row whose channel
        // never got classified still gets a usable pick.
        _         => Channel::Nightly,
    }
}

fn pick_asset_for(release: &Release, channel: Channel) -> Result<&ReleaseAsset> {
    let names: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
    if let Some(a) = pick_for_host(&release.assets, channel) { return Ok(a); }
    Err(anyhow!(
        "no suitable asset for this platform among {} assets; available: {:?}",
        release.assets.len(), names
    ))
}

/// True when filename contains the channel's marker, OR (for marker-free
/// filenames like Brave's portable `.zip`s) carries no other channel's
/// marker. This is safe because the caller already filtered the release
/// at the channel level — a marker-free zip in a Nightly release IS a
/// Nightly artifact.
fn name_compatible(n: &str, channel: Channel) -> bool {
    let l = n.to_lowercase();
    if l.contains("origin") || l.contains("core") { return false; }
    let has_nightly = l.contains("nightly");
    let has_beta    = l.contains("beta");
    let has_dev     = l.contains("dev");
    match channel {
        Channel::Nightly => has_nightly || (!has_beta && !has_dev),
        Channel::Beta    => has_beta    || (!has_nightly && !has_dev),
        Channel::Release => !has_nightly && !has_beta && !has_dev,
    }
}

#[cfg(windows)]
fn pick_for_host(assets: &[ReleaseAsset], channel: Channel) -> Option<&ReleaseAsset> {
    let host_arch = std::env::consts::ARCH;
    let want_arm = host_arch == "aarch64";

    let exe_ok = |n: &str| -> bool {
        n.ends_with(".exe") && name_compatible(n, channel)
    };
    let zip_clean = |n: &str| -> bool {
        let l = n.to_lowercase();
        l.ends_with(".zip")
            && !l.contains("pdb") && !l.contains("symbol") && !l.contains("debug")
            && name_compatible(n, channel)
    };
    // Windows-only — explicitly require a Windows OS marker AND reject
    // macOS/Linux markers, otherwise `brave-v…-darwin-x64.zip` would
    // false-match on a bare "x64" substring.
    let is_windows_zip = |n: &str| -> bool {
        let l = n.to_lowercase();
        (l.contains("win32") || l.contains("win64") || l.contains("win-")
         || l.contains("windows-") || l.contains("-win"))
            && !l.contains("darwin") && !l.contains("linux")
            && !l.contains("mac") && !l.contains("osx")
    };

    let zip_x64 = |n: &str| -> bool {
        zip_clean(n) && is_windows_zip(n)
            && (n.contains("x64") || n.contains("amd64"))
            && !n.to_lowercase().contains("arm")
    };
    let zip_arm = |n: &str| -> bool {
        zip_clean(n) && is_windows_zip(n)
            && (n.contains("arm64") || n.contains("aarch64"))
    };
    // `zip_any` is the fallback for windows zips with no architecture
    // marker. Critically, it must EXCLUDE the opposite architecture —
    // otherwise on x64 Windows we'd happily pick `*-win-arm64.zip` and
    // the user gets `ERROR_EXE_MACHINE_TYPE_MISMATCH` (os error 216)
    // when launching, since an ARM PE can't run on x64 Windows.
    let zip_any = |n: &str| -> bool {
        let l = n.to_lowercase();
        zip_clean(n) && is_windows_zip(n)
            && if want_arm {
                !(l.contains("x64") || l.contains("amd64"))
            } else {
                !(l.contains("arm64") || l.contains("aarch64") || l.contains("-arm"))
            }
    };

    let silent_standalone_x64 = |n: &str| -> bool {
        exe_ok(n) && n.contains("Standalone") && n.contains("Silent")
            && !n.to_lowercase().contains("arm")
    };
    let silent_standalone_arm = |n: &str| -> bool {
        exe_ok(n) && n.contains("Standalone") && n.contains("Silent")
            && n.to_lowercase().contains("arm")
    };
    let standalone_x64 = |n: &str| -> bool {
        exe_ok(n) && n.contains("Standalone")
            && !n.to_lowercase().contains("arm")
    };
    let standalone_arm = |n: &str| -> bool {
        exe_ok(n) && n.contains("Standalone")
            && n.to_lowercase().contains("arm")
    };

    // Cross-arch fallback rules: Windows 11 on ARM can emulate x64, but
    // x64 Windows has no ARM emulator — running an ARM PE on x64 fails
    // with ERROR_EXE_MACHINE_TYPE_MISMATCH (216). So the order must
    // never end in zip_arm/standalone_arm for an x64 host. If Brave
    // didn't ship an x64 build for that tag (some recent nightlies
    // shipped arm-only), return None — surfaces as "no installer" in
    // the GUI rather than silently installing an unrunnable binary.
    // Native only, both ways. On an ARM host the x86-64 build is no
    // longer a silent fallback here — it is offered as its own `[x86]`
    // row via `pick_x86_for_host`, so a release with no ARM asset
    // correctly reports "no installer" on this side.
    let order: Vec<&dyn Fn(&str) -> bool> = if want_arm {
        vec![&zip_arm, &zip_any, &silent_standalone_arm, &standalone_arm]
    } else {
        // x64 host: x64 only. No cross-arch fallback.
        vec![&zip_x64, &zip_any, &silent_standalone_x64, &standalone_x64]
    };
    for matcher in order {
        if let Some(a) = assets.iter().find(|a| matcher(&a.name)) { return Some(a); }
    }
    None
}

/// The x86-64 Windows asset, for the emulated `[x86]` row on an ARM
/// host. Deliberately never returns an ARM or arch-less asset — this
/// row exists to be unambiguously the x86-64 build.
#[cfg(windows)]
fn pick_x86_for_host(assets: &[ReleaseAsset], channel: Channel)
    -> Option<&ReleaseAsset>
{
    let exe_ok = |n: &str| -> bool {
        n.ends_with(".exe") && name_compatible(n, channel)
    };
    let is_windows_zip = |n: &str| -> bool {
        let l = n.to_lowercase();
        (l.contains("win32") || l.contains("win64") || l.contains("win-")
         || l.contains("windows-") || l.contains("-win"))
            && !l.contains("darwin") && !l.contains("linux")
            && !l.contains("mac") && !l.contains("osx")
    };
    let zip_x64 = |n: &str| -> bool {
        let l = n.to_lowercase();
        l.ends_with(".zip") && name_compatible(n, channel) && is_windows_zip(n)
            && !l.contains("pdb") && !l.contains("symbol") && !l.contains("debug")
            && (l.contains("x64") || l.contains("amd64"))
            && !l.contains("arm")
    };
    let silent_standalone_x64 = |n: &str| -> bool {
        exe_ok(n) && n.contains("Standalone") && n.contains("Silent")
            && !n.to_lowercase().contains("arm")
    };
    let standalone_x64 = |n: &str| -> bool {
        exe_ok(n) && n.contains("Standalone")
            && !n.to_lowercase().contains("arm")
    };
    let order: [&dyn Fn(&str) -> bool; 3] =
        [&zip_x64, &silent_standalone_x64, &standalone_x64];
    for matcher in order {
        if let Some(a) = assets.iter().find(|a| matcher(&a.name)) { return Some(a); }
    }
    None
}

/// No x86 emulation on ARM Linux, and an x64 host has no ARM row to
/// pair with — `host_can_run_x86_under_emulation` gates the call, this
/// keeps the symbol resolvable per-platform.
#[cfg(all(unix, not(target_os = "macos")))]
fn pick_x86_for_host(_assets: &[ReleaseAsset], _channel: Channel)
    -> Option<&ReleaseAsset> { None }

/// Module-level so both macOS pickers can use it. It was a closure
/// inside `pick_for_host`, and `pick_x86_for_host` referencing it from
/// another fn is a hard compile error on macOS — invisible from Linux,
/// which instantiates neither, which is how it reached master.
#[cfg(target_os = "macos")]
fn is_macos_zip(n: &str) -> bool {
    let l = n.to_lowercase();
    (l.contains("darwin") || l.contains("macos") || l.contains("osx")
     || l.contains("mac-"))
        && !l.contains("linux") && !l.contains("win32") && !l.contains("win64")
}

#[cfg(target_os = "macos")]
fn pick_for_host(assets: &[ReleaseAsset], channel: Channel) -> Option<&ReleaseAsset> {
    let host_arch = std::env::consts::ARCH;
    let want_arm = host_arch == "aarch64";

    // Reject `*-symbols.zip` and other debug-info bundles. Without this,
    // alphabetical asset order puts `…-arm64-symbols.zip` before the real
    // `…-arm64.zip` and the picker grabs the symbols archive.
    let zip_clean = |n: &str| -> bool {
        let l = n.to_lowercase();
        n.ends_with(".zip") && name_compatible(n, channel) && is_macos_zip(n)
            && !l.contains("symbol") && !l.contains("pdb") && !l.contains("debug")
    };
    let zip_arm = |n: &str| -> bool {
        zip_clean(n) && (n.contains("arm64") || n.contains("aarch64"))
    };
    let zip_x64 = |n: &str| -> bool {
        zip_clean(n) && !n.contains("arm") && !n.contains("aarch64")
    };
    // Excludes the opposite arch on an ARM host, matching the Windows
    // twin. Without it "Native only" still installed a Rosetta build
    // under the bare tag for an x64-only release — contradicting the
    // documented contract — and the native_is_same guard in
    // expand_arch_rows then suppressed the [x86] row meant to carry it.
    // Symmetric, and matching the Windows twin's marker set: an ARM
    // host must not take an x64 asset as its *native* pick (that is the
    // [x86] row's job, and letting it through here suppresses that row
    // via the native_is_same guard), and an x64 host must never take an
    // arm one, which cannot execute at all.
    let opposite_arch = |n: &str| -> bool {
        let l = n.to_lowercase();
        if want_arm {
            l.contains("x64") || l.contains("x86_64") || l.contains("amd64")
        } else {
            l.contains("arm64") || l.contains("aarch64")
        }
    };
    let zip_any = |n: &str| -> bool { zip_clean(n) && !opposite_arch(n) };
    let dmg_arm = |n: &str| -> bool {
        n.ends_with(".dmg") && name_compatible(n, channel)
            && (n.contains("arm64") || n.contains("aarch64"))
    };
    let dmg_x64 = |n: &str| -> bool {
        n.ends_with(".dmg") && name_compatible(n, channel)
            && (n.contains("x64") || n.contains("x86_64"))
    };
    let dmg_uni = |n: &str| -> bool {
        n.ends_with(".dmg") && name_compatible(n, channel)
            && n.to_lowercase().contains("universal")
    };
    let dmg_any = |n: &str| -> bool {
        n.ends_with(".dmg") && name_compatible(n, channel) && !opposite_arch(n)
    };

    // Native only — Rosetta 2 builds get their own `[x86]` row.
    let order: Vec<&dyn Fn(&str) -> bool> = if want_arm {
        vec![&zip_arm, &zip_any, &dmg_arm, &dmg_uni, &dmg_any]
    } else {
        // No cross-arch tail: an ARM Mach-O will not run on Intel, so a
        // release shipping only arm64 must report "no installer" rather
        // than install something that cannot start.
        vec![&zip_x64, &zip_any, &dmg_x64, &dmg_uni, &dmg_any]
    };
    for matcher in order {
        if let Some(a) = assets.iter().find(|a| matcher(&a.name)) { return Some(a); }
    }
    None
}

/// The x86-64 macOS asset, for the Rosetta 2 `[x86]` row.
#[cfg(target_os = "macos")]
fn pick_x86_for_host(assets: &[ReleaseAsset], channel: Channel)
    -> Option<&ReleaseAsset>
{
    let zip_x64 = |n: &str| -> bool {
        let l = n.to_lowercase();
        n.ends_with(".zip") && name_compatible(n, channel) && is_macos_zip(n)
            && !l.contains("symbol") && !l.contains("pdb") && !l.contains("debug")
            && !l.contains("arm") && !l.contains("aarch64")
    };
    let dmg_x64 = |n: &str| -> bool {
        let l = n.to_lowercase();
        n.ends_with(".dmg") && name_compatible(n, channel)
            && (l.contains("x64") || l.contains("x86_64") || l.contains("amd64"))
    };
    let order: [&dyn Fn(&str) -> bool; 2] = [&zip_x64, &dmg_x64];
    for matcher in order {
        if let Some(a) = assets.iter().find(|a| matcher(&a.name)) { return Some(a); }
    }
    None
}

/// One commit returned by GitHub's `compare` endpoint.
#[derive(Debug, Clone)]
pub struct CommitRow {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author:  String,
    pub date:    String,
    pub html_url: String,
}

#[derive(Debug, Clone)]
pub struct CompareResult {
    pub base:    String,
    pub head:    String,
    pub total:   u32,
    pub commits: Vec<CommitRow>,
    /// True when GitHub's `compare` capped at 250 commits but more exist.
    pub truncated: bool,
}

/// One-shot per-tag fetch of a full `Release` (name, published_at,
/// prerelease, all assets) via the REST `releases/tags/<tag>` endpoint —
/// **one** API call instead of paginating. Used by the GUI's "Add by
/// tag" affordance so a user can pull a specific older release without
/// walking back through every release in between.
pub async fn fetch_release_by_tag(tag: &str, token: Option<&str>) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/tags/{tag}");
    let mut req = reqwest::Client::builder()
        .user_agent("brave-regress")
        .build()?
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    if let Some((t, _)) = header_token(token)? {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("github release {tag}: HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await?;
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let published_at = body.get("published_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let prerelease = body.get("prerelease").and_then(|v| v.as_bool()).unwrap_or(false);
    let assets: Vec<ReleaseAsset> = body.get("assets")
        .and_then(|v| v.as_array()).cloned().unwrap_or_default()
        .into_iter().filter_map(|a| {
            let n = a.get("name")?.as_str()?.to_string();
            let s = a.get("size")?.as_u64()?;
            let u = a.get("browser_download_url")?.as_str()?.to_string();
            Some(ReleaseAsset { name: n, size: s, browser_download_url: u })
        }).collect();
    let mut candidate = Release {
        tag: tag.to_string(),
        name, published_at, prerelease, assets,
        host_asset: None,
    };
    let channel = detect_release_channel(&candidate);
    candidate.host_asset = pick_asset_for(&candidate, channel).ok().map(|a| a.name.clone());
    Ok(candidate)
}

/// One-shot per-tag fetch of `brave/brave-browser` release metadata so the
/// GUI can populate the pinned Chromium version + date for an installed
/// tag that isn't in the currently-loaded available window. Single API
/// call; far cheaper than expanding the global fetch window. Returns
/// `(name, published_at, prerelease)` — only the bits we care about.
pub async fn fetch_release_metadata(tag: &str, token: Option<&str>)
    -> Result<(String, String, bool)>
{
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/tags/{tag}");
    let mut req = reqwest::Client::builder()
        .user_agent("brave-regress")
        .build()?
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    if let Some((t, _)) = header_token(token)? {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("github release {tag}: HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await?;
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let published = body.get("published_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let prerelease = body.get("prerelease").and_then(|v| v.as_bool()).unwrap_or(false);
    Ok((name, published, prerelease))
}

/// Fetch the commit list between two refs in `brave/brave-core` via the
/// REST `compare` endpoint. Token-aware to dodge anonymous rate limits.
pub async fn compare_commits(base: &str, head: &str, token: Option<&str>) -> Result<CompareResult> {
    let url = format!("https://api.github.com/repos/{OWNER}/brave-core/compare/{base}...{head}");
    let mut req = reqwest::Client::builder()
        .user_agent("brave-regress")
        .build()?
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    if let Some((t, _)) = header_token(token)? {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("github compare {base}...{head}: HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await?;
    let total = body.get("total_commits").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let arr = body.get("commits").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let truncated = (total as usize) > arr.len();
    let commits: Vec<CommitRow> = arr.into_iter().filter_map(|c| {
        let sha = c.get("sha")?.as_str()?.to_string();
        let html_url = c.get("html_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let commit_obj = c.get("commit")?;
        let message = commit_obj.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let subject = message.lines().next().unwrap_or("").to_string();
        let author_obj = commit_obj.get("author");
        let author = author_obj.and_then(|a| a.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let date = author_obj.and_then(|a| a.get("date")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let short: String = sha.chars().take(7).collect();
        Some(CommitRow { sha, short, subject, author, date, html_url })
    }).collect();
    Ok(CompareResult { base: base.into(), head: head.into(), total, commits, truncated })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pick_for_host(assets: &[ReleaseAsset], channel: Channel) -> Option<&ReleaseAsset> {
    let host_arch = std::env::consts::ARCH;
    let want_arm = host_arch == "aarch64";

    // Linux .zip — Brave ships portable Linux zips with no channel marker
    // in the filename; channel comes from the release. Require linux marker
    // AND reject darwin/win markers so a Brave macOS / Windows zip can't
    // false-match this picker on bare arch tokens.
    let is_linux_zip = |n: &str| -> bool {
        let l = n.to_lowercase();
        (l.contains("linux") || l.contains("ubuntu") || l.contains("debian"))
            && !l.contains("darwin") && !l.contains("mac")
            && !l.contains("win32") && !l.contains("win64")
    };
    // Reject `*-symbols.zip` / debug bundles for the same reason as the
    // Windows / macOS pickers — alphabetical order can put them ahead of
    // the real archive.
    let zip_clean = |n: &str| -> bool {
        let l = n.to_lowercase();
        n.ends_with(".zip") && name_compatible(n, channel) && is_linux_zip(n)
            && !l.contains("symbol") && !l.contains("pdb") && !l.contains("debug")
    };
    let zip_x64 = |n: &str| -> bool {
        zip_clean(n) && !n.contains("arm") && !n.contains("aarch64")
    };
    let zip_arm = |n: &str| -> bool {
        zip_clean(n) && (n.contains("arm64") || n.contains("aarch64"))
    };
    let zip_any = |n: &str| -> bool { zip_clean(n) };
    let deb_amd64 = |n: &str| -> bool {
        n.ends_with(".deb") && n.contains("amd64") && name_compatible(n, channel)
    };
    let deb_arm64 = |n: &str| -> bool {
        n.ends_with(".deb") && (n.contains("arm64") || n.contains("aarch64"))
            && name_compatible(n, channel)
    };

    let order: [&dyn Fn(&str) -> bool; 5] = if want_arm
        { [&zip_arm, &zip_any, &deb_arm64, &deb_amd64, &zip_x64] }
        else { [&zip_x64, &zip_any, &deb_amd64, &deb_arm64, &zip_arm] };
    for matcher in order {
        if let Some(a) = assets.iter().find(|a| matcher(&a.name)) { return Some(a); }
    }
    None
}
