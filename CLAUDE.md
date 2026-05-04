# CLAUDE.md

Working notes for AI assistants editing this repo. Not user-facing —
keep this terse and focused on facts that aren't obvious from the
source.

## What this is

`brave-regress` (display name: "Brave Regression Manager") — a Rust +
egui GUI for bisecting regressions between Brave (Nightly / Beta /
Release) builds and between adblock list configurations. Single binary,
no daemon. Data lives under `data_root()` (see `src/paths.rs`).

## Build / verify

```sh
# Native (Linux / macOS host):
cargo build --release

# Cross-compile to Windows from Linux/WSL (existing dev-box flow):
cargo build --release --target x86_64-pc-windows-gnu
# Output: target/x86_64-pc-windows-gnu/release/brave-regress.exe (~8 MB)
```

`cargo check --release` is fine for fast verification on Linux. Always
finish a change set with at least the cross-Windows release build —
that's the primary target the user runs. End every change set with a
`cargo clippy --release --all-targets` pass too; the codebase carries
zero warnings.

The `release` profile is size-tuned (`lto=fat`, `opt-level=z`,
`panic=abort`, `strip=symbols`). For iteration, `--profile
release-quick` exists (thin LTO, opt-level=3).

CI (`.github/workflows/release.yml`) builds three triples on `v*` tag
push: `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
`aarch64-apple-darwin`. (The dev-box cross-build is GNU; CI's Windows
artefact is MSVC. Both are functionally identical — see the README.)

## Code style

- Terse, no narration. Don't write comments that just restate the
  next line's code.
- Comments are reserved for *why* something non-obvious is the way it
  is (a workaround for a real bug, a constraint that isn't visible
  locally, an invariant a future reader would otherwise break).
- Prefer editing the existing file over creating a new one.
- No backwards-compat shims for code we own — if a signature changes,
  update every caller in the same change.
- Don't add features the user didn't ask for. A bug fix doesn't need
  surrounding cleanup.
- ASCII-only glyphs in user-visible strings. egui's default font on
  Windows tofus most non-ASCII (▲ ▼ ⧉ → 👍 etc.) — every fancy glyph
  we tried has been replaced. Stick to `^ v -> +` and the like.

## Things to know about this codebase

### Platform paths are **channel-aware**

`paths::brave_binary(tag)` probes each platform's known binary names
(Nightly / Beta / Release / portable-zip flat layout) and returns the
first that exists. `flatten_top_level_subdir` in `src/versions/install.rs`
matches the same set. If you add a new channel or layout, update both.

### `paths::versions_dir()` honours a runtime override

There's a `OnceLock<PathBuf>` set during `App::new` from
`config.gui.versions_dir`. Empty value keeps the default
`<data-root>/versions/`; non-empty relocates Brave installs only
(profiles, cache/downloads, db are unaffected). Changes take effect on
next launch — existing on-disk installs are NOT auto-migrated. The
override is set lock-free; never call `set_versions_dir_override` mid-
session.

### The launch precedence is layered

Three sources of `--user-data-dir`, in order:

1. Per-row override (sqlite — `verdict::user_data_dir(tag)`).
2. **Clean profile per launch** (Settings) — generates a fresh
   `profiles/throwaway-<tag>-<unix-ts>/` per launch when on.
3. App-wide default (Settings → Default profile folder —
   `state.default_profile_dir_enabled`).
4. App's standard `paths::profile_dir(profile)`.

Two sources of extra args, in order:

1. Per-row launch args (sqlite — `verdict::launch_args(tag)`).
2. App-wide default (`state.default_args_enabled`).

Both `tab_versions::ui` (Launch button) and `tab_lists::ui` (Apply &
Launch) implement these chains — keep them in sync.

### `--remote-debugging-port` is poison on real profiles

Chromium (post-2022 CVE) refuses to enable `--remote-debugging-port`
when the user-data-dir contains a real personal profile, and exits
within seconds. We pass the flag only when caller sets a non-zero
port; default 0 omits it entirely. Don't re-add an unconditional
`--remote-debugging-port=0` to the launch path.

### Channel detection

`versions::github::detect_release_channel(release)` decides Nightly /
Beta / Release using these signals, in priority order:

1. **Release title prefix** ("Release v…" / "Beta v…" / "Nightly v…").
   Most reliable — Brave's release tooling enforces this.
2. Asset-name marker scan (separator-bounded so a stray
   `…Nightly-symbols.txt` shipped alongside a stable build doesn't
   false-positive).
3. The GitHub `prerelease` flag — last resort. **Brave marks every
   release as prerelease=true**, so this can't distinguish Stable from
   Nightly on its own; never treat it as authoritative.

The asset picker (`pick_for_host`) accepts the channel and uses
`name_compatible(name, channel)`. The macOS / Linux `.zip` matchers
reject `*-symbols.zip` / `*-pdb.zip` / `*-debug.zip` (they sort
alphabetically before the real archive).

### Windows asset picker — no cross-arch fallback on x64

x64 Windows can't run an ARM PE (no emulator the other direction; only
Win11-on-ARM emulates x64). On x64 hosts the picker order ends at
x64-only — if no x64 asset exists for the tag, it returns None and the
GUI surfaces "no installer". On ARM hosts the order falls through to
x64 since Win11-on-ARM emulates x64 fine. Don't restore the old
catch-all `zip_arm` last-resort matcher for x64 hosts.

### Defensive arch check at install time

The Install button is disabled when `is_opposite_arch_asset(name)`
trips on the cached `host_asset` filename. This catches a stale
`releases.json` populated by an older (looser) picker — without it the
user could end up with an arm64 zip on x64 Windows even after the
picker fix.

### Cache + commits

`ReleaseCache` (`src/gui/state.rs`) persists the available-releases
listing to `<data-root>/cache/releases.json` so installs can go
direct to S3 without re-querying GitHub on launch. New fields on
`ReleaseRow` need `#[serde(default)]` and back-fill logic in
`ReleaseRow::ensure_channel`-style helpers — old caches must keep
loading.

