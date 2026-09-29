// ABOUTME: Helpers for the end-to-end tests: a temporary HOME with a Claude session and a git project.
// ABOUTME: The beam binary runs with that HOME, so the real ~/.claude and ~/.beam stay untouched.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const SESSION: &str = "e2e-session";

pub fn sh(dir: &Path, script: &str) -> String {
    let out = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{script}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub struct Env {
    _tmp: tempfile::TempDir,
    pub real_home: PathBuf,
    pub home: PathBuf,
    pub project: PathBuf,
}

impl Env {
    /// A project with a commit, a staged file, an untracked file, .env, node_modules, and beam.toml.
    pub fn new(beam_toml: &str) -> Env {
        let real_home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let project = root.join("home/dev/app");
        std::fs::create_dir_all(&project).unwrap();
        sh(
            &project,
            "git init -q -b main . && git config user.name Tester && git config user.email t@example.com \
             && printf 'node_modules/\\n.env\\n' > .gitignore && echo hello > README.md && git add -A \
             && git commit -q -m first && echo staged > staged.txt && git add staged.txt \
             && echo untracked > notes.txt && echo KEY=1 > .env && mkdir node_modules && echo x > node_modules/big",
        );
        std::fs::write(project.join("beam.toml"), beam_toml).unwrap();
        let sessions = home.join(".claude/projects").join(encode(&project));
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{SESSION}.jsonl")),
            "{\"type\":\"user\",\"message\":\"local\"}\n",
        )
        .unwrap();
        Env {
            _tmp: tmp,
            real_home,
            home,
            project,
        }
    }

    pub fn beam(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.real_home.join(".steel/bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_beam"))
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("BEAM_E2E_VAR", "forwarded-value")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("ANTHROPIC_BASE_URL")
            .output()
            .unwrap()
    }

    pub fn state(&self) -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.project.join(".beam/state.json")).unwrap(),
        )
        .unwrap()
    }

    pub fn transcript(&self) -> String {
        let p = self
            .home
            .join(".claude/projects")
            .join(encode(&self.project))
            .join(format!("{SESSION}.jsonl"));
        std::fs::read_to_string(p).unwrap()
    }

    /// Checks that are the same for all targets after "beam down".
    pub fn assert_home_again(&self) {
        let pr = &self.project;
        assert_eq!(sh(pr, "git log --format=%s"), "agent commit\nfirst\n");
        assert_eq!(sh(pr, "cat README.md"), "hello\nuncommitted line\n");
        assert_eq!(
            sh(pr, "cat sandbox-work.txt setup-ran.txt notes.txt"),
            "work from sandbox\nok\nuntracked\n"
        );
        assert_eq!(
            sh(pr, "git status --porcelain=v1 -uall"),
            " M README.md\n?? beam.toml\n?? notes.txt\n?? setup-ran.txt\n"
        );
        assert!(
            pr.join("node_modules/big").exists(),
            "local ignored files stay"
        );
        assert!(
            !pr.join(".beam/state.json").exists(),
            "the lock must be gone"
        );
        assert!(
            self.transcript()
                .ends_with("{\"type\":\"user\",\"message\":\"from sandbox\"}\n"),
            "{}",
            self.transcript()
        );
    }
}

pub fn encode(p: &Path) -> String {
    p.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// What the agent does in the sandbox, as a shell line that runs in the project directory.
/// The fake claude in tests/e2e/Dockerfile does the same.
pub fn agent_work(transcript: &str) -> String {
    format!(
        "printf '{{\"type\":\"user\",\"message\":\"from sandbox\"}}\\n' >> '{transcript}' \
         && echo 'work from sandbox' > sandbox-work.txt && git add sandbox-work.txt \
         && git commit -q -m 'agent commit' && echo 'uncommitted line' >> README.md"
    )
}
