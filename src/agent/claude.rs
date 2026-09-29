// ABOUTME: Claude Code adapter: finds the session for a directory and lists the files it needs.
// ABOUTME: Also knows how to resume a session, which env vars hold auth, and how to filter user settings.

use super::{
    Adapter, Defaults, Session,
    evidence::{Capabilities, Observation},
};
use crate::util::{run, sh_quote};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Env vars that let Claude Code log in without the macOS Keychain.
pub const AUTH_ENV: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
];

/// User-level items that beam copies when they exist (paths relative to $HOME).
const USER_ITEMS: &[&str] = &[
    ".claude/CLAUDE.md",
    ".claude/skills",
    ".claude/agents",
    ".claude/commands",
];

/// Keys in ~/.claude/settings.json that usually call local programs.
const LOCAL_ONLY_SETTINGS: &[&str] = &[
    "hooks",
    "statusLine",
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
];

/// Claude Code names the project directory from the cwd: all characters that are not ASCII
/// letters or digits become "-".
pub fn encode_path(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn projects_rel(cwd: &Path) -> String {
    format!(".claude/projects/{}", encode_path(&cwd.to_string_lossy()))
}

/// Find the session to move. Without an id, use the transcript that changed last.
pub fn find_session(home: &Path, cwd: &Path, id: Option<&str>) -> Result<Session> {
    let project_dir = home.join(projects_rel(cwd));
    if !project_dir.is_dir() {
        bail!(
            "no Claude Code sessions for {} (looked in {})",
            cwd.display(),
            project_dir.display()
        );
    }
    let mut found: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&project_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter(|p| id.is_none_or(|id| p.file_stem().is_some_and(|s| s == id)))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    found.sort();
    let Some((modified, transcript)) = found.pop() else {
        match id {
            Some(id) => bail!("session {id} not found in {}", project_dir.display()),
            None => bail!("no session transcripts in {}", project_dir.display()),
        }
    };
    let id = transcript
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let text = std::fs::read_to_string(&transcript).unwrap_or_default();
    let turns = text
        .lines()
        .filter(|l| l.contains("\"type\":\"user\""))
        .count();
    Ok(Session {
        id: id.clone(),
        title: transcript_title(&transcript, &id),
        cwd: cwd.to_path_buf(),
        modified,
        turns,
    })
}

/// Files for the session, as paths relative to $HOME. Directories are included whole.
pub fn session_paths(home: &Path, cwd: &Path, s: &Session) -> Vec<String> {
    let proj = projects_rel(cwd);
    let mut v = vec![format!("{proj}/{}.jsonl", s.id)];
    for extra in [
        format!("{proj}/{}", s.id),
        format!("{proj}/memory"),
        format!(".claude/file-history/{}", s.id),
    ] {
        if home.join(&extra).exists() {
            v.push(extra);
        }
    }
    v
}

/// User-level config paths (relative to $HOME) that exist.
pub fn user_paths(home: &Path) -> Vec<String> {
    USER_ITEMS
        .iter()
        .filter(|p| home.join(p).exists())
        .map(|p| p.to_string())
        .collect()
}

/// Paths (relative to $HOME) that "beam down" brings home.
pub fn back_paths(cwd: &Path) -> Vec<String> {
    vec![projects_rel(cwd), ".claude/file-history".into()]
}

/// Remove settings that call local programs. Returns the new JSON and the removed keys.
pub fn filter_settings(json: &str) -> Result<(String, Vec<String>)> {
    let mut v: serde_json::Value =
        serde_json::from_str(json).context("bad ~/.claude/settings.json")?;
    let mut removed = vec![];
    if let Some(obj) = v.as_object_mut() {
        for k in LOCAL_ONLY_SETTINGS {
            if obj.remove(*k).is_some() {
                removed.push(k.to_string());
            }
        }
    }
    Ok((serde_json::to_string_pretty(&v)?, removed))
}

/// A ~/.claude.json for a sandbox where Claude Code never ran: no first-run screens,
/// and the project is trusted.
pub fn default_claude_json(cwd: &Path) -> String {
    serde_json::json!({
        "hasCompletedOnboarding": true,
        "projects": { cwd.to_string_lossy(): { "hasTrustDialogAccepted": true } }
    })
    .to_string()
}

/// Shell function body that resumes the session with the handoff message in "$1".
pub fn resume_fn(id: &str) -> String {
    format!("resume() {{ claude --resume {} \"$1\"; }}", sh_quote(id))
}

/// True when a Claude Code process has its cwd in `dir`.
pub fn is_running(dir: &Path) -> bool {
    let Ok(ps) = run(Command::new("ps").args(["-Ao", "pid=,comm="])) else {
        return false;
    };
    let pids: Vec<&str> = ps
        .lines()
        .filter_map(|l| l.trim().split_once(' '))
        .filter(|(_, comm)| comm.trim().rsplit('/').next() == Some("claude"))
        .map(|(pid, _)| pid)
        .collect();
    if pids.is_empty() {
        return false;
    }
    let Ok(out) = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-Fn", "-p", &pids.join(",")])
        .output()
    else {
        return false;
    };
    let dir = dir.to_string_lossy();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .any(|p| p == dir || p.starts_with(&format!("{dir}/")))
}

/// List sessions with a readable title for an interactive selection.
pub fn sessions(home: &Path, cwd: &Path) -> Result<Vec<Session>> {
    let dir = home.join(projects_rel(cwd));
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().is_some_and(|e| e == "jsonl")
            && let Some(id) = p.file_stem().and_then(|s| s.to_str())
        {
            out.push(find_session(home, cwd, Some(id))?);
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(out)
}

fn transcript_title(transcript: &Path, id: &str) -> String {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(transcript) else {
        return id.to_owned();
    };
    for line in std::io::BufReader::new(file).lines().take(200).flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["type"] == "user"
            && let Some(s) = v["message"]["content"]
                .as_str()
                .or_else(|| v["message"].as_str())
        {
            return s.chars().filter(|c| !c.is_control()).take(70).collect();
        }
    }
    id.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_paths_like_claude_code() {
        assert_eq!(encode_path("/Users/air/dev/beam"), "-Users-air-dev-beam");
        assert_eq!(
            encode_path("/Users/air/dev/awesome-agentic-patterns"),
            "-Users-air-dev-awesome-agentic-patterns"
        );
        assert_eq!(encode_path("/tmp/my.app_x"), "-tmp-my-app-x");
    }

    #[test]
    fn finds_newest_session() {
        let home = tempfile::tempdir().unwrap();
        let cwd = Path::new("/work/app");
        let dir = home.path().join(projects_rel(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.jsonl"), "{}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            dir.join("new.jsonl"),
            "{\"type\":\"user\"}\n{\"type\":\"user\"}\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join(".claude/file-history/new")).unwrap();

        let s = find_session(home.path(), cwd, None).unwrap();
        assert_eq!(s.id, "new");
        assert_eq!(s.turns, 2);
        assert_eq!(
            find_session(home.path(), cwd, Some("old")).unwrap().id,
            "old"
        );
        assert!(find_session(home.path(), cwd, Some("nope")).is_err());
        assert_eq!(
            session_paths(home.path(), cwd, &s),
            vec![
                format!("{}/new.jsonl", projects_rel(cwd)),
                ".claude/file-history/new".to_string()
            ]
        );
    }

    #[test]
    fn filters_local_only_settings() {
        let (out, removed) = filter_settings(
            r#"{"model":"opus","hooks":{"Stop":[]},"statusLine":{"type":"command"}}"#,
        )
        .unwrap();
        assert_eq!(removed, vec!["hooks", "statusLine"]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, serde_json::json!({"model":"opus"}));
    }

    #[test]
    fn detects_no_agent_in_empty_dir() {
        let d = tempfile::tempdir().unwrap();
        assert!(!is_running(d.path()));
    }
}

pub struct Claude;
impl Adapter for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn label(&self) -> &'static str {
        "Claude Code"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_transfer: true,
            terminal_heuristics: true,
            agent_reports: true,
            structured_events: false,
        }
    }
    fn sessions(&self, home: &Path, cwd: &Path) -> Result<Vec<Session>> {
        sessions(home, cwd)
    }
    fn find_session(&self, home: &Path, cwd: &Path, id: &str) -> Result<Session> {
        find_session(home, cwd, Some(id))
    }
    fn is_running(&self, root: &Path) -> bool {
        is_running(root)
    }
    fn session_paths(&self, home: &Path, session: &Session) -> Vec<String> {
        session_paths(home, &session.cwd, session)
    }
    fn user_paths(&self, home: &Path) -> Vec<String> {
        user_paths(home)
    }
    fn return_paths(&self, cwd: &Path) -> Vec<String> {
        back_paths(cwd)
    }
    fn defaults(&self, home: &Path, cwd: &Path) -> Result<Defaults> {
        let mut defaults = Defaults::default();
        match std::fs::read_to_string(home.join(".claude/settings.json")) {
            Ok(text) => {
                let (json, removed) = filter_settings(&text)?;
                defaults
                    .files
                    .push((".claude/settings.json".into(), json.into_bytes()));
                defaults.removed_settings = removed;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        defaults
            .files
            .push((".claude.json".into(), default_claude_json(cwd).into_bytes()));
        Ok(defaults)
    }
    fn auth_env(&self) -> &'static [&'static str] {
        AUTH_ENV
    }
    fn has_auth(&self, env: &[(String, String)]) -> bool {
        env.iter()
            .any(|(key, value)| AUTH_ENV[..3].contains(&key.as_str()) && !value.is_empty())
    }
    fn tools(&self) -> &'static [&'static str] {
        &["claude"]
    }
    fn bootstrap(&self) -> &'static str {
        include_str!("../../scripts/agents/claude_bootstrap.sh")
    }
    fn resume_fn(&self, session: &str) -> String {
        resume_fn(session)
    }
    fn resume_command(&self, session: &str) -> Option<String> {
        Some(format!("claude --resume {}", sh_quote(session)))
    }
    fn observation_script(&self, _stage: &str, tmux: &str) -> Option<String> {
        Some(crate::remote::with_vars(
            &[("T", tmux)],
            include_str!("../../scripts/agents/claude_observe.sh"),
        ))
    }
    fn decode_observation(&self, output: &str) -> Result<Observation> {
        Ok(serde_json::from_str(output)?)
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use crate::agent::evidence::{Kind, Source};
    fn capture(screen: Option<&str>) -> Observation {
        let script = Claude.observation_script("unused", "test").unwrap();
        let stub = match screen {
            Some(text) => format!("tmux() {{ printf '%s\\n' {}; }}\n", sh_quote(text)),
            None => "tmux() { return 1; }\n".into(),
        };
        let out = run(Command::new("sh").args(["-c", &(stub + &script)])).unwrap();
        Claude.decode_observation(&out).unwrap()
    }
    #[test]
    fn visible_footers_are_labeled_as_heuristics() {
        for footer in [
            "Enter to confirm · Esc to cancel",
            "Press Enter to continue or Escape to cancel",
            "Enter to confirm ·\nEsc to cancel",
            "Esc to cancel · Tab to amend",
        ] {
            let observation = capture(Some(&format!("Gateway notice\n{footer}\n\n")));
            assert_eq!(observation.kind, Kind::InputNeeded, "{footer}");
            assert_eq!(observation.source, Source::TerminalHeuristic);
        }
    }
    #[test]
    fn stale_quoted_and_unknown_text_do_not_claim_input_or_activity() {
        for text in [
            "Enter to confirm · Esc to cancel\nWorking...",
            "The docs say Enter to confirm · Esc to cancel",
            "> Enter to confirm · Esc to cancel",
            "",
            "Unrecognized dialog",
        ] {
            let observation = capture(Some(text));
            assert_eq!(observation.kind, Kind::InputResolved, "{text}");
            assert_eq!(
                crate::agent::evidence::assess(&[observation]).state,
                crate::agent::evidence::Task::Unknown
            );
        }
        assert_eq!(capture(None).source, Source::Unavailable);
    }
}