**Incremental release cache** (Settings, on by default): every
release we ever fetch lands in a sqlite `release_cache` table (full
ReleaseRow as JSON, keyed by tag). Subsequent fetches break out of
pagination on the first known tag. Fetches force `ChannelFilter::all()`
in this mode so the cache grows uniformly; the Available row render
applies a client-side channel filter on top. Manually-added tags
(see below) are exempted from that filter.

When `state.date_from < oldest_cached`, the known-tag short-circuit
is **skipped** so the deep walk reaches the requested date — every
new tag along the way is upserted, hydrating the cache for the whole
range in one fetch.

`compare_commits()` in `src/versions/github.rs` hits GitHub's REST
`compare` endpoint directly via `reqwest` (not octocrab). Cap is 250
commits — show a "open on GitHub for full list" hint when truncated.

`fetch_release_by_tag()` is a single-API-call path used by the
"Add release by tag" UI; gets a specific tag without paginating.

### Manual release tags

Tags pulled via the Add-by-tag UI are recorded in the
`manual_release_tags` sqlite table. The Available row render
**exempts these from the channel display filter** (so a manually-added
Release tag still shows when only Nightly is ticked) and also from
the date filter. They're floated to the top of the Available list
with a separator below, and rendered with a cyan **Tag** column for
visual distinction. Per-row Remove button drops the manual mark +
deletes the on-disk install + force-kills any running Brave for that
tag (verdicts/notes preserved in sqlite).

### Sqlite store

