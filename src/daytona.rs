// ABOUTME: Daytona allocation through its REST API, with binary-safe OpenSSH transport.
// ABOUTME: Allocation receipts prevent duplicate resources after ambiguous API responses.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn api(method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>> {
    let key = std::env::var("DAYTONA_API_KEY")
        .context("Daytona requires DAYTONA_API_KEY in your shell environment")?;
    if key.is_empty() || key.contains(['\r', '\n']) {
        bail!("DAYTONA_API_KEY is empty or invalid");
    }
    // Keep the API key out of process arguments and command diagnostics.
    let mut headers = tempfile::NamedTempFile::new()?;
    writeln!(headers, "Authorization: Bearer {key}")?;
    writeln!(headers, "Content-Type: application/json")?;
    headers.flush()?;
    let base =
        std::env::var("DAYTONA_API_URL").unwrap_or_else(|_| "https://app.daytona.io/api".into());
    let mut c = Command::new("curl");
    c.args([
        "--silent",
        "--show-error",
        "--connect-timeout",
        "15",
        "--max-time",
        "180",
        "--request",
        method,
        "--header",
    ])
    .arg(format!("@{}", headers.path().display()))
    .args(["--write-out", "\n%{http_code}"])
    .arg(format!("{}{path}", base.trim_end_matches('/')))
    .stdin(Stdio::null());
    if let Some(body) = body {
        c.args(["--data-binary", &serde_json::to_string(body)?]);
    }
    let out = c.output().context("Daytona requires curl on PATH")?;
    if !out.status.success() {
        bail!(
            "Daytona API request failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8(out.stdout)?;
    let (body, code) = text
        .rsplit_once('\n')
        .context("Daytona API returned no HTTP status")?;
    if code == "404" {
        return Ok(None);
    }
    if !code.starts_with('2') {
        // Do not print API response bodies: SSH responses can contain credentials.
        bail!("Daytona API {method} {path} failed (HTTP {code})");
    }
    Ok(Some(if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).context("Daytona API returned invalid JSON")?
    }))
}

fn resource_path(id: &str) -> Result<String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        bail!("invalid Daytona sandbox ID");
    }
    Ok(format!("/sandbox/{id}"))
}

fn get(id: &str) -> Result<Option<Value>> {
    api("GET", &resource_path(id)?, None)
}

fn owned(data: &Value, owner: &str) -> Result<()> {
    if data["labels"]["beam.session"].as_str() != Some(owner) {
        bail!("Daytona sandbox is not owned by this transfer; nothing was changed");
    }
    Ok(())
}

pub fn preflight() -> Result<()> {
    crate::util::run(Command::new("ssh").arg("-V")).context("Daytona requires OpenSSH on PATH")?;
    api("GET", "/sandbox?limit=1", None)?.context("Daytona sandbox API is unavailable")?;
    Ok(())
}

pub fn recover(id: &str, owner: &str) -> Result<String> {
    let data = get(id)?.context("Daytona sandbox was not found")?;
    owned(&data, owner)?;
    let id = data["id"]
        .as_str()
        .context("Daytona returned no sandbox ID")?;
    resource_path(id)?;
    Ok(id.to_string())
}

pub fn create(snapshot: Option<&str>, o: &crate::sandbox::CreateOpts<'_>) -> Result<String> {
    if o.receipt.exists() {
        let data: Value = serde_json::from_slice(&std::fs::read(o.receipt)?)
            .context("Daytona allocation was interrupted. Inspect your Daytona dashboard, then run `beam --recover-sandbox ID`; do not allocate another sandbox")?;
        let id = data["id"].as_str().context("Daytona allocation has no sandbox ID. Inspect your Daytona dashboard, then run `beam --recover-sandbox ID`")?;
        return recover(id, o.session_id);
    }
    // Save a marker before making the irreversible allocation request.
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(o.receipt)?;
    file.sync_all()?;
    std::fs::File::open(
        o.receipt
            .parent()
            .context("allocation receipt has no parent")?,
    )?
    .sync_all()?;
    let mut body = json!({
        "name": o.name, "user": "root",
        "labels": {"beam.session": o.session_id},
        "autoStopInterval": o.timeout_secs.div_ceil(60),
        "autoDeleteInterval": -1,
        "ttlMinutes": 0
    });
    if let Some(snapshot) = snapshot {
        body["snapshot"] = json!(snapshot);
    }
    let data = api("POST", "/sandbox", Some(&body))?
        .context("Daytona allocation endpoint was not found")?;
    crate::util::atomic_write(o.receipt, &serde_json::to_vec(&data)?)?;
    let id = data["id"]
        .as_str()
        .context("Daytona returned no sandbox ID; use `beam --recover-sandbox ID`")?;
    resource_path(id)?;
    Ok(id.to_string())
}

