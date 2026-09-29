// ABOUTME: Claude Code adapter: finds the session for a directory and lists the files it needs.
// ABOUTME: Also knows how to resume a session, which env vars hold auth, and how to filter user settings.

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

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub transcript: PathBuf,
    pub modified: SystemTime,
    pub turns: usize,
}

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
        id,
        transcript,
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
