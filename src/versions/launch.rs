use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::config::BraveLogLevel;
use crate::paths;

pub struct LaunchOpts {
    pub remote_debugging_port: u16, // 0 = auto
    pub disable_component_update: bool,
    pub headless: bool,
    pub extra_args: Vec<String>,
    pub log_level: BraveLogLevel,
    /// When `Some`, used as `--user-data-dir` verbatim; otherwise the app
    /// computes a path under its standard profile directory from `profile`.
    pub custom_user_data_dir: Option<PathBuf>,
}
impl Default for LaunchOpts {
    fn default() -> Self {
        Self {
            remote_debugging_port: 0,
            disable_component_update: true,
            headless: false,
            extra_args: vec![],
            log_level: BraveLogLevel::Quiet,
            custom_user_data_dir: None,
        }
    }
}

/// Launch Brave for `tag`, isolated to `profile`.
pub fn launch(tag: &str, profile: &str) -> Result<Child> {
    launch_with(tag, profile, &LaunchOpts::default())
}

/// Same as `launch` but pipes Brave's stderr line-by-line into the console
/// log so the user can see what the browser is logging in real time.
/// `log_level` controls how chatty Brave is.
/// `freeze_components` controls whether `--disable-component-update` (and
/// the poison-URL fallback) is applied — `true` keeps adblock components
/// pinned, `false` lets Brave fetch fresh lists from the update server.
/// `extra_args` are appended verbatim to the launch command line — used
/// for per-version custom flags configured in the GUI.
pub fn launch_with_console(tag: &str, profile: &str,
                           console: crate::console::Handle,
                           log_level: BraveLogLevel,
                           freeze_components: bool,
                           extra_args: Vec<String>,
                           custom_user_data_dir: Option<PathBuf>) -> Result<Child> {
    let opts = LaunchOpts {
        log_level,
        disable_component_update: freeze_components,
        extra_args,
        custom_user_data_dir,
        ..LaunchOpts::default()
    };
    let mut child = launch_internal(tag, profile, &opts, /*pipe_stderr=*/true)?;
    if let Some(stderr) = child.stderr.take() {
        let label = format!("brave/{tag}");
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }
                crate::console::brave(&console, &label, trimmed);
            }
        });
    }
    Ok(child)
}

pub fn launch_with(tag: &str, profile: &str, opts: &LaunchOpts) -> Result<Child> {
    launch_internal(tag, profile, opts, /*pipe_stderr=*/false)
}

