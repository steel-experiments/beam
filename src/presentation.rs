// ABOUTME: Shared user-facing transfer state and next action.
use crate::{
    agent::evidence::{Process, Task},
    monitor::Snapshot,
    state::{Phase, State},
    util,
};

pub const RETAINED_NOTICE: &str = "Further sandbox edits will not return through beam down.";

pub struct Summary {
    pub phase: &'static str,
    pub message: &'static str,
    pub next: &'static str,
}

pub fn summary(phase: Phase, remote: Option<&Snapshot>, conflicts: bool) -> Summary {
    let (label, message, next) = match phase {
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
    println!("{}  {}", view.phase, st.describe());
    println!("{}", view.message);
    if let Some(snapshot) = remote {
        if snapshot.process != Process::Running
            && matches!(st.phase, Phase::Starting | Phase::Remote | Phase::Retained)
        {
            println!("Remote state: {}", snapshot.remote);
        }
        if snapshot.task.state != Task::Unknown {
            println!(
                "Evidence: {} — {}",
                snapshot.task.evidence.source.label(),
                snapshot.task.evidence.detail
            );
        }
    }
    if st.phase == Phase::Retained {
        println!("{RETAINED_NOTICE}");
    } else if remote
        .is_some_and(|s| s.process == Process::Running && s.capabilities.session_transfer)
    {
        println!("Avoid running the same agent locally.");
    }
    if !view.next.is_empty() {
        println!("Next: {}", view.next);
    }
}

pub fn recovery(st: &State) {
    println!("Saved recovery: {}", st.dir().display());
    for conflict in &st.conflicts {
        println!("! {conflict}");
    }
    if let Some(path) = &st.recovery {
        println!("Remote worktree: {}", path.display());
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
