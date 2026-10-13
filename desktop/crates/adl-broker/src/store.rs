//! Registered agents and their grants. Tokens are shown once at registration; only their
//! SHA-256 is stored. The file is written atomically with mode 0600.

use crate::util::{ct_eq, now_ms, random_id, sha256_hex};
use adl_schema::{AdlError, AgentInfo, ErrorCode, Policy, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    #[serde(flatten)]
    info: AgentInfo,
    token_sha256: String,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    v: u32,
    agents: Vec<Record>,
}

pub struct AgentStore {
    path: Option<PathBuf>,
    file: Mutex<File>,
}

impl AgentStore {
    /// In-memory store (tests).
    pub fn memory() -> Self {
        Self { path: None, file: Mutex::new(File { v: 1, agents: vec![] }) }
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        let file = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| AdlError::internal(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File { v: 1, agents: vec![] },
            Err(e) => return Err(AdlError::internal(format!("{}: {e}", path.display()))),
        };
        Ok(Self { path: Some(path), file: Mutex::new(file) })
    }

    fn save(&self, f: &File) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let io = |e: std::io::Error| AdlError::internal(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut out = crate::util::private_file().write(true).create(true).truncate(true).open(&tmp).map_err(io)?;
            out.write_all(&serde_json::to_vec_pretty(f).unwrap()).map_err(io)?;
            out.sync_all().map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.file.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register an agent. Returns its info and the bearer token, which is never stored.
    pub fn add(&self, name: &str, policy: Policy) -> Result<(AgentInfo, String)> {
        if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
            return Err(AdlError::invalid("agent name must be 1-64 characters of [A-Za-z0-9._-]"));
        }
        let mut f = self.lock();
        if f.agents.iter().any(|r| r.info.name == name && !r.info.revoked) {
            return Err(AdlError::invalid(format!("an active agent named {name} already exists")));
        }
        let token = random_id("adl", 32);
        let info = AgentInfo { id: random_id("agent", 8), name: name.into(), policy, created_at_ms: now_ms(), revoked: false };
        f.agents.push(Record { info: info.clone(), token_sha256: sha256_hex(token.as_bytes()) });
        self.save(&f)?;
        Ok((info, token))
    }

    pub fn authenticate(&self, token: &str) -> Result<AgentInfo> {
        let digest = sha256_hex(token.as_bytes());
        let f = self.lock();
        let r = f.agents.iter().find(|r| ct_eq(r.token_sha256.as_bytes(), digest.as_bytes()));
        match r {
            Some(r) if !r.info.revoked => Ok(r.info.clone()),
            Some(_) => Err(AdlError::new(ErrorCode::Unauthenticated, "agent has been revoked")),
            None => Err(AdlError::new(ErrorCode::Unauthenticated, "unknown agent token")),
        }
    }

    pub fn get(&self, id: &str) -> Option<AgentInfo> {
        self.lock().agents.iter().find(|r| r.info.id == id).map(|r| r.info.clone())
    }

    /// Look up by id, or by name among active agents.
    pub fn resolve(&self, id_or_name: &str) -> Result<AgentInfo> {
        let f = self.lock();
        f.agents
            .iter()
            .find(|r| r.info.id == id_or_name)
            .or_else(|| f.agents.iter().find(|r| r.info.name == id_or_name && !r.info.revoked))
            .map(|r| r.info.clone())
            .ok_or_else(|| AdlError::not_found(format!("no agent {id_or_name}")))
    }

    pub fn list(&self) -> Vec<AgentInfo> {
        self.lock().agents.iter().map(|r| r.info.clone()).collect()
    }

    pub fn update_policy(&self, id: &str, policy: Policy) -> Result<AgentInfo> {
        let mut f = self.lock();
        let r = f.agents.iter_mut().find(|r| r.info.id == id).ok_or_else(|| AdlError::not_found(format!("no agent {id}")))?;
        r.info.policy = policy;
        let info = r.info.clone();
        self.save(&f)?;
        Ok(info)
    }

    pub fn revoke(&self, id: &str) -> Result<()> {
        let mut f = self.lock();
        let r = f.agents.iter_mut().find(|r| r.info.id == id).ok_or_else(|| AdlError::not_found(format!("no agent {id}")))?;
        r.info.revoked = true;
        r.info.policy = Policy::default();
        self.save(&f)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut f = self.lock();
        let before = f.agents.len();
        f.agents.retain(|r| r.info.id != id);
        if f.agents.len() == before {
            return Err(AdlError::not_found(format!("no agent {id}")));
        }
        self.save(&f)
    }

    /// Uninstall: every grant becomes invalid.
    pub fn revoke_all(&self) -> Result<()> {
        let mut f = self.lock();
        for r in &mut f.agents {
            r.info.revoked = true;
            r.info.policy = Policy::default();
        }
        self.save(&f)
    }
}
