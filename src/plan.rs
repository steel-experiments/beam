// ABOUTME: A single transfer plan supplies the preview, validation, and archive inputs.
use crate::{
    claude,
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
    pub session: Option<claude::Session>,
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
        if a.agent != "auto" && a.agent != "claude" && a.agent != "shell" {
            bail!("agent must be auto, claude, or shell");
        }
        if a.agent == "shell" && a.session.is_some() {
            bail!("--session cannot be used with --agent shell");
        }
        let mut warnings = vec![];
        let session = if a.agent == "shell" {
            None
        } else if let Some(id) = &a.session {
            Some(claude::find_session(&home, &cwd, Some(id))?)
        } else {
            let mut sessions = claude::sessions(&home, &cwd)?;
            if sessions.is_empty() && cwd != root {
                sessions = claude::sessions(&home, &root)?;
            }
            if sessions.is_empty() {
                if a.agent == "claude" {
                    bail!("no Claude session found. Start Claude locally, or use --agent shell");
                }
                None
            } else if sessions.len() > 1 && !a.yes && std::io::stdin().is_terminal() {
                for (i, s) in sessions.iter().enumerate() {
                    println!(
                        "  {}. {} · {} · {}m ago",
                        i + 1,
                        claude::title(s),
                        s.id,
                        s.modified.elapsed().unwrap_or_default().as_secs() / 60
                    );
                }
                let answer = ask("Session [1]:")?;
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
        let cwd = if let Some(s) = &session {
            if s.transcript.parent() == Some(home.join(claude::projects_rel(&root)).as_path()) {
                root.clone()
            } else {
                cwd
            }
        } else {
            cwd
        };
        if !a.force && claude::is_running(&root) {
            bail!("Claude Code is still running in this project. Exit it first, or use --force");
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
            for rel in claude::session_paths(&home, &cwd, s) {
                agent_files.extend(user_files(&home, &rel)?);
            }
            for rel in claude::user_paths(&home) {
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
            names.extend(claude::AUTH_ENV.iter().map(|s| s.to_string()));
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
        if session.is_some()
            && !env
                .iter()
                .any(|(n, v)| claude::AUTH_ENV[..3].contains(&n.as_str()) && !v.is_empty())
        {
            warnings
                .push("Claude authentication is not forwarded. Log in after `beam attach`".into());
        }
        let setup = config.setup(&root);
        let mut tools = vec!["git".into(), "tmux".into(), "tar".into(), "gzip".into()];
        if session.is_some() {
            tools.push("claude".into());
        }
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
        crate::up::step(
            "session",
            self.session
                .as_ref()
                .map(|s| {
                    format!(
                        "Claude · {} · {} · {} turns",
                        claude::title(s),
                        s.id,
                        s.turns
                    )
                })
                .unwrap_or_else(|| "shell workspace (no agent session)".into()),
        );
        crate::up::step("target", &self.target);
        if let Some(n) = git::unpushed_count(&self.root) {
            crate::up::step("commits", format!("{n} unpushed"));
        }
        crate::up::step(
            "worktree",
            format!(
                "{} changed paths; complete reachable Git history also transfers",
                self.changed.len()
            ),
        );
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
        for pin in &self.versions {
            crate::up::step("toolchain", format!("{} {}", pin.tool, pin.version));
        }
        for warning in &self.warnings {
            println!("! {warning}");
        }
    }
}
