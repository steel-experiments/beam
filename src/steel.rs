// ABOUTME: Steel computers as beam sandboxes, through the `steel` CLI (preview 0.5 or later).
// ABOUTME: exec has exit codes but no stdin; ssh has stdin but no exit codes and no tty for commands.

use crate::util::{now_unix, run, sh_quote};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const BOOTSTRAP_SH: &str = include_str!("../scripts/steel_bootstrap.sh");

/// The longest command that "steel computer exec" allows.
const EXEC_TIMEOUT: &str = "3600";

/// The steel CLI: from PATH, or from the place where its installer puts it.
pub fn steel() -> Command {
    let in_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("steel").is_file()))
        .unwrap_or(false);
    let bin = if in_path {
        PathBuf::from("steel")
    } else {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".steel/bin/steel"))
            .unwrap_or_else(|| PathBuf::from("steel"))
    };
    let mut c = Command::new(bin);
    c.stdin(Stdio::null());
    c
}

/// Run a steel command with --json and return the "data" object.
fn json(cmd: &mut Command) -> Result<Value> {
    let out = run(cmd.arg("--json"))?;
    let v: Value =
        serde_json::from_str(&out).with_context(|| format!("steel printed no JSON: {out}"))?;
    if v["success"] != Value::Bool(true) {
        bail!("steel failed: {out}");
    }
    Ok(v["data"].clone())
}

/// Create a computer (or restore a checkpoint) and wait until it runs. Returns its id.
pub fn create(checkpoint: Option<&str>, timeout_secs: u64) -> Result<String> {
    let mut c = steel();
    match checkpoint {
        Some(id) => c.args(["checkpoint", "restore", id]),
        None => c.args(["computer", "create"]),
    };
    c.args([
        "--wait",
        "--auto-pause",
        "--timeout",
        &timeout_secs.to_string(),
    ]);
    let data = json(&mut c)?;
    let id = data["id"]
        .as_str()
        .with_context(|| format!("no computer id in {data}"))?;
    Ok(id.to_string())
}

/// Run a script with "steel computer exec". Stdout and stderr come back together.
pub fn exec(id: &str, script: &str) -> Result<String> {
    let data = json(steel().args([
        "computer",
        "exec",
        id,
        "--timeout",
        EXEC_TIMEOUT,
        "-c",
        script,
    ]))?;
    let output = data["output"].as_str().unwrap_or_default().to_string();
    if data["timedOut"] == Value::Bool(true) {
        bail!("command timed out in the Steel computer: {output}");
    }
    if data["truncated"] == Value::Bool(true) {
        bail!("the output of a command in the Steel computer was cut off");
    }
    let code = data["exitCode"].as_i64().unwrap_or(-1);
    if code != 0 {
        bail!(
            "command failed in the Steel computer (exit {code}): {}",
            output.trim()
        );
    }
    Ok(output.trim().to_string())
}

/// Run a script with "steel computer ssh", with stdin and stdout connected to `cmd`'s pipes.
/// ssh always exits 0, so the script writes its exit code to a file that exec then reads.
pub fn ssh_output(id: &str, script: &str, stdin: Stdio) -> Result<Vec<u8>> {
    let rc = format!("/tmp/beam-rc-{}-{}", std::process::id(), now_unix());
    let wrapper = format!("sh -c {} ; echo $? > {}", sh_quote(script), sh_quote(&rc));
    let out = steel()
        .args(["computer", "ssh", id, "--", "sh", "-c", &wrapper])
        .stdin(stdin)
        .output()
        .context("cannot start steel computer ssh")?;
    let code = exec(
        id,
        &format!(
            "cat {rc} 2>/dev/null || echo missing; rm -f {rc}",
            rc = sh_quote(&rc)
        ),
    )?;
    if code.trim() != "0" {
        bail!(
            "command failed in the Steel computer (exit {}): {}",
            code.trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Connect this terminal to `script`. Only a login shell has a terminal, so the script goes
/// into ~/.beam-attach and the hook in ~/.bashrc (see steel_bootstrap.sh) runs it.
pub fn interactive(id: &str, script: &str) -> Result<()> {
    exec(
        id,
        &format!("printf '%s' {} > \"$HOME/.beam-attach\"", sh_quote(script)),
    )?;
    let status = steel()
        .args(["computer", "ssh", id])
        .stdin(Stdio::inherit())
        .status()
        .context("cannot start steel computer ssh")?;
    if !status.success() {
        bail!("steel computer ssh stopped with {status}");
    }
    Ok(())
}

/// Resume the computer when it paused itself (after --auto-pause).
pub fn wake(id: &str) -> Result<()> {
    let data = json(steel().args(["computer", "get", id]))?;
    match data["status"].as_str().unwrap_or_default() {
        "running" => Ok(()),
        "paused" => {
            println!("▸ {:<10} resuming the paused Steel computer", "sandbox");
            json(steel().args(["computer", "resume", id, "--wait"])).map(|_| ())
        }
        s => bail!("the Steel computer {id} is {s:?}. It must be running or paused"),
    }
}

pub fn delete(id: &str) -> Result<()> {
    json(steel().args(["computer", "delete", id])).map(|_| ())
}
