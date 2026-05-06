use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verdict {
    /// Tested and works.
    Good,
    /// Tested and broken — the regression target.
    Bad,
    /// Works but has visible bugs / glitches that aren't full breakage.
    Buggy,
    /// Tested briefly but the verdict isn't clear-cut yet.
    Unsure,
    /// Installed but not tested at all.
    Untested,
    /// No verdict ("Clear" in the GUI).
    Unknown,
}

impl Verdict {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "good"     | "g" => Self::Good,
            "bad"      | "b" => Self::Bad,
            "buggy"    | "u" => Self::Buggy,
            "unsure"   | "?" => Self::Unsure,
            "untested" | "n" => Self::Untested,
            "clear" | "unknown" | "" => Self::Unknown,
            other => return Err(anyhow!(
                "verdict must be good|bad|buggy|unsure|untested|clear, got {other}")),
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Good     => "good",
            Self::Bad      => "bad",
            Self::Buggy    => "buggy",
            Self::Unsure   => "unsure",
            Self::Untested => "untested",
            Self::Unknown  => "unknown",
        }
    }
    fn from_db(s: &str) -> Self {
        match s {
            "good"     => Self::Good,
            "bad"      => Self::Bad,
            "buggy"    => Self::Buggy,
            "unsure"   => Self::Unsure,
            "untested" => Self::Untested,
            _          => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionVerdict {
    pub tag:        String,
    pub verdict:    Verdict,
    pub note:       Option<String>,
    pub marked_at:  DateTime<Utc>,
    pub related_url: Option<String>,
}

/// Process-wide sqlite connection. The GUI calls accessors like
/// `version_verdict(tag)` and `note(tag)` per row, per frame — at 60fps
/// over ~70 rows that was ~8000 syscalls/sec opening fresh connections
/// (each one re-running every `CREATE TABLE IF NOT EXISTS`). Cache one
/// connection behind a Mutex and keep it open for the whole process.
static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

fn init_conn() -> Result<Connection> {
    paths::ensure_dirs()?;
    let path = paths::db_dir().join("verdicts.sqlite");
    let conn = Connection::open(path)?;
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS version_verdict (
            tag TEXT PRIMARY KEY, verdict TEXT NOT NULL, note TEXT,
            marked_at INTEGER NOT NULL, related_url TEXT
        );
        CREATE TABLE IF NOT EXISTS list_verdict (
            list TEXT NOT NULL, list_sha TEXT NOT NULL,
            verdict TEXT NOT NULL, note TEXT, marked_at INTEGER NOT NULL,
            PRIMARY KEY(list, list_sha)
        );
        CREATE TABLE IF NOT EXISTS cell_verdict (
            run_id TEXT NOT NULL, version TEXT NOT NULL,
            list_config TEXT NOT NULL, url TEXT NOT NULL,
            verdict TEXT NOT NULL, note TEXT, marked_at INTEGER NOT NULL,
            PRIMARY KEY(run_id, version, list_config, url)
        );
        CREATE TABLE IF NOT EXISTS launch_args (
            tag TEXT PRIMARY KEY,
            args TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS user_data_dir (
            tag TEXT PRIMARY KEY,
            path TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS notes (
            tag        TEXT PRIMARY KEY,
            body       TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tag_metadata (
            tag              TEXT PRIMARY KEY,
            chromium_version TEXT,
            published_at     TEXT,
            channel          TEXT
        );
        CREATE TABLE IF NOT EXISTS release_cache (
            tag  TEXT PRIMARY KEY,
            json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS manual_release_tags (
            tag TEXT PRIMARY KEY
        );
        CREATE TABLE IF NOT EXISTS launch_args_history (
            args      TEXT PRIMARY KEY,
            last_used TEXT NOT NULL
        );
    "#)?;
    Ok(conn)
}

/// Lock the cached sqlite connection. Schema setup runs the first time
/// only; every subsequent call is a single mutex acquisition. Returning
/// a `MutexGuard<Connection>` lets callers keep using the existing
/// `conn.query_row(…)` / `conn.execute(…)` shape unchanged.
fn open() -> Result<MutexGuard<'static, Connection>> {
    let cell = match DB.get() {
        Some(c) => c,
        None => {
            let conn = init_conn()?;
            // OnceLock::set returns Err if another thread won the race.
            // Either outcome is fine — `get_or_init`-style read after.
            let _ = DB.set(Mutex::new(conn));
            DB.get().expect("DB just initialised")
        }
    };
    cell.lock().map_err(|e| anyhow!("verdict db mutex poisoned: {e}"))
}

/// Read the per-version extra command-line args saved for `tag`.
/// Empty string when none configured.
pub fn launch_args(tag: &str) -> String {
    let conn = match open() { Ok(c) => c, Err(_) => return String::new() };
    conn.query_row(
        "SELECT args FROM launch_args WHERE tag=?1",
        params![tag], |r| r.get::<_, String>(0)
    ).unwrap_or_default()
}

/// Persist per-version extra args. Empty string clears the row.
pub fn set_launch_args(tag: &str, args: &str) -> Result<()> {
    let conn = open()?;
    if args.trim().is_empty() {
        conn.execute("DELETE FROM launch_args WHERE tag=?1", params![tag])?;
    } else {
        conn.execute(
            "INSERT INTO launch_args(tag, args) VALUES (?1, ?2)
             ON CONFLICT(tag) DO UPDATE SET args = excluded.args",
            params![tag, args])?;
    }
    Ok(())
}

/// Remember a non-empty args string in `launch_args_history` so it
/// shows up in the per-row dropdown for reuse on other tags. Same
/// string used twice just bumps `last_used` so the most-recently-
/// used entries float to the top of the dropdown.
pub fn add_launch_args_to_history(args: &str) -> Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() { return Ok(()); }
    let conn = open()?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO launch_args_history(args, last_used) VALUES (?1, ?2)
         ON CONFLICT(args) DO UPDATE SET last_used = excluded.last_used",
        params![trimmed, now])?;
    Ok(())
}

/// Most-recently-used `limit` distinct launch-arg strings. Used by
/// the GUI's per-row dropdown.
pub fn recent_launch_args(limit: usize) -> Result<Vec<String>> {
    let conn = open()?;
    let mut stmt = conn.prepare(
        "SELECT args FROM launch_args_history
         ORDER BY last_used DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows { out.push(r?); }
    Ok(out)
}

/// Drop a single args string from history (the per-row dropdown's
/// "Forget" item). No-op when not present.
pub fn forget_launch_args(args: &str) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "DELETE FROM launch_args_history WHERE args = ?1",
        params![args])?;
    Ok(())
}

/// Wipe every row from `launch_args_history` — used by the Settings
/// panel's "Clear args history" button. Returns the count of rows
/// removed so the GUI can show a status summary. Per-tag launch_args
/// (the values typed into each Installed row) are NOT touched.
pub fn clear_launch_args_history() -> Result<usize> {
    let conn = open()?;
    Ok(conn.execute("DELETE FROM launch_args_history", [])?)
}

/// Split a saved args string into a Vec<String>, treating it as a shell-ish
/// whitespace-separated list (no quoting parser yet — keep it simple).
pub fn parse_launch_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

/// Read the per-tag override for `--user-data-dir`. Empty string means the
/// app's standard profile dir is used.
pub fn user_data_dir(tag: &str) -> String {
    let conn = match open() { Ok(c) => c, Err(_) => return String::new() };
    conn.query_row(
        "SELECT path FROM user_data_dir WHERE tag=?1",
        params![tag], |r| r.get::<_, String>(0)
    ).unwrap_or_default()
}

/// Persist a per-tag custom `--user-data-dir`. Empty / whitespace clears it.
pub fn set_user_data_dir(tag: &str, path: &str) -> Result<()> {
    let conn = open()?;
    if path.trim().is_empty() {
        conn.execute("DELETE FROM user_data_dir WHERE tag=?1", params![tag])?;
    } else {
        conn.execute(
            "INSERT INTO user_data_dir(tag, path) VALUES (?1, ?2)
             ON CONFLICT(tag) DO UPDATE SET path = excluded.path",
            params![tag, path])?;
    }
    Ok(())
}

/// Read the freeform note attached to a tag. Empty string means none.
pub fn note(tag: &str) -> String {
    let conn = match open() { Ok(c) => c, Err(_) => return String::new() };
    conn.query_row(
        "SELECT body FROM notes WHERE tag=?1",
        params![tag], |r| r.get::<_, String>(0)
    ).unwrap_or_default()
}

/// Persist a freeform note for a tag. Empty / whitespace clears it.
pub fn set_note(tag: &str, body: &str) -> Result<()> {
    let conn = open()?;
    if body.trim().is_empty() {
        conn.execute("DELETE FROM notes WHERE tag=?1", params![tag])?;
    } else {
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO notes(tag, body, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(tag) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![tag, body, now])?;
    }
    Ok(())
}

pub fn mark(kind: &str, target: &str, verdict: &str, note: Option<&str>) -> Result<()> {
    let v = Verdict::parse(verdict)?;
    let now = Utc::now().timestamp();
    let conn = open()?;
    match kind {
        "version" => {
            conn.execute(
                "INSERT INTO version_verdict(tag,verdict,note,marked_at,related_url)
                 VALUES (?1,?2,?3,?4,NULL)
                 ON CONFLICT(tag) DO UPDATE SET verdict=excluded.verdict, note=excluded.note, marked_at=excluded.marked_at",
                params![target, v.as_str(), note, now])?;
        }
        "list" => {
            // target = "<name>@<sha>"
            let (name, sha) = target.split_once('@').ok_or_else(|| anyhow!("list target must be name@sha"))?;
            conn.execute(
                "INSERT INTO list_verdict(list,list_sha,verdict,note,marked_at)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(list,list_sha) DO UPDATE SET verdict=excluded.verdict, note=excluded.note, marked_at=excluded.marked_at",
                params![name, sha, v.as_str(), note, now])?;
        }
        "cell" => {
            // target = "<run>:<version>:<list_config>:<url>"
            let parts: Vec<&str> = target.splitn(4, ':').collect();
            if parts.len() != 4 { return Err(anyhow!("cell target must be run:version:list_config:url")); }
            conn.execute(
                "INSERT INTO cell_verdict(run_id,version,list_config,url,verdict,note,marked_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(run_id,version,list_config,url) DO UPDATE SET verdict=excluded.verdict, note=excluded.note, marked_at=excluded.marked_at",
                params![parts[0], parts[1], parts[2], parts[3], v.as_str(), note, now])?;
        }
        other => return Err(anyhow!("unknown verdict kind: {other}")),
    }
    println!("marked {kind} {target} = {}", v.as_str());
    Ok(())
}

pub fn list_version_verdicts() -> Result<Vec<VersionVerdict>> {
    let conn = open()?;
    let mut stmt = conn.prepare("SELECT tag,verdict,note,marked_at,related_url FROM version_verdict ORDER BY tag")?;
    let rows = stmt.query_map([], |r| {
        let v: String = r.get(1)?;
        Ok(VersionVerdict {
            tag: r.get(0)?,
            verdict: Verdict::from_db(&v),
            note: r.get(2)?,
            marked_at: DateTime::from_timestamp(r.get::<_, i64>(3)?, 0).unwrap_or_else(Utc::now),
            related_url: r.get(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Wipe every row from `version_verdict` whose tag is NOT in the
/// supplied list. Used by the GUI's Clear → Verdicts menu so the
/// verdicts attached to currently-installed tags are preserved
/// (those are the ones the user is actively bisecting); only stale
/// verdicts for tags they've since uninstalled get dropped. When
/// `keep_tags` is empty this is equivalent to clearing all rows.
pub fn clear_uninstalled_version_verdicts(keep_tags: &[String]) -> Result<usize> {
    let conn = open()?;
    if keep_tags.is_empty() {
        return Ok(conn.execute("DELETE FROM version_verdict", [])?);
    }
    // Build a parameterised IN-list — placeholders + boxed params,
    // so SQLite doesn't have to re-parse the query for each tag.
    let placeholders = vec!["?"; keep_tags.len()].join(",");
    let sql = format!(
        "DELETE FROM version_verdict WHERE tag NOT IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> =
        keep_tags.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let n = conn.execute(&sql, params.as_slice())?;
    Ok(n)
}

/// Wipe every row from `notes` — used by the GUI's Clear → Comments
/// menu. Returns the deleted-row count.
pub fn clear_all_notes() -> Result<usize> {
    let conn = open()?;
    let n = conn.execute("DELETE FROM notes", [])?;
    Ok(n)
}

pub fn version_verdict(tag: &str) -> Result<Verdict> {
    let conn = open()?;
    let v: Option<String> = conn.query_row(
        "SELECT verdict FROM version_verdict WHERE tag=?1",
        params![tag], |r| r.get(0)).ok();
    Ok(v.as_deref().map(Verdict::from_db).unwrap_or(Verdict::Unknown))
}

/// Bulk-load every (tag, verdict) pair in one sqlite query — used by
/// the Available list render so we don't pay an O(n) per-row sqlite
/// hit (and an O(n log n) sort comparator hit) every frame.
pub fn all_version_verdicts() -> std::collections::HashMap<String, Verdict> {
    let conn = match open() { Ok(c) => c, Err(_) => return Default::default() };
    let mut out = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT tag, verdict FROM version_verdict") {
        if let Ok(rows) = stmt.query_map([], |r| {
            let tag: String = r.get(0)?;
            let v: String = r.get(1)?;
            Ok((tag, Verdict::from_db(&v)))
        }) {
            out.extend(rows.filter_map(|r| r.ok()));
        }
    }
    out
}

/// Bulk-load every (tag, note body) pair. Same motivation as
/// `all_version_verdicts` — collapses N per-row reads + N comparator
/// reads into one query per frame.
pub fn all_notes() -> std::collections::HashMap<String, String> {
    let conn = match open() { Ok(c) => c, Err(_) => return Default::default() };
    let mut out = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT tag, body FROM notes") {
        if let Ok(rows) = stmt.query_map([], |r| {
            let tag: String = r.get(0)?;
            let body: String = r.get(1)?;
            Ok((tag, body))
        }) {
            out.extend(rows.filter_map(|r| r.ok()));
        }
    }
    out
}

/// Persist a per-tag (chromium_version, published_at, channel) so the
/// GUI can fall back to it when a tag isn't in the currently-fetched
/// available list (e.g. an older installed tag, or a tag from before
/// the active filter window).
pub fn upsert_tag_metadata(
    tag: &str,
    chromium_version: Option<&str>,
    published_at: Option<&str>,
    channel: Option<&str>,
) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO tag_metadata(tag, chromium_version, published_at, channel)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(tag) DO UPDATE SET
             chromium_version = COALESCE(excluded.chromium_version, tag_metadata.chromium_version),
             published_at     = COALESCE(excluded.published_at,     tag_metadata.published_at),
             channel          = COALESCE(excluded.channel,          tag_metadata.channel)",
        params![tag, chromium_version, published_at, channel])?;
    Ok(())
}

/// Persist a release row as a JSON blob keyed by tag. Used by the
/// incremental release-cache mode so every release we've ever seen is
/// remembered across sessions and an "early 2024" date filter doesn't
/// re-paginate through 2025/2026 each time.
pub fn upsert_release_cache_row(tag: &str, json: &str) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO release_cache(tag, json) VALUES (?1, ?2)
         ON CONFLICT(tag) DO UPDATE SET json = excluded.json",
        params![tag, json])?;
    Ok(())
}

/// Read every release JSON blob, newest tag last (lexicographic — the
/// caller re-sorts). Empty Vec when the table is empty / unreadable.
pub fn all_release_cache_rows() -> Vec<String> {
    let conn = match open() { Ok(c) => c, Err(_) => return Vec::new() };
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT json FROM release_cache") {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            out.extend(rows.filter_map(|r| r.ok()));
        }
    }
    out
}

/// Mark a tag as user-added (via the "Add release by tag" UI). Used to
/// exempt the row from the channel display filter — when the user
/// explicitly pulls a Release/Beta tag they expect to see it even if
/// only Nightly is ticked.
pub fn mark_manual_release(tag: &str) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO manual_release_tags(tag) VALUES (?1) ON CONFLICT(tag) DO NOTHING",
        params![tag])?;
    Ok(())
}

