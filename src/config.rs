use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)] pub retention: Retention,
    #[serde(default)] pub lists: Lists,
    #[serde(default)] pub launch: Launch,
    #[serde(default)] pub gui: Gui,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gui {
    /// How many GitHub releases to fetch (paginated). Step size 50, 50..=500.
    pub release_count: u32,
    pub hide_no_installer: bool,
    /// Optional inclusive date range (ISO yyyy-mm-dd) for filtering the
    /// available-releases panel. Empty string = unset.
    #[serde(default)] pub date_from: String,
    #[serde(default)] pub date_to:   String,
    /// How chatty Brave's logging should be when launched from the GUI.
    #[serde(default)] pub brave_log_level: BraveLogLevel,
    /// Personal access token (no scopes needed) — bumps GitHub's anonymous
    /// 60 req/hr ceiling to 5,000 req/hr. Empty string disables.
    #[serde(default)] pub github_token: String,
    /// When true, every Brave launch passes `--disable-component-update`
    /// plus the poisoned-URL flag so adblock components stay pinned.
    /// Default OFF — most workflows want fresh components on launch.
    #[serde(default)] pub freeze_components: bool,
    /// When true, Brave's bundled "Application Launcher for Drive"
    /// extension (Chrome Web Store id
    /// `lmjegmlicamnimmfhcmpkclmigmmcbeh`) is added to the
    /// per-profile `extensions.install.deny_list`,
    /// `extensions.external_uninstalls`, and its tombstone
    /// `extensions.settings.<id>` is removed so Brave behaves as
    /// if the extension was never installed (no "Action required"
    /// badge). Default ON — most regression-testing workflows
    /// don't want stock Brave's bundled Drive integration.
    #[serde(default = "default_true")] pub block_drive_launcher: bool,
    /// Suppress Brave's first-run P3A telemetry consent banner by
    /// pre-writing the relevant `Local State` keys before launch:
    ///   brave.p3a.notice_acknowledged = true
    ///   brave.p3a.enabled             = false
    ///   brave.stats.reporting_enabled = false
    /// Default ON — most regression-testing workflows don't want
    /// the banner re-appearing on every fresh throwaway and don't
    /// want the test browser pinging home either.
    #[serde(default = "default_true")] pub suppress_p3a_banner: bool,
    /// When true, every Brave launch appends `auto_open_url` as a
    /// positional argument so Chromium opens the URL in a new tab
    /// at startup. Useful for regression-testing a specific page
    /// against multiple Brave versions without typing the URL into
    /// each launch.
    #[serde(default)] pub auto_open_url_enabled: bool,
    #[serde(default)] pub auto_open_url:         String,
    /// Path to the user's preferred external text editor. The
    /// list editor's "Open in External editor" button (always
    /// shown in the bottom action row) hands the file to this
    /// program when set. Empty (default) falls back to the OS
    /// default handler: `cmd /c start` on Windows, `open` on
    /// macOS, `xdg-open` on Linux. Single-executable paths only;
    /// we don't parse shell argv (use Browse to avoid quoting
    /// issues with paths containing spaces).
    #[serde(default)] pub preferred_external_editor: String,
    /// Saved position of the per-tag Note editor window so a user
    /// who's dragged it doesn't have to re-place it every time
    /// they reopen the app. `[x, y]` in egui screen coords. None
    /// until the window has been moved at least once.
    #[serde(default)] pub note_window_pos: Option<[f32; 2]>,
    #[serde(default)] pub regression_report_pos: Option<[f32; 2]>,
    /// UI theme: `"dark"` (default) or `"light"`. Anything else falls
    /// back to dark.
    #[serde(default = "default_theme")] pub theme: String,
    /// Which Brave release channels to include in the available list.
    /// At least one must be true (default: only Nightly).
    #[serde(default)]                pub channel_release: bool,
    #[serde(default)]                pub channel_beta:    bool,
    #[serde(default = "default_true")] pub channel_nightly: bool,
    /// App-wide override for `--user-data-dir`. Per-version overrides
    /// (set on a row) still take precedence; this is the default applied
    /// to any version that doesn't have its own. Disabled by default.
    #[serde(default)] pub default_profile_dir_enabled: bool,
    #[serde(default)] pub default_profile_dir:         String,
    /// App-wide default extra Brave arguments. Per-version row args still
    /// take precedence; this fills in when a row has no extra args. Off
    /// by default.
    #[serde(default)] pub default_args_enabled: bool,
    #[serde(default)] pub default_args:         String,
    /// When true, every Launch / Apply & Launch creates a fresh
    /// throwaway profile dir under `<data-root>/profiles/throwaway-…`
    /// instead of reusing the selected profile. Use this to dodge
    /// state-corrupted profiles when bisecting regressions. Off by
    /// default; ignored when a per-row user_data_dir is set.
    #[serde(default)] pub clean_profile_per_launch: bool,
    /// When **clean_profile_per_launch** is on AND this is true, the
    /// first launch of a given tag in the current session generates
    /// a throwaway dir and remembers it; subsequent relaunches of
    /// the same tag re-use that dir so settings/lists/cookies the
    /// user added in the first launch persist. Restarting the app
    /// (or toggling clean_profile_per_launch off and on) rotates to
    /// a fresh throwaway.
    #[serde(default)] pub reuse_clean_profile: bool,
    /// When true, every release we ever fetch is persisted to a sqlite
    /// `release_cache` table. Subsequent fetches break out of
    /// pagination as soon as they hit a tag we already know about, so
    /// re-fetching after picking a deep date range only walks the few
    /// pages newer than the latest cached tag — instead of walking
    /// every release in between every time. On by default since it
    /// strictly reduces GitHub API load with no downside for typical
    /// regression-bisection workflows.
    #[serde(default = "default_true")] pub incremental_release_cache: bool,
    /// Launch Brave with Windows UAC elevation ("Run as administrator")
    /// when true. Off by default. Ignored on non-Windows hosts. Note:
    /// elevated launches go through `powershell Start-Process -Verb
    /// RunAs`, so stderr-pipe and the per-row Stop force-kill don't
    /// work for those launches — the spawned Child handle represents
    /// the launcher, not Brave itself.
    #[serde(default)] pub launch_as_admin: bool,
    /// Override for the directory Brave installs are extracted into.
    /// Default (empty) keeps the standard `<data_root>/versions/`. Set
    /// this to relocate the heavy install tree off the default drive
    /// (e.g. AppData on C: → a roomier D: partition). Other data dirs
    /// — profiles, cache/downloads, db — are unaffected.
    #[serde(default)] pub versions_dir: String,
    /// Where the Settings collapsing panel is shown:
    /// `"versions"` (default) / `"lists"` / `"both"`. Lets the user
    /// reach Settings without switching tabs while testing.
    #[serde(default = "default_settings_location")] pub settings_location: String,
}

