// ABOUTME: End-to-end test on a real Steel computer: beam up, agent work in the computer, beam down.
// ABOUTME: Needs STEEL_API_KEY and the steel preview CLI. Run: cargo test --test e2e_steel -- --ignored

mod common;

use common::{Env, SESSION, agent_work, encode, text};
use std::process::Command;
use std::time::{Duration, Instant};

fn steel() -> Command {
    let home = std::env::var("HOME").unwrap();
    let mut c = Command::new(format!("{home}/.steel/bin/steel"));
    c.stdin(std::process::Stdio::null());
    c
}

/// Run a shell line in the computer. Returns (exit code, output).
fn exec(id: &str, script: &str) -> (i64, String) {
    let out = steel()
        .args(["computer", "exec", id, "--json", "-c", script])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("steel exec: {}", String::from_utf8_lossy(&out.stderr)));
    (
        v["data"]["exitCode"].as_i64().unwrap_or(-1),
        v["data"]["output"].as_str().unwrap_or("").to_string(),
    )
}

#[test]
#[ignore = "needs a Steel account; run with --ignored"]
fn beam_up_and_down_on_steel() {
    assert!(
        std::env::var_os("STEEL_API_KEY").is_some(),
        "set STEEL_API_KEY"
    );
    let env = Env::new(
        "[beam]\nto = \"steel\"\n[env]\nforward = [\"BEAM_E2E_VAR\"]\n\
         [sandbox]\ntimeout = \"20m\"\nsetup = [\"echo ok > setup-ran.txt\"]\n",
    );

    let up = env.beam(&["--yes", "--detach"]);
    assert!(up.status.success(), "beam up failed:\n{}", text(&up));
    let st = env.state();
    let id = st["sandbox"]["id"].as_str().unwrap().to_string();
    let stage = st["stage"].as_str().unwrap().to_string();

    // run.sh writes the handoff after setup, just before it starts Claude Code.
    let start = Instant::now();
    let handoff = loop {
        let (code, out) = exec(&id, &format!("cat '{stage}/handoff.txt'"));
        if code == 0 {
            break out;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "no handoff in the computer"
        );
        std::thread::sleep(Duration::from_secs(2));
    };
    assert!(
        handoff.starts_with("[beam] You were moved from"),
        "{handoff}"
    );
    assert!(
        handoff.contains("Setup command succeeded: echo ok > setup-ran.txt"),
        "{handoff}"
    );
    let (_, env_file) = exec(&id, &format!("cat '{stage}/env'"));
    assert_eq!(env_file.trim(), "BEAM_E2E_VAR=forwarded-value");

    let p = env.project.to_string_lossy().to_string();
    let (_, ls) = exec(&id, &format!("ls -a '{p}'"));
    assert!(ls.contains(".env") && !ls.contains("node_modules"), "{ls}");
    let status = env.beam(&["status", "--json"]);
    assert!(status.status.success(), "{}", text(&status));
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["process"], "running");

    // The agent works. The real Claude Code has no login here, so the test does the work.
    let transcript = format!(
        "{}/.claude/projects/{}/{SESSION}.jsonl",
        env.home.display(),
        encode(&env.project)
    );
    let (code, out) = exec(&id, &format!("cd '{p}' && {}", agent_work(&transcript)));
    assert_eq!(code, 0, "{out}");

    let down = env.beam(&["down"]);
    assert!(down.status.success(), "beam down failed:\n{}", text(&down));
    env.assert_home_again();

    let got = steel()
        .args(["computer", "get", &id, "--json"])
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&got.stdout);
    assert!(
        !status.contains("\"status\":\"running\""),
        "the computer must be deleted: {status}"
    );
}
