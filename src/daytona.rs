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

pub fn wake(id: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let mut requested = false;
    loop {
        match status(id)?.as_str() {
            "running" => return Ok(()),
            "stopped" | "archived" if !requested => {
                api("POST", &format!("{}/start", resource_path(id)?), None)?;
                requested = true;
            }
            "creating" | "starting" | "pending" | "restoring" | "stopped" | "archived" => {}
            state => bail!("Daytona sandbox {id} is {state}; inspect it in the Daytona dashboard"),
        }
        if std::time::Instant::now() >= deadline {
            bail!("Daytona sandbox is still starting; run `beam` to retry");
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
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
    .arg(format!("sh -c {}", crate::util::sh_quote(script)));
    Ok(c)
}

pub fn delete(id: &str, owner: &str) -> Result<()> {
    let Some(data) = get(id)? else {
        return Ok(());
    };
    owned(&data, owner)?;
    api("DELETE", &resource_path(id)?, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
