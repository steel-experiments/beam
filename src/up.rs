// ABOUTME: "beam": moves the agent session from this machine to a sandbox.
// ABOUTME: Plan → confirm → snapshot → create sandbox → upload → restore → start the agent in tmux.

use crate::claude;
use crate::config::{Config, DEFAULT_IMAGE, DEFAULT_MAX_FILE_SIZE, DEFAULT_TIMEOUT};
use crate::git;
use crate::handoff;
use crate::pack::Archive;
use crate::remote;
use crate::sandbox::{Sandbox, Target};
use crate::scan;
use crate::state::State;
use crate::util::{human_size, now_unix, parse_size, sh_quote, sha256_file};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

pub struct UpArgs {
    pub path: PathBuf,
    pub to: Option<String>,
    pub session: Option<String>,
    pub yes: bool,
    pub detach: bool,
    pub force: bool,
    pub allow_large: bool,
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
    let home = home_dir()?;
    let cwd = a
        .path
        .canonicalize()
        .with_context(|| format!("{} does not exist", a.path.display()))?;
    let root = git::toplevel(&cwd)?;
    git::ensure_has_commits(&root)?;
    if let Some(s) = State::load(&root)? {
        bail!(
            "this project is already beamed to {} (session {}). Use `beam down` or `beam kill` first",
            s.sandbox.describe(),
            s.session_id
        );
    }
    let cfg = Config::load(&root)?;
    let target_s = a.to.clone().or(cfg.beam.to.clone()).context(
        "no target. Use --to docker|docker+ssh://HOST|ssh://HOST|steel, or set [beam] to = ... in beam.toml",
    )?;
    let target = Target::parse(&target_s)?;

