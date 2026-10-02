// ABOUTME: Durable transfer records, project pointers, and process locks.
// ABOUTME: The session list is derived from records; it has no separate mutable index.

use crate::git::Snap;
use crate::sandbox::Sandbox;
use crate::util::{atomic_write, private_dir, sha256_bytes};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Planned,
    Allocating,
    Created,
    Prepared,
    Uploaded,
    Restored,
    Starting,
    #[default]
    Remote,
    Returning,
    Downloaded,
    Applied,
    Retained,
    Closed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Planned
            | Self::Allocating
            | Self::Created
            | Self::Prepared
            | Self::Uploaded
            | Self::Restored
            | Self::Starting => "preparing",
            Self::Remote => "remote",
            Self::Returning | Self::Downloaded => "returning",
            Self::Applied => "returned; cleanup pending",
            Self::Retained => "returned; sandbox retained",
            Self::Closed => "local",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    #[serde(default)]
    pub transfer_id: String,
    pub session_id: String,
    pub agent: String,
    pub project_root: PathBuf,
    pub agent_cwd: PathBuf,
    pub home: PathBuf,
    pub target: String,
    pub sandbox: Option<Sandbox>,
    pub stage: String,
    #[serde(default)]
    pub remote_home: String,
    pub tmux: String,
    pub sent: Snap,
    pub sent_files: BTreeMap<String, String>,
    #[serde(default)]
    pub sent_extras: BTreeMap<String, String>,
    #[serde(default)]
    pub return_extras: Vec<String>,
    #[serde(default)]
    pub env_names: Vec<String>,
    /// GitHub authentication was forwarded. Upload retry resolves its token again.
    #[serde(default)]
    pub github_auth: bool,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub timeout_secs: u64,
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub recovery: Option<PathBuf>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at: u64,
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join(".beam/state.json")
}

/// An OS lock is released even if the process crashes. No stale lock deletion is needed.
pub struct ProjectLock {
    _file: std::fs::File,
}
impl ProjectLock {
    pub fn acquire(home: &Path, root: &Path) -> Result<Self> {
        let dir = home.join(".beam/locks");
        private_dir(&dir)?;
        let key = sha256_bytes(root.as_os_str().as_encoded_bytes());
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(key))?;
        file.try_lock().map_err(|_| {
            anyhow::anyhow!(
                "another beam command is changing {}. Wait for it to finish",
                root.display()
            )
        })?;
        Ok(Self { _file: file })
    }
}

/// Unlock explicitly. A child process that forks while the lock is held shares the open file until
/// its exec, so closing this file alone could leave the lock held after drop.
impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

impl State {
    pub fn load(root: &Path) -> Result<Option<Self>> {
        let p = state_path(root);
        let text = match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Recover an interrupted pointer write from the authoritative records.
                if let Ok(home) = crate::up::home_dir() {
                    let mut states = records(&home)?;
                    states.retain(|s| s.project_root == root && s.phase != Phase::Closed);
                    states.sort_by_key(|s| s.created_at);
                    return Ok(states.pop());
                }
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let v: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("bad {}", p.display()))?;
        if let Some(record) = v.get("record").and_then(|v| v.as_str()) {
            let st = Self::read_record(Path::new(record))?;
            if st.project_root != root {
                bail!("transfer record belongs to a different project");
            }
            return Ok((st.phase != Phase::Closed).then_some(st));
        }
        // Migrate v1 records on the next save. Legacy SSH cleanup is deliberately disabled.
        let mut st: Self = serde_json::from_value(v)?;
        if st.version != 1 {
            bail!("unsupported beam state version {}", st.version);
        }
        st.version = 2;
        st.transfer_id = format!("legacy-{}", st.session_id);
        st.remote_home = st.home.to_string_lossy().into_owned();
        Ok(Some(st))
    }

    fn read_record(path: &Path) -> Result<Self> {
        let st: Self = serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("bad transfer record {}", path.display()))?;
        if st.version != 2 {
            bail!("unsupported beam state version {}", st.version);
        }
        Ok(st)
    }

    pub fn has_unresolved_recovery(&self) -> bool {
        !self.conflicts.is_empty() && !self.dir().join("resolved.json").exists()
    }

    pub fn dir(&self) -> PathBuf {
        self.home.join(".beam/transfers").join(&self.transfer_id)
    }
    pub fn sandbox(&self) -> Result<&Sandbox> {
        self.sandbox
            .as_ref()
            .context("sandbox has not been created. Run `beam` to continue")
    }
    pub fn describe(&self) -> String {
        self.sandbox
            .as_ref()
            .map(Sandbox::describe)
            .unwrap_or_else(|| self.target.clone())
    }

    pub fn save(&self) -> Result<()> {
        let record = self.dir().join("state.json");
        atomic_write(&record, &serde_json::to_vec_pretty(self)?)?;
        if self.phase != Phase::Closed {
            atomic_write(
                &state_path(&self.project_root),
                &serde_json::to_vec_pretty(&serde_json::json!({"version": 2, "record": record}))?,
            )?;
        }
        Ok(())
    }

    pub fn advance(&mut self, phase: Phase) -> Result<()> {
        let previous = self.phase;
        self.phase = phase;
        crate::monitor::record_phase(self, previous)?;
        self.last_error = None;
        self.save()
    }

    pub fn remove(&mut self) -> Result<()> {
        self.advance(Phase::Closed)?;
        let p = state_path(&self.project_root);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        let _ = std::fs::remove_dir(p.parent().unwrap());
        // Keep receipts and recovery files, but remove the outgoing archive and secrets.
        for name in ["snapshot.tar.gz", "repo.bundle", "env"] {
            let _ = std::fs::remove_file(self.dir().join(name));
        }
        Ok(())
    }
}

pub fn records(home: &Path) -> Result<Vec<State>> {
    let dir = home.join(".beam/transfers");
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut states = vec![];
    for e in std::fs::read_dir(dir)? {
        let path = e?.path().join("state.json");
        if path.is_file() {
            states.push(State::read_record(&path)?);
        }
    }
    states.sort_by_key(|s| s.created_at);
    Ok(states)
}

/// Receipts cannot tell whether the user has manually merged saved work.
pub fn latest_recovery(home: &Path, root: &Path) -> Result<Option<State>> {
    Ok(records(home)?.into_iter().rev().find(|s| {
        s.project_root == root && s.phase == Phase::Closed && s.has_unresolved_recovery()
    }))
}

pub fn index_load(home: &Path) -> Result<Vec<State>> {
    Ok(records(home)?
        .into_iter()
        .filter(|s| s.phase != Phase::Closed)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_lock_excludes_another_writer_and_releases_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let lock = ProjectLock::acquire(d.path(), d.path()).unwrap();
        assert!(ProjectLock::acquire(d.path(), d.path()).is_err());
        drop(lock);
        assert!(ProjectLock::acquire(d.path(), d.path()).is_ok());
    }
    #[test]
    fn atomic_write_replaces_whole_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.json");
        atomic_write(&p, b"long old value").unwrap();
        atomic_write(&p, b"new").unwrap();
        assert_eq!(std::fs::read(p).unwrap(), b"new");
    }
}
