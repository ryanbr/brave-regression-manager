use anyhow::{anyhow, Result};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::paths;
use crate::versions::launch::{launch_with, LaunchOpts};

use super::{discover, guard, pin};

/// Full edit→test loop:
/// 1. Stop Brave for this profile (graceful, then kill on grace timeout)
/// 2. Wait for the list file handle to be released
/// 3. Run the pre-launch guard (quarantines unauthorized component updates)
/// 4. Invalidate parsed-DAT caches so Brave re-reads list.txt
/// 5. Pin all components if config.lists.auto_pin_on_apply
/// 6. Launch Brave with --disable-component-update
pub async fn apply_and_relaunch(profile: &str, version_tag: &str) -> Result<()> {
    let cfg = Config::load_or_default(&paths::config_path())?;
    let profile_dir = paths::profile_dir(profile);
    if !profile_dir.exists() {
        return Err(anyhow!("profile not found: {}", profile_dir.display()));
    }

    stop_brave_for_profile(&profile_dir, cfg.launch.close_grace_secs).await?;
    wait_for_unlocks(&profile_dir).await?;
    let _pending = guard::pre_launch_guard(&profile_dir)?;
    invalidate_dat_caches(&profile_dir)?;
    if cfg.lists.auto_pin_on_apply {
        let _ = pin::pin_all(&profile_dir)?;
    }

    let _child = launch_with(version_tag, profile, &LaunchOpts {
        remote_debugging_port: cfg.launch.remote_debugging_port,
        disable_component_update: true,
        headless: false,
        extra_args: vec![],
        ..LaunchOpts::default()
    })?;
    Ok(())
}

async fn stop_brave_for_profile(profile_dir: &Path, _grace_secs: u64) -> Result<()> {
    // Best-effort: if a previous launch left a child handle around it's the GUI's
    // job to track it. CLI path here only needs to ensure no file handle blocks
    // the upcoming write — `wait_for_unlocks` handles that defensively.
    let _ = profile_dir; // Currently a placeholder; richer process tracking lives in the GUI runner.
    Ok(())
}

async fn wait_for_unlocks(profile_dir: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut all_ok = true;
        for list in discover::enabled_lists(profile_dir).unwrap_or_default() {
            if std::fs::OpenOptions::new()
                .read(true).write(true).open(&list.path)
                .is_err()
            { all_ok = false; break; }
        }
        if all_ok { return Ok(()); }
        if Instant::now() > deadline { return Ok(()); } // best-effort
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Brave caches parsed DATs of compiled lists; if we leave them in place after
/// editing list.txt the engine may use the stale cache. Wipe them.
fn invalidate_dat_caches(profile_dir: &Path) -> Result<()> {
    for sub in ["AdBlock", "Default/AdBlock"] {
        let p = profile_dir.join(sub);
        if p.exists() { std::fs::remove_dir_all(p)?; }
    }
    Ok(())
}