    // Agent session.
    let session = claude::find_session(&home, &cwd, a.session.as_deref())?;
    if !a.force && claude::is_running(&cwd) {
        bail!(
            "Claude Code is still running in {}. Exit it first (or use --force)",
            cwd.display()
        );
    }
    let age = session
        .modified
        .elapsed()
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);
    step(
        "agent",
        format!(
            "claude  session {} (updated {age} min ago, {} turns)",
            session.id, session.turns
        ),
    );

    // Git worktree.
    let changed = git::changed_files(&root)?;
    let max = parse_size(
        cfg.files
            .max_file_size
            .as_deref()
            .unwrap_or(DEFAULT_MAX_FILE_SIZE),
    )?;
    let large: Vec<&(String, u64)> = changed.iter().filter(|(_, n)| *n > max).collect();
    if !large.is_empty() && !a.allow_large {
        let list: Vec<String> = large
            .iter()
            .map(|(f, n)| format!("  {f} ({})", human_size(*n)))
            .collect();
        bail!(
            "these files are larger than {}:\n{}\nAdd them to .gitignore, or use --allow-large",
            human_size(max),
            list.join("\n")
        );
    }
    let unpushed = match git::unpushed_count(&root) {
        Some(n) => format!("+{n} unpushed commits"),
        None => "no upstream".into(),
    };
    let head_name = crate::util::run(git::git(&root).args(["rev-parse", "--abbrev-ref", "HEAD"]))
        .unwrap_or_default();
    let head_short = crate::util::run(git::git(&root).args(["rev-parse", "--short", "HEAD"]))
        .unwrap_or_default();
    step("repo", format!("{head_name} @ {head_short}  {unpushed}"));
    step(
        "worktree",
        format!("{} changed or untracked files", changed.len()),
    );

    // Extras, env, settings.
    let extras: Vec<String> = cfg
        .extras()
        .into_iter()
        .filter(|e| root.join(e).exists())
        .collect();
    if !extras.is_empty() {
        step("extras", extras.join(", "));
    }
    let mut env_names: Vec<String> = claude::AUTH_ENV.iter().map(|s| s.to_string()).collect();
    env_names.extend(cfg.env.forward.iter().cloned());
    env_names.dedup();
    let env: Vec<(String, String)> = env_names
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
        .collect();
    let env_found: Vec<String> = env.iter().map(|(k, _)| k.clone()).collect();
    let missing_fwd: Vec<&String> = cfg
        .env
        .forward
        .iter()
        .filter(|k| !env_found.contains(k))
        .collect();
    step(
        "env",
        if env_found.is_empty() {
            "none".into()
        } else {
            env_found.join(", ")
        },
    );
    if !missing_fwd.is_empty() {
        println!(
            "! these env vars are in beam.toml but not set here: {}",
            join(&missing_fwd)
        );
    }
    if !claude::AUTH_ENV
        .iter()
        .any(|k| env_found.iter().any(|f| f == k))
    {
        println!(
            "! no Claude auth env var is set. Claude Code in the sandbox will ask you to log in."
        );
        println!("  Tip: run `claude setup-token` and export CLAUDE_CODE_OAUTH_TOKEN.");
    }
    let setup = cfg.setup(&root);
    step(
        "setup",
        if setup.is_empty() {
            "none".into()
        } else {
            setup.join(" && ")
        },
    );

    // Secret scan of what leaves this machine.
    let mut text = std::fs::read_to_string(&session.transcript).unwrap_or_default();
    for e in &extras {
        text.push_str(&std::fs::read_to_string(root.join(e)).unwrap_or_default());
    }
    let hits = scan::scan(&text);
    let mut warn = format!("the transcript and extras go to {target_s}.");
    if !hits.is_empty() {
        let h: Vec<String> = hits.iter().map(|(l, n)| format!("{n}× {l}")).collect();
        warn = format!(
            "possible secrets found ({}). They go to {target_s}.",
            h.join(", ")
        );
    }
    if !a.yes {
        confirm(&format!("! {warn} Continue?"))?;
    } else {
        println!("! {warn}");
    }

    // Snapshot.
    git::exclude_beam_dir(&root)?;
    let tmp = tempdir()?;
    let bundle = tmp.join("repo.bundle");
    let up_ref = format!("refs/beam/{}/up", session.id);
    let sent = git::snapshot(&root, &up_ref, Some(&bundle), None)?;

    let short = &session.id[..session.id.len().min(8)];
    let name = format!("beam-{short}-{}", now_unix() % 100_000);
    let home_s = home.to_string_lossy().to_string();
    let stage = format!("{home_s}/.beam/{}", session.id);
    let root_s = root.to_string_lossy().to_string();
    let cwd_s = cwd.to_string_lossy().to_string();

    let (removed_settings, settings_json) =
        match std::fs::read_to_string(home.join(".claude/settings.json")) {
            Ok(t) => {
                let (j, r) = claude::filter_settings(&t)?;
                (r, Some(j))
            }
            Err(_) => (vec![], None),
        };
    let head_msg = handoff::head(&handoff::Facts {
        from: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        to: target_s.clone(),
        extras: &extras,
        env_names: &env_found,
        removed_settings: &removed_settings,
    });

    let archive_path = tmp.join("snapshot.tar.gz");
    let mut ar = Archive::create(&archive_path)?;
    ar.add_path("repo.bundle", &bundle)?;
    ar.add_bytes("snapshot.sh", git::SNAPSHOT_SH.as_bytes())?;
    let resume = claude::resume_fn(&session.id);
    let run = remote::run_script(&remote::RunVars {
        stage: &stage,
        cwd: &cwd_s,
        home: &home_s,
        handoff_head: &head_msg,
        setup: &setup,
        resume_fn: &resume,
    });
    ar.add_bytes("run.sh", run.as_bytes())?;
    for e in &extras {
        ar.add_path(&format!("extras/{e}"), &root.join(e))?;
    }
    let mut sent_files = BTreeMap::new();
    let mut remote_paths = vec![root_s.clone(), stage.clone()];
    for rel in claude::session_paths(&home, &cwd, &session) {
        ar.add_path(&format!("home/{rel}"), &home.join(&rel))?;
        hash_tree(&home, &rel, &mut sent_files)?;
        remote_paths.push(format!("{home_s}/{rel}"));
    }
    for rel in claude::user_paths(&home) {
        ar.add_path(&format!("defaults/{rel}"), &home.join(&rel))?;
    }
    if let Some(j) = settings_json {
        ar.add_bytes("defaults/.claude/settings.json", j.as_bytes())?;
    }
    ar.add_bytes(
        "defaults/.claude.json",
        claude::default_claude_json(&cwd).as_bytes(),
    )?;
    let manifest = serde_json::json!({
        "beam_version": env!("CARGO_PKG_VERSION"),
        "created_at": now_unix(),
        "source": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "home": home_s },
        "project": { "root": root_s, "cwd": cwd_s },
        "git": sent,
        "extras": extras,
        "env_keys": env_found,
        "setup": setup,
        "agent": { "kind": "claude", "session_id": session.id },
    });
    ar.add_bytes(
        "manifest.json",
        serde_json::to_string_pretty(&manifest)?.as_bytes(),
    )?;
    let size = ar.finish()?;
    step("snapshot", human_size(size));

    // Sandbox.
    let image = cfg.sandbox.image.clone().unwrap_or(DEFAULT_IMAGE.into());
    let timeout =
        crate::util::parse_duration(cfg.sandbox.timeout.as_deref().unwrap_or(DEFAULT_TIMEOUT))?;
    let sb = target.create(&crate::sandbox::CreateOpts {
        name: &name,
        image: &image,
        session_id: &session.id,
        timeout_secs: timeout,
    })?;
    step("sandbox", sb.describe());
    let tmux = format!("beam-{short}");
    let result = (|| -> Result<()> {
        sb.exec(&remote::prepare(&home_s, &root_s, &stage))?;
        sb.exec_file(&remote::unpack(&stage), &archive_path)?;
        step("upload", "done");
        let origin = git::config_get(&root, "remote.origin.url");
        sb.exec(&remote::restore(&remote::RestoreVars {
            stage: &stage,
            project: &root_s,
            home: &home_s,
            refname: &up_ref,
            branch: &sent.branch,
            head: &sent.head,
            idx_tree: &sent.idx_tree,
            wt_tree: &sent.wt_tree,
            origin: &origin,
            git_name: &git::config_get(&root, "user.name"),
            git_email: &git::config_get(&root, "user.email"),
        }))?;
        step("restore", "done");
        let env_file: String = env
            .iter()
            .map(|(k, v)| format!("{k}={}\n", sh_quote(v)))
            .collect();
        sb.exec_input(&remote::write_env(&stage), env_file.as_bytes())?;
        sb.exec(&remote::start_tmux(&stage, &tmux))?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("✗ beam failed. Removing the sandbox.");
        let _ = sb.destroy(&remote::cleanup(&tmux, &remote_paths));
        git::delete_refs(&root, &format!("refs/beam/{}/", session.id));
        return Err(e);
    }
    step(
        "resume",
        format!("claude --resume {}  (tmux: {tmux})", session.id),
    );

    let state = State {
        version: 1,
        session_id: session.id.clone(),
        agent: "claude".into(),
        project_root: root.clone(),
        agent_cwd: cwd.clone(),
        home: home.clone(),
        target: target_s,
        sandbox: sb.clone(),
        stage,
        tmux: tmux.clone(),
        sent,
        sent_files,
        remote_paths,
        created_at: now_unix(),
    };
    state.save()?;
    let _ = std::fs::remove_dir_all(&tmp);
    println!(
        "✓ Session is live on {}. The local copy is locked.",
        sb.describe()
    );
    println!();
    println!("  beam attach      open the session");
    println!("  beam down        bring it home");

    if !a.detach && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        attach_tmux(&sb, &tmux)?;
    }
    Ok(())
}

