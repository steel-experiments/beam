// ABOUTME: Opt-in shell round trip against a real Daytona sandbox.
// ABOUTME: Uses the optional Daytona CLI to simulate remote work, with cleanup on failure.
mod common;

use common::{Env, sh, text};
use std::process::Command;

struct Cleanup<'a>(&'a Env);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if self.0.project.join(".beam/state.json").exists() {
            let out = self.0.beam(&["kill", "--yes"]);
            if !out.status.success() {
                eprintln!("Daytona test cleanup needs attention: {}", text(&out));
            }
        }
    }
}

#[test]
#[ignore = "allocates a Daytona sandbox; requires DAYTONA_API_KEY and authenticated Daytona CLI"]
fn shell_round_trip_on_daytona() {
    assert!(
        std::env::var_os("DAYTONA_API_KEY").is_some(),
        "set DAYTONA_API_KEY"
    );
    let target = std::env::var("BEAM_E2E_DAYTONA_TARGET").unwrap_or_else(|_| "daytona".into());
    let env = Env::new(&format!(
        "[beam]\nto = {target:?}\n[sandbox]\nsetup = [\"echo ready > setup-ran.txt\"]\n"
    ));
    let _cleanup = Cleanup(&env);
    let up = env.beam(&["--agent", "shell", "--yes", "--detach"]);
    assert!(up.status.success(), "{}", text(&up));
    let st = env.state();
    assert_eq!(st["sandbox"]["kind"], "daytona");
    let id = st["sandbox"]["id"].as_str().unwrap();
    let script = format!(
        "set -eu; cd {}; test -f setup-ran.txt; printf '\\000\\377binary\\n' > binary.dat; printf 'remote edit\\n' >> README.md; git add binary.dat; printf done",
        quote(&env.project.to_string_lossy())
    );
    let edited = Command::new("daytona")
        .args(["exec", id, "--", "sh", "-c", &script])
        .output()
        .unwrap();
    assert!(edited.status.success(), "{}", text(&edited));
    assert!(text(&edited).contains("done"), "{}", text(&edited));
    let down = env.beam(&["down"]);
    assert!(down.status.success(), "{}", text(&down));
    assert_eq!(
        std::fs::read(env.project.join("binary.dat")).unwrap(),
        b"\0\xffbinary\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.project.join("README.md")).unwrap(),
        "hello\nremote edit\n"
    );
    assert!(sh(&env.project, "git diff --cached --name-only").contains("binary.dat"));
    assert_eq!(
        std::fs::read_to_string(env.project.join("notes.txt")).unwrap(),
        "untracked\n"
    );
    let info = Command::new("daytona")
        .args(["info", id, "--format", "json"])
        .output()
        .unwrap();
    assert!(
        !info.status.success(),
        "sandbox was not removed: {}",
        text(&info)
    );
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
