// ABOUTME: The lock and the record of a beamed session: <project>/.beam/state.json.
// ABOUTME: An index in ~/.beam/sessions.json lists all beamed sessions for "beam ls".

use crate::git::Snap;
use crate::sandbox::Sandbox;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub session_id: String,
    pub agent: String,
    pub project_root: PathBuf,
    pub agent_cwd: PathBuf,
    pub home: PathBuf,
    pub target: String,
    pub sandbox: Sandbox,
    /// Directory in the sandbox that holds the unpacked snapshot and the scripts.
    pub stage: String,
    pub tmux: String,
    pub sent: Snap,
    /// sha256 of each agent file that beam sent (paths relative to $HOME).
    pub sent_files: BTreeMap<String, String>,
    /// Paths that beam put in the sandbox. "beam kill" removes them on SSH hosts.
    pub remote_paths: Vec<String>,
    pub created_at: u64,
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join(".beam/state.json")
}

impl State {
    pub fn load(root: &Path) -> Result<Option<State>> {
        let p = state_path(root);
        match std::fs::read_to_string(&p) {
            Ok(t) => Ok(Some(
                serde_json::from_str(&t).with_context(|| format!("bad {}", p.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = state_path(&self.project_root);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        index_update(&self.home, |v| {
            v.retain(|e| e.project != self.project_root);
            v.push(IndexEntry {
                project: self.project_root.clone(),
                session_id: self.session_id.clone(),
                target: self.sandbox.describe(),
                created_at: self.created_at,
            });
        })
    }

    pub fn remove(&self) -> Result<()> {
        let p = state_path(&self.project_root);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        let _ = std::fs::remove_dir(p.parent().unwrap());
        index_update(&self.home, |v| v.retain(|e| e.project != self.project_root))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub project: PathBuf,
    pub session_id: String,
    pub target: String,
    pub created_at: u64,
}

fn index_path(home: &Path) -> PathBuf {
    home.join(".beam/sessions.json")
}

pub fn index_load(home: &Path) -> Result<Vec<IndexEntry>> {
    match std::fs::read_to_string(index_path(home)) {
        Ok(t) => Ok(serde_json::from_str(&t).context("bad ~/.beam/sessions.json")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

fn index_update(home: &Path, f: impl FnOnce(&mut Vec<IndexEntry>)) -> Result<()> {
    let mut v = index_load(home)?;
    f(&mut v);
    let p = index_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}
