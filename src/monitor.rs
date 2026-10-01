// ABOUTME: Evidence from remote events, phase timings, and opt-in terminal notifications.
use crate::{
    state::{Phase, State},
    util,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub at: u64,
    pub kind: String,
    pub detail: String,
}
use crate::agent::{
    self, Adapter,
    evidence::{self, Assessment, Capabilities, Kind, Observation, Process, Source, Task},
};

#[derive(Debug, Serialize)]
pub struct InputRequest<'a> {
    pub source: Source,
    pub kind: &'a str,
    pub next_action: &'static str,
}
pub fn input_request(task: &Assessment) -> Option<InputRequest<'_>> {
    matches!(task.state, Task::InputNeeded | Task::PossibleInput).then_some(InputRequest {
        source: task.evidence.source,
        kind: &task.evidence.detail,
        next_action: "beam attach",
    })
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub remote: String,
    pub process: Process,
    pub capabilities: Capabilities,
    pub observations: Vec<Observation>,
    pub task: Assessment,
    pub events: Vec<Event>,
}
// An adapter probe can fail without making the process state unavailable.
pub fn observe(
    adapter: &dyn Adapter,
    remote: String,
    events: Vec<Event>,
    probe: impl FnOnce(&str) -> Result<String>,
    stage: &str,
    tmux: &str,
) -> Snapshot {
    let process = Process::parse(&remote);
    let mut observations = Vec::new();
    for event in &events {
        let (kind, source) = match event.kind.as_str() {
            "agent-started" => {
                observations.clear();
                (Kind::ProcessStarted, Source::Process)
            }
            "agent-exited" => (Kind::ProcessExited, Source::Process),
            "agent-reported-working" => (Kind::Activity, Source::AgentReport),
            "agent-reported-waiting" => (Kind::InputNeeded, Source::AgentReport),
            "agent-reported-finished" => (Kind::CompletionReported, Source::AgentReport),
            "agent-reported-failed" => (Kind::FailureReported, Source::AgentReport),
            _ => continue,
        };
        observations.push(Observation {
            kind,
            source,
            detail: event.detail.clone(),
            at: Some(event.at),
        });
    }
    if matches!(process, Process::Running | Process::Repairing) {
        let observation = match adapter.observation_script(stage, tmux) {
            Some(script) => probe(&script)
                .and_then(|out| adapter.decode_observation(&out))
                .unwrap_or_else(|_| Observation::unknown("The adapter observation is unavailable")),
            None => Observation::unknown("This adapter has no live observation source"),
        };
        observations.push(observation);
    } else {
        observations.push(Observation::unknown(
            "No live task observation is available",
        ));
    }
    let task = evidence::assess(&observations);
    Snapshot {
        remote,
        process,
        capabilities: adapter.capabilities(),
        observations,
        task,
        events,
    }
}
pub fn snapshot(st: &State) -> Result<Snapshot> {
    let adapter = agent::get(&st.agent)?;
    let remote = if matches!(st.phase, Phase::Starting | Phase::Remote | Phase::Retained) {
        st.sandbox()?
            .process_status(&st.stage, &st.tmux)
            .unwrap_or_else(|e| format!("unavailable: {e}"))
    } else {
        st.phase.label().into()
    };
    Ok(observe(
        adapter,
        remote,
        events(st),
        |script| st.sandbox()?.exec(script),
        &st.stage,
        &st.tmux,
    ))
}
#[derive(Default)]
struct TaskNotifications {
    current: Option<(Task, Source, String)>,
    run: Option<u64>,
}
impl TaskNotifications {
    fn observe(&mut self, snapshot: &Snapshot) -> bool {
        if let Some(start) = snapshot
            .observations
            .iter()
            .rev()
            .find(|o| o.kind == Kind::ProcessStarted)
            .and_then(|o| o.at)
            && self.run != Some(start)
        {
            self.current = None;
            self.run = Some(start);
        }
        if !matches!(snapshot.process, Process::Running | Process::Repairing) {
            if snapshot.process == Process::Stopped {
                self.current = None;
            }
            return false;
        }
        if matches!(
            snapshot.task.state,
            Task::InputNeeded
                | Task::PossibleInput
                | Task::CompletionReported
                | Task::FailureReported
        ) {
            let key = (
                snapshot.task.state.clone(),
                snapshot.task.evidence.source,
                snapshot.task.evidence.detail.clone(),
            );
            let changed = self.current.as_ref() != Some(&key);
            self.current = Some(key);
            changed
        } else {
            // An unavailable probe does not prove that a prompt disappeared.
            if self.current.as_ref().is_some_and(|(_, source, _)| {
                snapshot.observations.iter().any(|o| {
                    o.source >= *source
                        && matches!(o.kind, Kind::Activity | Kind::InputResolved | Kind::Unknown)
                })
            }) {
                self.current = None;
            }
            false
        }
    }
}

