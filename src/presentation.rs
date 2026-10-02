// ABOUTME: Shared user-facing transfer state and next action.
use crate::{
    agent::evidence::{Process, Task},
    monitor::{Event, Snapshot},
    state::{Phase, State},
    ui, util,
};

pub const RETAINED_NOTICE: &str = "Further sandbox edits will not return through beam down.";

pub struct Summary {
    pub phase: &'static str,
    pub message: &'static str,
    pub next: &'static str,
}

pub fn summary(phase: Phase, remote: Option<&Snapshot>, conflicts: bool) -> Summary {
    let sandbox_return = remote.and_then(|s| crate::monitor::sandbox_return(&s.events));
    let (label, message, next) = match phase {
        Phase::Starting | Phase::Remote if sandbox_return.is_some() => match sandbox_return {
            Some("return-ready") => (
                "packed for return",
                "`beam down` ran in the sandbox and packed the work. Later sandbox edits do not return.",
                "beam down",
            ),
            Some("return-failed") => (
                "sandbox pack failed",
                "`beam down` in the sandbox could not pack the work. Local beam down tries again.",
                "beam down",
            ),
            _ => (
                "packing for return",
                "`beam down` ran in the sandbox. The agent stops and its work is packed.",
                "beam down --wait",
            ),
        },
        Phase::Returning | Phase::Downloaded => (
            "returning",
            "Return is incomplete. Local apply or cleanup may remain.",
            "beam down",
        ),
        Phase::Applied if conflicts => (
            "returned; cleanup pending",
            "Local changes were preserved. Remote work is saved separately. Sandbox cleanup remains.",
            "beam down",
        ),
        Phase::Applied => (
            "returned; cleanup pending",
            "Return data is saved locally. Sandbox cleanup remains.",
            "beam down",
        ),
        Phase::Retained if conflicts => (
            "returned; sandbox retained",
            "Local changes were preserved. Saved recovery and a sandbox are available for inspection.",
            "beam attach",
        ),
        Phase::Retained => (
            "returned; sandbox retained",
            "Return finished. Sandbox kept for inspection.",
            "beam attach",
        ),
        Phase::Closed if conflicts => (
            "local; saved recovery",
            "Local changes were preserved. Saved recovery is available for review.",
            "",
        ),
        Phase::Closed => ("local", "Workspace is local.", "beam"),
        Phase::Starting | Phase::Remote => match remote.map(|s| &s.process) {
            Some(Process::Running) => match remote.map(|s| &s.task.state) {
                Some(Task::PossibleInput) => (
                    "possible-input",
                    "A terminal heuristic found a possible input prompt.",
                    "beam attach",
                ),
                Some(Task::InputNeeded) => (
                    "waiting-for-input",
                    "The agent reports that input is needed.",
                    "beam attach",
                ),
                Some(Task::ActivityObserved) => (
                    "remote",
                    "Agent activity was reported. Task completion is unverified.",
                    "beam status --watch --notify",
                ),
                Some(Task::CompletionReported) => (
                    "completion-reported",
                    "The agent reported completion. Review the returned work to verify it.",
                    "beam down --review",
                ),
                Some(Task::FailureReported) => (
                    "failure-reported",
                    "The agent reported a failure.",
                    "beam attach",
                ),
                _ => (
                    "remote",
                    "Remote process is running. Task state is unknown.",
                    "beam status --watch --notify",
                ),
            },
            Some(Process::Repairing) => (
                "environment repair",
                "The agent was given environment repair instructions. Environment checks have not passed; repair may require input.",
                "beam attach",
            ),
            Some(Process::NeedsAttention | Process::Stopped)
                if remote.is_some_and(|s| crate::monitor::repair_unfinished(&s.events)) =>
            {
                (
                    "environment repair unfinished",
                    "The agent stopped before environment checks passed. Open the remote terminal to inspect or continue repair.",
                    "beam attach",
                )
            }
            Some(Process::NeedsAttention) => (
                "needs attention",
                "Remote setup or project checks failed. Local files are unchanged by this transfer.",
                "beam logs",
            ),
            Some(Process::Stopped) => (
                "stopped",
                "Remote process stopped. Open the remote terminal to inspect it.",
                "beam attach",
            ),
            Some(Process::Unknown) => (
                "unavailable",
                "Remote state could not be checked. The transfer record is saved.",
                "beam status",
            ),
            Some(Process::Paused) => ("paused", "The sandbox is paused.", "beam attach"),
            Some(Process::Preparing) => ("preparing", "Remote setup is running.", "beam logs"),
            None if phase == Phase::Starting => {
                ("preparing", "Remote startup is incomplete.", "beam")
            }
            None => (
                "remote",
                "Workspace is remote. Task state is unknown.",
                "beam attach",
            ),
        },
        _ => (
            "preparing",
            "Upload is incomplete. Local files are unchanged by this transfer.",
            "beam",
        ),
    };
    Summary {
        phase: label,
        message,
        next,
    }
}

