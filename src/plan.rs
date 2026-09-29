// ABOUTME: A single transfer plan supplies the preview, validation, and archive inputs.
use crate::{
    agent::{self, Adapter, Session},
    config::{Config, DEFAULT_MAX_FILE_SIZE, UserConfig},
    git,
    sandbox::Target,
    util,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

pub struct Plan {
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub config: Config,
    pub target: String,
    pub session: Option<Session>,
    pub agent: &'static dyn Adapter,
    pub extras: Vec<String>,
    pub return_extras: Vec<String>,
    pub extra_files: Vec<String>,
    pub agent_files: Vec<String>,
    pub defaults: Vec<String>,
    pub env: Vec<(String, String)>,
    pub setup: Vec<String>,
    pub tools: Vec<String>,
    pub versions: Vec<crate::config::ToolVersion>,
    pub changed: Vec<(String, u64)>,
    pub warnings: Vec<String>,
}

pub fn ask(prompt: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("{prompt} Supply the option explicitly when stdin is not a terminal");
    }
    print!("{prompt} ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn session_age(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3600 => (seconds / 60, "minute"),
        3600..172800 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// Expand files without following links in extras. User configuration may link within HOME.
pub fn files(base: &Path, rel: &str) -> Result<Vec<String>> {
    files_inner(base, rel, false, &mut std::collections::BTreeSet::new())
}
fn user_files(base: &Path, rel: &str) -> Result<Vec<String>> {
    files_inner(base, rel, true, &mut std::collections::BTreeSet::new())
}
fn files_inner(
    base: &Path,
    rel: &str,
    follow: bool,
    parents: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<Vec<String>> {
    util::relative_path(rel)?;
    let path = base.join(rel);
    if !path.exists() && !path.is_symlink() {
        return Ok(vec![]);
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot read {}", path.display()))?;
    if !canonical.starts_with(base.canonicalize()?) {
        bail!(
            "{} points outside {}. Copy it inside first",
            path.display(),
            base.display()
        );
    }
    if path.is_symlink() && !follow {
        bail!(
            "extra {} is a symlink. Use a regular file or directory",
            path.display()
        );
    }
    let meta = std::fs::metadata(&path)?;
    if meta.is_file() {
        return Ok(vec![rel.into()]);
    }
    if !meta.is_dir() {
        bail!("unsupported file type: {}", path.display());
    }
    if !parents.insert(canonical.clone()) {
        bail!("symlink cycle in {}", path.display());
    }
    let mut out = vec![];
    for e in std::fs::read_dir(path)? {
        let name = e?
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("file names must be UTF-8"))?;
        out.extend(files_inner(
            base,
            &format!("{rel}/{name}"),
            follow,
            parents,
        )?);
    }
    parents.remove(&canonical);
    out.sort();
    Ok(out)
}

pub fn hashes(base: &Path, names: &[String]) -> Result<BTreeMap<String, String>> {
    names
        .iter()
        .map(|n| Ok((n.clone(), util::sha256_file(&base.join(n))?)))
        .collect()
}

impl Plan {
    pub fn preflight_tools(&self) -> Vec<String> {
        if self.agent.capabilities().environment_repair {
            agent::transfer_tools(self.agent)
        } else {
            self.tools.clone()
        }
    }

    pub fn preflight_versions(&self) -> &[crate::config::ToolVersion] {
        if self.agent.capabilities().environment_repair {
            &[]
        } else {
            &self.versions
        }
    }

    pub fn build(a: &crate::up::UpArgs) -> Result<Self> {
        let home = crate::up::home_dir()?;
        let cwd = a
            .path
            .canonicalize()
            .with_context(|| format!("{} does not exist", a.path.display()))?;
        let root = git::toplevel(&cwd)?;
        git::ensure_has_commits(&root)?;
        let config = Config::load(&root)?;
        let user = UserConfig::load(&home)?;
        let target = if let Some(t) = a.to.clone().or(config.beam.to.clone()).or(user.to) {
            t
        } else {
            if a.yes {
                bail!(
                    "no target. Supply the option explicitly with --to TARGET, or configure a personal default"
                );
            }
            println!(
                "Choose a destination. Docker runs locally; docker+ssh://HOST uses a remote Docker host."
            );
            if util::succeeds(std::process::Command::new("docker").args([
                "info",
                "--format",
                "{{.ServerVersion}}",
            ])) {
                println!("Docker is available on this machine.");
            }
            let t = ask("Destination [docker]:")?;
            let t = if t.is_empty() { "docker".into() } else { t };
            Target::parse(&t)?;
            if !a.dry_run {
                UserConfig {
                    to: Some(t.clone()),
                }
                .save(&home)?;
            }
            t
        };
        Target::parse(&target)?;
        let mut adapter = if a.agent == "auto" {
            agent::default_adapter()
        } else {
            agent::get(&a.agent)?
        };
        if !adapter.capabilities().session_transfer && a.session.is_some() {
            bail!("--session cannot be used with --agent {}", adapter.id());
        }
        let mut warnings = vec![];
        let session = if !adapter.capabilities().session_transfer {
            None
        } else if let Some(id) = &a.session {
            Some(adapter.find_session(&home, &cwd, id)?)
        } else {
            let mut sessions = adapter.sessions(&home, &cwd)?;
            if sessions.is_empty() && cwd != root {
                sessions = adapter.sessions(&home, &root)?;
            }
            if sessions.is_empty() {
                if a.agent != "auto" {
                    bail!(
                        "no {} session found. Start the agent locally, or use --agent shell",
                        adapter.label()
                    );
                }
                None
            } else if sessions.len() > 1 && !a.yes && std::io::stdin().is_terminal() {
                println!(
                    "\nFound {} saved {} sessions for {}.",
                    sessions.len(),
                    adapter.label(),
                    root.display()
                );
                println!("Choose a conversation to continue on {target}.");
                println!("Titles come from saved conversations; they are not commands to run.\n");
                for (i, s) in sessions.iter().enumerate() {
                    let default = if i == 0 { " (default)" } else { "" };
                    println!("  {}. {:?}{default}", i + 1, s.title);
                    println!(
                        "     Last updated: {} | Session ID: {}\n",
                        session_age(s.modified.elapsed().unwrap_or_default().as_secs()),
                        s.id
                    );
                }
                println!("Press Enter to select 1, the most recently updated session.");
                println!("For workspace only, press Ctrl-C and rerun with --agent shell.\n");
                let answer = ask(&format!(
                    "Choose a session [1-{}; default: 1]:",
                    sessions.len()
                ))?;
                let selected = if answer.is_empty() {
                    1
                } else {
                    answer.parse::<usize>().context("enter a session number")?
                };
                if selected == 0 || selected > sessions.len() {
                    bail!("session number is out of range");
                }
                Some(sessions.remove(selected - 1))
            } else {
                Some(sessions.remove(0))
            }
        };
        let cwd = session.as_ref().map(|s| s.cwd.clone()).unwrap_or(cwd);
        if !a.force && adapter.is_running(&root) {
            bail!(
                "{} is still running in this project. Exit it first, or use --force",
                adapter.label()
            );
        }
        if a.agent == "auto" && session.is_none() {
            adapter = agent::get("shell")?;
        }
        let mut extras = config.extras();
        if config.files.extras.is_none() {
            extras.retain(|rel| {
                root.join(rel).exists()
                    && util::succeeds(git::git(&root).args(["check-ignore", "-q", "--", rel]))
            });
        }
        for rel in &config.files.return_extras {
            if !extras.contains(rel) {
                extras.push(rel.clone());
            }
        }
        let mut extra_files = vec![];
        for rel in &extras {
            util::relative_path(rel)?;
            if rel == ".git"
                || rel.starts_with(".git/")
                || rel == ".beam"
                || rel.starts_with(".beam/")
            {
                bail!("cannot transfer internal path {rel}");
            }
            let found = files(&root, rel)?;
            if found.is_empty() && !config.files.return_extras.contains(rel) {
                warnings.push(format!("extra {rel} is absent"));
            }
            extra_files.extend(found);
        }
        extra_files.sort();
        extra_files.dedup();
        for rel in &extra_files {
            if util::succeeds(git::git(&root).args(["ls-files", "--error-unmatch", "--", rel])) {
                bail!("extra {rel} is tracked by Git. Remove it from [files] extras/return_extras");
            }
            if !util::succeeds(git::git(&root).args(["check-ignore", "-q", "--", rel])) {
                bail!("extra {rel} must be ignored by Git");
            }
        }
        let mut agent_files = vec![];
        let mut defaults = vec![];
        if let Some(s) = &session {
            for rel in adapter.session_paths(&home, s) {
                agent_files.extend(user_files(&home, &rel)?);
            }
            for rel in adapter.user_paths(&home) {
                defaults.extend(user_files(&home, &rel)?);
            }
        }
        let changed = git::changed_files(&root)?;
        let max = util::parse_size(
            config
                .files
                .max_file_size
                .as_deref()
                .unwrap_or(DEFAULT_MAX_FILE_SIZE),
        )?;
        let mut large: Vec<(String, u64)> = vec![];
        // Validate every current Git file as well as index blobs, extras, and agent files.
        let output = git::git(&root)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .output()?;
        if !output.status.success() {
            bail!("cannot list Git files");
        }
        for rel in String::from_utf8(output.stdout)?
            .split('\0')
            .filter(|p| !p.is_empty())
        {
            if rel.starts_with(".beam/") {
                continue;
            }
            let p = root.join(rel);
            if p.file_name().is_some_and(|n| n == ".gitattributes")
                && std::fs::read_to_string(&p).is_ok_and(|s| s.contains("filter=lfs"))
            {
                bail!(
                    "Git LFS is configured in {rel}. Beam cannot transfer its separate object store yet"
                );
            }
            if let Ok(meta) = std::fs::symlink_metadata(&p) {
                if meta.file_type().is_symlink()
                    && !p.canonicalize().is_ok_and(|p| p.starts_with(&root))
                {
                    warnings.push(format!("{rel}: external symlink target is not transferred"));
                }
                if meta.len() > max {
                    large.push((rel.into(), meta.len()));
                }
            }
        }
        for (name, size) in git::indexed_sizes(&root)? {
            if size > max {
                large.push((format!("{name} (staged)"), size));
            }
        }
        for (base, names) in [
            (&root, &extra_files),
            (&home, &agent_files),
            (&home, &defaults),
        ] {
            for rel in names {
                let size = std::fs::metadata(base.join(rel))?.len();
                if size > max {
                    large.push((rel.clone(), size));
                }
            }
        }
        if !a.allow_large && !large.is_empty() {
            bail!(
                "these files are larger than {}:\n{}\nUse --allow-large to include them",
                util::human_size(max),
                large
                    .iter()
                    .map(|(n, s)| format!("  {n} ({})", util::human_size(*s)))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let mut names = config.env.forward.clone();
        if session.is_some() {
            names.extend(adapter.auth_env().iter().map(|s| s.to_string()));
        }
        names.sort();
        names.dedup();
        for name in &names {
            if name.is_empty()
                || !name.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
            {
                bail!("invalid environment variable name {name:?}");
            }
        }
        let env: Vec<_> = names
            .iter()
            .filter_map(|n| std::env::var(n).ok().map(|v| (n.clone(), v)))
            .collect();
        for n in &config.env.forward {
            if !env.iter().any(|(k, _)| k == n) {
                warnings.push(format!("{n} is not set locally"));
            }
        }
        if session.is_some() && !adapter.has_auth(&env) {
            warnings.push(format!(
                "{} authentication is not forwarded. Log in after `beam attach`",
                adapter.label()
            ));
        }
        for input in &config.sandbox.setup_inputs {
            util::relative_path(input)?;
        }
        let setup = config.setup(&root);
        let mut tools = agent::transfer_tools(adapter);
        if config.sandbox.setup.is_none() {
            tools.extend(
                crate::config::detected_rules(&root)
                    .iter()
                    .map(|r| r.tool.into()),
            );
        }
        let (versions, version_warnings) = crate::config::tool_versions(&root)?;
        warnings.extend(version_warnings);
        for pin in &versions {
            if !tools.contains(&pin.tool) {
                tools.push(pin.tool.clone());
            }
        }
        let return_extras = config.files.return_extras.clone();
        Ok(Self {
            root,
            cwd,
            home,
            config,
            target,
            session,
            agent: adapter,
            extras,
            return_extras,
            extra_files,
            agent_files,
            defaults,
            env,
            setup,
            tools,
            versions,
            changed,
            warnings,
        })
    }

    pub fn show(&self) {
        crate::up::step("project", self.root.display().to_string());
        crate::up::step("destination", &self.target);
        if self.agent.capabilities().environment_repair {
            println!(
                "Project tools are checked after upload. The agent receives failed checks and repairs the environment before continuing."
            );
        }
        crate::up::step(
            "session",
            self.session
                .as_ref()
                .map(|s| format!("{} · {} · {} turns", self.agent.label(), s.title, s.turns))
                .unwrap_or_else(|| "shell workspace (no agent session)".into()),
        );
        println!(
            "\nSend: complete reachable Git history, staged and unstaged changes, and untracked files."
        );
        if self.session.is_some() {
            println!(
                "Send: {} session and configuration. Return: session files.",
                self.agent.label()
            );
        }
        println!(
            "Return: remote Git work. Beam combines supported separate edits and saves conflicts for review."
        );
        println!("Stay local: running processes, databases, and ignored build output.");
        for rel in &self.extras {
            crate::up::step(
                "extra",
                format!(
                    "{rel} — {}",
                    if self.return_extras.contains(rel) {
                        "send and return"
                    } else {
                        "send only; remote edits do not return"
                    }
                ),
            );
        }
        if let Some(n) = git::unpushed_count(&self.root) {
            crate::up::step("commits", format!("{n} unpushed"));
        }
        crate::up::step("worktree", format!("{} changed paths", self.changed.len()));
        crate::up::step(
            "env",
            self.env
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        crate::up::step(
            "setup",
            if self.setup.is_empty() {
                "none".into()
            } else {
                self.setup.join("; ")
            },
        );
        crate::up::step(
            "verify",
            if self.config.sandbox.verify.is_empty() {
                "none; project readiness is unverified".into()
            } else {
                self.config.sandbox.verify.join("; ")
            },
        );
        print!("{}", self.config.task.handoff());
        if let Some(session) = &self.session {
            crate::up::step("session id", &session.id);
        }
        for pin in &self.versions {
            crate::up::step("toolchain", format!("{} {}", pin.tool, pin.version));
        }
        for warning in &self.warnings {
            println!("! {warning}");
        }
    }
}
