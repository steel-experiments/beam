// ABOUTME: Shell scripts that beam runs in the sandbox, with their variables filled in.
// ABOUTME: Each function returns a full script as a string. The provider runs it with "sh -c".

use crate::util::sh_quote;

pub const RESTORE_SH: &str = include_str!("../scripts/restore.sh");
pub const RUN_SH: &str = include_str!("../scripts/run.sh");
pub const PACK_BACK_SH: &str = include_str!("../scripts/pack_back.sh");

/// Put `NAME='value'` lines before a script body.
pub fn with_vars(vars: &[(&str, &str)], body: &str) -> String {
    let mut s = String::new();
    for (k, v) in vars {
        s.push_str(&format!("{k}={}\n", sh_quote(v)));
    }
    s.push_str(body);
    s
}

/// Claim only newly created paths. Existing paths must have this transfer's owner marker.
pub fn prepare(home: &str, project: &str, stage: &str, owner: &str) -> String {
    with_vars(
        &[("H", home), ("P", project), ("S", stage), ("OWNER", owner)],
        r#"set -eu
umask 077
claim() {
  path=$1 marker=$2
  if [ -e "$path" ]; then
    actual=$(cat "$path/$marker" 2>/dev/null || true)
    if [ "$marker" = .beam-owner ] && [ -f "$path/.git/beam-owner" ]; then actual=$(cat "$path/.git/beam-owner"); fi
    [ ! -L "$path" ] && [ "$actual" = "$OWNER" ] || {
      echo "beam: $path already exists and is not owned by this transfer" >&2; exit 3;
    }
  else
    parent=$(dirname "$path")
    mkdir -p "$parent" 2>/dev/null || sudo -n mkdir -p "$parent"
    if ! mkdir "$path" 2>/dev/null; then
      sudo -n mkdir "$path" && sudo -n chown "$(id -u):$(id -g)" "$path"
    fi
    printf '%s' "$OWNER" > "$path/$marker"
  fi
}
claim "$S" owner
claim "$P" .beam-owner
mkdir -p "$H"
"#,
    )
}

pub fn unpack(stage: &str) -> String {
    with_vars(
        &[("S", stage)],
        "set -eu\nmkdir -p \"$S\"\ntar xzf - -C \"$S\" --no-same-owner\n",
    )
}

pub fn write_env(stage: &str) -> String {
    with_vars(&[("S", stage)], "set -eu\numask 077\ncat > \"$S/env\"\n")
}

pub struct RestoreVars<'a> {
    pub stage: &'a str,
    pub project: &'a str,
    pub home: &'a str,
    pub refname: &'a str,
    pub branch: &'a str,
    pub head: &'a str,
    pub idx_tree: &'a str,
    pub wt_tree: &'a str,
    pub origin: &'a str,
    pub git_name: &'a str,
    pub git_email: &'a str,
}

pub fn restore(v: &RestoreVars) -> String {
    with_vars(
        &[
            ("S", v.stage),
            ("P", v.project),
            ("H", v.home),
            ("REF", v.refname),
            ("BRANCH", v.branch),
            ("HEAD_SHA", v.head),
            ("IDX_TREE", v.idx_tree),
            ("WT_TREE", v.wt_tree),
            ("ORIGIN", v.origin),
            ("GIT_NAME", v.git_name),
            ("GIT_EMAIL", v.git_email),
        ],
        RESTORE_SH,
    )
}

pub struct RunVars<'a> {
    pub stage: &'a str,
    pub cwd: &'a str,
    pub home: &'a str,
    pub handoff_head: &'a str,
    pub setup: &'a [String],
    pub resume_fn: &'a str,
}

/// The launcher that tmux runs. It keeps the PATH entries of the sandbox user, then uses $H as HOME.
pub fn run_script(v: &RunVars) -> String {
    let setup = v.setup.join("\n");
    let mut s = with_vars(
        &[
            ("S", v.stage),
            ("P", v.cwd),
            ("H", v.home),
            ("HANDOFF_HEAD", v.handoff_head),
            ("HANDOFF_TAIL", crate::handoff::TAIL),
            ("SETUP", &setup),
        ],
        "",
    );
    s.push_str("export PATH=\"$PATH:$HOME/.local/bin:$HOME/.npm-global/bin:/usr/local/bin\"\n");
    s.push_str(v.resume_fn);
    s.push('\n');
    s.push_str(RUN_SH);
    s
}

