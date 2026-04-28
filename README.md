# Brave Regression Manager

A Rust + egui GUI for bisecting regressions between
[Brave](https://github.com/brave/brave-browser) Nightly / Beta / Release
builds and between adblock list configurations. Pick two builds, mark one
GOOD and one BAD, and the app surfaces the brave-core commit range
between them — inline, with click-to-open links straight to GitHub.

<img width="998" height="757" alt="regress" src="https://github.com/user-attachments/assets/ca592df8-cefc-489f-af8a-0ba0ef195901" />

## Why

Brave ships a new Nightly almost daily. When something breaks, narrowing
"which build broke it" by hand is tedious: download, install side-by-side,
compare. This tool turns that into a few clicks: install any tag from
GitHub, label rows GOOD or BAD, and hit *Load* on the bracket panel to see
exactly which commits to bisect.

## Features

- **Multi-channel install** — fetches Brave **Nightly**, **Beta**, and
  **Release** tags from GitHub. Per-platform asset picker prefers the
  portable `.zip` and falls back to `.deb` (Linux) or `.dmg` (macOS); the
  Windows portable zip is preferred over the proprietary installer.
- **Per-tag verdicts** — mark each installed version GOOD / BAD / Unknown.
  Colour-coded dot in every row for at-a-glance status.
- **brave-core compare panel** — automatically detects the closest
  GOOD/BAD pair, then either opens the GitHub *compare* page or fetches
  the commit list inline (SHA · date · author · subject; click any SHA to
  open the commit on GitHub).
- **Per-version overrides** — every Installed row can override its
  `--user-data-dir` (Profile…) and append extra Brave flags (extra args).
- **App-wide defaults** — Settings exposes optional default values for
  user-data-dir and extra args, applied whenever a row's per-version
  override is empty.
- **Channel + date filter** — show only Release / Beta / Nightly, narrow
  by date with year/month dropdowns or quick presets (7d / 30d / 60d /
  90d / 120d / 150d). Date filter triggers a smart re-fetch that uses
  GitHub's pagination as a `stop_at` to avoid wasted API calls.
- **Light + dark themes** — toggle from Settings (☀ / ☾), persisted to
  config.
- **Console panel** — every background event (fetch, download, install,
  launch, brave-core compare, errors) streamed to a third tab with
  level colouring. Brave's stderr is piped here too when log level is
  raised.
- **Adblock Lists tab** — discovers per-profile component-updater lists,
  edits them with find-highlighted multi-line editing, undo/redo,
  diff view, and an Apply & Launch button that pins the edits and
  relaunches Brave against the same profile.
- **WSL aware** — when run from WSL2, Brave gets `--no-sandbox`,
  `--ozone-platform=x11`, and `--disable-gpu` automatically; GitHub
  links open in your *Windows* default browser via `explorer.exe <url>`
  instead of in an in-WSL X11 browser.

## Screenshot

(Tab 1, regression bracket detected, commits loaded inline:)

> _add a screenshot once you've run the app once; PNG drop into the_
> _repo and link from here._

## Building

```sh
# Linux / macOS native
cargo build --release

# Cross-compile to Windows from Linux/WSL (requires mingw-w64):
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/brave-regress.exe (~8 MB)
```

The release profile is tuned for size (`lto=fat`, `opt-level=z`,
`panic=abort`, `strip=symbols`). For faster iteration use the
`release-quick` profile (`thin LTO`, opt-level=3) which trades binary
size for build speed.

## Running

```sh
./brave-regress           # launches the GUI (default)
./brave-regress gui       # same
```

A small CLI surface exists for scripting:

```sh
./brave-regress versions available           # list Brave Nightlies on GitHub
./brave-regress versions installed
./brave-regress versions install v1.91.119
./brave-regress versions launch  v1.91.119 --profile default
./brave-regress mark version v1.91.119 good
./brave-regress prune --keep 6 --dry-run
./brave-regress lists show       <profile>
./brave-regress lists apply      <profile> --version v1.91.119
./brave-regress bisect versions  --good v1.91.114 --bad v1.91.119 --url https://example.com
```

Run `./brave-regress --help` for the full command tree.

## Where data lives

| Path | Purpose |
|---|---|
| `<data-root>/versions/<tag>/` | Extracted Brave install for each tag |
| `<data-root>/profiles/<name>/` | Default `--user-data-dir` per named profile |
| `<data-root>/cache/downloads/` | Downloaded installer artifacts |
| `<data-root>/cache/releases.json` | On-disk cache of the GitHub releases list |
| `<data-root>/db/verdicts.sqlite` | Per-tag verdicts, launch-args, custom user-data-dirs |
| `<data-root>/config.toml` | UI settings (theme, channels, default folder/args, GitHub token) |

`<data-root>` defaults to `dirs::data_local_dir()` joined with
`brave-regress`. Override with the `BRAVE_REGRESS_HOME` env var if you
want everything in a different location.

## Platform compatibility

| Platform | Status |
|---|---|
| Windows x86_64 | ✅ portable `.zip` install + launch |
| Windows ARM64 | ✅ uses `*arm64*.zip` when present |
| Linux x86_64 | ✅ portable `.zip` (no root) + `.deb` fallback (pure-Rust ar/tar/{zst,xz,gz}) |
| Linux aarch64 | ⚠ depends on the release — Brave's Nightlies often skip ARM64 Linux |
| macOS Apple Silicon | ✅ `*-darwin-arm64.zip` + `*-arm64.dmg` fallback (uses `hdiutil`) |
| macOS Intel | ✅ `*-darwin-x64.zip` + `*-x64.dmg` fallback |
| RPM-only distros | ❌ no RPM extractor — install Brave from your package manager and skip per-tag installs |

On macOS, post-extract the install runs `xattr -dr com.apple.quarantine`
so Gatekeeper doesn't block first launch.

## Configuration

GitHub's anonymous API limit is 60 req/hr. Set a personal access token
(no scopes needed) under **Settings → GitHub token** to bump the ceiling
to 5,000 req/hr. This dramatically helps when paginating large date
windows.

## Architecture

```
brave-regress
├── src/cli.rs              clap entrypoint
├── src/gui/                eframe / egui app
│   ├── app.rs              top-level eframe::App
│   ├── tab_versions.rs     Tab 1 — installs, available list, compare panel
│   ├── tab_lists.rs        Tab 2 — adblock list editor + apply
│   ├── list_editor.rs      multi-line editor (find/diff/undo)
│   ├── console_panel.rs    Tab 3 — log stream
│   └── state.rs            AppState + AsyncSlots
├── src/versions/
│   ├── github.rs           GitHub API: list + compare endpoints
│   ├── install.rs          download + extract (zip/deb/dmg)
│   ├── launch.rs           Command-builder for Brave (channel-agnostic)
│   └── retention.rs        prune policy
├── src/lists/              adblock list discovery + apply + pin
├── src/verdict/mod.rs      sqlite store: verdicts, launch_args, user_data_dir
└── src/paths.rs            data-root layout, channel-aware brave_binary()
```

## License

MIT (or your preferred — drop a `LICENSE` file in to make it explicit).

## Acknowledgements

- [Brave Software](https://brave.com/) for the browser and the public
  release stream.
- [egui / eframe](https://github.com/emilk/egui) for the GUI.
- [octocrab](https://github.com/XAMPPRocky/octocrab) +
  [reqwest](https://github.com/seanmonstar/reqwest) for GitHub API
  access.