pub fn attach_tmux(sb: &Sandbox, tmux: &str) -> Result<()> {
    sb.wake()?;
    sb.interactive(&format!("tmux attach -t {}", sh_quote(tmux)))
}

/// sha256 of each file under `rel` (a file or a directory relative to `home`).
fn hash_tree(home: &Path, rel: &str, out: &mut BTreeMap<String, String>) -> Result<()> {
    let p = home.join(rel);
    if p.is_dir() {
        for e in std::fs::read_dir(&p)? {
            let e = e?;
            hash_tree(
                home,
                &format!("{rel}/{}", e.file_name().to_string_lossy()),
                out,
            )?;
        }
    } else if p.is_file() {
        out.insert(rel.to_string(), sha256_file(&p)?);
    }
    Ok(())
}

fn confirm(question: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("{question}\nstdin is not a terminal. Use --yes to continue without a question");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if !matches!(line.trim(), "y" | "Y" | "yes") {
        bail!("stopped. Nothing was uploaded");
    }
    Ok(())
}

fn join(v: &[&String]) -> String {
    v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

pub fn tempdir() -> Result<PathBuf> {
    let p = std::env::temp_dir().join(format!("beam-{}-{}", std::process::id(), now_unix()));
    std::fs::create_dir_all(&p)?;
    Ok(p)
}
