// ABOUTME: Plans and resumes durable uploads. Each remote phase can be retried.
use crate::{
    agent,
    config::{DEFAULT_IMAGE, DEFAULT_TIMEOUT},
    git, handoff,
    pack::Archive,
    plan::Plan,
    presentation, remote,
    sandbox::{Sandbox, Target},
    state::{Phase, ProjectLock, State},
    ui::{self, Hue},
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
    pub permission_mode: Option<String>,
    pub continue_with: Option<String>,
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}
pub fn step(label: &str, text: impl AsRef<str>) {
    ui::step(label, text.as_ref());
}

pub fn up(a: UpArgs) -> Result<()> {
    crate::transporter::direction(false);
    let cwd = a.path.canonicalize()?;
    let root = git::toplevel(&cwd)?;
    let home = home_dir()?;
    let lock = ProjectLock::acquire(&home, &root)?;
    if let Some(mut st) = State::load(&root)? {
        if a.dry_run {
            presentation::show(&st, None);
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
        let mut busy = ui::Busy::start();
        let started = std::time::Instant::now();
        if let Err(e) = continue_up(&mut st, &a) {
            busy.fail();
            return Err(saved_failure(&mut st, e));
        }
        drop(busy);
        arrived(&st, started);
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
        step("image", "building Docker image…");
        target.build_image(image)?;
    }
    if let Err(e) = ui::task("check", "checking destination prerequisites…", || {
        target.preflight(
            image,
            &plan.preflight_tools(),
            plan.preflight_versions(),
            &root,
        )
    }) {
        if !a.yes
            && !a.dry_run
            && std::io::stdin().is_terminal()
            && image == DEFAULT_IMAGE
            && e.to_string().contains("image")
        {
            confirm("Build the default Docker image now?", false)?;
            target.build_image(image)?;
            target
                .preflight(
                    image,
                    &plan.preflight_tools(),
                    plan.preflight_versions(),
                    &root,
                )
                .map_err(|e| plan.target_failure(e))?;
        } else {
            return Err(plan.target_failure(e));
        }
    }
    if a.dry_run {
        ui::success("Plan checked. No transfer was created.");
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
        ui::warn(format!(
            "Possible secrets in transferred files: {}",
            hits.iter()
                .map(|(k, n)| format!("{n}× {k}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
        confirm(&question, true)?;
    }
    let mut busy = ui::Busy::start();
    let started = std::time::Instant::now();
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
    let sent = ui::task("snapshot", "saving workspace snapshot…", || {
        git::snapshot(&root, &up_ref, Some(&dir.join("repo.bundle")), None)
    })?;
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
        agent: plan.agent.id().into(),
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
    if let Err(e) = continue_up(&mut st, &a) {
        busy.fail();
        return Err(saved_failure(&mut st, e));
    }
    drop(busy);
    arrived(&st, started);
    drop(lock);
    maybe_attach(&st, a.detach)
}

/// Record a failed upload, then show the error, the saved state, and the next step in that order.
fn saved_failure(st: &mut State, e: anyhow::Error) -> anyhow::Error {
    st.last_error = Some(format!("{e:#}"));
    let _ = st.save();
    eprintln!("{}", ui::error(&format!("{e:#}")));
    eprintln!("Transfer saved.");
    if st.phase != Phase::Remote {
        presentation::show(st, None);
    }
    ui::Reported.into()
}

/// Signal a slow arrival and sometimes add flavor text, but only when the session is running.
fn arrived(st: &State, started: std::time::Instant) {
    if st.phase == Phase::Remote {
        ui::arrived(started, &format!("beam: workspace is on {}", st.describe()));
        ui::flavor(false);
    }
}

fn build_archive(plan: &Plan, st: &State) -> Result<()> {
    let adapter = agent::get(&st.agent)?;
    let defaults = adapter.defaults(&plan.home, &plan.cwd)?;
    let mut head = handoff::head(&handoff::Facts {
        from: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        to: plan.target.clone(),
        extras: &plan
            .extras
            .iter()
            .filter(|p| plan.root.join(p).exists())
            .cloned()
            .collect::<Vec<_>>(),
        env_names: &st.env_names,
        removed_settings: &defaults.removed_settings,
    });
    head.push_str(&plan.config.task.handoff());
    if let Some(text) = &plan.continue_with {
        head.push_str(&handoff::continue_note(text));
    }
    head.push_str("\nReport task progress with: sh \"$BEAM_REPORT\" working|waiting|finished|failed \"brief evidence\". These are agent reports, not independent verification.\n");
    util::atomic_write(
        &st.dir().join("task.json"),
        &serde_json::to_vec_pretty(&plan.config.task)?,
    )?;
    let resume = adapter.resume_fn(&st.session_id, plan.permission_mode.as_deref());
    let run = remote::run_script(&remote::RunVars {
        stage: &st.stage,
        cwd: &st.agent_cwd.to_string_lossy(),
        home: &st.remote_home,
        handoff_head: &head,
        setup: &plan.setup,
        verify: &plan.config.sandbox.verify,
        reuse_setup: plan.config.sandbox.reuse_setup,
        setup_inputs: &plan.config.sandbox.setup_inputs,
        tools: &plan.tools,
        versions: &plan.versions,
        environment_repair: adapter.capabilities().environment_repair,
        resume_fn: &resume,
    });
    let mut archive = Archive::create(&st.dir().join("snapshot.tar.gz"))?;
    archive.add_path("repo.bundle", &st.dir().join("repo.bundle"))?;
    archive.add_bytes("snapshot.sh", git::SNAPSHOT_SH.as_bytes())?;
    archive.add_bytes("run.sh", run.as_bytes())?;
    archive.add_bytes("report.sh", include_bytes!("../scripts/report.sh"))?;
    archive.add_bytes(
        "return.sh",
        remote::return_script(
            &st.stage,
            &st.tmux,
            adapter.graceful_stop(),
            &crate::down::pack_script(st)?,
        )
        .as_bytes(),
    )?;
    archive.add_bytes("project", st.project_root.to_string_lossy().as_bytes())?;
    for name in &plan.extra_files {
        archive.add_path(&format!("extras/{name}"), &plan.root.join(name))?;
    }
    for name in &plan.agent_files {
        archive.add_path(
            &format!("home/{name}"),
            &plan.home.join(name).canonicalize()?,
        )?;
    }
    // An adapter default replaces a user file with the same path, such as the local beam skill.
    for name in plan
        .defaults
        .iter()
        .filter(|name| !defaults.files.iter().any(|(path, _)| path == *name))
    {
        archive.add_path(
            &format!("defaults/{name}"),
            &plan.home.join(name).canonicalize()?,
        )?;
    }
    for (name, bytes) in defaults.files {
        util::relative_path(&name)?;
        archive.add_bytes(&format!("defaults/{name}"), &bytes)?;
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
        let created = ui::task("sandbox", "creating sandbox…", || {
            Ok(if let Some(id) = &a.recover_sandbox {
                match target {
                    Target::Steel { .. } => Sandbox::Steel { id: id.clone() },
                    Target::Daytona { .. } => Sandbox::Daytona {
                        id: crate::daytona::recover(id, &st.transfer_id)?,
                    },
                    _ => bail!("--recover-sandbox is only for interrupted cloud allocation"),
                }
            } else {
                target.create(&crate::sandbox::CreateOpts {
                    name: &format!("beam-{}", st.transfer_id),
                    image: &st.image,
                    session_id: &st.transfer_id,
                    timeout_secs: st.timeout_secs,
                    receipt: &st.dir().join("allocation.json"),
                })?
            })
        })?;
        st.sandbox = Some(created);
        st.advance(Phase::Created)?;
    }
    let sb = st.sandbox()?.clone();
    if st.phase == Phase::Created {
        if matches!(&sb, Sandbox::Steel { .. } | Sandbox::Daytona { .. }) {
            match &sb {
                Sandbox::Steel { id } => {
                    ui::task("setup", "preparing Steel tools", || {
                        crate::steel::ready(id)?;
                        sb.exec(crate::steel::BOOTSTRAP_SH)
                    })?;
                }
                Sandbox::Daytona { .. } => {
                    ui::task("setup", "preparing Daytona tools", || {
                        sb.wake()?;
                        sb.exec(include_str!("../scripts/daytona_bootstrap.sh"))
                    })?;
                }
                _ => unreachable!(),
            }
            // Daytona starts with its own HOME. Install user tools under the HOME
            // that the transferred launcher will use, and check that same path.
            let cloud_exec = |script: &str| -> Result<String> {
                if matches!(&sb, Sandbox::Daytona { .. }) {
                    sb.exec(&format!(
                        "export HOME={}; mkdir -p \"$HOME\";\n{script}",
                        util::sh_quote(&st.remote_home)
                    ))
                } else {
                    sb.exec(script)
                }
            };
            let bootstrap = agent::get(&st.agent)?.bootstrap();
            if !bootstrap.is_empty() {
                cloud_exec(bootstrap)?;
            }
            let checks: serde_json::Value =
                serde_json::from_slice(&std::fs::read(st.dir().join("tools.json"))?)?;
            let tools: Vec<String> = serde_json::from_value(checks["tools"].clone())?;
            let versions: Vec<crate::config::ToolVersion> =
                serde_json::from_value(checks["versions"].clone())?;
            let adapter = agent::get(&st.agent)?;
            let (tools, versions) = if adapter.capabilities().environment_repair {
                (agent::transfer_tools(adapter), vec![])
            } else {
                (tools, versions)
            };
            cloud_exec(&crate::sandbox::prerequisite_script(&tools, &versions)).context(
                "required startup tools are missing; repair the saved sandbox before retrying",
            )?;
        }
        ui::task("prepare", "preparing remote directories…", || {
            sb.exec(&remote::prepare(
                &st.remote_home,
                &st.project_root.to_string_lossy(),
                &st.stage,
                &st.transfer_id,
            ))
        })?;
        st.advance(Phase::Prepared)?;
    }
    if st.phase == Phase::Prepared {
        let started = std::time::Instant::now();
        ui::task("upload", "uploading workspace…", || {
            sb.exec_file(
                &remote::unpack(&st.stage),
                &st.dir().join("snapshot.tar.gz"),
            )
        })?;
        step(
            "upload",
            util::size_and_rate(
                std::fs::metadata(st.dir().join("snapshot.tar.gz"))?.len(),
                started.elapsed(),
            ),
        );
        st.advance(Phase::Uploaded)?;
    }
    if st.phase == Phase::Uploaded {
        let script = crate::pack::launcher(&st.dir().join("snapshot.tar.gz"))?;
        let checks: serde_json::Value =
            serde_json::from_slice(&std::fs::read(st.dir().join("tools.json"))?)?;
        let tools = serde_json::from_value::<Vec<String>>(checks["tools"].clone())?;
        let versions =
            serde_json::from_value::<Vec<crate::config::ToolVersion>>(checks["versions"].clone())?;
        if let Some(script) = remote::upgrade_run_script(
            &script,
            &crate::sandbox::prerequisite_script(&tools, &versions),
            agent::get(&st.agent)?.capabilities().environment_repair,
        )? {
            let dest = util::sh_quote(&format!("{}/run.sh", st.stage));
            sb.exec_input(
                &format!("umask 077; cat > {dest}.new && mv {dest}.new {dest}"),
                script.as_bytes(),
            )?;
        }
        ui::task("restore", "restoring workspace…", || {
            sb.exec(&remote::install_cli(
                remote::beam_root(&st.stage),
                !matches!(sb, Sandbox::Ssh { .. }),
            ))?;
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
            }))
        })?;
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
        let status = sb.process_status(&st.stage, &st.tmux)?;
        if status == "needs-attention" {
            sb.exec(&remote::retry_setup(&st.stage, &st.tmux))?;
            st.advance(Phase::Starting)?;
        } else {
            presentation::show(st, Some(&crate::monitor::snapshot(st)?));
            return Ok(());
        }
    }
    if st.phase == Phase::Starting {
        // The wait ends with a settled status, or with None after 10 seconds.
        let settled = ui::task("launch", "starting remote agent and checks…", || {
            sb.exec(&remote::start_tmux(&st.stage, &st.tmux))?;
            let start = std::time::Instant::now();
            loop {
                let status = sb.process_status(&st.stage, &st.tmux)?;
                if status == "running"
                    || status == "repairing"
                    || status == "needs-attention"
                    || status.starts_with("stopped")
                {
                    return Ok(Some(status));
                }
                if start.elapsed().as_secs() >= 10 {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        })?;
        if settled.is_some() {
            st.advance(Phase::Remote)?;
        }
        presentation::show(st, Some(&crate::monitor::snapshot(st)?));
        if let Some(status) = settled
            && (status == "needs-attention" || status.starts_with("stopped"))
        {
            bail!("remote session {status}. After fixing setup, run `beam` again");
        }
    }
    Ok(())
}

fn maybe_attach(st: &State, detach: bool) -> Result<()> {
    if !detach && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        println!(
            "Opening remote terminal. Detach with {}, then {}. Return work with {}.",
            ui::bold(Hue::Turquoise, "Ctrl-b"),
            ui::bold(Hue::Turquoise, "d"),
            ui::bold(Hue::Blue, "beam down")
        );
        attach(st)?;
    }
    Ok(())
}

/// Why a transfer cannot continue after `beam down` in the sandbox.
const SANDBOX_RETURN: &str = "`beam down` ran in the sandbox, and its work is packed for the return. Run `beam down` to bring it home";

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
        "if [ -f {3} ]; then printf return-requested; elif tmux has-session -t {0} 2>/dev/null; then printf '%s' {0}; else tmux has-session -t {1} 2>/dev/null || tmux new-session -d -s {1} {2}; printf '%s' {1}; fi",
        util::sh_quote(&st.tmux),
        util::sh_quote(&repair),
        util::sh_quote(&command),
        util::sh_quote(&format!("{}/return-requested", st.stage))
    );
    let terminal = sb.exec(&script)?;
    if terminal == "return-requested" {
        bail!("{SANDBOX_RETURN}");
    }
    if terminal != st.tmux && terminal != repair {
        bail!("cannot prepare the remote terminal");
    }
    drop(lock);
    let result = sb.interactive(&format!("tmux attach -t {}", util::sh_quote(&terminal)));
    ui::say("Remote terminal closed.");
    // Detachment or exit is not proof of completion. Refresh state after the live terminal.
    if let Ok(snapshot) = crate::monitor::snapshot(&st) {
        presentation::show(&st, Some(&snapshot));
    }
    result
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
            Target::Steel { .. } | Target::Daytona { .. } => bail!(
                "Cloud allocation may have completed. Run `beam --recover-sandbox ID` before removing this transfer"
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

pub fn confirm(question: &str, default_yes: bool) -> Result<()> {
    let choices = if default_yes { "[Y/n]" } else { "[y/N]" };
    let answer = crate::plan::ask(&format!("{question} {choices}"))?;
    if !(matches!(answer.as_str(), "y" | "Y" | "yes") || (default_yes && answer.is_empty())) {
        bail!("stopped; no transfer was started");
    }
    Ok(())
}

pub fn tempdir() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new().prefix("beam-").tempdir()?)
}

/// Restart is explicit: a live session is never terminated by this command.
pub fn restart(path: &std::path::Path) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let _lock = ProjectLock::acquire(&home_dir()?, &root)?;
    let mut st = State::load(&root)?.context("there is no active transfer")?;
    if !matches!(st.phase, Phase::Remote | Phase::Starting) {
        bail!("only a remote session can restart");
    }
    let sb = st.sandbox()?;
    sb.wake()?;
    let status = sb.process_status(&st.stage, &st.tmux)?;
    if !status.starts_with("stopped") && status != "needs-attention" {
        bail!("remote session is {status}; stop it before restarting");
    }
    if sb.exec(&remote::return_progress(&st.stage))? != "none" {
        bail!("{SANDBOX_RETURN}");
    }
    sb.exec(&remote::with_vars(
        &[("S", &st.stage), ("T", &st.tmux)],
        r#"
tmux kill-session -t "$T" 2>/dev/null || true
rm -f "$S/started" "$S/agent.exit" "$S/go"
rmdir "$S/run-lock" "$S/check-lock" 2>/dev/null || true
printf preparing > "$S/phase"
"#,
    ))?;
    st.advance(Phase::Starting)?;
    st.sandbox()?
        .exec(&remote::start_tmux(&st.stage, &st.tmux))?;
    ui::success("Remote setup and project checks restarted.");
    ui::next("beam status --watch");
    Ok(())
}
