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
}

impl ConsoleLog {
    pub fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity), capacity }
    }
    pub fn push(&mut self, e: Entry) {
        if self.entries.len() == self.capacity { self.entries.pop_front(); }
        self.entries.push_back(e);
    }
    pub fn entries(&self) -> impl Iterator<Item = &Entry> { self.entries.iter() }
    pub fn len(&self)   -> usize { self.entries.len() }
    pub fn clear(&mut self)      { self.entries.clear(); }
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
