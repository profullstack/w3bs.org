//! The shared engine. The CLI, the curl bootstrap and any future surface call these
//! operations; none of them have a private install path.

mod activate;
mod lifecycle;
mod maint;
mod repos;

pub use lifecycle::{InstallRequest, UpdateRequest};

use crate::db;
use crate::error::{Error, Result};
use crate::layout::Layout;
use crate::lock::{MutationLock, timeout_from_env};
use crate::repo::{self, LoadedRepo};
use rusqlite::Connection;
use std::collections::HashMap;
use std::rc::Rc;
use w3bs_schema::result::OperationResult;

/// Interaction with whoever is running the operation. The CLI reads `/dev/tty`, never stdin,
/// so a piped installer script cannot answer its own questions.
pub trait Interact {
    /// Ask a yes/no question. `None` when no terminal is available.
    fn confirm(&self, prompt: &str) -> Option<bool>;
    /// Human-readable progress (stderr). Silent in quiet mode.
    fn progress(&self, msg: &str);
}

/// A non-interactive implementation (tests, agents).
pub struct NoTerminal;
impl Interact for NoTerminal {
    fn confirm(&self, _: &str) -> Option<bool> {
        None
    }
    fn progress(&self, _: &str) {}
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    pub non_interactive: bool,
}

pub struct Engine {
    pub layout: Layout,
    pub opts: Options,
    io: Box<dyn Interact>,
    conn: Option<Connection>,
    _lock: Option<MutationLock>,
    repos: HashMap<String, std::result::Result<Rc<LoadedRepo>, Error>>,
}

pub const MANAGER_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Engine {
    pub fn new(layout: Layout, opts: Options, io: Box<dyn Interact>) -> Self {
        Self { layout, opts, io, conn: None, _lock: None, repos: HashMap::new() }
    }

    /// Open state for reading. Never creates directories or recovers journals.
    fn read_state(&mut self) -> Result<Option<&Connection>> {
        if self.conn.is_none() {
            self.conn = db::open_existing(&self.layout.db_path())?;
        }
        Ok(self.conn.as_ref())
    }

    /// Prepare for mutation: create the store, take the lock, reconcile journals.
    fn begin_mutation(&mut self) -> Result<()> {
        if self._lock.is_some() {
            return Ok(());
        }
        self.layout.ensure()?;
        self._lock = Some(MutationLock::acquire(&self.layout.lock_path(), timeout_from_env())?);
        self.conn = Some(db::open(&self.layout.db_path())?);
        let recovered = activate::recover_all(self)?;
        for r in recovered {
            self.io.progress(&format!("recovered interrupted operation: {r}"));
        }
        Ok(())
    }

    /// Open state for a dry run: read-only, but refuse when unresolved journals exist so the
    /// plan is never computed from a half-applied state.
    fn begin_read(&mut self) -> Result<()> {
        if !self.layout.db_path().exists() {
            return Ok(());
        }
        self.read_state()?;
        let pending = activate::pending_journals(&self.layout)?;
        if !pending.is_empty() {
            return Err(Error::preflight(
                "RECOVERY_REQUIRED",
                format!("{} interrupted operation(s) need recovery; run `w3bs repair`", pending.len()),
            ));
        }
        Ok(())
    }

    fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("state opened")
    }

    /// Approve an ordinary planned operation for established trust.
    fn approve(&self, prompt: &str) -> Result<()> {
        if self.opts.yes {
            return Ok(());
        }
        if self.opts.non_interactive {
            return Err(Error::approval("APPROVAL_REQUIRED", "approval required: re-run with --yes"));
        }
        match self.io.confirm(prompt) {
            Some(true) => Ok(()),
            Some(false) => Err(Error::approval("DECLINED", "declined; nothing was changed")),
            None => Err(Error::approval("APPROVAL_REQUIRED", "no terminal to ask for approval: re-run with --yes")),
        }
    }

    /// Approve a trust decision. `--yes` alone never accepts an unknown root: it only
    /// confirms a root the operator pre-provisioned (`--root` file or `--root-fingerprint`).
    fn approve_trust(&self, pre_provisioned: bool, prompt: &str) -> Result<()> {
        if pre_provisioned && self.opts.yes {
            return Ok(());
        }
        if self.opts.non_interactive || self.opts.yes {
            return Err(Error::approval(
                "TRUST_DECISION_REQUIRED",
                "a new repository trust root needs an interactive decision, or a pre-provisioned --root / --root-fingerprint together with --yes",
            ));
        }
        match self.io.confirm(prompt) {
            Some(true) => Ok(()),
            Some(false) => Err(Error::approval("DECLINED", "trust declined; nothing was changed")),
            None => Err(Error::approval("TRUST_DECISION_REQUIRED", "no terminal for the first-use trust decision; provide --root or --root-fingerprint with --yes")),
        }
    }

    fn progress(&self, m: &str) {
        self.io.progress(m);
    }

    /// Load (once per run) and TUF-verify a configured repository.
    async fn repo(&mut self, alias: &str) -> Result<Rc<LoadedRepo>> {
        if let Some(r) = self.repos.get(alias) {
            return r.clone();
        }
        let row = match self.read_state()? {
            Some(c) => db::repo(c, alias)?,
            None => None,
        }
        .ok_or_else(|| Error::usage("UNKNOWN_REPOSITORY", format!("no repository named `{alias}`; see `w3bs repo list`")))?;
        self.progress(&format!("refreshing {alias} ({})", row.base_url));
        let r = repo::load(&self.layout, alias, &row.base_url, &row.repo_key).await.map(Rc::new);
        self.repos.insert(alias.to_owned(), r.clone());
        r
    }

    fn repo_aliases(&mut self) -> Result<Vec<String>> {
        Ok(match self.read_state()? {
            Some(c) => db::repos(c)?.into_iter().map(|r| r.alias).collect(),
            None => vec![],
        })
    }
}

pub fn txn_id() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("txn-{ms}-{}", std::process::id())
}

/// Turn an engine error into a complete result object.
pub fn failed(operation: &str, e: &Error) -> OperationResult {
    let mut r = OperationResult::new(operation);
    r.result = e.overall();
    r.error = Some(e.info());
    r
}

/// Test hook: crash at a named durable checkpoint (`W3BS_TEST_CRASH_AT=<name>`).
pub(crate) fn checkpoint(name: &str) {
    if std::env::var("W3BS_TEST_CRASH_AT").is_ok_and(|v| v == name) {
        eprintln!("w3bs: simulated crash at checkpoint {name}");
        std::process::exit(99);
    }
}
