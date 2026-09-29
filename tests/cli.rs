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

fn receipt(env: &Env, id: &str, phase: &str, created: u64, conflicts: bool) -> std::path::PathBuf {
    let dir = env.home.join(".beam/transfers").join(id);
    std::fs::create_dir_all(dir.join("worktree")).unwrap();
    let record = serde_json::json!({
        "version": 2, "transfer_id": id, "session_id": "test", "agent": "shell",
        "project_root": env.project, "agent_cwd": env.project, "home": env.home,
        "target": "docker", "sandbox": null, "stage": "", "tmux": "test",
        "sent": {"head":"", "branch":"", "idx_tree":"", "wt_tree":"", "wt_commit":""},
        "sent_files": {}, "phase": phase, "created_at": created,
        "recovery": dir.join("worktree"),
        "conflicts": if conflicts {vec!["remote work saved separately"]} else {vec![]}
    });
    std::fs::write(dir.join("state.json"), serde_json::to_vec(&record).unwrap()).unwrap();
    dir
}

#[test]
fn status_finds_latest_saved_recovery_without_hiding_active_transfer() {
    let env = Env::new("");
    receipt(&env, "old", "closed", 1, true);
    let latest = receipt(&env, "latest", "closed", 2, true);
    receipt(&env, "clean", "closed", 3, false);
    let json = env.beam(&["status", "--json"]);
    assert!(json.status.success(), "{}", text(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["phase"], "local");
    assert_eq!(
        value["saved_recovery"]["receipt"],
        latest.to_string_lossy().as_ref()
    );
    assert!(value["next_action"].as_str().unwrap().starts_with("cd "));
    assert!(!env.project.join(".beam/state.json").exists());

    receipt(&env, "active", "applied", 4, false);
    let out = env.beam(&["status"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Sandbox cleanup remains"));
    assert!(text(&out).contains(&latest.display().to_string()));
    assert_eq!(text(&out).matches("Next:").count(), 1);
    assert!(text(&out).contains("Next: beam down"));
}

#[test]
fn recovery_receipts_are_scoped_to_the_project_and_remain_read_only() {
    let env = Env::new("");
    let dir = receipt(&env, "other-project", "closed", 1, true);
    let path = dir.join("state.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["project_root"] = "/another/project".into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let out = env.beam(&["status", "--json"]);
    assert!(out.status.success(), "{}", text(&out));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(value["saved_recovery"].is_null());
    assert_eq!(value["next_action"], "beam");
    assert_eq!(std::fs::read(path).unwrap(), before);
}
