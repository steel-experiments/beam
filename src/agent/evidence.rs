// ABOUTME: Shared observations and evidence precedence. No client names or terminal patterns.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub session_transfer: bool,
    pub structured_events: bool,
    pub terminal_heuristics: bool,
    pub agent_reports: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    ProcessStarted,
    Activity,
    InputNeeded,
    InputResolved,
    ProcessExited,
    CompletionReported,
    FailureReported,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Unavailable,
    Process,
    TerminalHeuristic,
    AgentReport,
    ClientEvent,
}
impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Process => "process",
            Self::TerminalHeuristic => "terminal-heuristic",
            Self::AgentReport => "agent-report",
            Self::ClientEvent => "client-event",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub kind: Kind,
    pub source: Source,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}
impl Observation {
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            kind: Kind::Unknown,
            source: Source::Unavailable,
            detail: detail.into(),
            at: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Process {
    Preparing,
    Running,
    Stopped,
    NeedsAttention,
    Paused,
    Unknown,
}
impl Process {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "running" => Self::Running,
            "preparing" => Self::Preparing,
            "needs-attention" => Self::NeedsAttention,
            "paused" => Self::Paused,
            value if value.starts_with("stopped") => Self::Stopped,
            _ => Self::Unknown,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Task {
    Unknown,
    ActivityObserved,
    InputNeeded,
    PossibleInput,
    CompletionReported,
    FailureReported,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assessment {
    pub state: Task,
    pub evidence: Observation,
}
/// The latest observation within each source wins. A weak source cannot clear a stronger one.
pub fn assess(observations: &[Observation]) -> Assessment {
    let mut latest = std::collections::BTreeMap::new();
    for observation in observations {
        if observation.kind == Kind::ProcessStarted {
            latest.clear();
        }
        if observation.source != Source::Process && observation.source != Source::Unavailable {
            latest.insert(observation.source, observation);
        }
    }
    for (source, observation) in latest.into_iter().rev() {
        let state = match observation.kind {
            Kind::InputNeeded if source == Source::TerminalHeuristic => Task::PossibleInput,
            Kind::InputNeeded => Task::InputNeeded,
            Kind::Activity if source != Source::TerminalHeuristic => Task::ActivityObserved,
            Kind::CompletionReported if source != Source::TerminalHeuristic => {
                Task::CompletionReported
            }
            Kind::FailureReported if source != Source::TerminalHeuristic => Task::FailureReported,
            // A structured resolution clears older weaker observations as well.
            Kind::InputResolved | Kind::Unknown if source != Source::TerminalHeuristic => {
                Task::Unknown
            }
            Kind::InputResolved | Kind::Unknown => continue,
            _ => continue,
        };
        return Assessment {
            state,
            evidence: observation.clone(),
        };
    }
    Assessment {
        state: Task::Unknown,
        evidence: Observation::unknown("No current task evidence"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation(kind: Kind, source: Source) -> Observation {
        Observation {
            kind,
            source,
            detail: "test".into(),
            at: None,
        }
    }
    #[test]
    fn evidence_strength_and_resolution_control_task_state() {
        let heuristic = observation(Kind::InputNeeded, Source::TerminalHeuristic);
        let report = observation(Kind::Activity, Source::AgentReport);
        let client = observation(Kind::InputNeeded, Source::ClientEvent);
        assert_eq!(
            assess(std::slice::from_ref(&heuristic)).state,
            Task::PossibleInput
        );
        assert_eq!(
            assess(&[report.clone(), heuristic.clone()]).state,
            Task::ActivityObserved
        );
        assert_eq!(
            assess(&[client.clone(), report, heuristic.clone()]).state,
            Task::InputNeeded
        );
        assert_eq!(
            assess(&[
                client.clone(),
                observation(Kind::InputResolved, Source::TerminalHeuristic)
            ])
            .state,
            Task::InputNeeded
        );
        assert_eq!(
            assess(&[
                client,
                heuristic,
                observation(Kind::InputResolved, Source::ClientEvent)
            ])
            .state,
            Task::Unknown
        );
    }
    #[test]
    fn process_liveness_and_exit_never_prove_completion() {
        assert_eq!(assess(&[]).state, Task::Unknown);
        for kind in [Kind::ProcessStarted, Kind::ProcessExited] {
            assert_eq!(
                assess(&[observation(kind, Source::Process)]).state,
                Task::Unknown
            );
        }
        assert_eq!(
            assess(&[
                observation(Kind::CompletionReported, Source::AgentReport),
                observation(Kind::ProcessStarted, Source::Process)
            ])
            .state,
            Task::Unknown
        );
        assert_eq!(
            assess(&[observation(Kind::CompletionReported, Source::ClientEvent)]).state,
            Task::CompletionReported
        );
    }
}