pub fn start_tmux(stage: &str, name: &str) -> String {
    let command = format!(
        "while [ ! -f {} ]; do sleep 0.1; done; sh {}",
        sh_quote(&format!("{stage}/go")),
        sh_quote(&format!("{stage}/run.sh"))
    );
    let pipe = format!("cat >> {}", sh_quote(&format!("{stage}/terminal.log")));
    with_vars(
        &[
            ("S", stage),
            ("T", name),
            ("CMD", &command),
            ("PIPE", &pipe),
        ],
        r#"set -eu
if [ -f "$S/started" ]; then exit 0; fi
tmux has-session -t "$T" 2>/dev/null || tmux new-session -d -s "$T" -x 200 -y 50 "$CMD"
tmux pipe-pane -o -t "$T" "$PIPE"
touch "$S/go"
"#,
    )
}

pub fn retry_setup(stage: &str, name: &str) -> String {
    with_vars(
        &[("S", stage), ("T", name)],
        r#"set -eu
[ "$(cat "$S/phase" 2>/dev/null)" = needs-attention ] || exit 0
tmux kill-session -t "$T" 2>/dev/null || true
rm -f "$S/started" "$S/agent.exit" "$S/go"
rmdir "$S/run-lock" 2>/dev/null || true
printf preparing > "$S/phase"
"#,
    )
}

/// Stop the agent: two Ctrl-C (Claude Code exits on the second one), then kill after 15 s.
pub fn stop_agent(name: &str) -> String {
    with_vars(
        &[("T", name)],
        r#"tmux has-session -t "$T" 2>/dev/null || exit 0
tmux send-keys -t "$T" C-c
sleep 1
tmux send-keys -t "$T" C-c
i=0
while tmux has-session -t "$T" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -gt 30 ]; then tmux kill-session -t "$T"; break; fi
  sleep 0.5
done
exit 0
"#,
    )
}

/// Report setup and process state without claiming authentication succeeded.
pub fn agent_status(stage: &str, name: &str) -> String {
    with_vars(
        &[("S", stage), ("T", name)],
        r#"phase=$(cat "$S/phase" 2>/dev/null || echo preparing)
if [ "$phase" = needs-attention ]; then echo needs-attention
elif tmux has-session -t "$T" 2>/dev/null; then echo "$phase"
else echo "stopped $(cat "$S/agent.exit" 2>/dev/null || echo '?')"; fi"#,
    )
}

pub fn pack_back(
    stage: &str,
    project: &str,
    home: &str,
    refname: &str,
    sent: &str,
    agent_paths: &[String],
    extras: &[String],
) -> String {
    let paths = agent_paths.join("\n");
    let extras = extras.join("\n");
    with_vars(
        &[
            ("S", stage),
            ("P", project),
            ("H", home),
            ("REF", refname),
            ("SENT", sent),
            ("AGENT_PATHS", &paths),
            ("EXTRAS", &extras),
        ],
        PACK_BACK_SH,
    )
}

pub fn cat(path: &str) -> String {
    format!("cat {}", sh_quote(path))
}

