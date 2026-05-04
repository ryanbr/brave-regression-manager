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
    /// Windows-only: when true, route the launch through
    /// `powershell Start-Process -Verb RunAs` so Brave starts elevated
    /// (UAC prompt). Ignored on non-Windows hosts. The Child handle in
    /// that case represents the launcher, not Brave.
    pub run_as_admin: bool,
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
            run_as_admin: false,
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
#[allow(clippy::too_many_arguments)]
pub fn launch_with_console(tag: &str, profile: &str,
                           console: crate::console::Handle,
                           log_level: BraveLogLevel,
                           freeze_components: bool,
                           extra_args: Vec<String>,
                           custom_user_data_dir: Option<PathBuf>,
                           run_as_admin: bool) -> Result<Child> {
    let opts = LaunchOpts {
        log_level,
        disable_component_update: freeze_components,
        extra_args,
        custom_user_data_dir,
        run_as_admin,
        ..LaunchOpts::default()
    };
    let mut child = launch_internal(tag, profile, &opts, /*pipe_stderr=*/true, Some(&console))?;
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
    launch_internal(tag, profile, opts, /*pipe_stderr=*/false, /*console=*/None)
}

fn launch_internal(tag: &str, profile: &str, opts: &LaunchOpts,
                   pipe_stderr: bool,
                   console: Option<&crate::console::Handle>) -> Result<Child> {
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
       .arg("--no-first-run")
       .arg("--no-default-browser-check")
       .arg("--disable-brave-update");
    // Chromium (May 2022, post-CVE) refuses to enable
    // --remote-debugging-port when --user-data-dir contains a normal
    // user's personal profile — Brave exits within seconds. Only
    // pass the flag when the caller actually requested a non-zero
    // port; with port=0 we just skip it entirely so launches against
    // real BraveSoftware\Brave-Browser-* profiles work.
    if opts.remote_debugging_port != 0 {
        cmd.arg(format!("--remote-debugging-port={}", opts.remote_debugging_port));
    }

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

    // Privilege escalation. Per-platform mechanism, but the same
    // tradeoffs everywhere: the spawned Child represents the elevation
    // launcher (powershell / osascript / pkexec), not Brave itself, so
    // stderr piping and the per-row Stop force-kill don't apply.
    // Debugging affordance only.
    if opts.run_as_admin {
        // On Linux Chromium refuses to run as root unless --no-sandbox
        // is set. WSL launches already pass --no-sandbox, but a native
        // Linux admin launch without it would just bail. Add it for
        // every elevated launch on unix.
        #[cfg(all(unix, not(target_os = "macos")))]
        cmd.arg("--no-sandbox");

        #[cfg(windows)]
        {
            // PowerShell Start-Process -Verb RunAs: triggers UAC prompt.
            // Each arg quoted in single quotes so embedded spaces / `=`
            // don't get re-tokenised by PowerShell.
            let argv: Vec<String> = cmd.get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            let quoted: Vec<String> = argv.iter()
                .map(|a| format!("'{}'", a.replace('\'', "''")))
                .collect();
            let arg_list = quoted.join(",");
            let ps_cmd = format!(
                "Start-Process -FilePath '{}' -Verb RunAs -ArgumentList {}",
                bin.display().to_string().replace('\'', "''"),
                arg_list);
            let mut shell = Command::new("powershell");
            shell.args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd]);
            tracing::info!("launching ELEVATED {} (profile={})",
                bin.display(), user_data.display());
            log_argv_to_console(console, "launch (elevated/win)",
                std::path::Path::new("powershell"), &shell);
            return Ok(shell.spawn()?);
        }

        #[cfg(target_os = "macos")]
        {
            // osascript bridge: `do shell script "..." with administrator
            // privileges` triggers the macOS authentication prompt.
            // Args are joined with spaces and each quoted with single
            // quotes (escape stray quotes by closing-and-reopening
            // around an escaped single quote).
            let mut script = String::new();
            script.push('\'');
            script.push_str(&bin.display().to_string().replace('\'', "'\\''"));
            script.push('\'');
            for a in cmd.get_args() {
                let s = a.to_string_lossy();
                script.push(' ');
                script.push('\'');
                script.push_str(&s.replace('\'', "'\\''"));
                script.push('\'');
            }
            // The whole shell script is then itself an AppleScript
            // string literal — single double-quote escape.
            let osa = format!(
                "do shell script \"{}\" with administrator privileges",
                script.replace('\\', "\\\\").replace('"', "\\\""));
            let mut shell = Command::new("osascript");
            shell.args(["-e", &osa]);
            tracing::info!("launching ELEVATED {} (profile={})",
                bin.display(), user_data.display());
            log_argv_to_console(console, "launch (elevated/macos)",
                std::path::Path::new("osascript"), &shell);
            return Ok(shell.spawn()?);
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // pkexec is the graphical equivalent of sudo on
            // freedesktop.org Linux — drives the polkit auth agent
            // (e.g. polkit-gnome / lxqt-policykit) for a GUI prompt.
            // Falls back to a hard error if pkexec isn't installed;
            // we don't try sudo because it requires a TTY.
            let argv: Vec<String> = cmd.get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            let mut shell = Command::new("pkexec");
            shell.arg(&bin);
            for a in argv { shell.arg(a); }
            tracing::info!("launching ELEVATED {} (profile={})",
                bin.display(), user_data.display());
            log_argv_to_console(console, "launch (elevated/linux)",
                std::path::Path::new("pkexec"), &shell);
            return Ok(shell.spawn()?);
        }
    }

    if pipe_stderr {
        cmd.stderr(Stdio::piped());
    }

    tracing::info!("launching {} (profile={})", bin.display(), user_data.display());
    log_argv_to_console(console, "launch", &bin, &cmd);
    Ok(cmd.spawn()?)
}

/// Stream the full constructed command-line into the GUI Console
/// (one line: `<binary> arg arg arg …`). Helpful for diagnosing why
/// Brave behaves unexpectedly under a custom profile / arg combination
/// — you can see exactly what was passed.
fn log_argv_to_console(
    console: Option<&crate::console::Handle>,
    label: &str,
    bin: &std::path::Path,
    cmd: &Command,
) {
    let Some(c) = console else { return };
    let mut line = String::new();
    // Quote any arg containing whitespace so the printed line is
    // copy-pasteable for manual reproduction.
    let push_quoted = |out: &mut String, s: &str| {
        if s.chars().any(char::is_whitespace) {
            out.push('"');
            out.push_str(&s.replace('"', "\\\""));
            out.push('"');
        } else {
            out.push_str(s);
        }
    };
    push_quoted(&mut line, &bin.display().to_string());
    for a in cmd.get_args() {
        line.push(' ');
        push_quoted(&mut line, &a.to_string_lossy());
    }
    crate::console::info(c, label, format!("argv: {line}"));
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
