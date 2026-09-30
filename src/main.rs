// ABOUTME: beam CLI entry point: moves a coding-agent session up to a sandbox and down again.
// ABOUTME: Commands: beam [PATH] (or beam up), down, attach, status, ls, kill, doctor.

mod agent;
mod config;
mod daytona;
mod down;
mod git;
mod handoff;
mod monitor;
mod pack;
mod plan;
mod presentation;
mod remote;
mod return_files;
mod review;
#[cfg(test)]
mod roundtrip;
mod sandbox;
mod scan;
mod state;
mod steel;
mod up;
mod util;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "beam",
    version,
    about = "Move your workspace to a sandbox, and bring it home"
)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    up: UpOpts,
}

#[derive(clap::Args)]
struct UpOpts {
    /// Project directory (default: the current directory).
    path: Option<PathBuf>,
    /// Target: docker, docker+ssh://HOST, ssh://HOST, steel, steel:CHECKPOINT, daytona, or daytona:SNAPSHOT.
    #[arg(long)]
    to: Option<String>,
    /// Session id (default: the session that changed last).
    #[arg(long)]
    session: Option<String>,
    /// Do not ask questions.
    #[arg(long, short = 'y')]
    yes: bool,
    /// Do not attach the terminal after the move.
    #[arg(long, short = 'd')]
    detach: bool,
    /// Move the session also when Claude Code still runs in the project.
    #[arg(long)]
    force: bool,
    /// Upload files that are larger than max_file_size.
    #[arg(long)]
    allow_large: bool,
    /// auto resumes a discovered Claude session, or opens a shell when none exists.
    #[arg(long, default_value = "auto", value_parser = agent_values())]
    agent: String,
    /// Preview and check a transfer without creating it.
    #[arg(long)]
    dry_run: bool,
    /// Build the bundled Docker image before checking prerequisites.
    #[arg(long)]
    build_image: bool,
    /// Adopt a cloud sandbox after an interrupted allocation.
    #[arg(long)]
    recover_sandbox: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Move the workspace to a sandbox (the same as plain `beam`).
    Up {
        #[command(flatten)]
        opts: UpOpts,
    },
    /// Bring the work home and remove the sandbox.
    #[command(alias = "back")]
    Down {
        path: Option<PathBuf>,
        /// Keep the sandbox for inspection. Later edits will not return.
        #[arg(long)]
        keep: bool,
        /// Download a fixed return snapshot for review without changing local project files.
        #[arg(long, conflicts_with = "keep")]
        review: bool,
    },
    /// Inspect a saved return and apply it, or mark manual recovery resolved.
    Review {
        path: Option<PathBuf>,
        #[arg(long)]
        transfer: Option<String>,
        #[arg(long, conflicts_with_all = ["resolved", "diff", "json", "refresh", "open"])]
        apply: bool,
        #[arg(long, conflicts_with_all = ["diff", "json", "refresh", "open"])]
        resolved: bool,
        #[arg(long, conflicts_with = "json")]
        diff: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, requires = "apply")]
        keep: bool,
        /// Rebuild an unapplied plan after changing local extras or agent files.
        #[arg(long, conflicts_with_all = ["diff", "json", "open"])]
        refresh: bool,
        /// Open a shell in the saved remote worktree. Exit to return.
        #[arg(long, conflicts_with_all = ["diff", "json"])]
        open: bool,
    },
    /// Undo a return when local work has not changed since.
    Undo {
        path: Option<PathBuf>,
        #[arg(long)]
        transfer: Option<String>,
    },
    /// Open the remote agent or a repair shell.
    Attach { path: Option<PathBuf> },
    /// Restart a stopped session and rerun project checks in its existing sandbox.
    Restart { path: Option<PathBuf> },
    /// Show transfer state, saved recovery, and the next action.
    Status {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        watch: bool,
        /// Ring the terminal bell for new attention, completion reports, and exit events.
        #[arg(long, requires = "watch")]
        notify: bool,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..=60))]
        interval: u64,
        /// Stop watching after this many polls.
        #[arg(long, requires = "watch", value_parser = clap::value_parser!(u64).range(1..))]
        count: Option<u64>,
    },
    /// List all beamed sessions.
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Show setup logs and recent terminal output.
    Logs { path: Option<PathBuf> },
    /// Forget a lost sandbox after you have removed its resources yourself.
    Forget {
        path: Option<PathBuf>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Remove the sandbox. Work in the sandbox is lost.
    Kill {
        path: Option<PathBuf>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Check the transfer plan and destination prerequisites.
    Doctor {
        #[arg(long)]
        to: Option<String>,
        path: Option<PathBuf>,
        #[arg(long, default_value = "auto", value_parser = agent_values())]
        agent: String,
    },
}

fn agent_values() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(std::iter::once("auto").chain(agent::ids()))
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("✗ {e:#}");
        std::process::exit(1);
    }
}

