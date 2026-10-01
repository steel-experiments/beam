// ABOUTME: Steel computers as beam sandboxes, through the `steel` CLI (preview 0.5 or later).
// ABOUTME: exec has exit codes but no stdin; ssh has stdin but no exit codes and no tty for commands.

use crate::util::{now_unix, sh_quote};
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
    let output = cmd
        .arg("--json")
        .output()
        .context("cannot start the Steel command")?;
    let out = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        bail!(
            "Steel command failed ({}): {} {}",
            output.status,
            out.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let v: Value =
        serde_json::from_str(&out).with_context(|| format!("steel printed no JSON: {out}"))?;
    if v["success"] != Value::Bool(true) {
        bail!("steel failed: {out}");
    }
    Ok(v["data"].clone())
}

/// Create a computer (or restore a checkpoint) and wait until it runs. Returns its id.
pub fn create(
    checkpoint: Option<&str>,
    timeout_secs: u64,
    receipt: &std::path::Path,
) -> Result<String> {
    if receipt.exists() {
        let bytes = std::fs::read(receipt)?;
        let v: Value = serde_json::from_slice(&bytes).context("Steel allocation was interrupted. Find the computer ID with `steel computer list --json`, then run `beam --recover-sandbox ID`. Do not create another computer")?;
        return v["data"]["id"].as_str().map(str::to_string).context("Steel allocation has no computer ID. Inspect `steel computer list --json`, then use `beam --recover-sandbox ID`");
    }
    let mut c = steel();
    match checkpoint {
        Some(id) => c.args(["checkpoint", "restore", id]),
        None => c.args(["computer", "create"]),
    };
    c.args(["--auto-pause", "--timeout", &timeout_secs.to_string()]);
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(receipt)?;
    let out = c
        .arg("--json")
        .stdout(file.try_clone()?)
        .stderr(Stdio::piped())
        .output()?;
    file.sync_all()?;
    if !out.status.success() {
        bail!(
            "Steel allocation failed: {}. Inspect `steel computer list --json` before retrying",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let v: Value = serde_json::from_slice(&std::fs::read(receipt)?)?;
    let id = v["data"]["id"]
        .as_str()
        .context("Steel returned no computer ID")?;
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
pub fn status(id: &str) -> Result<String> {
    let data = json(steel().args(["computer", "get", id]))?;
    Ok(data["status"].as_str().unwrap_or("unknown").to_string())
}

pub fn wake(id: &str) -> Result<()> {
    let data = json(steel().args(["computer", "get", id]))?;
    match data["status"].as_str().unwrap_or_default() {
        "running" => Ok(()),
        "paused" => {
            crate::up::step("sandbox", "resuming the paused Steel computer");
            json(steel().args(["computer", "resume", id, "--wait"])).map(|_| ())
        }
        s => bail!("the Steel computer {id} is {s:?}. It must be running or paused"),
    }
}

pub fn delete(id: &str) -> Result<()> {
    match json(steel().args(["computer", "delete", id])) {
        Ok(_) => Ok(()),
        Err(e)
            if e.to_string().to_lowercase().contains("not found")
                || e.to_string().contains("404") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn download(id: &str, script: &str, file: &std::fs::File) -> Result<()> {
    let rc = format!("/tmp/beam-download-{}-{}", std::process::id(), now_unix());
    let wrapper = format!("sh -c {} ; echo $? > {}", sh_quote(script), sh_quote(&rc));
    let out = steel()
        .args(["computer", "ssh", id, "--", "sh", "-c", &wrapper])
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(Stdio::piped())
        .output()?;
    let code = exec(
        id,
        &format!(
            "cat {0} 2>/dev/null || echo missing; rm -f {0}",
            sh_quote(&rc)
        ),
    )?;
    if !out.status.success() || code.trim() != "0" {
        bail!(
            "Steel download failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn ready(id: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        let data = json(steel().args(["computer", "get", id]))?;
        match data["status"].as_str().unwrap_or_default() {
            "running" => return Ok(()),
            "paused" => {
                wake(id)?;
                return Ok(());
            }
            "creating" | "starting" | "pending" | "provisioning" => {}
            status => {
                bail!("Steel computer {id} is {status}. Inspect it with `steel computer get {id}`")
            }
        }
        if std::time::Instant::now() > deadline {
            bail!("Steel computer is still starting. Run `beam` to retry");
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_command_preserves_json_stdout_and_stderr() {
        let error = json(Command::new("sh").args([
            "-c",
            r#"printf '%s' '{"success":true,"data":{"exitCode":4,"output":"missing tools: cargo"}}'; echo 'diagnostic on stderr' >&2; exit 4"#,
        ]))
        .unwrap_err()
        .to_string();
        assert!(error.contains("missing tools: cargo"), "{error}");
        assert!(error.contains("diagnostic on stderr"), "{error}");
        assert!(error.contains("exit status: 4"), "{error}");
    }
}
