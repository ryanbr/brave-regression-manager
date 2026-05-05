use anyhow::Result;
use std::time::{Duration, Instant};

use crate::lists;
use crate::paths;
use crate::versions::launch::{launch_with, LaunchOpts};

/// Seed a profile's adblock list cache by launching Brave once with the
/// component updater ENABLED, polling until lists land, then closing.
/// Subsequent test launches use `--disable-component-update`.
pub async fn seed_lists(profile: &str, version_tag: &str) -> Result<()> {
    seed_lists_with_console(profile, version_tag, None).await
}

/// Same as `seed_lists` but emits per-phase progress to the GUI Console
/// when a handle is supplied — start, periodic poll heartbeat, success
/// or timeout. Without these the GUI's "Seeding…" button is silent for
/// the entire 120-240 s headless run.
pub async fn seed_lists_with_console(
    profile: &str, version_tag: &str,
    console: Option<crate::console::Handle>,
) -> Result<()> {
    let profile_dir = paths::profile_dir(profile);
    std::fs::create_dir_all(&profile_dir)?;

    if let Some(c) = &console {
        crate::console::info(c, "seed", format!(
            "{version_tag} -> profile '{profile}': launching headless Brave \
             with component updates enabled (poll every 2s, timeout {}s)",
            if crate::wsl::is_wsl() { 240 } else { 120 }));
    }

    let mut child = launch_with(version_tag, profile, &LaunchOpts {
        remote_debugging_port: 0,
        disable_component_update: false,
        headless: true,
        extra_args: vec![],
        ..LaunchOpts::default()
    })?;

    // WSL launches Brave with software rendering and slow IPC, so list pulls
    // can take noticeably longer than on native. Give it more headroom there.
    let timeout_secs = if crate::wsl::is_wsl() { 240 } else { 120 };
    let started_at = Instant::now();
    let deadline = started_at + Duration::from_secs(timeout_secs);
    let mut seeded = false;
    let mut last_heartbeat = Instant::now();
    while Instant::now() < deadline {
        let found = lists::discover::enabled_lists(&profile_dir).unwrap_or_default();
        if found.iter().any(|l| matches!(l.kind, lists::discover::ListKind::Default)) {
            seeded = true;
            break;
        }
        // Heartbeat every 10 s so the user can see progress in the
        // Console rather than staring at "Seeding…" with no signal.
        if last_heartbeat.elapsed() >= Duration::from_secs(10) {
            last_heartbeat = Instant::now();
            if let Some(c) = &console {
                crate::console::info(c, "seed", format!(
                    "still polling ({:.0}s elapsed, {} list file(s) on disk so far)",
                    started_at.elapsed().as_secs_f64(), found.len()));
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let _ = child.kill();
    let _ = child.wait();
    let elapsed = started_at.elapsed().as_secs_f64();
    if seeded {
        if let Some(c) = &console {
            crate::console::info(c, "seed",
                format!("lists landed in {elapsed:.1}s — done"));
        }
    } else {
        tracing::warn!("seeding timed out — lists may need a longer wait or a manual run");
        if let Some(c) = &console {
            crate::console::warn(c, "seed", format!(
                "timeout after {elapsed:.1}s — Brave's component updater \
                 didn't deliver any Default lists. Check your network, \
                 or run Brave once manually with this profile to see if \
                 it can reach the update server."));
        }
    }
    Ok(())
}
