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

/// Make the home and project directories. Use sudo only when a plain mkdir is not possible.
pub fn prepare(home: &str, project: &str, stage: &str) -> String {
    with_vars(
        &[("H", home), ("P", project), ("S", stage)],
        r#"set -eu
missing=""
for t in git tmux tar gzip; do command -v "$t" >/dev/null 2>&1 || missing="$missing $t"; done
if [ -n "$missing" ]; then echo "beam: the sandbox does not have:$missing" >&2; exit 4; fi
if [ -d "$P" ] && [ -n "$(ls -A "$P" 2>/dev/null)" ]; then
  echo "beam: $P already exists in the sandbox and is not empty" >&2
  exit 3
fi
mk() {
  if mkdir -p "$1" 2>/dev/null; then return 0; fi
  sudo -n mkdir -p "$1" && sudo -n chown "$(id -u):$(id -g)" "$1"
}
mk "$H"
[ -w "$H" ] || sudo -n chown "$(id -u):$(id -g)" "$H"
mk "$P"
[ -w "$P" ] || sudo -n chown "$(id -u):$(id -g)" "$P"
mkdir -p "$S"
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
    with_vars(
        &[("S", stage), ("T", name)],
        "set -eu\ntmux new-session -d -s \"$T\" -x 200 -y 50 \"sh '$S/run.sh'\"\n",
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

/// Prints "running", or "stopped <exit code>".
pub fn agent_status(stage: &str, name: &str) -> String {
    with_vars(
        &[("S", stage), ("T", name)],
        r#"if tmux has-session -t "$T" 2>/dev/null; then echo running; else echo "stopped $(cat "$S/agent.exit" 2>/dev/null || echo '?')"; fi"#,
    )
}

pub fn pack_back(
    stage: &str,
    project: &str,
    home: &str,
    refname: &str,
    sent: &str,
    agent_paths: &[String],
) -> String {
    let paths = agent_paths.join("\n");
    with_vars(
        &[
            ("S", stage),
            ("P", project),
            ("H", home),
            ("REF", refname),
            ("SENT", sent),
            ("AGENT_PATHS", &paths),
        ],
        PACK_BACK_SH,
    )
}

pub fn cat(path: &str) -> String {
    format!("cat {}", sh_quote(path))
}

/// Remove what beam put on an SSH host. `paths` are absolute.
pub fn cleanup(tmux: &str, paths: &[String]) -> String {
    let mut s = format!("tmux kill-session -t {} 2>/dev/null\n", sh_quote(tmux));
    for p in paths {
        s.push_str(&format!("rm -rf {}\n", sh_quote(p)));
    }
    s.push_str("exit 0\n");
    s
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
