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
    assert!(
        text(&up).contains("Remote process is running"),
        "{}",
        text(&up)
    );
    let c = container(&env);

    let again = env.beam(&["--yes", "--detach"]);
    assert!(
        again.status.success(),
        "a second beam must reuse the existing remote session"
    );
    assert!(
        text(&again).contains("Remote process is running"),
        "{}",
        text(&again)
    );

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
        text(&status).contains("Remote process is running"),
        "{}",
        text(&status)
    );
    assert!(text(&env.beam(&["ls"])).contains("e2e-session"));

    let down = env.beam(&["down"]);
    assert!(down.status.success(), "beam down failed:\n{}", text(&down));
    assert!(
        text(&down).contains("Remote work applied"),
        "{}",
        text(&down)
    );

    env.assert_home_again();

    let ps = docker(&["ps", "-a", "-q", "--filter", &format!("name={c}")], None);
    assert!(
        String::from_utf8_lossy(&ps.stdout).trim().is_empty(),
        "the container must be removed"
    );
}

#[test]
#[ignore = "needs Docker"]
fn a_session_that_beams_itself_continues_remotely_and_its_conversation_returns() {
    let env = setup();
    let up = env.beam(&[
        "--yes",
        "--detach",
        "--force",
        "--session",
        common::SESSION,
        "--permission-mode",
        "acceptEdits",
        "--continue",
        "finish the parser tests",
    ]);
    assert!(up.status.success(), "beam up failed:\n{}", text(&up));
    assert!(text(&up).contains("acceptEdits"), "{}", text(&up));
    let c = container(&env);
    wait_for_file(&c, "/tmp/fake-claude-ready");
    let args = wait_for_file(&c, "/tmp/fake-claude-args");
    assert!(args.contains("--permission-mode acceptEdits"), "{args}");
    let handoff = wait_for_file(&c, "/tmp/fake-claude-handoff");
    assert!(
        handoff.contains("Do not wait for a reply. Next step: finish the parser tests"),
        "{handoff}"
    );

    // The local session writes the beam tool result and its last reply after the send.
    let transcript = env
        .home
        .join(".claude/projects")
        .join(common::encode(&env.project))
        .join(format!("{}.jsonl", common::SESSION));
    let own_turn = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Close me.\"}]}}\n";
    std::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .and_then(|mut f| std::io::Write::write_all(&mut f, own_turn.as_bytes()))
        .unwrap();

    let down = env.beam(&["down"]);
    assert!(down.status.success(), "beam down failed:\n{}", text(&down));
    assert!(
        text(&down).contains("remote conversation applied"),
        "{}",
        text(&down)
    );
    assert!(
        text(&down)
            .contains("claude --resume e2e-session '[beam] You are back on the local machine"),
        "{}",
        text(&down)
    );
    env.assert_home_again();
    assert!(
        !env.transcript().contains("Close me."),
        "{}",
        env.transcript()
    );
}

#[test]
#[ignore = "needs Docker"]
fn beam_down_in_the_sandbox_packs_and_a_waiting_local_beam_brings_it_home() {
    let env = setup();
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "beam up failed:\n{}", text(&up));
    let c = container(&env);
    wait_for_file(&c, "/tmp/fake-claude-ready");
    let skill = env.home.join(".claude/skills/beam/SKILL.md");
    assert!(
        text(&exec(
            &c,
            &format!("cat {}", quote(&skill.to_string_lossy()))
        ))
        .contains("name: beam"),
        "the sandbox must have the beam skill"
    );
    assert!(!skill.exists(), "the skill is only in the sandbox");
    let help = exec(&c, "beam status");
    assert!(!help.status.success());
    assert!(
        text(&help).contains("runs on your local machine"),
        "{}",
        text(&help)
    );

    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| env.beam(&["down", "--wait"]));
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            !waiting.is_finished(),
            "beam down --wait must wait for the sandbox"
        );
        let remote = exec(&c, "beam down");
        assert!(remote.status.success(), "{}", text(&remote));
        assert!(text(&remote).contains("On your local machine, run: beam down"));
        let down = waiting.join().unwrap();
        assert!(
            down.status.success(),
            "beam down --wait failed:\n{}",
            text(&down)
        );
        assert!(
            text(&down).contains("Remote work applied"),
            "{}",
            text(&down)
        );
    });
    env.assert_home_again();
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
    assert!(text(&down).contains("Further sandbox edits will not return"));
    let status = env.beam(&["status"]);
    assert!(text(&status).contains("Further sandbox edits will not return"));
    assert_eq!(env.state()["phase"], "retained");
    assert!(text(&env.beam(&["ls"])).contains("sandbox retained"));
    assert!(
        exec(
            &c,
            &format!(
                "echo LATER=3 > {}",
                quote(&env.project.join(".env").to_string_lossy())
            )
        )
        .status
        .success()
    );
    let cleanup = env.beam(&["down"]);
    assert!(cleanup.status.success(), "{}", text(&cleanup));
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "REMOTE=2\n"
    );
    assert!(!env.project.join(".beam/state.json").exists());
}