pub fn status(id: &str) -> Result<String> {
    let data = get(id)?.context("Daytona sandbox was not found")?;
    Ok(match data["state"].as_str().unwrap_or("unknown") {
        "started" => "running",
        other => other,
    }
    .to_string())
}

fn wait_for(poll: impl FnMut() -> Result<bool>, timeout_message: &str) -> Result<()> {
    poll_until(
        poll,
        std::time::Duration::from_secs(180),
        std::time::Duration::from_secs(2),
        timeout_message,
    )
}

fn poll_until(
    mut poll: impl FnMut() -> Result<bool>,
    timeout: std::time::Duration,
    interval: std::time::Duration,
    timeout_message: &str,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if poll()? {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            bail!("{timeout_message}");
        }
        std::thread::sleep(
            interval.min(deadline.saturating_duration_since(std::time::Instant::now())),
        );
    }
}

pub fn wake(id: &str) -> Result<()> {
    let mut requested = false;
    wait_for(
        || {
            match status(id)?.as_str() {
                "running" => return Ok(true),
                "stopped" | "archived" if !requested => {
                    api("POST", &format!("{}/start", resource_path(id)?), None)?;
                    requested = true;
                }
                "creating" | "starting" | "pending" | "pending_build" | "building_snapshot"
                | "pulling_snapshot" | "restoring" | "stopping" | "archiving" | "resizing"
                | "snapshotting" | "forking" | "pausing" | "resuming" | "unknown" | "stopped"
                | "archived" => {}
                state => {
                    bail!("Daytona sandbox {id} is {state}; inspect it in the Daytona dashboard")
                }
            }
            Ok(false)
        },
        "Daytona sandbox is still starting; retry the command",
    )
}

fn root_script(script: &str) -> String {
    // The SSH gateway can use the snapshot's default user despite user=root
    // on the sandbox. Keep every operation under the requested root identity.
    // Through sudo, remove the SUDO_* variables so tools see plain root.
    // Some tools refuse to run when they find them (the Claude Code installer is one).
    let sudo_script = crate::util::sh_quote(&format!(
        "unset SUDO_USER SUDO_UID SUDO_GID SUDO_COMMAND; {script}"
    ));
    let script = crate::util::sh_quote(script);
    format!(
        "if [ \"$(id -u)\" = 0 ]; then exec sh -c {script}; \
         elif command -v sudo >/dev/null 2>&1 && sudo -n true; then exec sudo -n sh -c {sudo_script}; \
         else echo 'beam: Daytona requires root access or passwordless sudo in the snapshot' >&2; exit 4; fi"
    )
}

pub fn command(id: &str, script: &str, tty: bool) -> Result<Command> {
    let access = api(
        "POST",
        &format!("{}/ssh-access?expiresInMinutes=60", resource_path(id)?),
        None,
    )?
    .context("Daytona sandbox was not found")?;
    let token = access["token"]
        .as_str()
        .context("Daytona returned no SSH token")?;
    if token.is_empty()
        || token.starts_with('-')
        || token.chars().any(|c| c.is_whitespace() || c == '@')
    {
        bail!("Daytona returned an invalid SSH token");
    }
    let host = std::env::var("DAYTONA_SSH_HOST").unwrap_or_else(|_| "ssh.app.daytona.io".into());
    if host.is_empty()
        || host.starts_with('-')
        || !host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
    {
        bail!("DAYTONA_SSH_HOST must be a hostname");
    }
    let mut c = Command::new("ssh");
    c.args([
        if tty { "-tt" } else { "-T" },
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=15",
    ])
    .arg(format!("{token}@{host}"))
    .arg(format!(
        "sh -c {}",
        crate::util::sh_quote(&root_script(script))
    ));
    Ok(c)
}

