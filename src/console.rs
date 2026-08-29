//! Shared console-log buffer that the GUI side panel renders.
//!
//! Anything we want the user to see at runtime — install failures, launch
//! failures, raw Brave stderr lines, status events — gets pushed here and
//! the right-side panel shows the most recent N entries.

use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level { Info, Warn, Error, Brave }

#[derive(Debug, Clone)]
pub struct Entry {
    pub ts:     DateTime<Utc>,
    pub level:  Level,
    pub source: String,   // "install", "brave/v1.91.118", "github", …
    pub msg:    String,
}

pub struct ConsoleLog {
    entries:  VecDeque<Entry>,
    capacity: usize,
    /// Widest `source + msg` ever pushed, in chars, for the panel's
    /// horizontal scroll extent. That extent has to be stable across
    /// frames: `ScrollArea::show_rows` only paints the visible subset,
    /// so if those rows defined the content width it would collapse
    /// whenever the viewport sat on short lines — egui clamps the
    /// scroll offset to `content - viewport` every frame, snapping the
    /// user back to column 0 mid-read. Grows only; a ring eviction can
    /// leave it wider than the widest surviving line, which costs a
    /// little dead scroll range and nothing else.
    max_line_chars: usize,
}

impl ConsoleLog {
    pub fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity), capacity,
               max_line_chars: 0 }
    }
    /// The panel renders one entry per `ScrollArea::show_rows` row and
    /// budgets exactly one text-line of height for it. egui breaks on an
    /// explicit `\n` whatever the wrap mode, so a multi-line message
    /// would paint taller than its budget and shove every row after it
    /// out of alignment — the same defect as wrapping, arriving by a
    /// different route. Several failure paths do append a second line
    /// (`format!("launch failed: {e}\nhint: {h}")` and friends), so
    /// split at the door: one entry per line, sharing the timestamp.
    pub fn push(&mut self, e: Entry) {
        if !e.msg.contains('\n') { return self.push_line(e); }
        let Entry { ts, level, source, msg } = e;
        for line in msg.split('\n') {
            self.push_line(Entry {
                ts, level,
                source: source.clone(),
                msg: line.to_string(),
            });
        }
    }

    /// Ring insert for one already-single-line entry.
    fn push_line(&mut self, e: Entry) {
        if self.entries.len() == self.capacity { self.entries.pop_front(); }
        self.max_line_chars = self.max_line_chars
            .max(e.source.chars().count() + e.msg.chars().count());
        self.entries.push_back(e);
    }
    pub fn entries(&self) -> impl Iterator<Item = &Entry> { self.entries.iter() }
    pub fn len(&self)   -> usize { self.entries.len() }
    pub fn clear(&mut self)      { self.entries.clear(); self.max_line_chars = 0; }
    /// Widest `source + msg` seen since the last `clear()`, in chars.
    /// The panel adds its own fixed prefix width on top.
    pub fn max_line_chars(&self) -> usize { self.max_line_chars }
    /// O(1) index access by oldest-first position. Used by the
    /// Console panel's viewport-rendered ScrollArea so we can
    /// paint only the on-screen rows instead of laying out every
    /// entry per frame. Returns None for out-of-range indices.
    pub fn get(&self, idx: usize) -> Option<&Entry> { self.entries.get(idx) }
}

pub type Handle = Arc<Mutex<ConsoleLog>>;

pub fn new_handle() -> Handle {
    Arc::new(Mutex::new(ConsoleLog::new(1000)))
}

fn push(h: &Handle, level: Level, source: impl Into<String>, msg: impl Into<String>) {
    let entry = Entry { ts: Utc::now(), level, source: source.into(), msg: msg.into() };
    if let Ok(mut g) = h.lock() { g.push(entry); }
}

pub fn info (h: &Handle, source: &str, msg: impl Into<String>) { push(h, Level::Info,  source, msg); }
pub fn warn (h: &Handle, source: &str, msg: impl Into<String>) { push(h, Level::Warn,  source, msg); }
pub fn error(h: &Handle, source: &str, msg: impl Into<String>) { push(h, Level::Error, source, msg); }
pub fn brave(h: &Handle, source: &str, msg: impl Into<String>) { push(h, Level::Brave, source, msg); }
