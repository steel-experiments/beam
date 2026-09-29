// ABOUTME: A test-only adapter with a different file layout and native event format.
use super::{
    Adapter, Defaults, Session,
    evidence::{Capabilities, Kind, Observation, Source},
};
use crate::util::sh_quote;
use anyhow::{Result, bail};
use std::{path::Path, time::SystemTime};
pub struct Fixture;
impl Adapter for Fixture {
    fn id(&self) -> &'static str {
        "fixture"
    }
    fn label(&self) -> &'static str {
        "Fixture agent"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_transfer: true,
            structured_events: true,
            agent_reports: true,
            terminal_heuristics: false,
        }
    }
    fn sessions(&self, home: &Path, cwd: &Path) -> Result<Vec<Session>> {
        if !home.join(".fixture/conversations/session.txt").is_file() {
            return Ok(vec![]);
        }
        Ok(vec![Session {
            id: "session".into(),
            cwd: cwd.into(),
            title: "Test task".into(),
            modified: SystemTime::now(),
            turns: 1,
        }])
    }
    fn session_paths(&self, _home: &Path, session: &Session) -> Vec<String> {
        vec![format!(".fixture/conversations/{}.txt", session.id)]
    }
    fn return_paths(&self, _cwd: &Path) -> Vec<String> {
        vec![".fixture/conversations".into()]
    }
    fn defaults(&self, _home: &Path, _cwd: &Path) -> Result<Defaults> {
        Ok(Defaults {
            files: vec![(".fixture/config".into(), b"fixture configuration".to_vec())],
            removed_settings: vec![],
        })
    }
    fn resume_fn(&self, session: &str) -> String {
        format!(
            r#"resume() {{ printf '%s\n' "$1" >> "$H/"{}; printf 'done\tfixture completed\n' > "$S/native-event"; }}"#,
            sh_quote(&format!(".fixture/conversations/{session}.txt"))
        )
    }
    fn observation_script(&self, stage: &str, _tmux: &str) -> Option<String> {
        Some(format!(
            "cat {}",
            sh_quote(&format!("{stage}/native-event"))
        ))
    }
    fn decode_observation(&self, output: &str) -> Result<Observation> {
        let (state, detail) = output
            .trim_end()
            .split_once('\t')
            .ok_or_else(|| anyhow::anyhow!("invalid fixture event"))?;
        let kind = match state {
            "working" => Kind::Activity,
            "waiting" => Kind::InputNeeded,
            "resolved" => Kind::InputResolved,
            "done" => Kind::CompletionReported,
            "failed" => Kind::FailureReported,
            _ => bail!("unknown fixture event"),
        };
        Ok(Observation {
            kind,
            source: Source::ClientEvent,
            detail: detail.into(),
            at: None,
        })
    }
}