pub fn delete(id: &str, owner: &str) -> Result<()> {
    let mut requested = false;
    wait_for(
        || {
            let Some(data) = get(id)? else {
                return Ok(true);
            };
            owned(&data, owner)?;
            match data["state"].as_str() {
                Some("destroyed" | "deleted") => return Ok(true),
                Some("destroying" | "deleting") => requested = true,
                _ => {}
            }
            if data["desiredState"].as_str() == Some("destroyed") {
                requested = true;
            }
            if !requested {
                api("DELETE", &resource_path(id)?, None)?;
                requested = true;
            } else if data["state"].as_str() == Some("error") {
                bail!(
                    "Daytona sandbox deletion failed; inspect it in the Daytona dashboard, then retry cleanup"
                );
            }
            Ok(false)
        },
        "Daytona sandbox deletion is still pending; retry cleanup",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_transport_preserves_bytes_and_exit_codes_and_requires_privileges() {
        use std::os::unix::fs::PermissionsExt;
        for (user_id, sudo_allowed) in [("0", true), ("1000", true), ("1000", false)] {
            let dir = tempfile::tempdir().unwrap();
            let calls = dir.path().join("sudo-calls");
            let scripts = [
                ("id", format!("#!/bin/sh\nprintf '%s\\n' {user_id}\n")),
                (
                    "sudo",
                    format!(
                        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n{}\n",
                        crate::util::sh_quote(&calls.to_string_lossy()),
                        if sudo_allowed {
                            "shift; exec \"$@\""
                        } else {
                            "exit 1"
                        },
                    ),
                ),
            ];
            for (name, script) in scripts {
                let path = dir.path().join(name);
                std::fs::write(&path, script).unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let mut child = Command::new("sh")
                .args([
                    "-c",
                    &root_script("cat; printf '%s' \"a 'quoted' message\" >&2; exit 23"),
                ])
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        dir.path().display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"\0\xffbinary\n")
                .unwrap();
            let out = child.wait_with_output().unwrap();
            if user_id == "0" || sudo_allowed {
                assert_eq!(out.status.code(), Some(23));
                assert_eq!(out.stdout, b"\0\xffbinary\n");
                assert_eq!(out.stderr, b"a 'quoted' message");
            } else {
                assert_eq!(out.status.code(), Some(4));
                assert!(out.stdout.is_empty());
                assert!(String::from_utf8_lossy(&out.stderr).contains("requires root access"));
            }
            let calls = std::fs::read_to_string(calls).unwrap_or_default();
            assert_eq!(
                calls.lines().count(),
                if user_id == "0" {
                    0
                } else if sudo_allowed {
                    2
                } else {
                    1
                }
            );
        }
    }

    #[test]
    fn root_transport_through_sudo_looks_like_plain_root() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let scripts = [
            ("id", "#!/bin/sh\nprintf '%s\\n' 1000\n"),
            (
                "sudo",
                "#!/bin/sh\nexport SUDO_USER=daytona SUDO_UID=1000 SUDO_GID=1000 SUDO_COMMAND=sh\nshift; exec \"$@\"\n",
            ),
        ];
        for (name, script) in scripts {
            let path = dir.path().join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = Command::new("sh")
            .args([
                "-c",
                &root_script(
                    "printf '%s ' \"${SUDO_USER-unset}\" \"${SUDO_UID-unset}\" \"${SUDO_GID-unset}\" \"${SUDO_COMMAND-unset}\"",
                ),
            ])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "unset unset unset unset "
        );
    }

    #[test]
    fn polling_returns_success_errors_and_timeouts() {
        use std::time::Duration;
        let mut polls = 0;
        poll_until(
            || {
                polls += 1;
                Ok(polls == 3)
            },
            Duration::from_secs(1),
            Duration::ZERO,
            "pending",
        )
        .unwrap();
        assert_eq!(polls, 3);
        let timeout = poll_until(
            || Ok(false),
            Duration::ZERO,
            Duration::ZERO,
            "cleanup pending",
        )
        .unwrap_err();
        assert_eq!(timeout.to_string(), "cleanup pending");
        let failure = poll_until(
            || bail!("API unavailable"),
            Duration::from_secs(1),
            Duration::ZERO,
            "pending",
        )
        .unwrap_err();
        assert_eq!(failure.to_string(), "API unavailable");
    }

    #[test]
    fn validates_resource_ids_and_ownership() {
        for id in ["", "../other", "id?x=1", "id/other", "-x\n"] {
            assert!(resource_path(id).is_err());
        }
        assert_eq!(resource_path("abc-123").unwrap(), "/sandbox/abc-123");
        let data = json!({"labels": {"beam.session": "transfer-1"}});
        assert!(owned(&data, "transfer-1").is_ok());
        assert!(owned(&data, "transfer-2").is_err());
        assert!(owned(&json!({}), "transfer-1").is_err());
    }
}