fn dir(p: Option<PathBuf>) -> PathBuf {
    p.unwrap_or_else(|| PathBuf::from("."))
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        None => run_up(cli.up),
        Some(Cmd::Up { opts }) => run_up(opts),
        Some(Cmd::Down { path, keep, review }) => down::down(&dir(path), keep, review),
        Some(Cmd::Review {
            path,
            transfer,
            apply,
            resolved,
            diff,
            json,
            keep,
            refresh,
            open,
        }) => {
            let action = if apply {
                review::ReviewAction::Apply { keep }
            } else if resolved {
                review::ReviewAction::Resolved
            } else if diff {
                review::ReviewAction::Diff
            } else if json {
                review::ReviewAction::Json
            } else if refresh {
                review::ReviewAction::Refresh
            } else if open {
                review::ReviewAction::Open
            } else {
                review::ReviewAction::Show
            };
            review::review(&dir(path), transfer.as_deref(), action)
        }
        Some(Cmd::Undo { path, transfer }) => review::undo(&dir(path), transfer.as_deref()),
        Some(Cmd::Attach { path }) => {
            let st = down::load_state(&dir(path))?;
            up::attach(&st)
        }
        Some(Cmd::Status {
            path,
            json,
            watch,
            notify,
            interval,
            count,
        }) => {
            if watch {
                monitor::watch(&dir(path), json, interval, count, notify)
            } else {
                status(&dir(path), json)
            }
        }
        Some(Cmd::Restart { path }) => up::restart(&dir(path)),
        Some(Cmd::Ls { json }) => ls(json),
        Some(Cmd::Logs { path }) => logs(&dir(path)),
        Some(Cmd::Forget { path, yes }) => forget(&dir(path), yes),
        Some(Cmd::Kill { path, yes }) => kill(&dir(path), yes),
        Some(Cmd::Doctor { to, path, agent }) => doctor(&dir(path), to, agent),
    }
}

fn run_up(o: UpOpts) -> Result<()> {
    up::up(up::UpArgs {
        path: dir(o.path),
        to: o.to,
        session: o.session,
        yes: o.yes,
        detach: o.detach,
        force: o.force,
        allow_large: o.allow_large,
        agent: o.agent,
        dry_run: o.dry_run,
        build_image: o.build_image,
        recover_sandbox: o.recover_sandbox,
    })
}

