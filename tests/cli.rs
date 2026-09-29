// ABOUTME: Local command regressions. These run without a provider or credentials.
mod common;
use common::{Env, text};

#[test]
fn staged_file_and_extra_size_limits_run_before_provider_access() {
    let env = Env::new("[beam]\nto = 'ssh://not-used'\n[files]\nmax_file_size = '1KB'\n");
    std::fs::write(env.project.join("large"), vec![b'x'; 2048]).unwrap();
    common::sh(&env.project, "git add large");
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("these files are larger"),
        "{}",
        text(&out)
    );
    common::sh(&env.project, "git reset -q -- large && rm large");
    std::fs::write(env.project.join(".env"), vec![b'x'; 2048]).unwrap();
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("these files are larger"),
        "{}",
        text(&out)
    );
    assert!(!env.project.join(".beam/state.json").exists());
}

#[test]
fn missing_target_explains_noninteractive_setup() {
    let env = Env::new("");
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("Supply the option explicitly"),
        "{}",
        text(&out)
    );
}

#[test]
fn unsafe_extra_path_is_rejected_before_provider_access() {
    let env = Env::new("[beam]\nto = 'docker'\n[files]\nextras = ['../secret']\n");
    let out = env.beam(&["--yes", "--detach"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("relative path"), "{}", text(&out));
}

#[test]
fn explicit_missing_session_never_falls_back_to_a_shell() {
    let env = Env::new("[beam]\nto = 'docker'\n");
    let out = env.beam(&["--yes", "--detach", "--session", "missing"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("session missing not found"),
        "{}",
        text(&out)
    );
}
