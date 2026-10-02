// ABOUTME: Reads beam.toml from the project root. All fields are optional.
// ABOUTME: Also detects the setup commands (dependency install) from lockfiles.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    pub task: TaskSection,
    #[serde(default)]
    pub agent: AgentSection,
    #[serde(default)]
    pub workflow: WorkflowSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSection {
    /// Instruct the agent to commit, push, and create a draft pull request.
    #[serde(default)]
    pub pr: bool,
    /// Forward GitHub authentication for Git and the GitHub CLI.
    #[serde(default)]
    pub github_auth: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSection {
    /// Permission mode for the remote agent, for example acceptEdits or bypassPermissions.
    pub permission_mode: Option<String>,
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
    /// Ignored paths that also return home. Other extras are send-only.
    #[serde(default)]
    pub return_extras: Vec<String>,
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
    /// Cloud inactivity timeout: Steel pauses and Daytona stops, for example "4h".
    pub timeout: Option<String>,
    /// Commands that prepare the project in the sandbox. They replace the detected commands.
    pub setup: Option<Vec<String>>,
    /// Project checks run after setup and before the agent.
    #[serde(default)]
    pub verify: Vec<String>,
    /// Reuse successful setup in the same sandbox only when inputs still match.
    #[serde(default)]
    pub reuse_setup: bool,
    /// Additional files that determine setup. Missing files invalidate reuse.
    #[serde(default)]
    pub setup_inputs: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSection {
    pub objective: Option<String>,
    pub complete_when: Option<String>,
    pub last_verified: Option<String>,
    pub next_action: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
}
impl TaskSection {
    pub fn handoff(&self) -> String {
        let mut text = String::new();
        for (label, value) in [
            ("Objective", &self.objective),
            ("Complete when", &self.complete_when),
            ("Last verified by user", &self.last_verified),
            ("Next action", &self.next_action),
        ] {
            if let Some(value) = value {
                text.push_str(&format!("\n{label}: {value}"));
            }
        }
        for constraint in &self.constraints {
            text.push_str(&format!("\nConstraint: {constraint}"));
        }
        if text.is_empty() {
            text
        } else {
            format!("\n\nUser-provided task record:{text}\n")
        }
    }
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

/// One source for setup detection, prerequisite checks, and documentation.
pub struct SetupRule {
    pub file: &'static str,
    pub ecosystem: &'static str,
    pub command: &'static str,
    pub tool: &'static str,
}
pub const SETUP_RULES: &[SetupRule] = &[
    SetupRule {
        file: "pnpm-lock.yaml",
        ecosystem: "js",
        command: "pnpm install --frozen-lockfile",
        tool: "pnpm",
    },
    SetupRule {
        file: "package-lock.json",
        ecosystem: "js",
        command: "npm ci",
        tool: "npm",
    },
    SetupRule {
        file: "yarn.lock",
        ecosystem: "js",
        command: "yarn install --immutable",
        tool: "yarn",
    },
    SetupRule {
        file: "bun.lock",
        ecosystem: "js",
        command: "bun install --frozen-lockfile",
        tool: "bun",
    },
    SetupRule {
        file: "bun.lockb",
        ecosystem: "js",
        command: "bun install --frozen-lockfile",
        tool: "bun",
    },
    SetupRule {
        file: "uv.lock",
        ecosystem: "py",
        command: "uv sync",
        tool: "uv",
    },
    SetupRule {
        file: "poetry.lock",
        ecosystem: "py",
        command: "poetry install",
        tool: "poetry",
    },
    SetupRule {
        file: "requirements.txt",
        ecosystem: "py",
        command: "python3 -m venv .venv && .venv/bin/pip install -r requirements.txt",
        tool: "python3",
    },
    SetupRule {
        file: "Cargo.lock",
        ecosystem: "rust",
        command: "cargo fetch",
        tool: "cargo",
    },
    SetupRule {
        file: "go.sum",
        ecosystem: "go",
        command: "go mod download",
        tool: "go",
    },
    SetupRule {
        file: "Gemfile.lock",
        ecosystem: "ruby",
        command: "bundle install",
        tool: "bundle",
    },
];

pub fn detected_rules(root: &Path) -> Vec<&'static SetupRule> {
    let mut seen = vec![];
    SETUP_RULES
        .iter()
        .filter(|rule| {
            if !root.join(rule.file).is_file() || seen.contains(&rule.ecosystem) {
                return false;
            }
            seen.push(rule.ecosystem);
            true
        })
        .collect()
}

pub fn detect_setup(root: &Path) -> Vec<String> {
    detected_rules(root)
        .iter()
        .map(|r| r.command.to_string())
        .collect()
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    pub to: Option<String>,
}
impl UserConfig {
    pub fn path(home: &Path) -> std::path::PathBuf {
        Self::path_in(std::env::var_os("XDG_CONFIG_HOME"), home)
    }
    fn path_in(xdg: Option<std::ffi::OsString>, home: &Path) -> std::path::PathBuf {
        xdg.map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("beam/config.toml")
    }
    pub fn load(home: &Path) -> Result<Self> {
        Self::load_from(&Self::path(home))
    }
    pub fn save(&self, home: &Path) -> Result<()> {
        self.save_to(&Self::path(home))
    }
    fn load_from(p: &Path) -> Result<Self> {
        match std::fs::read_to_string(p) {
            Ok(s) => toml::from_str(&s).with_context(|| format!("bad {}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn save_to(&self, p: &Path) -> Result<()> {
        crate::util::atomic_write(p, toml::to_string(self)?.as_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolVersion {
    pub tool: String,
    pub version: String,
}

/// Read numeric version pins. Dynamic aliases remain explicit warnings.
pub fn tool_versions(root: &Path) -> Result<(Vec<ToolVersion>, Vec<String>)> {
    let mut pins = std::collections::BTreeMap::<String, String>::new();
    let mut warnings = vec![];
    let mut add = |tool: &str, version: &str| -> Result<()> {
        let tool = match tool {
            "nodejs" => "node",
            "python" => "python3",
            "golang" => "go",
            "rust" => "rustc",
            other => other,
        };
        if ![
            "node", "python3", "go", "rustc", "pnpm", "npm", "yarn", "bun", "uv", "poetry", "ruby",
        ]
        .contains(&tool)
        {
            warnings.push(format!(
                "version check for {tool} is not supported; configure the sandbox image"
            ));
            return Ok(());
        }
        let version = version
            .trim()
            .trim_start_matches('v')
            .split('+')
            .next()
            .unwrap_or("");
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
            warnings.push(format!(
                "{tool} version {version:?} is not a numeric pin; configure the sandbox image"
            ));
            return Ok(());
        }
        if let Some(old) = pins.get(tool) {
            if old != version
                && !old.starts_with(&format!("{version}."))
                && !version.starts_with(&format!("{old}."))
            {
                anyhow::bail!("conflicting {tool} versions: {old} and {version}");
            }
            if old.len() >= version.len() {
                return Ok(());
            }
        }
        pins.insert(tool.into(), version.into());
        Ok(())
    };
    let read = |name: &str| -> Result<Option<String>> {
        match std::fs::read_to_string(root.join(name)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    };
    if let Some(v) = read(".nvmrc")? {
        add("node", &v)?;
    }
    if let Some(v) = read(".tool-versions")? {
        for line in v
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
        {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                add(parts[0], parts[1])?;
            }
        }
    }
    if let Some(v) = read("rust-toolchain.toml")? {
        let v: toml::Value = toml::from_str(&v).context("bad rust-toolchain.toml")?;
        if let Some(channel) = v
            .get("toolchain")
            .and_then(|t| t.get("channel"))
            .and_then(|v| v.as_str())
        {
            add("rustc", channel)?;
        }
    }
    if let Some(v) = read("mise.toml")? {
        let v: toml::Value = toml::from_str(&v).context("bad mise.toml")?;
        if let Some(tools) = v.get("tools").and_then(|v| v.as_table()) {
            for (tool, version) in tools {
                add(tool, version.as_str().unwrap_or("complex specification"))?;
            }
        }
    }
    if let Some(v) = read("package.json")? {
        let v: serde_json::Value = serde_json::from_str(&v).context("bad package.json")?;
        if let Some(pm) = v["packageManager"].as_str()
            && let Some((tool, version)) = pm.split_once('@')
        {
            add(tool, version)?;
        }
    }
    Ok((
        pins.into_iter()
            .map(|(tool, version)| ToolVersion { tool, version })
            .collect(),
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_path_uses_xdg_config_home_when_set() {
        let home = Path::new("/home/u");
        assert_eq!(
            UserConfig::path_in(None, home),
            Path::new("/home/u/.config/beam/config.toml")
        );
        assert_eq!(
            UserConfig::path_in(Some("/xdg".into()), home),
            Path::new("/xdg/beam/config.toml")
        );
    }

    #[test]
    fn user_config_saves_and_clears_the_default_target() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("beam/config.toml");
        assert_eq!(UserConfig::load_from(&p).unwrap().to, None);
        UserConfig {
            to: Some("steel".into()),
        }
        .save_to(&p)
        .unwrap();
        assert_eq!(
            UserConfig::load_from(&p).unwrap().to.as_deref(),
            Some("steel")
        );
        UserConfig { to: None }.save_to(&p).unwrap();
        assert_eq!(UserConfig::load_from(&p).unwrap().to, None);
    }

    #[test]
    fn version_pins_are_combined_and_conflicts_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".nvmrc"), "v22\n").unwrap();
        std::fs::write(d.path().join("mise.toml"), "[tools]\nnode = '22.14.0'\n").unwrap();
        let (pins, warnings) = tool_versions(d.path()).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].version, "22.14.0");
        std::fs::write(d.path().join(".nvmrc"), "20\n").unwrap();
        assert!(tool_versions(d.path()).is_err());
    }

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