fn launch_internal(tag: &str, profile: &str, opts: &LaunchOpts, pipe_stderr: bool) -> Result<Child> {
    let bin = paths::brave_binary(tag);
    if !bin.exists() {
        return Err(anyhow!("brave binary missing for {tag}: {}", bin.display()));
    }
    let user_data = match &opts.custom_user_data_dir {
        Some(p) => p.clone(),
        None    => paths::profile_dir(profile),
    };
    std::fs::create_dir_all(&user_data)?;

    // Chromium leaves SingletonLock / SingletonSocket / SingletonCookie behind
    // when the browser exits ungracefully (or when our previous child got
    // SIGKILL'd). On the next launch those stale files can either make Brave
    // hang waiting for a phantom prior instance, or make it silently attach
    // to a dead process. Remove them before spawning. If Brave were actively
    // running against this profile, the OS would refuse the unlink and we
    // ignore the error — the spawn-time lock retry handles the rare race.
    for stale in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(user_data.join(stale));
    }

    let mut cmd = Command::new(&bin);
    cmd.arg(format!("--user-data-dir={}", user_data.display()))
       .arg(format!("--remote-debugging-port={}", opts.remote_debugging_port))
       .arg("--no-first-run")
       .arg("--no-default-browser-check")
       .arg("--disable-brave-update");

    if opts.disable_component_update {
        cmd.arg("--disable-component-update")
           .arg("--disable-features=ComponentUpdater")
           // Belt-and-suspenders: even if --disable-component-update is
           // ever silently ignored or removed in a future Chromium, point
           // the updater at an unreachable URL so any pull attempts fail
           // fast and keep our pinned adblock components untouched.
           .arg("--component-updater=url-source=http://0.0.0.0/");
    }
    if opts.headless { cmd.arg("--headless=new"); }

    // WSL needs --no-sandbox (user-namespace sandbox isn't reliable in the
    // default WSL container) and a forced X11 ozone backend (WSLg's Wayland
    // path varies by distro/release and frequently fails to negotiate).
    if crate::wsl::is_wsl() {
        cmd.arg("--no-sandbox");
        cmd.arg("--ozone-platform=x11");
        // dGPU passthrough often half-works in WSLg → disable GPU to dodge
        // the "GPU process isn't usable" repeats from the Brave logger.
        cmd.arg("--disable-gpu");
    }

    // Suppress every keychain / credential-store prompt Chromium would
    // otherwise raise. brave-regress is a regression-testing harness, not
    // a daily browser — encrypted credential persistence between throwaway
    // Nightly installs is friction without value.
    //
    // Two separate Chromium subsystems prompt:
    //   1. The password-manager backend (`--password-store`). Default
    //      auto-detects gnome-libsecret / KWallet / mac-keychain.
    //   2. `OSCrypt`, which encrypts cookies / per-site data with a master
    //      key stored in the OS keychain. `--password-store` does NOT
    //      cover this — on macOS we also need `--use-mock-keychain`,
    //      which is Chromium's testing flag for bypassing the real
    //      keychain entirely.
    //
    // Windows uses CryptProtectData with no user-visible prompt, so it
    // doesn't need either flag.
    #[cfg(unix)]
    {
        cmd.arg("--password-store=basic");
    }
    #[cfg(target_os = "macos")]
    {
        cmd.arg("--use-mock-keychain");
    }

    // Brave / Chromium logging flags. `--enable-logging=stderr` is the
    // critical bit — without it, LOG output goes to <profile>/chrome_debug.log
    // and never reaches our pipe, even if --v=N is set.
    match opts.log_level {
        BraveLogLevel::Quiet => {}
        BraveLogLevel::Normal => {
            cmd.arg("--enable-logging=stderr").arg("--log-level=0");
        }
        BraveLogLevel::Verbose => {
            cmd.arg("--enable-logging=stderr").arg("--log-level=0").arg("--v=1");
        }
        BraveLogLevel::VeryVerbose => {
            cmd.arg("--enable-logging=stderr").arg("--log-level=0").arg("--v=2")
               .arg("--vmodule=*adblock*=3,brave_*=2");
        }
    }

    for a in &opts.extra_args { cmd.arg(a); }

    if pipe_stderr {
        cmd.stderr(Stdio::piped());
    }

    tracing::info!("launching {} (profile={})", bin.display(), user_data.display());
    Ok(cmd.spawn()?)
}

/// Force-kill a Brave parent process AND every descendant. `Child::kill`
/// only nukes the parent; helper / renderer / GPU / network-service
/// children usually self-exit when the parent dies, but a hung parent or
/// a launcher fork can leave orphans behind. Walks the platform's
/// process tree to make sure nothing survives.
///
/// All errors are silenced — this is a best-effort nuke; if some PIDs
/// are already gone or unreachable we don't care.
pub fn force_kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        // /F = force, /T = whole tree (the spawned PID + every descendant).
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID"])
            .arg(pid.to_string())
            .status();
    }
    #[cfg(unix)]
    {
        // Two-step nuke: first SIGKILL every direct + indirect descendant,
        // then SIGKILL the parent. pkill -P walks one level at a time but
        // -P with --signal KILL kills children before they can spawn more.
        // The follow-up `kill -KILL <pid>` makes sure the parent itself
        // dies even if its children were hanging.
        let _ = std::process::Command::new("pkill")
            .args(["-KILL", "-P"])
            .arg(pid.to_string())
            .status();
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
}

/// Read the auto-picked CDP port from `<user-data-dir>/DevToolsActivePort`.
/// File format: first line is the port, second line is the websocket path.
pub fn read_cdp_port(profile: &str) -> Result<u16> {
    let p: PathBuf = paths::profile_dir(profile).join("DevToolsActivePort");
    let s = std::fs::read_to_string(&p)?;
    let port: u16 = s.lines().next().ok_or_else(|| anyhow!("empty DevToolsActivePort"))?
        .trim().parse()?;
    Ok(port)
}
