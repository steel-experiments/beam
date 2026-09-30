// ABOUTME: Offline Daytona lifecycle contract tests with isolated CLI fixtures.
// ABOUTME: Verify interrupted allocation, ownership, retry identity, and credential handling.
mod common;

use common::{Env, text};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

struct Fixture {
    env: Env,
    bin: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::new("[sandbox]\nsetup = []\n");
        let bin = env.home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let curl = bin.join("curl");
        std::fs::write(
            &curl,
            r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
home = Path(os.environ['HOME'])
method = args[args.index('--request') + 1]
url = next(a for a in args if a.startswith('https://'))
path = url.split('/api', 1)[1]
with (home / 'requests').open('a') as f:
    f.write(method + ' ' + path + '\n')
if path == '/sandbox' and method == 'POST':
    body = json.loads(args[args.index('--data-binary') + 1])
    assert body['user'] == 'root'
    assert body['autoDeleteInterval'] == -1
    assert body['autoStopInterval'] == 240
    assert body['snapshot'] == 'custom-snapshot'
    data = {'id': 'sandbox-1', 'state': 'started', 'labels': body['labels']}
    (home / 'remote.json').write_text(json.dumps(data))
    if (home / 'interrupt').exists():
        sys.exit(28)
elif path == '/sandbox?limit=1':
    data = {'items': []}
elif path == '/sandbox/sandbox-1/ssh-access?expiresInMinutes=60':
    data = {'token': 'fixture-token'}
elif path == '/sandbox/sandbox-1':
    if not (home / 'remote.json').exists():
        print('{}\n404', end='')
        sys.exit(0)
    data = json.loads((home / 'remote.json').read_text())
    if method == 'DELETE':
        (home / 'remote.json').unlink()
else:
    raise Exception('unexpected request: ' + method + ' ' + path)
print(json.dumps(data) + '\n200', end='')
"#,
        )
        .unwrap();
        let ssh = bin.join("ssh");
        std::fs::write(
            &ssh,
            r#"#!/bin/sh
case "$1" in -V) echo 'OpenSSH fixture' >&2; exit 0;; esac
# Let bootstrap pass, then deliberately fail the prerequisite check before upload.
for arg do
  case "$arg" in *missing=*) echo 'missing tools: fixture' >&2; exit 4;; esac
done
exit 0
"#,
        )
        .unwrap();
        for file in [curl, ssh] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { env, bin }
    }

    fn beam(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_beam"))
            .args(args)
            .current_dir(&self.env.project)
            .env("HOME", &self.env.home)
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("DAYTONA_API_KEY", "fixture-api-key")
            .env_remove("DAYTONA_API_URL")
            .env_remove("DAYTONA_SSH_HOST")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    }

    fn allocations(&self) -> usize {
        std::fs::read_to_string(self.env.home.join("requests"))
            .unwrap()
            .lines()
            .filter(|s| *s == "POST /sandbox")
            .count()
    }
}

#[test]
fn retry_reuses_allocation_and_cleanup_requires_ownership() {
    let f = Fixture::new();
    let up = f.beam(&[
        "--to",
        "daytona:custom-snapshot",
        "--agent",
        "shell",
        "--yes",
        "--detach",
    ]);
    assert!(!up.status.success(), "{}", text(&up));
    assert!(
        text(&up).contains("missing tools: fixture"),
        "{}",
        text(&up)
    );
    let state = f.env.state();
    assert_eq!(state["sandbox"]["kind"], "daytona");
    assert_eq!(state["sandbox"]["id"], "sandbox-1");
    let retry = f.beam(&["--yes", "--detach"]);
    assert!(!retry.status.success());
    assert_eq!(f.allocations(), 1);
    let pointer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.env.project.join(".beam/state.json")).unwrap())
            .unwrap();
    let receipt = std::path::Path::new(pointer["record"].as_str().unwrap())
        .parent()
        .unwrap()
        .join("allocation.json");
    let record = std::fs::read_to_string(receipt).unwrap();
    assert!(!text(&up).contains("fixture-api-key"));
    assert!(!record.contains("fixture-api-key"));
    assert!(!record.contains("fixture-token"));

    let remote = f.env.home.join("remote.json");
    let original = std::fs::read(&remote).unwrap();
    let mut foreign: serde_json::Value = serde_json::from_slice(&original).unwrap();
    foreign["labels"]["beam.session"] = serde_json::json!("someone-else");
    std::fs::write(&remote, serde_json::to_vec(&foreign).unwrap()).unwrap();
    let kill = f.beam(&["kill", "--yes"]);
    assert!(!kill.status.success());
    assert!(text(&kill).contains("not owned"), "{}", text(&kill));
    assert!(remote.exists());
    std::fs::write(&remote, original).unwrap();
    let kill = f.beam(&["kill", "--yes"]);
    assert!(kill.status.success(), "{}", text(&kill));
    assert!(!remote.exists());
}

#[test]
fn ambiguous_allocation_requires_explicit_owned_recovery() {
    let f = Fixture::new();
    std::fs::write(f.env.home.join("interrupt"), "").unwrap();
    let up = f.beam(&[
        "--to",
        "daytona:custom-snapshot",
        "--agent",
        "shell",
        "--yes",
        "--detach",
    ]);
    assert!(!up.status.success());
    let retry = f.beam(&["--yes", "--detach"]);
    assert!(!retry.status.success());
    assert!(
        text(&retry).contains("--recover-sandbox ID"),
        "{}",
        text(&retry)
    );
    assert_eq!(f.allocations(), 1);
    let kill = f.beam(&["kill", "--yes"]);
    assert!(!kill.status.success());
    assert!(f.env.home.join("remote.json").exists());

    let remote = f.env.home.join("remote.json");
    let original = std::fs::read(&remote).unwrap();
    let mut foreign: serde_json::Value = serde_json::from_slice(&original).unwrap();
    foreign["labels"]["beam.session"] = serde_json::json!("someone-else");
    std::fs::write(&remote, serde_json::to_vec(&foreign).unwrap()).unwrap();
    let rejected = f.beam(&["--recover-sandbox", "sandbox-1", "--yes", "--detach"]);
    assert!(text(&rejected).contains("not owned"), "{}", text(&rejected));
    std::fs::write(&remote, original).unwrap();
    let recovered = f.beam(&["--recover-sandbox", "sandbox-1", "--yes", "--detach"]);
    assert!(
        text(&recovered).contains("missing tools: fixture"),
        "{}",
        text(&recovered)
    );
    assert_eq!(f.env.state()["sandbox"]["id"], "sandbox-1");
    assert_eq!(f.allocations(), 1);
    assert!(f.beam(&["kill", "--yes"]).status.success());
}