fn status(path: &Path, json: bool) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let active = state::State::load(&root)?;
    let snapshot = active.as_ref().map(monitor::snapshot).transpose()?;
    status_with_snapshot(&root, json, active.as_ref(), snapshot.as_ref())
}
fn status_with_snapshot(
    root: &Path,
    json: bool,
    active: Option<&state::State>,
    snapshot: Option<&monitor::Snapshot>,
) -> Result<()> {
    let saved = state::latest_recovery(&up::home_dir()?, root)?;
    let Some(st) = active else {
        let view = presentation::summary(state::Phase::Closed, None, saved.is_some());
        let next = saved
            .as_ref()
            .map(presentation::recovery_action)
            .unwrap_or_else(|| view.next.into());
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "phase": "local", "summary": view.message, "next_action": next,
                    "saved_recovery": saved.as_ref().map(|s| serde_json::json!({
                        "receipt": s.dir(), "worktree": s.recovery, "conflicts": s.conflicts
                    }))
                }))?
            );
        } else {
            println!("{}", view.message);
            if let Some(st) = &saved {
                presentation::recovery(st);
            }
            println!("Next: {next}");
        }
        return Ok(());
    };
    let snapshot = snapshot.context("missing status snapshot")?;
    let view = presentation::for_state(st, Some(snapshot));
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "phase": view.phase, "operation_phase": st.phase, "remote": snapshot.remote,
                "agent": st.agent, "capabilities": snapshot.capabilities,
                "process": snapshot.process, "task": snapshot.task, "observations": snapshot.observations,
                "target": st.describe(), "session": st.session_id, "transfer": st.transfer_id,
                "recovery": st.recovery, "conflicts": st.conflicts, "last_error": st.last_error,
                "summary": view.message, "next_action": view.next,
                "recovery_resolved": !st.has_unresolved_recovery(),
                "events": snapshot.events,
                "input_request": monitor::input_request(&snapshot.task),
                "timings": std::fs::read(st.dir().join("timings.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()),
                "saved_recovery": saved.as_ref().map(|s| serde_json::json!({
                    "receipt": s.dir(), "worktree": s.recovery, "conflicts": s.conflicts
                }))
            }))?
        );
    } else {
        println!("Project: {}", root.display());
        if let Some(e) = &st.last_error {
            println!("Last error: {e}");
        }
        if st.has_unresolved_recovery() {
            presentation::recovery(st);
        }
        if let Some(previous) = &saved {
            presentation::recovery(previous);
        }
        presentation::show(st, Some(snapshot));
        monitor::show(&snapshot.events);
    }
    Ok(())
}

fn ls(json: bool) -> Result<()> {
    let entries = state::index_load(&up::home_dir()?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("no beamed sessions");
    }
    for e in entries {
        println!(
            "{}  {}  {}  {}",
            e.phase.label(),
            e.session_id,
            e.describe(),
            e.project_root.display()
        );
    }
    Ok(())
}

fn kill(path: &Path, yes: bool) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let _lock = state::ProjectLock::acquire(&up::home_dir()?, &root)?;
    let mut st = down::load_state(path)?;
    if !yes {
        up::confirm(
            &format!(
                "Remove {} and discard work that has not returned?",
                st.describe()
            ),
            false,
        )?;
    }
    up::cleanup(&st)?;
    if !st.dir().join("return-plan.json").exists() {
        git::delete_refs(&st.project_root, &format!("refs/beam/{}/", st.transfer_id));
    }
    st.remove()?;
    println!("✓ removed {}", st.describe());
    Ok(())
}

fn forget(path: &Path, yes: bool) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let _lock = state::ProjectLock::acquire(&up::home_dir()?, &root)?;
    let mut st = down::load_state(path)?;
    if !yes {
        bail!(
            "this removes only Beam's active record. Remote resources may still exist. Run `beam forget --yes` after removing them yourself"
        );
    }
    st.remove()?;
    println!(
        "Forgot the active transfer. Remote resources were not changed. Receipt: {}",
        st.dir().display()
    );
    Ok(())
}

fn logs(path: &Path) -> Result<()> {
    let st = down::load_state(path)?;
    let sb = st.sandbox()?;
    sb.wake()?;
    let script = format!(
        "cat {0}/terminal.log 2>/dev/null || cat {0}/setup.log 2>/dev/null || true; tmux capture-pane -p -t {1} -S -100 2>/dev/null || true; cat {0}/agent.exit 2>/dev/null || true",
        util::sh_quote(&st.stage),
        util::sh_quote(&st.tmux)
    );
    println!("{}", sb.exec(&script)?);
    Ok(())
}

fn doctor(path: &Path, to: Option<String>, agent: String) -> Result<()> {
    if !util::succeeds(Command::new("git").arg("--version")) {
        bail!("git is missing. Install git before using Beam");
    }
    let plan = plan::Plan::build(&up::UpArgs {
        path: path.into(),
        to,
        session: None,
        yes: true,
        detach: true,
        force: true,
        allow_large: false,
        agent,
        dry_run: true,
        build_image: false,
        recover_sandbox: None,
    })?;
    plan.show();
    sandbox::Target::parse(&plan.target)?.preflight(
        plan.config
            .sandbox
            .image
            .as_deref()
            .unwrap_or(config::DEFAULT_IMAGE),
        &plan.preflight_tools(),
        plan.preflight_versions(),
        &plan.root,
    )?;
    println!("✓ Transfer prerequisites passed. Cloud sandbox tools are checked after allocation.");
    Ok(())
}
