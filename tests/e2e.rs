// ABOUTME: End-to-end test: runs the beam binary up to a real Docker sandbox and down again.
// ABOUTME: Set BEAM_E2E_TARGET (for example docker+ssh://agent) and run: cargo test -- --ignored

mod common;

use common::{Env, text};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const IMAGE: &str = "beam-e2e:latest";

fn target() -> String {
    std::env::var("BEAM_E2E_TARGET").expect("set BEAM_E2E_TARGET, for example docker+ssh://agent")
}

/// Run a docker command where the target has Docker.
fn docker(args: &[&str], stdin: Option<&Path>) -> Output {
    let t = target();
    let mut c = match t.strip_prefix("docker+ssh://") {
        Some(host) => {
            let mut c = Command::new("ssh");
            c.arg("-T").arg(host).arg(format!(
                "docker {}",
                args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
            ));
            c
        }
        None => {
            assert_eq!(
                t, "docker",
                "the e2e test supports docker and docker+ssh:// targets"
            );
            let mut c = Command::new("docker");
            c.args(args);
            c
        }
    };
    if let Some(f) = stdin {
        c.stdin(std::fs::File::open(f).unwrap());
    }
    c.output().unwrap()
}

fn container(env: &Env) -> String {
    env.state()["sandbox"]["container"]
        .as_str()
        .unwrap()
        .to_string()
}

fn setup() -> Env {
    static IMAGE_READY: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    IMAGE_READY.get_or_init(|| {
        let dockerfile = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/Dockerfile");
        let b = docker(
            &["build", "-q", "--network", "host", "-t", IMAGE, "-"],
            Some(&dockerfile),
        );
        assert!(b.status.success(), "docker build: {}", text(&b));
    });
    Env::new(&format!(
        "[beam]\nto = \"{}\"\n[env]\nforward = [\"BEAM_E2E_VAR\"]\n[sandbox]\nimage = \"{IMAGE}\"\nsetup = [\"echo ok > setup-ran.txt\"]\n",
        target()
    ))
}

