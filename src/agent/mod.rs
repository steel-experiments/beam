// ABOUTME: Agent contract. Transfer and UI code depend on this interface, not client layouts.
mod claude;
pub mod evidence;
mod shell;
#[cfg(test)]
pub mod testing;

use anyhow::{Result, bail};
use evidence::{Capabilities, Observation};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub modified: SystemTime,
    pub turns: usize,
}
#[derive(Default)]
pub struct Defaults {
    /// Paths are relative to the private agent home, not the project.
    pub files: Vec<(String, Vec<u8>)>,
    pub removed_settings: Vec<String>,
}

/// Tools needed to upload, restore, and start the selected agent.
pub fn transfer_tools(adapter: &dyn Adapter) -> Vec<String> {
    ["git", "tmux", "tar", "gzip"]
        .into_iter()
        .chain(adapter.tools().iter().copied())
        .map(str::to_string)
        .collect()
}

pub trait Adapter: Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn sessions(&self, _home: &Path, _cwd: &Path) -> Result<Vec<Session>> {
        Ok(vec![])
    }
    fn find_session(&self, home: &Path, cwd: &Path, id: &str) -> Result<Session> {
        self.sessions(home, cwd)?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow::anyhow!("session {id} not found for {}", self.label()))
    }
    fn is_running(&self, _root: &Path) -> bool {
        false
    }
    fn session_paths(&self, _home: &Path, _session: &Session) -> Vec<String> {
        vec![]
    }
    fn user_paths(&self, _home: &Path) -> Vec<String> {
        vec![]
    }
    fn return_paths(&self, _cwd: &Path) -> Vec<String> {
        vec![]
    }
    fn defaults(&self, _home: &Path, _cwd: &Path) -> Result<Defaults> {
        Ok(Defaults::default())
    }
    fn auth_env(&self) -> &'static [&'static str] {
        &[]
    }
    fn has_auth(&self, _env: &[(String, String)]) -> bool {
        true
    }
    fn tools(&self) -> &'static [&'static str] {
        &[]
    }
    fn bootstrap(&self) -> &'static str {
        ""
    }
    fn resume_fn(&self, session: &str) -> String;
    /// True when the process must exit by itself to save its state, so stop sends Ctrl-C and waits.
    /// When false, stop ends the terminal immediately.
    fn graceful_stop(&self) -> bool {
        true
    }
    fn resume_command(&self, _session: &str) -> Option<String> {
        None
    }
    /// Probe the current run. Adapters decode their wire format into a shared observation.
    /// A probe must not return stale events from a previous process run.
    fn observation_script(&self, _stage: &str, _tmux: &str) -> Option<String> {
        None
    }
    fn decode_observation(&self, output: &str) -> Result<Observation> {
        Ok(serde_json::from_str(output)?)
    }
}

static CLAUDE: claude::Claude = claude::Claude;
static SHELL: shell::Shell = shell::Shell;
static ADAPTERS: &[&dyn Adapter] = &[&CLAUDE, &SHELL];
pub fn get(id: &str) -> Result<&'static dyn Adapter> {
    if let Some(adapter) = ADAPTERS.iter().find(|adapter| adapter.id() == id) {
        return Ok(*adapter);
    }
    bail!(
        "unsupported agent {id:?}; available agents: {}",
        ids().join(", ")
    )
}
pub fn ids() -> Vec<&'static str> {
    ADAPTERS.iter().map(|adapter| adapter.id()).collect()
}
pub fn default_adapter() -> &'static dyn Adapter {
    &CLAUDE
}
pub fn contains_path(scopes: &[String], path: &str) -> bool {
    scopes
        .iter()
        .any(|scope| path == scope || path.starts_with(&format!("{scope}/")))
}
