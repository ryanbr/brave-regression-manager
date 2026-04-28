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
that's the primary target the user runs.

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

## Things to know about this codebase

### Platform paths are **channel-aware**

`paths::brave_binary(tag)` probes each platform's known binary names
(Nightly / Beta / Release / portable-zip flat layout) and returns the
first that exists. `flatten_top_level_subdir` in `src/versions/install.rs`
matches the same set. If you add a new channel or layout, update both.

### The launch precedence is layered

Three sources of `--user-data-dir` and extra args, in order:

1. Per-row override (sqlite — `verdict::user_data_dir(tag)`,
   `verdict::launch_args(tag)`).
2. App-wide default (Settings → Default profile folder / Default
   arguments — `state.default_profile_dir_enabled`,
   `state.default_args_enabled`).
3. App's standard `paths::profile_dir(profile)` / no extra args.

Both `tab_versions::ui` (Launch button) and `tab_lists::ui` (Apply &
Launch) implement this same chain — keep them in sync.

### Channel detection

`versions::github::detect_release_channel(release)` decides Nightly /
Beta / Release by scanning asset filenames first, then falling back to
GitHub's `prerelease` flag. The asset picker (`pick_for_host`) accepts
the channel as a parameter and uses `name_compatible(name, channel)` —
which permits **channel-marker-free filenames** (e.g. Brave's portable
`.zip`s) to match because release-level filtering already happened.

The macOS / Linux `.zip` matchers reject `*-symbols.zip` /
`*-pdb.zip` / `*-debug.zip` — these sort *before* the real archive
alphabetically and would otherwise be picked first.

### Cache + commits

`ReleaseCache` (`src/gui/state.rs`) persists the available-releases
listing to `<data-root>/cache/releases.json` so installs can go
direct to S3 without re-querying GitHub on launch. New fields on
`ReleaseRow` need `#[serde(default)]` and back-fill logic in
`ReleaseRow::ensure_channel`-style helpers — old caches must keep
loading.

`compare_commits()` in `src/versions/github.rs` hits GitHub's REST
`compare` endpoint directly via `reqwest` (not octocrab). Cap is 250
commits — show a "open on GitHub for full list" hint when truncated.

### Sqlite store

`src/verdict/mod.rs` opens `<data-root>/db/verdicts.sqlite` lazily and
uses `CREATE TABLE IF NOT EXISTS` — schema additions are
forwards-compatible. Tables today: `version_verdict`, `list_verdict`,
`cell_verdict`, `launch_args`, `user_data_dir`. Adding a new table is
the right move when you need new per-tag persisted data.

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

### rfd feature gotcha

`rfd` with the `xdg-portal` feature **requires** the `tokio` (or
`async-std`) feature too — Linux native check fails without it. Keep
both features enabled in `Cargo.toml`.

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

## Common tasks

- **Add a new Settings option:** four edits in lockstep —
  `src/config.rs` (struct field + Default impl), `src/gui/state.rs`
  (mirror field + new() default), `src/gui/app.rs` (load + save in
  `maybe_persist_settings`), and the row in `tab_versions::ui`'s
  Settings grid (set `config_dirty = true` on change).
- **Add a new background task:** four edits — `AsyncSlots` field
  in `state.rs`, drain block in `app.rs::drain_async_results`, repaint
  trigger in `app.rs::update`, and the spawn site that fills the slot.

## Testing notes for the user

The user dev-tests on Windows by:
1. Running `cargo build --release --target x86_64-pc-windows-gnu` from
   WSL.
2. Copying `target/x86_64-pc-windows-gnu/release/brave-regress.exe` to
   their Windows host.

So: every change must compile cleanly cross-Windows. Verify with
`cargo build --target x86_64-pc-windows-gnu --release` before
declaring "done".