/// SSH cleanup checks ownership before each deletion and never deletes shared home files.
pub fn cleanup(stage: &str, project: &str, owner: &str, tmux: &str, project_owned: bool) -> String {
    with_vars(
        &[
            ("S", stage),
            ("P", project),
            ("OWNER", owner),
            ("T", tmux),
            ("EXPECTED", if project_owned { "yes" } else { "no" }),
        ],
        r#"set -eu
if [ -e "$S" ]; then
  [ ! -L "$S" ] && [ "$(cat "$S/owner" 2>/dev/null || true)" = "$OWNER" ] || {
    echo 'beam: stage ownership cannot be verified; nothing was deleted' >&2; exit 3;
  }
fi
remove_project=no
if [ -e "$P" ]; then
  actual=$(cat "$P/.git/beam-owner" 2>/dev/null || cat "$P/.beam-owner" 2>/dev/null || true)
  if [ ! -L "$P" ] && [ "$actual" = "$OWNER" ]; then
    remove_project=yes
  else
    echo 'beam: existing project is not owned by this transfer; it remains untouched' >&2
    if [ "$EXPECTED" = yes ]; then exit 3; fi
  fi
fi
# Do not touch a tmux session unless this transfer owns its stage.
if [ -f "$S/owner" ]; then
  tmux kill-session -t "$T" 2>/dev/null || true
  tmux kill-session -t "$T-repair" 2>/dev/null || true
fi
if [ "$remove_project" = yes ]; then rm -rf -- "$P"; fi
if [ -f "$S/owner" ]; then rm -rf -- "$S"; fi
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vars_are_quoted() {
        let s = with_vars(&[("A", "x y"), ("B", "it's")], "echo \"$A|$B\"");
        let out = crate::util::run(std::process::Command::new("sh").arg("-c").arg(&s)).unwrap();
        assert_eq!(out, "x y|it's");
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::process::Command;
    fn shell(script: &str) -> std::process::Output {
        Command::new("sh").args(["-c", script]).output().unwrap()
    }
    #[test]
    fn existing_project_survives_prepare_and_cleanup() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let home = d.path().join("home");
        let stage = d.path().join("stage");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("keep"), "user work").unwrap();
        assert!(
            !shell(&prepare(
                home.to_str().unwrap(),
                project.to_str().unwrap(),
                stage.to_str().unwrap(),
                "ours"
            ))
            .status
            .success()
        );
        shell(&cleanup(
            stage.to_str().unwrap(),
            project.to_str().unwrap(),
            "ours",
            "absent-beam-test",
            false,
        ));
        assert_eq!(
            std::fs::read_to_string(project.join("keep")).unwrap(),
            "user work"
        );
    }
    #[test]
    fn owned_paths_can_be_prepared_twice_and_removed_twice() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project with spaces");
        let stage = d.path().join("stage");
        let home = stage.join("home");
        let prepare = prepare(
            home.to_str().unwrap(),
            project.to_str().unwrap(),
            stage.to_str().unwrap(),
            "ours",
        );
        assert!(shell(&prepare).status.success());
        assert!(shell(&prepare).status.success());
        let cleanup = cleanup(
            stage.to_str().unwrap(),
            project.to_str().unwrap(),
            "ours",
            "absent-beam-test",
            true,
        );
        assert!(shell(&cleanup).status.success());
        assert!(shell(&cleanup).status.success());
        assert!(!project.exists());
        assert!(!stage.exists());
    }
    #[test]
    fn ownership_survives_git_clean_and_repeated_preparation() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let stage = d.path().join("stage");
        let home = stage.join("home");
        let prepare = prepare(
            home.to_str().unwrap(),
            project.to_str().unwrap(),
            stage.to_str().unwrap(),
            "ours",
        );
        assert!(shell(&prepare).status.success());
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("-C")
                .arg(&project)
                .args(["init", "-q"])
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::rename(project.join(".beam-owner"), project.join(".git/beam-owner")).unwrap();
        std::fs::write(project.join("temporary"), "discard").unwrap();
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("-C")
                .arg(&project)
                .args(["clean", "-fdx"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(shell(&prepare).status.success());
        assert!(
            shell(&cleanup(
                stage.to_str().unwrap(),
                project.to_str().unwrap(),
                "ours",
                "absent-beam-test",
                true
            ))
            .status
            .success()
        );
        assert!(!project.exists());
    }

    #[test]
    fn missing_project_ownership_keeps_the_transfer_recoverable() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let stage = d.path().join("stage");
        assert!(
            shell(&prepare(
                stage.join("home").to_str().unwrap(),
                project.to_str().unwrap(),
                stage.to_str().unwrap(),
                "ours"
            ))
            .status
            .success()
        );
        std::fs::remove_file(project.join(".beam-owner")).unwrap();
        assert!(
            !shell(&cleanup(
                stage.to_str().unwrap(),
                project.to_str().unwrap(),
                "ours",
                "absent-beam-test",
                true
            ))
            .status
            .success()
        );
        assert!(stage.exists());
        assert!(project.exists());
    }

    #[test]
    fn wrong_owner_prevents_cleanup() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        let stage = d.path().join("stage");
        assert!(
            shell(&prepare(
                stage.join("home").to_str().unwrap(),
                project.to_str().unwrap(),
                stage.to_str().unwrap(),
                "theirs"
            ))
            .status
            .success()
        );
        assert!(
            !shell(&cleanup(
                stage.to_str().unwrap(),
                project.to_str().unwrap(),
                "ours",
                "absent-beam-test",
                true
            ))
            .status
            .success()
        );
        assert!(project.exists());
        assert!(stage.exists());
    }
}
