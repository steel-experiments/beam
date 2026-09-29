// ABOUTME: A shell workspace has no session files or live client probe.
use super::{Adapter, evidence::Capabilities};
pub struct Shell;
impl Adapter for Shell {
    fn id(&self) -> &'static str {
        "shell"
    }
    fn label(&self) -> &'static str {
        "Shell"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            agent_reports: true,
            ..Capabilities::default()
        }
    }
    fn resume_fn(&self, _session: &str) -> String {
        "resume() { sh -i; }".into()
    }
}
