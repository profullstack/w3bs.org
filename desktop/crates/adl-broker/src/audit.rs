//! Local audit metadata (PRD section 12): who, what, which decision, which backend, what outcome.
//! Never screenshots, typed text, clipboard contents or message bodies. One JSONL file per UTC
//! day; files older than the retention window are deleted.

use crate::util::now_ms;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AuditRecord {
    pub ts_ms: u64,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub decision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    /// Structured metadata. Callers must not put content here; see `Action::kind`.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

pub struct Audit {
    dir: Option<PathBuf>,
    retention_days: u64,
    memory: Mutex<Vec<AuditRecord>>,
    last_prune_day: Mutex<u64>,
}

const DAY_MS: u64 = 86_400_000;

impl Audit {
    pub fn memory() -> Self {
        Self { dir: None, retention_days: 7, memory: Mutex::new(vec![]), last_prune_day: Mutex::new(0) }
    }

    pub fn open(dir: PathBuf, retention_days: u64) -> Self {
        let a = Self { dir: Some(dir), retention_days: retention_days.max(1), memory: Mutex::new(vec![]), last_prune_day: Mutex::new(0) };
        a.prune();
        a
    }

    fn file_for(&self, day: u64) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("audit-{day}.jsonl")))
    }

    pub fn record(&self, r: AuditRecord) {
        let day = r.ts_ms / DAY_MS;
        if let Some(path) = self.file_for(day) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = crate::util::private_file().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{}", serde_json::to_string(&r).unwrap());
            }
            let mut last = self.last_prune_day.lock().unwrap_or_else(|e| e.into_inner());
            if *last != day {
                *last = day;
                drop(last);
                self.prune();
            }
        } else {
            self.memory.lock().unwrap_or_else(|e| e.into_inner()).push(r);
        }
    }

    pub fn log(&self, op: &str, agent: Option<&str>, task: Option<&str>, decision: &str, outcome: &str, meta: serde_json::Value) {
        self.record(AuditRecord {
            ts_ms: now_ms(), op: op.into(), agent: agent.map(Into::into), task: task.map(Into::into), decision: decision.into(),
            backend: String::new(), outcome: outcome.into(), meta,
        });
    }

    fn day_files(&self) -> Vec<(u64, PathBuf)> {
        let Some(dir) = &self.dir else { return vec![] };
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut out: Vec<_> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let day = name.strip_prefix("audit-")?.strip_suffix(".jsonl")?.parse().ok()?;
                Some((day, e.path()))
            })
            .collect();
        out.sort();
        out
    }

    pub fn prune(&self) {
        let today = now_ms() / DAY_MS;
        for (day, path) in self.day_files() {
            if day + self.retention_days <= today {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    pub fn export(&self) -> Vec<AuditRecord> {
        if self.dir.is_none() {
            return self.memory.lock().unwrap_or_else(|e| e.into_inner()).clone();
        }
        let mut out = vec![];
        for (_, path) in self.day_files() {
            if let Ok(s) = std::fs::read_to_string(path) {
                out.extend(s.lines().filter_map(|l| serde_json::from_str(l).ok()));
            }
        }
        out
    }

    pub fn delete_all(&self) -> usize {
        if self.dir.is_none() {
            return std::mem::take(&mut *self.memory.lock().unwrap_or_else(|e| e.into_inner())).len();
        }
        self.day_files().into_iter().filter(|(_, p)| std::fs::remove_file(p).is_ok()).count()
    }
}