pub fn parse(text: &str) -> Vec<Event> {
    text.lines()
        .filter_map(|line| {
            let mut cols = line.splitn(3, '\t');
            Some(Event {
                at: cols.next()?.parse().ok()?,
                kind: cols.next()?.into(),
                detail: cols
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(500)
                    .collect(),
            })
        })
        .collect()
}
pub fn events(st: &State) -> Vec<Event> {
    let text = if matches!(st.phase, Phase::Starting | Phase::Remote | Phase::Retained) {
        st.sandbox()
            .and_then(|sb| {
                sb.exec(&format!(
                    "tail -n 100 {} 2>/dev/null || true",
                    util::sh_quote(&format!("{}/events.tsv", st.stage))
                ))
            })
            .unwrap_or_default()
    } else {
        std::fs::read_to_string(st.dir().join("events.tsv")).unwrap_or_default()
    };
    parse(&text)
}
pub fn show(events: &[Event]) {
    if let Some(check) = events
        .iter()
        .rev()
        .take_while(|e| e.kind != "setup-started")
        .find(|e| e.kind.starts_with("verification-"))
    {
        let hue = if check.kind.ends_with("passed") {
            crate::ui::Hue::Green
        } else {
            crate::ui::Hue::Orange
        };
        println!(
            "{}",
            crate::ui::field(
                "Project check",
                format!(
                    "{} {} {}",
                    crate::ui::paint(hue, &check.kind),
                    crate::ui::dim(&format!("(at {})", check.at)),
                    check.detail
                )
            )
        );
    }
    if let Some(e) = events
        .iter()
        .rev()
        .take_while(|e| e.kind != "agent-started")
        .find(|e| e.kind.starts_with("agent-reported-"))
    {
        println!(
            "{}",
            crate::ui::field(
                "Agent report",
                format!(
                    "{} {} {}",
                    crate::ui::bold(
                        crate::ui::Hue::Turquoise,
                        e.kind.trim_start_matches("agent-reported-")
                    ),
                    crate::ui::dim(&format!("(at {})", e.at)),
                    e.detail
                )
            )
        );
    }
    if let Some(e) = events.last() {
        println!(
            "{}",
            crate::ui::field(
                "Last event",
                format!(
                    "{} {} {}",
                    e.kind,
                    crate::ui::dim(&format!("(at {})", e.at)),
                    e.detail
                )
            )
        );
    }
}
pub fn record_phase(st: &State, from: Phase) -> Result<()> {
    let path = st.dir().join("timings.json");
    let mut events: Vec<serde_json::Value> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e.into()),
    };
    let now = util::now_unix();
    let start = events
        .last()
        .and_then(|v| v["at"].as_u64())
        .unwrap_or(st.created_at);
    events.push(serde_json::json!({"at": now, "from": from, "to": st.phase, "elapsed_seconds": now.saturating_sub(start)}));
    util::atomic_write(&path, &serde_json::to_vec_pretty(&events)?)
}
pub fn watch(
    path: &Path,
    json: bool,
    interval: u64,
    count: Option<u64>,
    notify: bool,
) -> Result<()> {
    let root = crate::git::toplevel(&path.canonicalize()?)?;
    let mut last = String::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut iteration = 0;
    let mut input_notifications = TaskNotifications::default();
    loop {
        let st = State::load(&root)?;
        let snapshot = st.as_ref().map(snapshot).transpose()?;
        let events = snapshot
            .as_ref()
            .map(|s| s.events.clone())
            .unwrap_or_default();
        let key = format!(
            "{:?}:{}",
            st.as_ref().map(|s| s.phase),
            serde_json::to_string(&snapshot)?
        );
        if key != last {
            crate::status_with_snapshot(&root, json, st.as_ref(), snapshot.as_ref())?;
            last = key;
        }
        if snapshot
            .as_ref()
            .is_some_and(|s| input_notifications.observe(s))
            && notify
        {
            eprint!("\x07");
            let message = if snapshot
                .as_ref()
                .is_some_and(|s| s.process == Process::Repairing)
            {
                "Environment repair needs attention. Beam checks have not passed. Run beam attach."
            } else {
                match snapshot.as_ref().map(|s| &s.task.state) {
                    Some(Task::PossibleInput) => {
                        "Possible input prompt (terminal heuristic). Run beam attach."
                    }
                    Some(Task::CompletionReported) => {
                        "The agent reported completion. Run beam down --review."
                    }
                    Some(Task::FailureReported) => "The agent reported a failure. Run beam attach.",
                    _ => "The agent reports that input is needed. Run beam attach.",
                }
            };
            eprintln!("{}", crate::ui::notice(message));
        }
        for event in events {
            let key = format!("{}:{}:{}", event.at, event.kind, event.detail);
            let new = seen.insert(key);
            if notify
                && new
                && iteration > 0
                && matches!(event.kind.as_str(), "needs-attention" | "agent-exited")
            {
                eprint!("\x07");
                eprintln!(
                    "{}",
                    crate::ui::notice(&format!("{} {}", event.kind, event.detail))
                );
            }
        }
        iteration += 1;
        if count.is_some_and(|n| iteration >= n) || st.is_none() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(interval));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{get, testing::Fixture};
    fn fixture(event: &str) -> Snapshot {
        observe(
            &Fixture,
            "running".into(),
            vec![],
            |_| Ok(event.into()),
            "stage",
            "tmux",
        )
    }
    #[test]
    fn repair_status_keeps_live_input_notifications_without_claiming_readiness() {
        let snapshot = observe(
            &Fixture,
            "repairing".into(),
            vec![],
            |_| Ok("waiting\tAuthentication required".into()),
            "stage",
            "tmux",
        );
        assert_eq!(snapshot.process, Process::Repairing);
        assert_eq!(snapshot.task.state, Task::InputNeeded);
        let summary = crate::presentation::summary(Phase::Remote, Some(&snapshot), false);
        assert_eq!(summary.phase, "environment repair");
        assert_eq!(summary.next, "beam attach");
        let mut notices = TaskNotifications::default();
        assert!(notices.observe(&snapshot));
        assert!(!notices.observe(&snapshot));
    }

    #[test]
    fn client_events_drive_the_same_ui_and_notifications() {
        let mut notices = TaskNotifications::default();
        let waiting = fixture("waiting\tApprove the change");
        assert_eq!(waiting.task.state, Task::InputNeeded);
        assert_eq!(
            input_request(&waiting.task).unwrap().source,
            Source::ClientEvent
        );
        assert_eq!(
            crate::presentation::summary(Phase::Remote, Some(&waiting), false).next,
            "beam attach"
        );
        assert!(notices.observe(&waiting));
        assert!(!notices.observe(&waiting));
        let unavailable = fixture("invalid event");
        assert_eq!(unavailable.process, Process::Running);
        assert_eq!(unavailable.task.state, Task::Unknown);
        assert!(!notices.observe(&unavailable));
        assert!(!notices.observe(&waiting));
        assert!(!notices.observe(&fixture("resolved\tInput accepted")));
        assert!(notices.observe(&waiting));
        for wire in ["done\tTests passed", "failed\tTests failed"] {
            let state = fixture(wire);
            assert!(notices.observe(&state));
            assert!(!notices.observe(&state));
        }
    }
    #[test]
    fn fallback_is_possible_input_and_clears_without_inventing_progress() {
        let snapshot = |kind: &str| {
            observe(
                get("claude").unwrap(),
                "running".into(),
                vec![],
                |_| {
                    Ok(format!(
                        r#"{{"kind":"{kind}","source":"terminal-heuristic","detail":"confirmation"}}"#
                    ))
                },
                "s",
                "t",
            )
        };
        let mut notices = TaskNotifications::default();
        let prompt = snapshot("input-needed");
        assert_eq!(
            crate::presentation::summary(Phase::Remote, Some(&prompt), false).phase,
            "possible-input"
        );
        assert!(notices.observe(&prompt));
        assert!(!notices.observe(&prompt));
        let cleared = snapshot("input-resolved");
        assert_eq!(cleared.task.state, Task::Unknown);
        assert!(!notices.observe(&cleared));
        assert!(notices.observe(&prompt));
        for phase in [Phase::Returning, Phase::Downloaded, Phase::Applied] {
            assert_eq!(
                crate::presentation::summary(phase, Some(&prompt), true).next,
                "beam down"
            );
        }
    }
    #[test]
    fn unsupported_and_stopped_sessions_do_not_probe_or_claim_progress() {
        let shell = observe(
            get("shell").unwrap(),
            "running".into(),
            vec![],
            |_| panic!("shell must not probe"),
            "s",
            "t",
        );
        assert_eq!(shell.task.state, Task::Unknown);
        let stopped = observe(
            &Fixture,
            "stopped 0".into(),
            vec![],
            |_| panic!("stopped must not probe"),
            "s",
            "t",
        );
        assert_eq!(stopped.task.state, Task::Unknown);
        assert_eq!(
            crate::presentation::summary(Phase::Remote, Some(&stopped), false).phase,
            "stopped"
        );
    }
    #[test]
    fn weak_resolution_does_not_reset_a_stronger_notification() {
        let mut notices = TaskNotifications::default();
        let waiting = fixture("waiting\tApprove the change");
        assert!(notices.observe(&waiting));
        let weak = observe(
            get("claude").unwrap(),
            "running".into(),
            vec![],
            |_| {
                Ok(r#"{"kind":"input-resolved","source":"terminal-heuristic","detail":"No footer"}"#.into())
            },
            "s",
            "t",
        );
        assert!(!notices.observe(&weak));
        assert!(!notices.observe(&waiting));
        let mut restarted = fixture("waiting\tApprove the change");
        restarted.observations.insert(
            0,
            Observation {
                kind: Kind::ProcessStarted,
                source: Source::Process,
                detail: String::new(),
                at: Some(123),
            },
        );
        assert!(notices.observe(&restarted));
        assert!(!notices.observe(&restarted));
    }

    #[test]
    fn restart_discards_reports_from_the_previous_run() {
        let events =
            parse("1\tagent-started\t\n2\tagent-reported-finished\tOld work\n3\tagent-started\t");
        let snapshot = observe(
            get("shell").unwrap(),
            "running".into(),
            events,
            |_| unreachable!(),
            "s",
            "t",
        );
        assert_eq!(snapshot.task.state, Task::Unknown);
    }
}
