// ABOUTME: Reads beam.toml from the project root. All fields are optional.
// ABOUTME: Also detects the setup commands (dependency install) from lockfiles.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub beam: BeamSection,
    #[serde(default)]
    pub files: FilesSection,
    #[serde(default)]
    pub env: EnvSection,
    #[serde(default)]
    pub sandbox: SandboxSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamSection {
    /// Default target, for example "docker" or "ssh://host".
    pub to: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesSection {
    /// Files ignored by git that beam copies too. Paths are relative to the project root.
    pub extras: Option<Vec<String>>,
    pub max_file_size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSection {
    /// Names of env vars to copy from the local env into the sandbox.
    #[serde(default)]
    pub forward: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSection {
    pub image: Option<String>,
    /// How long a Steel computer runs before it pauses itself, for example "4h".
    pub timeout: Option<String>,
    /// Commands that prepare the project in the sandbox. They replace the detected commands.
    pub setup: Option<Vec<String>>,
}

pub const DEFAULT_EXTRAS: &[&str] = &[".env", ".env.local"];
pub const DEFAULT_IMAGE: &str = "beam-base:latest";
pub const DEFAULT_MAX_FILE_SIZE: &str = "50MB";
pub const DEFAULT_TIMEOUT: &str = "4h";

impl Config {
    pub fn load(root: &Path) -> Result<Config> {
        let path = root.join("beam.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("bad {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn extras(&self) -> Vec<String> {
        match &self.files.extras {
            Some(v) => v.clone(),
            None => DEFAULT_EXTRAS.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn setup(&self, root: &Path) -> Vec<String> {
        match &self.sandbox.setup {
            Some(v) => v.clone(),
            None => detect_setup(root),
        }
    }
}

/// Lockfile → install command. The first match in each ecosystem wins.
const SETUP_RULES: &[(&str, &str)] = &[
    ("pnpm-lock.yaml", "pnpm install --frozen-lockfile"),
    ("package-lock.json", "npm ci"),
    ("yarn.lock", "yarn install --immutable"),
    ("bun.lock", "bun install --frozen-lockfile"),
    ("bun.lockb", "bun install --frozen-lockfile"),
    ("uv.lock", "uv sync"),
    ("poetry.lock", "poetry install"),
    (
        "requirements.txt",
        "python3 -m venv .venv && .venv/bin/pip install -r requirements.txt",
    ),
    ("Cargo.lock", "cargo fetch"),
    ("go.sum", "go mod download"),
    ("Gemfile.lock", "bundle install"),
];

const ECOSYSTEM: &[(&str, &str)] = &[
    ("pnpm-lock.yaml", "js"),
    ("package-lock.json", "js"),
    ("yarn.lock", "js"),
    ("bun.lock", "js"),
    ("bun.lockb", "js"),
    ("uv.lock", "py"),
    ("poetry.lock", "py"),
    ("requirements.txt", "py"),
    ("Cargo.lock", "rust"),
    ("go.sum", "go"),
    ("Gemfile.lock", "ruby"),
];

pub fn detect_setup(root: &Path) -> Vec<String> {
    let mut seen = vec![];
    let mut cmds = vec![];
    for (file, cmd) in SETUP_RULES {
        let eco = ECOSYSTEM
            .iter()
            .find(|(f, _)| f == file)
            .map(|(_, e)| *e)
            .unwrap_or(file);
        if root.join(file).is_file() && !seen.contains(&eco) {
            seen.push(eco);
            cmds.push(cmd.to_string());
        }
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_one_command_per_ecosystem() {
        let d = tempfile::tempdir().unwrap();
        for f in ["pnpm-lock.yaml", "package-lock.json", "Cargo.lock"] {
            std::fs::write(d.path().join(f), "").unwrap();
        }
        assert_eq!(
            detect_setup(d.path()),
            vec!["pnpm install --frozen-lockfile", "cargo fetch"]
        );
    }

    #[test]
    fn config_overrides_defaults() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.lock"), "").unwrap();
        assert_eq!(
            Config::load(d.path()).unwrap().extras(),
            vec![".env", ".env.local"]
        );

        std::fs::write(
            d.path().join("beam.toml"),
            "[beam]\nto = \"docker\"\n[files]\nextras = [\"cfg.json\"]\n[sandbox]\nsetup = []\n",
        )
        .unwrap();
        let c = Config::load(d.path()).unwrap();
        assert_eq!(c.beam.to.as_deref(), Some("docker"));
        assert_eq!(c.extras(), vec!["cfg.json"]);
        assert!(c.setup(d.path()).is_empty());
    }

    #[test]
    fn rejects_unknown_keys() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("beam.toml"), "[beam]\ntoo = 1\n").unwrap();
        assert!(Config::load(d.path()).is_err());
    }
}
