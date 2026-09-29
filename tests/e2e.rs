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
            c.arg("-T")
                .arg(host)
                .arg(format!("docker {}", args.join(" ")));
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
    let dockerfile = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/Dockerfile");
    let b = docker(
        &["build", "-q", "--network", "host", "-t", IMAGE, "-"],
        Some(&dockerfile),
    );
    assert!(b.status.success(), "docker build: {}", text(&b));
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
        !again.status.success(),
        "a second beam must fail while the session is away"
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
