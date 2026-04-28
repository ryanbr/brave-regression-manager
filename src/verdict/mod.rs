use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verdict { Good, Bad, Unknown }

impl Verdict {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "good" | "g" => Self::Good,
            "bad"  | "b" => Self::Bad,
            "clear" | "unknown" | "?" | "" => Self::Unknown,
            other => return Err(anyhow!("verdict must be good|bad|clear, got {other}")),
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self { Self::Good => "good", Self::Bad => "bad", Self::Unknown => "unknown" }
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

fn open() -> Result<Connection> {
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
    "#)?;
    Ok(conn)
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
            verdict: match v.as_str() {
                "good" => Verdict::Good, "bad" => Verdict::Bad, _ => Verdict::Unknown,
            },
            note: r.get(2)?,
            marked_at: DateTime::from_timestamp(r.get::<_, i64>(3)?, 0).unwrap_or_else(Utc::now),
            related_url: r.get(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn version_verdict(tag: &str) -> Result<Verdict> {
    let conn = open()?;
    let v: Option<String> = conn.query_row(
        "SELECT verdict FROM version_verdict WHERE tag=?1",
        params![tag], |r| r.get(0)).ok();
    Ok(match v.as_deref() {
        Some("good") => Verdict::Good,
        Some("bad")  => Verdict::Bad,
        _            => Verdict::Unknown,
    })
}
