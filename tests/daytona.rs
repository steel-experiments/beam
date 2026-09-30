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
    receipts = list((home / '.beam/transfers').glob('*/allocation.json'))
    assert len(receipts) == 1 and receipts[0].read_bytes() == b''
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
elif path == '/sandbox/sandbox-1/start':
    data = json.loads((home / 'remote.json').read_text())
    assert data['state'] in ('stopped', 'archived')
    data['state'] = 'starting'
    (home / 'remote.json').write_text(json.dumps(data))
elif path == '/sandbox/sandbox-1':
    if not (home / 'remote.json').exists():
        print('{}\n404', end='')
        sys.exit(0)
    data = json.loads((home / 'remote.json').read_text())
    if method == 'DELETE':
        if (home / 'delete-states.json').exists():
            data['state'] = 'destroying'
            data['desiredState'] = 'destroyed'
            (home / 'remote.json').write_text(json.dumps(data))
            if (home / 'delete-interrupt').exists():
                sys.exit(28)
        else:
            (home / 'remote.json').unlink()
    elif data['state'] == 'destroying' and (home / 'delete-poll-fails').exists():
        print('{}\n503', end='')
        sys.exit(0)
    else:
        if data['state'] == 'destroying':
            assert (home / 'dev/app/.beam/state.json').exists()
        queue = home / ('delete-states.json' if data['state'] == 'destroying' else 'states.json')
        if queue.exists():
            states = json.loads(queue.read_text())
            if states:
                data['state'] = states.pop(0)
                queue.write_text(json.dumps(states))
                (home / 'remote.json').write_text(json.dumps(data))
                if data['state'] == 'missing':
                    (home / 'remote.json').unlink()
                    print('{}\n404', end='')
                    sys.exit(0)
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

    fn up(&self) -> std::process::Output {
        self.beam(&[
            "--to",
            "daytona:custom-snapshot",
            "--agent",
            "shell",
            "--yes",
            "--detach",
        ])
    }

    fn states(&self, file: &str, states: &[&str]) {
        std::fs::write(
            self.env.home.join(file),
            serde_json::to_vec(states).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn snapshot_preparation_waits_until_the_sandbox_starts() {
    let f = Fixture::new();
    f.states(
        "states.json",
        &[
            "pending_build",
            "building_snapshot",
            "pulling_snapshot",
            "creating",
            "starting",
            "started",
        ],
    );
    let up = f.up();
    assert!(
        text(&up).contains("missing tools: fixture"),
        "{}",
        text(&up)
    );
    assert_eq!(f.allocations(), 1);
    assert!(f.beam(&["kill", "--yes"]).status.success());
}

#[test]
fn stopping_and_archiving_finish_before_start_is_requested() {
    for states in [
        vec!["stopping", "stopped", "starting", "started"],
        vec!["archiving", "archived", "restoring", "started"],
    ] {
        let f = Fixture::new();
        f.states("states.json", &states);
        let up = f.up();
        assert!(
            text(&up).contains("missing tools: fixture"),
            "{}",
            text(&up)
        );
        let requests = std::fs::read_to_string(f.env.home.join("requests")).unwrap();
        assert_eq!(
            requests
                .lines()
                .filter(|line| *line == "POST /sandbox/sandbox-1/start")
                .count(),
            1
        );
        assert!(f.beam(&["kill", "--yes"]).status.success());
    }
}

#[test]
fn terminal_startup_failure_keeps_the_allocation_for_retry() {
    let f = Fixture::new();
    f.states("states.json", &["build_failed"]);
    let up = f.up();
    assert!(text(&up).contains("is build_failed"), "{}", text(&up));
    assert_eq!(f.env.state()["sandbox"]["id"], "sandbox-1");
    let retry = f.beam(&["--yes", "--detach"]);
    assert!(text(&retry).contains("is build_failed"), "{}", text(&retry));
    assert_eq!(f.allocations(), 1);
    assert!(f.beam(&["kill", "--yes"]).status.success());
}

#[test]
fn transport_failure_preserves_the_sandbox_and_remote_exit_code() {
    let f = Fixture::new();
    std::fs::write(
        f.bin.join("ssh"),
        "#!/bin/sh\ncase \"$1\" in -V) exit 0;; esac\necho 'remote bootstrap failed' >&2\nexit 37\n",
    ).unwrap();
    let up = f.up();
    assert!(!up.status.success(), "{}", text(&up));
    assert!(
        text(&up).contains("remote bootstrap failed"),
        "{}",
        text(&up)
    );
    assert!(text(&up).contains("37"), "{}", text(&up));
    assert!(!text(&up).contains("fixture-api-key"));
    assert_eq!(f.env.state()["sandbox"]["id"], "sandbox-1");
    assert!(f.beam(&["kill", "--yes"]).status.success());
}

#[test]
fn interrupted_deletion_resumes_without_another_delete_request() {
    let f = Fixture::new();
    assert!(text(&f.up()).contains("missing tools: fixture"));
    f.states("delete-states.json", &["destroyed"]);
    std::fs::write(f.env.home.join("delete-interrupt"), "").unwrap();
    let kill = f.beam(&["kill", "--yes"]);
    assert!(!kill.status.success(), "{}", text(&kill));
    assert!(f.env.project.join(".beam/state.json").exists());
    let retry = f.beam(&["kill", "--yes"]);
    assert!(retry.status.success(), "{}", text(&retry));
    assert!(!f.env.project.join(".beam/state.json").exists());
    let remote: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.env.home.join("remote.json")).unwrap()).unwrap();
    assert_eq!(remote["state"], "destroyed");
    let requests = std::fs::read_to_string(f.env.home.join("requests")).unwrap();
    assert_eq!(
        requests
            .lines()
            .filter(|line| *line == "DELETE /sandbox/sandbox-1")
            .count(),
        1
    );
}

#[test]
fn deletion_error_keeps_cleanup_pending() {
    let f = Fixture::new();
    assert!(text(&f.up()).contains("missing tools: fixture"));
    f.states("delete-states.json", &["error"]);
    let kill = f.beam(&["kill", "--yes"]);
    assert!(!kill.status.success(), "{}", text(&kill));
    assert!(text(&kill).contains("deletion failed"), "{}", text(&kill));
    assert!(f.env.project.join(".beam/state.json").exists());
    assert!(f.env.home.join("remote.json").exists());
}

#[test]
fn cleanup_waits_for_deletion_and_retries_after_a_poll_failure() {
    let f = Fixture::new();
    assert!(text(&f.up()).contains("missing tools: fixture"));
    f.states("delete-states.json", &["destroying", "missing"]);
    std::fs::write(f.env.home.join("delete-poll-fails"), "").unwrap();
    let kill = f.beam(&["kill", "--yes"]);
    assert!(!kill.status.success(), "{}", text(&kill));
    assert!(text(&kill).contains("HTTP 503"), "{}", text(&kill));
    assert!(f.env.project.join(".beam/state.json").exists());
    assert!(f.env.home.join("remote.json").exists());
    std::fs::remove_file(f.env.home.join("delete-poll-fails")).unwrap();
    let kill = f.beam(&["kill", "--yes"]);
    assert!(kill.status.success(), "{}", text(&kill));
    assert!(!f.env.project.join(".beam/state.json").exists());
    assert!(!f.env.home.join("remote.json").exists());
    let requests = std::fs::read_to_string(f.env.home.join("requests")).unwrap();
    assert_eq!(
        requests
            .lines()
            .filter(|line| *line == "DELETE /sandbox/sandbox-1")
            .count(),
        1
    );
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
