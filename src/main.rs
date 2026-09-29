// ABOUTME: beam CLI entry point: moves a coding-agent session up to a sandbox and down again.
// ABOUTME: Commands: beam [PATH] (or beam up), down, attach, status, ls, kill, doctor.

mod claude;
mod config;
mod down;
mod git;
mod handoff;
mod pack;
mod plan;
mod remote;
#[cfg(test)]
mod roundtrip;
mod sandbox;
mod scan;
mod state;
mod steel;
mod up;
mod util;

use anyhow::{Result, bail};
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
    /// Target: docker, docker+ssh://HOST, ssh://HOST, steel, or steel:CHECKPOINT.
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
    #[arg(long, default_value = "auto", value_parser = ["auto", "claude", "shell"])]
    agent: String,
    /// Preview and check a transfer without creating it.
    #[arg(long)]
    dry_run: bool,
    /// Build the bundled Docker image before checking prerequisites.
    #[arg(long)]
    build_image: bool,
    /// Adopt a Steel computer after an interrupted allocation.
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
        /// Do not remove the sandbox.
        #[arg(long)]
        keep: bool,
    },
    /// Open the remote agent or a repair shell.
    Attach { path: Option<PathBuf> },
    /// Show where the session is live.
    Status {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
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
        #[arg(long, default_value = "auto", value_parser = ["auto", "claude", "shell"])]
        agent: String,
    },
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
        Some(Cmd::Down { path, keep }) => down::down(&dir(path), keep),
        Some(Cmd::Attach { path }) => {
            let st = down::load_state(&dir(path))?;
            up::attach(&st)
        }
        Some(Cmd::Status { path, json }) => status(&dir(path), json),
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
    let Some(st) = state::State::load(&root)? else {
        println!(
            "{}",
            if json {
                r#"{"phase":"local"}"#
            } else {
                "local — run `beam` to send this workspace"
            }
        );
        return Ok(());
    };
    let remote = if matches!(
        st.phase,
        state::Phase::Remote | state::Phase::Starting | state::Phase::Retained
    ) {
        st.sandbox()?
            .status(&st.stage, &st.tmux)
            .unwrap_or_else(|e| format!("unavailable: {e}"))
    } else {
        st.phase.label().into()
    };
    let display_phase = if remote == "needs-attention" {
        "needs attention"
    } else if st.phase == state::Phase::Starting && remote == "running" {
        "remote"
    } else {
        st.phase.label()
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"phase":display_phase,"operation_phase":st.phase,"remote":remote,"target":st.describe(),"session":st.session_id,"transfer":st.transfer_id,"recovery":st.recovery,"last_error":st.last_error})
            )?
        );
    } else {
        println!("{}  {}", display_phase, st.describe());
        println!("session {}", st.session_id);
        println!("agent   {remote}");
        if let Some(e) = st.last_error {
            println!("last error: {e}");
        }
        println!(
            "{}",
            match st.phase {
                state::Phase::Returning | state::Phase::Downloaded | state::Phase::Applied =>
                    "Next: beam down",
                state::Phase::Retained => "Next: beam attach to inspect; beam kill --yes to remove",
                state::Phase::Remote => "Next: beam attach or beam down; beam logs for details",
                _ => "Next: beam to continue; beam logs for setup output",
            }
        );
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
        up::confirm(&format!(
            "Remove {} and discard work that has not returned?",
            st.describe()
        ))?;
    }
    up::cleanup(&st)?;
    git::delete_refs(&st.project_root, &format!("refs/beam/{}/", st.transfer_id));
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
        &plan.tools,
        &plan.versions,
        &plan.root,
    )?;
    println!(
        "✓ Transfer prerequisites passed. Steel checkpoint tools are checked after allocation."
    );
    Ok(())
}