`src/verdict/mod.rs` opens `<data-root>/db/verdicts.sqlite` lazily and
uses `CREATE TABLE IF NOT EXISTS` — schema additions are
forwards-compatible. Tables today:
- `version_verdict`, `list_verdict`, `cell_verdict` (verdicts)
- `launch_args`, `user_data_dir` (per-tag launch overrides)
- `notes` (per-tag freeform notes)
- `tag_metadata` (chromium_version + published_at + channel cache —
  fallback when a bracket tag isn't in `state.available`)
- `release_cache` (incremental-cache JSON blobs by tag)
- `manual_release_tags` (user-added tags exempt from channel filter)

The connection is cached behind a `OnceLock<Mutex<Connection>>` —
schema setup runs once at first access; every subsequent accessor is
a mutex acquisition + query. Don't go back to opening fresh
connections per call.

### Per-frame data discipline (Available render)

The Available render loop with N≈4000 rows is the perf-sensitive path.
Conventions to preserve:

- `state.available` is `Arc<Vec<ReleaseRow>>`. Per-frame snapshot is
  `Arc::clone()` (O(1)). Mutations go through `Arc::make_mut`.
- Sort happens on a `Vec<usize>` of indices via
  `sort_available_indices` — never deep-clone the row Vec for sort.
- `verdict::all_version_verdicts()` and `all_notes()` are bulk-loaded
  ONCE per frame into HashMaps and passed into both row render and
  sort comparator. Don't add per-row `verdict::version_verdict(tag)`
  / `verdict::note(tag)` calls back into the loop.
- `render_compare_section` builds a `tag -> (chromium, published_at)`
  HashMap once before iterating channels — `render_compare_one` looks
  up O(1). Don't reintroduce `state.available.iter().find(...)` per
  bracket endpoint.

### Concurrency

- Up to **3 parallel installs** — `MAX_CONCURRENT_INSTALLS = 3` in
  `state.rs`. Install button enables when
  `!state.installing.contains(tag) && state.installing.len() < MAX`.
- `state.slots.install_done` is a Vec-backed queue (`InstallQueue`)
  so multiple completions in the same frame don't clobber each other.
- `ProgressSink` is `Arc<Mutex<HashMap<String, DownloadProgress>>>`
  keyed by tag — per-tag progress avoids the flicker that a single
  Option slot caused with parallel downloads.
- Per-channel compare-commit results stack the same way:
  `state.compare_loading` is a `HashSet<String>` (channel names),
  `compare_results` and `compare_errors` are `HashMap<channel, _>`.

### Startup cache load is deferred

`App::new` returns immediately with `state.loading_startup_cache =
true`. The releases.json read + JSON parse + sqlite incremental merge
runs as a tokio task; result lands via `slots.startup_cache_done`
into the drain. Don't move heavy disk/parse work back inline in
`App::new` — the window-paint latency was noticeable on multi-MB
caches.

### Console + diagnostics conventions

- Single startup `[settings]` line dumps every persisted GUI setting.
  GitHub token is masked (`present (N chars)` / `absent`); never log
  the value.
- Every persisted-setting toggle emits a focused `[config]` line.
- Launch path emits a `[profile]` line with source / dir_exists /
  Local State presence / SingletonLock + sub-profile inventory + a
  schema-downgrade warning if the launching version is older than
  `last_browser_version` in Local State.
- Launch path emits a `[launch] argv: <full command>` echo before
  spawn (for all four spawn paths — normal + Win/macOS/Linux
  elevated). Whitespace args are quoted so the line is paste-runnable.
- Failure-hint helpers (`launch_failure_hint` / `install_failure_hint`
  / `fetch_failure_hint`) pattern-match on common OS / HTTP error
  strings and append actionable advice (e.g. 403 → paste a token,
  os err 14001 → install VC++ Redist).

### WSL specifics

`src/wsl::is_wsl()` gates a few launch flags
(`--no-sandbox`, `--ozone-platform=x11`, `--disable-gpu`) and changes
the URL/file opener (uses `explorer.exe` so links go to the host's
default browser instead of an in-WSL X11 one).

### macOS quarantine

After every install on macOS, `install_tag_with_asset` runs
`xattr -dr com.apple.quarantine <dest>` so Gatekeeper doesn't block
first launch of the freshly-extracted Brave. Best-effort — if `xattr`
is missing or fails, install continues.

### Privilege escalation (Launch as administrator)

Settings checkbox routes the launch through a per-platform wrapper:
- Windows: `powershell Start-Process -Verb RunAs` (UAC)
- macOS: `osascript … with administrator privileges`
- Linux: `pkexec` (polkit graphical auth — sudo needs a TTY the GUI
  doesn't have)

Linux launches automatically add `--no-sandbox` (Chromium refuses to
run as root otherwise). The Child handle in all three cases is the
elevation launcher, NOT Brave — so stderr-pipe and the per-row Stop
force-kill don't apply to elevated launches. Document accordingly.

### rfd feature gotcha

`rfd` with the `xdg-portal` feature **requires** `pollster` (or
`tokio` / `async-std`) too — Linux native check fails without it. We
use `pollster`. Don't drop it.

## Don't do

- **Don't rename the crate / binary** (`brave-regress`). It's the data
  dir name (`<AppData>/brave-regress/`) too — renaming orphans every
  user's installs and verdicts. The display name "Brave Regression
  Manager" lives only in the window title and Cargo description.
- **Don't change `paths::data_root()` defaults.** Same reason.
- **Don't commit `target/`** (already in `.gitignore`).
- **Don't reach for the GUI persistence feature on `eframe`.** It
  was deliberately disabled — it overrode our `InnerSize` viewport
  command with stale window state from prior runs.
- **Don't mock the database in tests.** When tests get added, point
  them at a tempdir-backed sqlite file so the schema/migrations are
  exercised end-to-end.
- **Don't catch errors you can't act on.** Propagate via `anyhow`;
  the GUI funnels them into the Console panel.
- **Don't log GitHub tokens.** The masked `present (N chars)` form is
  the only acceptable representation.

## Common tasks

- **Add a new Settings option:** four edits in lockstep —
  `src/config.rs` (struct field + Default impl), `src/gui/state.rs`
  (mirror field + new() default), `src/gui/app.rs` (load + save in
  `maybe_persist_settings`, plus add to the startup `[settings]`
  line), and the row in `tab_versions::ui`'s Settings grid (set
  `config_dirty = true` on change AND emit a `[config]` echo line
  describing the new value).
- **Add a new background task:** four edits — `AsyncSlots` field
  in `state.rs`, drain block in `app.rs::drain_async_results`, repaint
  trigger in `app.rs::update`, and the spawn site that fills the slot.
  When the result needs per-channel/per-tag keying, model it after
  `compare_done` (Vec-backed queue) rather than a single `AsyncSlot`.
- **Add a new sqlite table:** add the `CREATE TABLE IF NOT EXISTS` to
  `verdict::open()` (forwards-compatible — never DROP). New accessors
  go in `verdict/mod.rs`; bulk loaders for the Available render path
  belong here too.

## Testing notes for the user

The user dev-tests on Windows by:
1. Running `cargo build --release --target x86_64-pc-windows-gnu` from
   WSL.
2. Copying `target/x86_64-pc-windows-gnu/release/brave-regress.exe` to
   their Windows host.

So: every change must compile cleanly cross-Windows. Verify with
`cargo build --target x86_64-pc-windows-gnu --release` before
declaring "done".