fn default_settings_location() -> String { "versions".into() }

fn default_true() -> bool { true }

fn default_theme() -> String { "dark".into() }
impl Default for Gui {
    fn default() -> Self {
        Self {
            release_count: 50,
            hide_no_installer: true,
            date_from: String::new(),
            date_to:   String::new(),
            brave_log_level: BraveLogLevel::default(),
            github_token: String::new(),
            freeze_components: false,
            block_drive_launcher: true,
            suppress_p3a_banner: true,
            auto_open_url_enabled: false,
            auto_open_url: String::new(),
            preferred_external_editor: String::new(),
            note_window_pos: None,
            regression_report_pos: None,
            theme: "dark".into(),
            channel_release: false,
            channel_beta:    false,
            channel_nightly: true,
            default_profile_dir_enabled: false,
            default_profile_dir: String::new(),
            default_args_enabled: false,
            default_args: String::new(),
            clean_profile_per_launch: false,
            reuse_clean_profile: false,
            incremental_release_cache: true,
            launch_as_admin: false,
            versions_dir: String::new(),
            settings_location: "versions".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum BraveLogLevel {
    /// Default — only what Brave writes to stderr unprompted.
    #[default] Quiet,
    /// `--enable-logging=stderr --log-level=0` — INFO+, on stderr.
    Normal,
    /// adds `--v=1` — verbose 1 across all modules.
    Verbose,
    /// adds `--v=2` plus per-module 3 for adblock & brave_*.
    VeryVerbose,
}
impl BraveLogLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Quiet       => "Quiet",
            Self::Normal      => "Normal",
            Self::Verbose     => "Verbose",
            Self::VeryVerbose => "Very verbose",
        }
    }
    pub const ALL: [BraveLogLevel; 4] =
        [Self::Quiet, Self::Normal, Self::Verbose, Self::VeryVerbose];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retention {
    pub keep_versions: usize,
    pub keep_runs: usize,
    pub auto_prune: bool,
    pub protect_marked: bool,
    pub keep_component_versions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lists {
    pub auto_pin_on_apply: bool,
    pub quarantine_new_versions: bool,
    pub watcher_enabled: bool,
    /// prompt | ignore | auto_merge | auto_overwrite
    pub on_upstream_update: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launch {
    pub remote_debugging_port: u16, // 0 = auto-pick
    pub close_grace_secs: u64,
}

impl Default for Retention {
    fn default() -> Self {
        Self { keep_versions: 6, keep_runs: 50, auto_prune: true,
               protect_marked: true, keep_component_versions: 2 }
    }
}
impl Default for Lists {
    fn default() -> Self {
        Self { auto_pin_on_apply: true, quarantine_new_versions: true,
               watcher_enabled: true, on_upstream_update: "prompt".into() }
    }
}
impl Default for Launch {
    fn default() -> Self { Self { remote_debugging_port: 0, close_grace_secs: 5 } }
}
impl Config {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() { return Ok(Self::default()); }
        let s = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&s)?)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