#[test]
#[ignore = "needs Docker"]
fn failed_setup_is_visible_and_retry_does_not_allocate_another_sandbox() {
    let env = setup();
    write_config(&env, "", "test -f /tmp/beam-setup-fixed");
    let up = env.beam(&["--yes", "--detach", "--agent", "shell"]);
    assert!(!up.status.success(), "{}", text(&up));
    assert!(text(&up).contains("needs-attention"));
    assert!(!text(&up).contains("Remote process is running"));
    let c = container(&env);
    let status = env.beam(&["status"]);
    assert!(
        text(&status).contains("Next: beam logs"),
        "{}",
        text(&status)
    );
    assert!(!text(&status).contains("Next: beam attach"));
    let logs = env.beam(&["logs"]);
    assert!(logs.status.success());
    assert!(text(&logs).contains("Setup failed"));
    assert!(exec(&c, "touch /tmp/beam-setup-fixed").status.success());
    let retry = env.beam(&["--yes", "--detach", "--agent", "shell"]);
    assert!(retry.status.success(), "{}", text(&retry));
    assert_eq!(container(&env), c);
    assert!(text(&env.beam(&["status"])).contains("Remote process is running"));
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
    let status = env.beam(&["status"]);
    assert!(status.status.success());
    assert!(text(&status).contains(&records.display().to_string()));
    assert!(text(&status).contains("Next: cd "));
    let json = env.beam(&["status", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        json["saved_recovery"]["receipt"],
        records.to_string_lossy().as_ref()
    );
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

fn remote_script(env: &Env, script: &str) -> Output {
    docker(
        &[
            "exec",
            &container(env),
            "sh",
            "-c",
            &format!("cd {} && {script}", quote(env.project.to_str().unwrap())),
        ],
        None,
    )
}

#[test]
#[ignore = "needs Docker"]
fn reviewed_merge_preserves_staging_and_undo_restores_extras() {
    let env = setup();
    let config = std::fs::read_to_string(env.project.join("beam.toml")).unwrap();
    std::fs::write(
        env.project.join("beam.toml"),
        format!("{config}\n[files]\nreturn_extras = ['.env']\n"),
    )
    .unwrap();
    let up = env.beam(&["--yes", "--detach", "--agent", "shell"]);
    assert!(up.status.success(), "{}", text(&up));
    common::sh(
        &env.project,
        "printf 'local staged\n' > README.md && git add README.md && printf 'local unstaged\n' >> README.md",
    );
    let before = common::sh(&env.project, "git status --porcelain=v1 -uall");
    let remote = remote_script(
        &env,
        "printf 'remote\n' > notes.txt && printf 'KEY=2\n' > .env && rm staged.txt && printf '\\000\\001' > binary && chmod +x binary && ln -s notes.txt link",
    );
    assert!(remote.status.success(), "{}", text(&remote));
    let review = env.beam(&["down", "--review"]);
    assert!(review.status.success(), "{}", text(&review));
    assert_eq!(
        common::sh(&env.project, "git status --porcelain=v1 -uall"),
        before
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "KEY=1\n"
    );
    let plan = env.beam(&["review", "--json"]);
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(plan["target"].is_object(), "{plan}");
    assert!(plan["conflicts"].as_array().unwrap().is_empty());
    let applied = env.beam(&["review", "--apply", "--keep"]);
    assert!(applied.status.success(), "{}", text(&applied));
    assert_eq!(
        common::sh(&env.project, "git show :README.md"),
        "local staged\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join("README.md")).unwrap(),
        "local staged\nlocal unstaged\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "remote\n"
    );
    assert!(!env.project.join("staged.txt").exists());
    assert_eq!(common::sh(&env.project, "git show :staged.txt"), "staged\n");
    assert_eq!(
        std::fs::read(env.project.join("binary")).unwrap(),
        b"\0\x01"
    );
    assert!(env.project.join("link").is_symlink());
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "KEY=2\n"
    );
    std::fs::write(env.project.join("notes.txt"), "later work\n").unwrap();
    let undo = env.beam(&["undo"]);
    assert!(!undo.status.success(), "undo must preserve later edits");
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "later work\n"
    );
    std::fs::write(env.project.join("notes.txt"), "remote\n").unwrap();
    let undo = env.beam(&["undo"]);
    assert!(undo.status.success(), "{}", text(&undo));
    assert_eq!(
        common::sh(&env.project, "git status --porcelain=v1 -uall"),
        before
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "KEY=1\n"
    );
    assert!(env.beam(&["undo"]).status.success());
    assert!(env.beam(&["kill", "--yes"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn stale_review_rebuilds_and_conflicts_can_be_resolved() {
    let env = setup();
    assert!(
        env.beam(&["--yes", "--detach", "--agent", "shell"])
            .status
            .success()
    );
    assert!(
        remote_script(&env, "echo remote > notes.txt")
            .status
            .success()
    );
    assert!(env.beam(&["down", "--review"]).status.success());
    std::fs::write(env.project.join("README.md"), "late local edit\n").unwrap();
    let stale = env.beam(&["review", "--apply"]);
    assert!(!stale.status.success(), "{}", text(&stale));
    assert!(
        text(&stale).contains("plan was rebuilt"),
        "{}",
        text(&stale)
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "untracked\n"
    );
    std::fs::write(env.project.join("notes.txt"), "local conflict\n").unwrap();
    let refreshed = env.beam(&["review", "--refresh"]);
    assert!(refreshed.status.success(), "{}", text(&refreshed));
    assert!(!env.beam(&["review", "--apply"]).status.success());
    let returned = env.beam(&["down"]);
    assert!(
        !returned.status.success(),
        "conflicts are reported with failure"
    );
    assert!(
        text(&returned).contains("Sandbox removed"),
        "{}",
        text(&returned)
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "local conflict\n"
    );
    assert!(text(&env.beam(&["status"])).contains("Saved recovery"));
    let resolved = env.beam(&["review", "--resolved"]);
    assert!(resolved.status.success(), "{}", text(&resolved));
    assert!(!text(&env.beam(&["status"])).contains("Saved recovery"));
    assert!(env.beam(&["review", "--diff"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn task_checks_reports_watch_and_restart_reuse() {
    let env = setup();
    let config = format!(
        "[beam]\nto = {:?}\n[sandbox]\nimage = '{IMAGE}'\nsetup = ['echo setup >> /tmp/beam-setup-count']\nverify = ['test -f README.md']\nreuse_setup = true\n[task]\nobjective = 'Fix redirect'\ncomplete_when = 'Redirect test passes'\nlast_verified = 'Failure reproduced'\nnext_action = 'Inspect callback'\nconstraints = ['Keep the public API']\n",
        target()
    );
    std::fs::write(env.project.join("beam.toml"), config).unwrap();
    let up = env.beam(&["--yes", "--detach", "--agent", "shell"]);
    assert!(up.status.success(), "{}", text(&up));
    let st = env.state();
    let stage = st["stage"].as_str().unwrap();
    let handoff = remote_script(
        &env,
        &format!("cat {}", quote(&format!("{stage}/handoff.txt"))),
    );
    assert!(text(&handoff).contains("Objective: Fix redirect"));
    assert!(text(&handoff).contains("Project check passed"));
    assert!(
        remote_script(
            &env,
            &format!(
                "sh {} waiting 'Need a decision'",
                quote(&format!("{stage}/report.sh"))
            )
        )
        .status
        .success()
    );
    let status = env.beam(&["status", "--watch", "--json", "--count", "1"]);
    assert!(status.status.success(), "{}", text(&status));
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(
        value["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "agent-reported-waiting")
    );
    let tmux = st["tmux"].as_str().unwrap();
    assert!(
        remote_script(
            &env,
            &format!("tmux send-keys -t {} 'exit' Enter", quote(tmux))
        )
        .status
        .success()
    );
    let start = Instant::now();
    while !text(&env.beam(&["status"])).contains("stopped") {
        assert!(start.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(100));
    }
    let restart = env.beam(&["restart"]);
    assert!(restart.status.success(), "{}", text(&restart));
    let start = Instant::now();
    loop {
        let status = text(&env.beam(&["status"]));
        if status.contains("Remote process is running") {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "{status}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let events = remote_script(
        &env,
        &format!("cat {}", quote(&format!("{stage}/events.tsv"))),
    );
    assert!(text(&events).contains("setup-reused"), "{}", text(&events));
    let count = remote_script(&env, "wc -l < /tmp/beam-setup-count");
    assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1");
    assert!(
        !env.beam(&["restart"]).status.success(),
        "must not stop a running session"
    );
    assert!(
        remote_script(&env, "echo changed >> README.md")
            .status
            .success()
    );
    assert!(
        remote_script(
            &env,
            &format!("tmux send-keys -t {} 'exit' Enter", quote(tmux))
        )
        .status
        .success()
    );
    let start = Instant::now();
    while !text(&env.beam(&["status"])).contains("stopped") {
        assert!(start.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(env.beam(&["restart"]).status.success());
    let start = Instant::now();
    loop {
        let count = remote_script(&env, "wc -l < /tmp/beam-setup-count");
        if String::from_utf8_lossy(&count.stdout).trim() == "2" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "setup did not invalidate"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(env.beam(&["down"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn failed_project_check_starts_agent_repair_and_requires_real_checks() {
    let env = setup();
    let config = std::fs::read_to_string(env.project.join("beam.toml")).unwrap();
    std::fs::write(
        env.project.join("beam.toml"),
        format!("{config}\nverify = ['test -f /tmp/beam-check-fixed']\n"),
    )
    .unwrap();
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    wait_for_file(&container(&env), "/tmp/fake-claude-ready");
    let handoff = wait_for_file(&container(&env), "/tmp/fake-claude-handoff");
    assert!(handoff.contains("Project check FAILED"), "{handoff}");
    assert!(
        handoff.contains("Repair the sandbox environment"),
        "{handoff}"
    );
    let st = env.state();
    let stage = st["stage"].as_str().unwrap();
    assert!(
        remote_script(
            &env,
            &format!(
                "sh {} finished 'claims repaired'",
                quote(&format!("{stage}/report.sh"))
            )
        )
        .status
        .success()
    );
    let status = text(&env.beam(&["status", "--json"]));
    let value: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(value["process"], "repairing");
    assert_eq!(value["next_action"], "beam attach");
    assert!(
        !remote_script(&env, &format!("sh {}", quote(&format!("{stage}/check.sh"))))
            .status
            .success()
    );
    assert!(
        remote_script(
            &env,
            &format!(
                "touch /tmp/beam-check-fixed && sh {}",
                quote(&format!("{stage}/check.sh"))
            )
        )
        .status
        .success()
    );
    let status = env.beam(&["status", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["process"], "running");
    assert!(env.beam(&["down"]).status.success());
}

fn exited_repair_agent_return(interrupt: bool) {
    let env = setup();
    let config = std::fs::read_to_string(env.project.join("beam.toml")).unwrap();
    std::fs::write(
        env.project.join("beam.toml"),
        format!("{config}\nverify = ['false']\n"),
    )
    .unwrap();
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    wait_for_file(&container(&env), "/tmp/fake-claude-ready");
    let st = env.state();
    let tmux = st["tmux"].as_str().unwrap();
    let exit = if interrupt {
        format!("tmux send-keys -t {} C-c", quote(tmux))
    } else {
        "touch /tmp/fake-claude-exit".into()
    };
    assert!(remote_script(&env, &exit).status.success());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = env.beam(&["status", "--json"]);
        assert!(status.status.success(), "{}", text(&status));
        let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        if value["process"] == "needs-attention" {
            assert_eq!(value["phase"], "environment repair unfinished");
            assert_eq!(value["next_action"], "beam attach");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "repair did not settle: {}",
            text(&status)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    if !interrupt {
        let stage = st["stage"].as_str().unwrap();
        let code = remote_script(
            &env,
            &format!("cat {}", quote(&format!("{stage}/agent.exit"))),
        );
        assert_eq!(String::from_utf8_lossy(&code.stdout).trim(), "0");
    }
    assert!(
        remote_script(&env, &format!("tmux has-session -t {}", quote(tmux)))
            .status
            .success()
    );
    let down = env.beam(&["down"]);
    assert!(down.status.success(), "{}", text(&down));
    assert!(
        text(&down)
            .contains("Remote repair conversation returned; environment checks did not pass."),
        "{}",
        text(&down)
    );
    assert!(!env.project.join(".beam/state.json").exists());
}

#[test]
#[ignore = "needs Docker"]
fn exited_repair_agent_keeps_inspection_shell_and_returns_unfinished_note() {
    exited_repair_agent_return(false);
}

#[test]
#[ignore = "needs Docker"]
fn interrupted_repair_agent_keeps_inspection_shell_and_returns_unfinished_note() {
    exited_repair_agent_return(true);
}

#[test]
#[ignore = "needs Docker"]
fn review_checks_extra_edits_and_retries_after_git_apply() {
    let env = setup();
    let config = std::fs::read_to_string(env.project.join("beam.toml")).unwrap();
    std::fs::write(
        env.project.join("beam.toml"),
        format!("{config}\n[files]\nreturn_extras = ['.env']\n"),
    )
    .unwrap();
    assert!(
        env.beam(&["--yes", "--detach", "--agent", "shell"])
            .status
            .success()
    );
    assert!(
        remote_script(&env, "echo remote > notes.txt && echo KEY=remote > .env")
            .status
            .success()
    );
    assert!(env.beam(&["down", "--review"]).status.success());
    std::fs::write(env.project.join(".env"), "KEY=local\n").unwrap();
    let changed = env.beam(&["review", "--apply"]);
    assert!(!changed.status.success(), "{}", text(&changed));
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "untracked\n"
    );
    assert!(env.beam(&["review", "--refresh"]).status.success());
    let st = env.state();
    let record_dir = env
        .home
        .join(".beam/transfers")
        .join(st["transfer_id"].as_str().unwrap());
    let plan: serde_json::Value =
        serde_json::from_slice(&std::fs::read(record_dir.join("return-plan.json")).unwrap())
            .unwrap();
    // Simulate interruption after Git checkout, before the auxiliary file merge and phase save.
    common::sh(
        &env.project,
        &format!(
            "git read-tree --reset -u {} && git read-tree {}",
            quote(plan["target"]["wt_tree"].as_str().unwrap()),
            quote(plan["target"]["idx_tree"].as_str().unwrap())
        ),
    );
    std::fs::write(record_dir.join("apply-started"), "started").unwrap();
    let retry = env.beam(&["down", "--keep"]);
    assert!(
        !retry.status.success(),
        "auxiliary conflict should be reported"
    );
    assert_eq!(env.state()["phase"], "retained");
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "remote\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "KEY=local\n"
    );
    assert!(env.beam(&["review", "--resolved"]).status.success());
    assert!(!text(&env.beam(&["status"])).contains("Saved recovery"));
    let undo = env.beam(&["undo"]);
    assert!(undo.status.success(), "{}", text(&undo));
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "untracked\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join(".env")).unwrap(),
        "KEY=local\n"
    );
    assert!(env.beam(&["kill", "--yes"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn watch_notifies_once_for_a_new_report() {
    use std::process::Stdio;
    let env = setup();
    assert!(
        env.beam(&["--yes", "--detach", "--agent", "shell"])
            .status
            .success()
    );
    let output_path = env.home.join("watch-output");
    let mut watch = Command::new(env!("CARGO_BIN_EXE_beam"))
        .args([
            "status",
            "--watch",
            "--notify",
            "--interval",
            "1",
            "--count",
            "3",
        ])
        .current_dir(&env.project)
        .env("HOME", &env.home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdout(std::fs::File::create(&output_path).unwrap())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while std::fs::metadata(&output_path).unwrap().len() == 0 {
        if start.elapsed() > Duration::from_secs(15) {
            let _ = watch.kill();
            panic!("watch did not print initial status");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = env.state();
    let report = format!("{}/report.sh", st["stage"].as_str().unwrap());
    assert!(
        remote_script(
            &env,
            &format!("sh {} waiting 'Need review'", quote(&report))
        )
        .status
        .success()
    );
    let output = watch.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        output.stderr.iter().filter(|b| **b == 7).count(),
        1,
        "{}",
        text(&output)
    );
    assert!(
        std::fs::read_to_string(output_path)
            .unwrap()
            .contains("Agent report: waiting")
    );
    assert!(env.beam(&["down"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn blocked_claude_notice_is_visible_notified_and_clears_after_input() {
    use std::process::Stdio;
    let env = setup();
    std::fs::write(env.project.join(".beam-test-input-prompt"), "test").unwrap();
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    assert!(
        text(&up).contains("possible-input") || text(&up).contains("Task state is unknown"),
        "{}",
        text(&up)
    );
    wait_for_file(&container(&env), "/tmp/fake-claude-notice");
    let state = env.state();
    let stage = state["stage"].as_str().unwrap();
    let tmux = state["tmux"].as_str().unwrap();
    let status = env.beam(&["status", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["phase"], "possible-input");
    assert_eq!(value["input_request"]["source"], "terminal-heuristic");
    assert_eq!(value["input_request"]["kind"], "confirmation");
    assert_eq!(value["next_action"], "beam attach");
    // Repeating upload must not mistake an input request for failed setup or restart it.
    let again = env.beam(&["--yes", "--detach"]);
    assert!(again.status.success(), "{}", text(&again));
    assert_eq!(env.state()["sandbox"], state["sandbox"]);
    let watch = Command::new(env!("CARGO_BIN_EXE_beam"))
        .args([
            "status",
            "--watch",
            "--notify",
            "--count",
            "2",
            "--interval",
            "1",
        ])
        .current_dir(&env.project)
        .env("HOME", &env.home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(watch.status.success(), "{}", text(&watch));
    assert_eq!(watch.stderr.iter().filter(|b| **b == 7).count(), 1);
    assert!(text(&watch).contains("Run beam attach"));
    // Status and watch do not answer the notice or expose the pane's body.
    assert!(!text(&watch).contains("Gateway notice"));
    assert!(
        !remote_script(&env, "test -f /tmp/fake-claude-ready")
            .status
            .success()
    );
    assert!(!env.beam(&["restart"]).status.success());
    assert!(
        remote_script(&env, &format!("tmux send-keys -t {} Enter", quote(tmux)))
            .status
            .success()
    );
    wait_for_file(&container(&env), "/tmp/fake-claude-ready");
    let resumed = env.beam(&["status", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(value["remote"], "running");
    assert!(value["input_request"].is_null());
    let events = remote_script(
        &env,
        &format!("cat {}", quote(&format!("{stage}/events.tsv"))),
    );
    assert_eq!(text(&events).matches("agent-started").count(), 1);
    assert!(env.beam(&["down"]).status.success());
}

#[test]
#[ignore = "needs Docker"]
fn missing_cargo_is_repaired_by_the_agent_after_upload() {
    let env = setup();
    let config = std::fs::read_to_string(env.project.join("beam.toml")).unwrap();
    std::fs::write(
        env.project.join("beam.toml"),
        config.replace(
            "setup = [\"echo ok > setup-ran.txt\"]",
            "verify = ['test -f cargo-fetched']",
        ),
    )
    .unwrap();
    std::fs::write(env.project.join("Cargo.lock"), "").unwrap();
    std::fs::write(env.project.join(".beam-test-repair-cargo"), "").unwrap();
    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    let c = container(&env);
    wait_for_file(&c, "/tmp/fake-claude-ready");
    let handoff = wait_for_file(&c, "/tmp/fake-claude-handoff");
    assert!(handoff.contains("missing tools: cargo"), "{handoff}");
    assert!(handoff.contains("cargo fetch"), "{handoff}");
    assert!(
        remote_script(&env, "test -f cargo-fetched")
            .status
            .success()
    );
    let status = env.beam(&["status", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["process"], "running");
    assert!(
        value["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "verification-passed")
    );
    assert!(env.beam(&["down"]).status.success());
    assert!(env.project.join("cargo-fetched").is_file());
}