fn wait_for_file(c: &str, path: &str) -> String {
    let start = Instant::now();
    loop {
        let o = docker(&["exec", c, "cat", path], None);
        if o.status.success() {
            return String::from_utf8_lossy(&o.stdout).to_string();
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "timeout: {path} not in the sandbox"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs Docker; set BEAM_E2E_TARGET and run with --ignored"]
fn beam_up_and_down() {
    let env = setup();

    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "beam up failed:\n{}", text(&up));
    assert!(text(&up).contains("Session is live"), "{}", text(&up));
    let c = container(&env);

    let again = env.beam(&["--yes", "--detach"]);
    assert!(
        again.status.success(),
        "a second beam must reuse the existing remote session"
    );
    assert!(text(&again).contains("already beamed"), "{}", text(&again));

    wait_for_file(&c, "/tmp/fake-claude-ready");
    let handoff = wait_for_file(&c, "/tmp/fake-claude-handoff");
    assert!(
        handoff.starts_with("[beam] You were moved from"),
        "{handoff}"
    );
    assert!(
        handoff.contains("Setup command succeeded: echo ok > setup-ran.txt"),
        "{handoff}"
    );
    assert!(handoff.contains("except: .env."), "{handoff}");
    assert_eq!(
        wait_for_file(&c, "/tmp/fake-claude-env").trim(),
        "forwarded-value"
    );
    let p = env.project.to_string_lossy().to_string();
    let listing = docker(&["exec", &c, "ls", "-a", &p], None);
    assert!(
        !text(&listing).contains("node_modules"),
        "ignored dirs must not be copied: {}",
        text(&listing)
    );

    let status = env.beam(&["status"]);
    assert!(
        text(&status).contains("agent   running"),
        "{}",
        text(&status)
    );
    assert!(text(&env.beam(&["ls"])).contains("e2e-session"));

    let down = env.beam(&["down"]);
    assert!(down.status.success(), "beam down failed:\n{}", text(&down));
    assert!(text(&down).contains("Session is home"), "{}", text(&down));

    env.assert_home_again();

    let ps = docker(&["ps", "-a", "-q", "--filter", &format!("name={c}")], None);
    assert!(
        String::from_utf8_lossy(&ps.stdout).trim().is_empty(),
        "the container must be removed"
    );
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn exec(c: &str, script: &str) -> Output {
    docker(&["exec", c, "sh", "-c", script], None)
}
fn write_config(env: &Env, extra: &str, setup: &str) {
    std::fs::write(
        env.project.join("beam.toml"),
        format!(
            "[beam]\nto = {:?}\n{extra}\n[sandbox]\nimage = {IMAGE:?}\nsetup = [{setup:?}]\n",
            target()
        ),
    )
    .unwrap();
}

#[test]
#[ignore = "needs Docker"]
fn shell_workspace_returns_extras_and_keeps_a_managed_sandbox() {
    let env = setup();
    std::fs::remove_dir_all(env.home.join(".claude")).unwrap();
    write_config(&env, "[files]\nreturn_extras = ['.env']", "true");
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    assert_eq!(env.state()["agent"], "shell");
    let c = container(&env);
    assert!(
        exec(
            &c,
            &format!(
                "printf 'REMOTE=2\\n' > {}",
                quote(&env.project.join(".env").to_string_lossy())
            )
        )
        .status
        .success()
    );
    let down = env.beam(&["down", "--keep"]);
    assert!(down.status.success(), "{}", text(&down));
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "REMOTE=2\n"
    );
    assert_eq!(env.state()["phase"], "retained");
    assert!(text(&env.beam(&["ls"])).contains("sandbox retained"));
    let kill = env.beam(&["kill", "--yes"]);
    assert!(kill.status.success(), "{}", text(&kill));
    assert!(!env.project.join(".beam/state.json").exists());
}

#[test]
#[ignore = "needs Docker"]
fn failed_setup_is_visible_and_retry_does_not_allocate_another_sandbox() {
    let env = setup();
    write_config(&env, "", "test -f /tmp/beam-setup-fixed");
    let up = env.beam(&["--yes", "--detach"]);
    assert!(!up.status.success(), "{}", text(&up));
    assert!(text(&up).contains("needs-attention"));
    assert!(!text(&up).contains("Session is live"));
    let c = container(&env);
    let logs = env.beam(&["logs"]);
    assert!(logs.status.success());
    assert!(text(&logs).contains("Setup failed"));
    assert!(exec(&c, "touch /tmp/beam-setup-fixed").status.success());
    let retry = env.beam(&["--yes", "--detach"]);
    assert!(retry.status.success(), "{}", text(&retry));
    assert_eq!(container(&env), c);
    wait_for_file(&c, "/tmp/fake-claude-ready");
    assert!(env.beam(&["down"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn concurrent_local_edits_get_a_recovery_worktree() {
    let env = setup();
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    let c = container(&env);
    wait_for_file(&c, "/tmp/fake-claude-ready");
    std::fs::write(env.project.join("README.md"), "local work\n").unwrap();
    let record = env.state();
    let down = env.beam(&["down"]);
    assert!(!down.status.success());
    assert!(
        text(&down).contains("Local changes were preserved"),
        "{}",
        text(&down)
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join("README.md")).unwrap(),
        "local work\n"
    );
    let records = env
        .home
        .join(".beam/transfers")
        .join(record["transfer_id"].as_str().unwrap());
    assert!(records.join("worktree/sandbox-work.txt").exists());
    assert!(
        std::fs::read_to_string(records.join("worktree/README.md"))
            .unwrap()
            .contains("uncommitted line")
    );
}

#[test]
#[ignore = "needs Docker"]
fn retry_after_apply_does_not_repeat_local_changes() {
    let env = setup();
    assert!(env.beam(&["--yes", "--detach"]).status.success());
    wait_for_file(&container(&env), "/tmp/fake-claude-ready");
    let down = env.beam(&["down", "--keep"]);
    assert!(down.status.success(), "{}", text(&down));
    let mut state = env.state();
    // Simulate interruption after apply and before resource cleanup.
    state["phase"] = "applied".into();
    let record = env
        .home
        .join(".beam/transfers")
        .join(state["transfer_id"].as_str().unwrap())
        .join("state.json");
    std::fs::write(&record, serde_json::to_vec(&state).unwrap()).unwrap();
    std::fs::write(env.project.join("README.md"), "new work after return\n").unwrap();
    let retry = env.beam(&["down"]);
    assert!(retry.status.success(), "{}", text(&retry));
    assert_eq!(
        std::fs::read_to_string(env.project.join("README.md")).unwrap(),
        "new work after return\n"
    );
}

#[test]
#[ignore = "needs Docker"]
fn missing_project_pointer_is_rebuilt_from_records() {
    let env = setup();
    assert!(env.beam(&["--yes", "--detach"]).status.success());
    let c = container(&env);
    std::fs::remove_file(env.project.join(".beam/state.json")).unwrap();
    let retry = env.beam(&["--yes", "--detach"]);
    assert!(retry.status.success(), "{}", text(&retry));
    assert!(text(&env.beam(&["status"])).contains(&c));
    assert!(env.project.join(".beam/state.json").exists());
    assert!(env.beam(&["down"]).status.success());
}