/// Remove a tag from both `manual_release_tags` and `release_cache` —
/// used when the user clicks Remove on a manually added row. We delete
/// from both tables so the row disappears entirely from state.available
/// on the next render, not just loses its channel-filter exemption.
pub fn unmark_manual_release(tag: &str) -> Result<()> {
    let conn = open()?;
    conn.execute("DELETE FROM manual_release_tags WHERE tag=?1", params![tag])?;
    conn.execute("DELETE FROM release_cache       WHERE tag=?1", params![tag])?;
    Ok(())
}

/// Set of tags the user has explicitly added via the manual Add-by-tag
/// flow. Loaded at startup; live-updated when Add succeeds.
pub fn manual_release_tags() -> std::collections::HashSet<String> {
    let conn = match open() { Ok(c) => c, Err(_) => return Default::default() };
    let mut out = std::collections::HashSet::new();
    if let Ok(mut stmt) = conn.prepare("SELECT tag FROM manual_release_tags") {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            out.extend(rows.filter_map(|r| r.ok()));
        }
    }
    out
}

/// Set of every tag currently stored in `release_cache`. Used by the
/// fetcher's incremental mode to break out of pagination as soon as it
/// re-encounters a tag we already know about.
pub fn known_release_cache_tags() -> std::collections::HashSet<String> {
    let conn = match open() { Ok(c) => c, Err(_) => return Default::default() };
    let mut out = std::collections::HashSet::new();
    if let Ok(mut stmt) = conn.prepare("SELECT tag FROM release_cache") {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            out.extend(rows.filter_map(|r| r.ok()));
        }
    }
    out
}

/// Read the persisted (chromium_version, published_at, channel) for a tag.
/// All three are Option — any subset may be missing.
pub fn tag_metadata(tag: &str) -> (Option<String>, Option<String>, Option<String>) {
    let conn = match open() { Ok(c) => c, Err(_) => return (None, None, None) };
    conn.query_row(
        "SELECT chromium_version, published_at, channel FROM tag_metadata WHERE tag=?1",
        params![tag],
        |r| Ok((r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?)))
        .unwrap_or((None, None, None))
}