pub fn for_state(st: &State, remote: Option<&Snapshot>) -> Summary {
    if st.phase == Phase::Downloaded && st.dir().join("return-plan.json").exists() {
        Summary {
            phase: "ready for review",
            message: "Remote work is saved locally. Review the return before applying it.",
            next: "beam review",
        }
    } else {
        summary(st.phase, remote, st.has_unresolved_recovery())
    }
}
pub fn show(st: &State, remote: Option<&Snapshot>) {
    let view = for_state(st, remote);
    println!("{}  {}", ui::strong(view.phase), st.describe());
    println!("{}", view.message);
    if let Some(snapshot) = remote {
        if snapshot.process != Process::Running
            && matches!(st.phase, Phase::Starting | Phase::Remote | Phase::Retained)
        {
            println!("{}", ui::field("Remote state", &snapshot.remote));
        }
        if (matches!(
            snapshot.process,
            Process::Repairing | Process::NeedsAttention
        ) || (snapshot.process == Process::Stopped
            && crate::monitor::repair_unfinished(&snapshot.events)))
            && let Some(detail) = failed_check(&snapshot.events)
        {
            println!("{}", ui::field("Failed check", detail));
        }
        if snapshot.task.state != Task::Unknown {
            println!(
                "{}",
                ui::field(
                    "Evidence",
                    format!(
                        "{} — {}",
                        snapshot.task.evidence.source.label(),
                        snapshot.task.evidence.detail
                    )
                )
            );
        }
    }
    if st.phase == Phase::Retained {
        ui::warn(RETAINED_NOTICE);
    } else if remote.is_some_and(|s| {
        matches!(s.process, Process::Running | Process::Repairing)
            && s.capabilities.session_transfer
    }) {
        ui::warn("Avoid running the same agent locally.");
    }
    if !view.next.is_empty() {
        ui::next(view.next);
    }
}

/// The failure from the latest run of the remote checks, when that run failed.
fn failed_check(events: &[Event]) -> Option<&str> {
    events
        .iter()
        .rev()
        .take_while(|e| e.kind != "setup-started")
        .find(|e| e.kind == "check-failed")
        .map(|e| e.detail.as_str())
}

pub fn recovery(st: &State) {
    println!("{}", ui::field("Saved recovery", ui::link(&st.dir())));
    for conflict in &st.conflicts {
        ui::warn(conflict);
    }
    if let Some(path) = &st.recovery {
        println!("{}", ui::field("Remote worktree", ui::link(path)));
    }
}

pub fn recovery_action(st: &State) -> String {
    let path = st
        .recovery
        .as_ref()
        .filter(|p| p.is_dir())
        .cloned()
        .unwrap_or_else(|| st.dir());
    format!("cd {}", util::sh_quote(&path.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::failed_check;
    use crate::monitor::Event;

    fn event(kind: &str, detail: &str) -> Event {
        Event {
            at: 0,
            kind: kind.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn an_exited_repair_agent_is_not_presented_as_success() {
        let adapter = crate::agent::get("shell").unwrap();
        let events = vec![
            event("setup-started", ""),
            event("check-failed", "missing pnpm"),
            event("environment-repair-started", ""),
            event("agent-exited", "exit=0"),
        ];
        for raw in ["stopped 0", "needs-attention"] {
            let mut snapshot = crate::monitor::observe(
                adapter,
                raw.into(),
                events.clone(),
                |_| panic!("stopped agent must not be probed"),
                "",
                "",
            );
            let summary = super::summary(crate::state::Phase::Remote, Some(&snapshot), false);
            assert_eq!(summary.phase, "environment repair unfinished");
            assert_eq!(summary.next, "beam attach");
            assert!(
                crate::monitor::repair_note(&snapshot.events)
                    .unwrap()
                    .contains("did not pass")
            );
            snapshot.events.push(event("setup-started", ""));
            snapshot.events.push(event("environment-checks-passed", ""));
            assert!(!crate::monitor::repair_unfinished(&snapshot.events));
            assert!(
                !super::summary(crate::state::Phase::Remote, Some(&snapshot), false)
                    .message
                    .contains("before environment checks passed")
            );
            assert!(
                crate::monitor::repair_note(&snapshot.events)
                    .unwrap()
                    .contains("checks passed")
            );
            snapshot.events.push(event("setup-started", ""));
            assert!(crate::monitor::repair_unfinished(&snapshot.events));
        }
    }

    #[test]
    fn a_sandbox_return_outranks_the_stopped_agent() {
        let adapter = crate::agent::get("shell").unwrap();
        let mut events = vec![event("agent-started", ""), event("return-requested", "")];
        let summary = |events: &[crate::monitor::Event]| {
            let snapshot = crate::monitor::observe(
                adapter,
                "stopped 0".into(),
                events.to_vec(),
                |_| panic!("stopped agent must not be probed"),
                "",
                "",
            );
            let s = super::summary(crate::state::Phase::Remote, Some(&snapshot), false);
            (s.phase, s.next)
        };
        assert_eq!(summary(&events), ("packing for return", "beam down --wait"));
        events.push(event("return-failed", "cannot pack the work"));
        assert_eq!(summary(&events), ("sandbox pack failed", "beam down"));
        events.push(event("return-requested", ""));
        events.push(event("return-ready", ""));
        assert_eq!(summary(&events), ("packed for return", "beam down"));
        assert_eq!(
            summary(&[event("agent-exited", "exit=0")]).0,
            "stopped",
            "a stop without a sandbox return keeps its meaning"
        );
    }

    #[test]
    fn failed_check_comes_from_the_latest_check_run() {
        let mut events = vec![
            event("setup-started", ""),
            event("check-failed", "prerequisites: missing tools: pnpm"),
            event("repairing", "environment checks failed"),
        ];
        assert_eq!(
            failed_check(&events),
            Some("prerequisites: missing tools: pnpm")
        );
        events.push(event("setup-started", ""));
        events.push(event("environment-checks-passed", ""));
        assert_eq!(failed_check(&events), None);
    }
}
