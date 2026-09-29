// ABOUTME: beam CLI entry point: moves a coding-agent session up to a sandbox and down again.
// ABOUTME: Commands: beam [PATH] (or beam up), down, attach, status, ls, kill, doctor.

mod claude;
mod config;
mod down;
mod git;
mod handoff;
mod pack;
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
    about = "Beam a coding-agent session up to a sandbox, and down again"
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
}

#[derive(Subcommand)]
enum Cmd {
    /// Move the session to a sandbox (the same as plain `beam`).
    Up {
        #[command(flatten)]
        opts: UpOpts,
    },
    /// Bring the session home and remove the sandbox.
    #[command(alias = "back")]
    Down {
        path: Option<PathBuf>,
        /// Do not remove the sandbox.
        #[arg(long)]
        keep: bool,
    },
    /// Attach the terminal to the agent in the sandbox.
    Attach { path: Option<PathBuf> },
    /// Show where the session is live.
    Status { path: Option<PathBuf> },
    /// List all beamed sessions.
    Ls,
    /// Remove the sandbox. Work in the sandbox is lost.
    Kill {
        path: Option<PathBuf>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Check the local tools and, with --to, the target.
    Doctor {
        #[arg(long)]
        to: Option<String>,
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
            up::attach_tmux(&st.sandbox, &st.tmux)
        }
        Some(Cmd::Status { path }) => status(&dir(path)),
        Some(Cmd::Ls) => ls(),
        Some(Cmd::Kill { path, yes }) => kill(&dir(path), yes),
        Some(Cmd::Doctor { to }) => doctor(to.as_deref()),
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
    })
}

fn status(path: &Path) -> Result<()> {
    let cwd = path.canonicalize()?;
    let root = git::toplevel(&cwd)?;
    let Some(st) = state::State::load(&root)? else {
        println!("local");
        return Ok(());
    };
    let agent = st
        .sandbox
        .exec(&remote::agent_status(&st.stage, &st.tmux))
        .unwrap_or_else(|e| format!("unknown ({e})"));
    println!("remote  {}", st.sandbox.describe());
    println!("session {}", st.session_id);
    println!("agent   {agent}");
    Ok(())
}

fn ls() -> Result<()> {
    let entries = state::index_load(&up::home_dir()?)?;
    if entries.is_empty() {
        println!("no beamed sessions");
    }
    for e in entries {
        println!("{}  {}  {}", e.session_id, e.target, e.project.display());
    }
    Ok(())
}

fn kill(path: &Path, yes: bool) -> Result<()> {
    let st = down::load_state(path)?;
    if !yes {
        bail!(
            "this removes {} and all work in it. Run again with --yes to do it",
            st.sandbox.describe()
        );
    }
    st.sandbox
        .destroy(&remote::cleanup(&st.tmux, &st.remote_paths))?;
    git::delete_refs(&st.project_root, &format!("refs/beam/{}/", st.session_id));
    st.remove()?;
    println!("✓ removed {}", st.sandbox.describe());
    Ok(())
}

fn doctor(to: Option<&str>) -> Result<()> {
    let mut ok = true;
    let mut check = |label: &str, good: bool, hint: &str| {
        println!(
            "{} {label}{}",
            if good { "✓" } else { "✗" },
            if good {
                String::new()
            } else {
                format!(" — {hint}")
            }
        );
        ok &= good;
    };
    let has = |bin: &str| util::succeeds(Command::new(bin).arg("--version"));
    check("git", has("git"), "install git");
    check("claude", has("claude"), "install Claude Code");
    let auth = claude::AUTH_ENV
        .iter()
        .take(3)
        .any(|k| std::env::var_os(k).is_some());
    check(
        "Claude auth env var",
        auth,
        "run `claude setup-token` and export CLAUDE_CODE_OAUTH_TOKEN",
    );
    if let Some(t) = to {
        let script = "for t in git tmux tar gzip claude; do command -v $t >/dev/null || echo \"missing $t\"; done";
        match sandbox::Target::parse(t)? {
            sandbox::Target::Ssh { host } => {
                let out = util::run(Command::new("ssh").arg(&host).arg(script));
                check(&format!("ssh {host}"), out.is_ok(), "cannot connect");
                if let Ok(o) = out {
                    check("sandbox tools", o.is_empty(), &o.replace('\n', ", "));
                }
            }
            sandbox::Target::Steel { .. } => {
                let out = util::run(steel::steel().args(["computer", "quota", "--json"]));
                check(
                    "steel CLI with computers (0.5 preview or later)",
                    out.is_ok(),
                    "install the steel preview CLI and set STEEL_API_KEY",
                );
                if let Ok(o) = out {
                    let v: serde_json::Value = serde_json::from_str(&o).unwrap_or_default();
                    let (n, max) = (&v["data"]["computerCount"], &v["data"]["computerLimit"]);
                    check(
                        &format!("steel quota ({n} of {max} computers)"),
                        n.as_i64() < max.as_i64(),
                        "delete computers you do not use",
                    );
                }
            }
            sandbox::Target::Docker { ssh_host } => {
                let mut cmd = match &ssh_host {
                    Some(h) => {
                        let mut c = Command::new("ssh");
                        c.arg(h)
                            .arg("docker version --format '{{.Server.Version}}'");
                        c
                    }
                    None => {
                        let mut c = Command::new("docker");
                        c.args(["version", "--format", "{{.Server.Version}}"]);
                        c
                    }
                };
                check(
                    "docker",
                    util::succeeds(&mut cmd),
                    "docker is not available",
                );
            }
        }
    }
    if !ok {
        bail!("some checks failed");
    }
    Ok(())
}
