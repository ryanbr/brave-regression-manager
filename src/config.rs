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
    /// When true, every release we ever fetch is persisted to a sqlite
    /// `release_cache` table. Subsequent fetches break out of
    /// pagination as soon as they hit a tag we already know about, so
    /// re-fetching after picking a deep date range only walks the few
    /// pages newer than the latest cached tag — instead of walking
    /// every release in between every time.
    #[serde(default)] pub incremental_release_cache: bool,
}

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
            theme: "dark".into(),
            channel_release: false,
            channel_beta:    false,
            channel_nightly: true,
            default_profile_dir_enabled: false,
            default_profile_dir: String::new(),
            default_args_enabled: false,
            default_args: String::new(),
            clean_profile_per_launch: false,
            incremental_release_cache: false,
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
