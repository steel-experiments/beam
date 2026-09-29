// ABOUTME: Plans and resumes durable uploads. Each remote phase can be retried.
use crate::{
    claude,
    config::{DEFAULT_IMAGE, DEFAULT_TIMEOUT},
    git, handoff,
    pack::Archive,
    plan::Plan,
    remote,
    sandbox::{Sandbox, Target},
    state::{Phase, ProjectLock, State},
    util,
};
use anyhow::{Context, Result, bail};
use std::io::IsTerminal;
use std::path::PathBuf;

pub struct UpArgs {
    pub path: PathBuf,
    pub to: Option<String>,
    pub session: Option<String>,
    pub yes: bool,
    pub detach: bool,
    pub force: bool,
    pub allow_large: bool,
    pub agent: String,
    pub dry_run: bool,
    pub build_image: bool,
    pub recover_sandbox: Option<String>,
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}
pub fn step(label: &str, text: impl AsRef<str>) {
    println!("▸ {label:<10} {}", text.as_ref());
}

pub fn up(a: UpArgs) -> Result<()> {
    let cwd = a.path.canonicalize()?;
    let root = git::toplevel(&cwd)?;
    let home = home_dir()?;
    let lock = ProjectLock::acquire(&home, &root)?;
    if let Some(mut st) = State::load(&root)? {
        if a.dry_run {
            println!(
                "{} · {}. Run `beam` to continue",
                st.phase.label(),
                st.describe()
            );
            return Ok(());
        }
        if matches!(
            st.phase,
            Phase::Returning | Phase::Downloaded | Phase::Applied
        ) {
            bail!("this transfer is returning. Run `beam down` to continue");
        }
        if st.phase == Phase::Retained {
            bail!(
                "the session is home and the sandbox is retained. Use `beam attach` to inspect it or `beam kill --yes` to remove it before another transfer"
            );
        }
        if a.recover_sandbox.is_some() && st.phase != Phase::Allocating {
            bail!("--recover-sandbox is only needed for an interrupted allocation");
        }
        if a.to.as_ref().is_some_and(|t| t != &st.target) {
            bail!(
                "this transfer already targets {}. Finish it with `beam down` or remove it with `beam kill --yes`",
                st.target
            );
        }
        st.save()?;
        let result = continue_up(&mut st, &a);
        if let Err(e) = &result {
            st.last_error = Some(format!("{e:#}"));
            let _ = st.save();
        }
        result?;
        drop(lock);
        return maybe_attach(&st, a.detach);
    }
    if a.recover_sandbox.is_some() {
        bail!("there is no interrupted transfer to recover");
    }
    let plan = Plan::build(&a)?;
    plan.show();
    let target = Target::parse(&plan.target)?;
    let image = plan
        .config
        .sandbox
        .image
        .as_deref()
        .unwrap_or(DEFAULT_IMAGE);
    if a.build_image {
        if a.dry_run {
            bail!("--build-image cannot be used with --dry-run");
        }
        target.build_image(image)?;
    }
    if let Err(e) = target.preflight(image, &plan.tools, &plan.versions, &root) {
        if !a.yes
            && !a.dry_run
            && std::io::stdin().is_terminal()
            && image == DEFAULT_IMAGE
            && e.to_string().contains("image")
        {
            confirm("Build the default Docker image now?")?;
            target.build_image(image)?;
            target.preflight(image, &plan.tools, &plan.versions, &root)?;
        } else {
            return Err(e);
        }
    }
    if a.dry_run {
        println!("✓ Plan checked. No transfer was created.");
        return Ok(());
    }
    let mut hits = std::collections::BTreeMap::<String, usize>::new();
    for (base, files) in [
        (&plan.home, &plan.agent_files),
        (&plan.root, &plan.extra_files),
    ] {
        for name in files {
            if let Ok(text) = std::fs::read_to_string(base.join(name)) {
                for (label, count) in crate::scan::scan(&text) {
                    *hits.entry(label.into()).or_default() += count;
                }
            }
        }
    }
    if !hits.is_empty() {
        println!(
            "! Possible secrets in transferred files: {}",
            hits.iter()
                .map(|(k, n)| format!("{n}× {k}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let question = format!(
        "Send this workspace{} to {}?",
        if plan.session.is_some() {
            " and transcript"
        } else {
            ""
        },
        plan.target
    );
    if !a.yes {
        confirm(&question)?;
    }
    git::exclude_beam_dir(&root)?;
    let id = format!(
        "{}-{}",
        util::now_unix(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .subsec_nanos()
    );
    let dir = home.join(".beam/transfers").join(&id);
    util::private_dir(&dir)?;
    let stage_base = target
        .login_home()?
        .unwrap_or_else(|| home.to_string_lossy().into_owned());
    let stage = format!("{stage_base}/.beam/remote/{id}");
    let remote_home = if matches!(target, Target::Ssh { .. }) {
        format!("{stage}/home")
    } else {
        home.to_string_lossy().into_owned()
    };
    let session_id = plan
        .session
        .as_ref()
        .map(|s| s.id.clone())
        .unwrap_or_else(|| format!("workspace-{id}"));
    let up_ref = format!("refs/beam/{id}/up");
    let sent = git::snapshot(&root, &up_ref, Some(&dir.join("repo.bundle")), None)?;
    let return_files: Vec<_> = plan
        .extra_files
        .iter()
        .filter(|p| {
            plan.return_extras
                .iter()
                .any(|r| *p == r || p.starts_with(&format!("{r}/")))
        })
        .cloned()
        .collect();
    let mut st = State {
        version: 2,
        transfer_id: id.clone(),
        session_id,
        agent: if plan.session.is_some() {
            "claude"
        } else {
            "shell"
        }
        .into(),
        project_root: root,
        agent_cwd: plan.cwd.clone(),
        home: home.clone(),
        target: plan.target.clone(),
        sandbox: None,
        stage,
        remote_home,
        tmux: format!("beam-{id}"),
        sent,
        sent_files: crate::plan::hashes(&home, &plan.agent_files)?,
        sent_extras: crate::plan::hashes(&plan.root, &return_files)?,
        return_extras: plan.return_extras.clone(),
        env_names: plan.env.iter().map(|(k, _)| k.clone()).collect(),
        image: image.into(),
        timeout_secs: util::parse_duration(
            plan.config
                .sandbox
                .timeout
                .as_deref()
                .unwrap_or(DEFAULT_TIMEOUT),
        )?,
        phase: Phase::Planned,
        recovery: None,
        conflicts: vec![],
        last_error: None,
        created_at: util::now_unix(),
    };
    build_archive(&plan, &st)?;
    st.save()?;
    let result = continue_up(&mut st, &a);
    if let Err(e) = &result {
        st.last_error = Some(format!("{e:#}"));
        let _ = st.save();
        eprintln!(
            "Transfer saved. Run `beam` to retry, or `beam kill --yes` to remove its resources."
        );
    }
    result?;
    drop(lock);
    maybe_attach(&st, a.detach)
}

fn build_archive(plan: &Plan, st: &State) -> Result<()> {
    let settings = if st.agent == "claude" {
        match std::fs::read_to_string(plan.home.join(".claude/settings.json")) {
            Ok(text) => Some(claude::filter_settings(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        }
    } else {
        None
    };
    let removed = settings
        .as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_default();
    let head = handoff::head(&handoff::Facts {
        from: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        to: plan.target.clone(),
        extras: &plan
            .extras
            .iter()
            .filter(|p| plan.root.join(p).exists())
            .cloned()
            .collect::<Vec<_>>(),
        env_names: &st.env_names,
        removed_settings: &removed,
    });
    let resume = if st.agent == "claude" {
        claude::resume_fn(&st.session_id)
    } else {
        "resume() { sh -i; }".into()
    };
    let run = remote::run_script(&remote::RunVars {
        stage: &st.stage,
        cwd: &st.agent_cwd.to_string_lossy(),
        home: &st.remote_home,
        handoff_head: &head,
        setup: &plan.setup,
        resume_fn: &resume,
    });
    let mut archive = Archive::create(&st.dir().join("snapshot.tar.gz"))?;
    archive.add_path("repo.bundle", &st.dir().join("repo.bundle"))?;
    archive.add_bytes("snapshot.sh", git::SNAPSHOT_SH.as_bytes())?;
    archive.add_bytes("run.sh", run.as_bytes())?;
    for name in &plan.extra_files {
        archive.add_path(&format!("extras/{name}"), &plan.root.join(name))?;
    }
    for name in &plan.agent_files {
        archive.add_path(
            &format!("home/{name}"),
            &plan.home.join(name).canonicalize()?,
        )?;
    }
    for name in &plan.defaults {
        archive.add_path(
            &format!("defaults/{name}"),
            &plan.home.join(name).canonicalize()?,
        )?;
    }
    if let Some((json, _)) = settings {
        archive.add_bytes("defaults/.claude/settings.json", json.as_bytes())?;
    }
    if st.agent == "claude" {
        archive.add_bytes(
            "defaults/.claude.json",
            claude::default_claude_json(&plan.cwd).as_bytes(),
        )?;
    }
    archive.add_bytes("manifest.json", &serde_json::to_vec_pretty(&serde_json::json!({"version":2,"transfer":st.transfer_id,"git":st.sent,"extras":plan.extras,"return_extras":plan.return_extras,"env_keys":st.env_names,"setup":plan.setup,"agent":st.agent,"session_id":st.session_id}))?)?;
    step("snapshot", util::human_size(archive.finish()?));
    util::atomic_write(
        &st.dir().join("tools.json"),
        &serde_json::to_vec(&serde_json::json!({"tools":plan.tools,"versions":plan.versions}))?,
    )?;
    Ok(())
}

fn continue_up(st: &mut State, a: &UpArgs) -> Result<()> {
    let target = Target::parse(&st.target)?;
    if st.phase == Phase::Planned {
        st.advance(Phase::Allocating)?;
    }
    if st.phase == Phase::Allocating {
        st.sandbox = Some(if let Some(id) = &a.recover_sandbox {
            if !matches!(target, Target::Steel { .. }) {
                bail!("--recover-sandbox is only for interrupted Steel allocation");
            }
            Sandbox::Steel { id: id.clone() }
        } else {
            target.create(&crate::sandbox::CreateOpts {
                name: &format!("beam-{}", st.transfer_id),
                image: &st.image,
                session_id: &st.transfer_id,
                timeout_secs: st.timeout_secs,
                receipt: &st.dir().join("allocation.json"),
            })?
        });
        st.advance(Phase::Created)?;
    }
    let sb = st.sandbox()?.clone();
    if st.phase == Phase::Created {
        if let Sandbox::Steel { id } = &sb {
            crate::steel::ready(id)?;
            step("setup", "preparing Steel tools");
            crate::steel::exec(id, crate::steel::BOOTSTRAP_SH)?;
            let checks: serde_json::Value =
                serde_json::from_slice(&std::fs::read(st.dir().join("tools.json"))?)?;
            let tools: Vec<String> = serde_json::from_value(checks["tools"].clone())?;
            let versions: Vec<crate::config::ToolVersion> =
                serde_json::from_value(checks["versions"].clone())?;
            sb.exec(&crate::sandbox::prerequisite_script(&tools, &versions))?;
        }
        sb.exec(&remote::prepare(
            &st.remote_home,
            &st.project_root.to_string_lossy(),
            &st.stage,
            &st.transfer_id,
        ))?;
        st.advance(Phase::Prepared)?;
    }
    if st.phase == Phase::Prepared {
        sb.exec_file(
            &remote::unpack(&st.stage),
            &st.dir().join("snapshot.tar.gz"),
        )?;
        step("upload", "done");
        st.advance(Phase::Uploaded)?;
    }
    if st.phase == Phase::Uploaded {
        sb.exec(&remote::restore(&remote::RestoreVars {
            stage: &st.stage,
            project: &st.project_root.to_string_lossy(),
            home: &st.remote_home,
            refname: &format!("refs/beam/{}/up", st.transfer_id),
            branch: &st.sent.branch,
            head: &st.sent.head,
            idx_tree: &st.sent.idx_tree,
            wt_tree: &st.sent.wt_tree,
            origin: &git::config_get(&st.project_root, "remote.origin.url"),
            git_name: &git::config_get(&st.project_root, "user.name"),
            git_email: &git::config_get(&st.project_root, "user.email"),
        }))?;
        st.advance(Phase::Restored)?;
    }
    if st.phase == Phase::Restored {
        let env: Result<Vec<String>> = st.env_names.iter().map(|name| {
            let value = std::env::var(name).with_context(|| format!("{name} was present in the transfer plan but is missing now. Export it and run `beam` again"))?;
            Ok(format!("{name}={}\n", util::sh_quote(&value)))
        }).collect();
        sb.exec_input(&remote::write_env(&st.stage), env?.concat().as_bytes())?;
        st.advance(Phase::Starting)?;
    }
    if st.phase == Phase::Remote {
        sb.wake()?;
        let status = sb.exec(&remote::agent_status(&st.stage, &st.tmux))?;
        if status == "needs-attention" {
            sb.exec(&remote::retry_setup(&st.stage, &st.tmux))?;
            st.advance(Phase::Starting)?;
        } else {
            step(
                "session",
                format!("already beamed to {}; {status}", sb.describe()),
            );
            return Ok(());
        }
    }
    if st.phase == Phase::Starting {
        sb.exec(&remote::start_tmux(&st.stage, &st.tmux))?;
        let start = std::time::Instant::now();
        loop {
            let status = sb.exec(&remote::agent_status(&st.stage, &st.tmux))?;
            if status == "running" {
                st.advance(Phase::Remote)?;
                println!(
                    "✓ Session is live on {}. The remote process is running.",
                    sb.describe()
                );
                println!("  Local files remain editable. Avoid running the same agent locally.");
                break;
            }
            if status == "needs-attention" || status.starts_with("stopped") {
                st.advance(Phase::Remote)?;
                bail!(
                    "remote session {status}. Run `beam logs` for details or `beam attach` to inspect it. After fixing setup, run `beam` again"
                );
            }
            if start.elapsed().as_secs() >= 10 {
                println!(
                    "Setup is still running. Use `beam status` or `beam logs` to follow progress."
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    println!("  beam attach      open the session (detach with Ctrl-b d)");
    println!("  beam down        bring the work home");
    Ok(())
}

fn maybe_attach(st: &State, detach: bool) -> Result<()> {
    if !detach && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        attach(st)?;
    }
    Ok(())
}

pub fn attach(st: &State) -> Result<()> {
    let lock = ProjectLock::acquire(&st.home, &st.project_root)?;
    let st = State::load(&st.project_root)?.context("this transfer is no longer active")?;
    if matches!(
        st.phase,
        Phase::Returning | Phase::Downloaded | Phase::Applied
    ) {
        bail!("the transfer is returning. Finish `beam down` before attaching");
    }
    let sb = st.sandbox()?;
    sb.wake()?;
    // A separate repair terminal cannot be mistaken for a running agent.
    let repair = format!("{}-repair", st.tmux);
    let command = format!(
        "if [ -d {0} ]; then export HOME={0}; fi; if [ -d {1} ]; then cd {1}; else cd \"$HOME\"; fi; exec sh -i",
        util::sh_quote(&st.remote_home),
        util::sh_quote(&st.agent_cwd.to_string_lossy())
    );
    let script = format!(
        "if tmux has-session -t {0} 2>/dev/null; then printf '%s' {0}; else tmux has-session -t {1} 2>/dev/null || tmux new-session -d -s {1} {2}; printf '%s' {1}; fi",
        util::sh_quote(&st.tmux),
        util::sh_quote(&repair),
        util::sh_quote(&command)
    );
    let terminal = sb.exec(&script)?;
    if terminal != st.tmux && terminal != repair {
        bail!("cannot prepare the remote terminal");
    }
    drop(lock);
    sb.interactive(&format!("tmux attach -t {}", util::sh_quote(&terminal)))
}

pub fn cleanup(st: &State) -> Result<()> {
    if let Some(sb) = &st.sandbox {
        if matches!(sb, Sandbox::Ssh { .. }) && st.transfer_id.starts_with("legacy-") {
            bail!(
                "this older SSH transfer has no ownership markers. Its files remain untouched. Remove its remote files manually, then use `beam forget --yes`"
            );
        }
        let owner = if st.transfer_id.starts_with("legacy-") {
            &st.session_id
        } else {
            &st.transfer_id
        };
        sb.destroy(
            &remote::cleanup(
                &st.stage,
                &st.project_root.to_string_lossy(),
                &st.transfer_id,
                &st.tmux,
                !matches!(
                    st.phase,
                    Phase::Planned | Phase::Allocating | Phase::Created
                ),
            ),
            owner,
        )?;
    } else if st.phase == Phase::Allocating {
        match Target::parse(&st.target)? {
            Target::Steel { .. } => bail!(
                "Steel allocation may have completed. Run `beam --recover-sandbox ID` before removing this transfer"
            ),
            Target::Docker { ssh_host } => Sandbox::Docker {
                ssh_host,
                container: format!("beam-{}", st.transfer_id),
            }
            .destroy("", &st.transfer_id)?,
            Target::Ssh { .. } => {}
        }
    }
    Ok(())
}

pub fn confirm(question: &str) -> Result<()> {
    let answer = crate::plan::ask(&format!("{question} [y/N]"))?;
    if !matches!(answer.as_str(), "y" | "Y" | "yes") {
        bail!("stopped; no transfer was started");
    }
    Ok(())
}

pub fn tempdir() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new().prefix("beam-").tempdir()?)
}
