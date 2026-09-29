// ABOUTME: Builds the first message that the agent gets after the move.
// ABOUTME: It tells the agent what is the same and what is different, so that the agent can fix the rest.

pub struct Facts<'a> {
    pub from: String,
    pub to: String,
    pub extras: &'a [String],
    pub env_names: &'a [String],
    pub removed_settings: &'a [String],
}

/// The part before the setup report.
pub fn head(f: &Facts) -> String {
    let mut s = format!(
        "[beam] You were moved from {} to a sandbox ({}).\n\
         The worktree, the git state (commits, staged and unstaged changes), and this conversation are the same.\n\
         These things are different:\n\
         - Files ignored by git were not copied",
        f.from, f.to
    );
    if f.extras.is_empty() {
        s.push('.');
    } else {
        s.push_str(&format!(", except: {}.", f.extras.join(", ")));
    }
    s.push_str(" Build output and installed packages (for example node_modules) are not there until setup makes them again.");
    if f.env_names.is_empty() {
        s.push_str("\n- No env vars were copied.");
    } else {
        s.push_str(&format!(
            "\n- Only these env vars were copied: {}.",
            f.env_names.join(", ")
        ));
    }
    if !f.removed_settings.is_empty() {
        s.push_str(&format!(
            "\n- These user settings were removed because they call local programs: {}.",
            f.removed_settings.join(", ")
        ));
    }
    s.push_str("\n- Local services (databases, MCP servers on localhost) are not available.");
    s
}

pub const TAIL: &str = "Verify the environment first (build and tests). Fix problems that the move caused. Then continue the task.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_differences() {
        let extras = vec![".env".to_string()];
        let env = vec!["DATABASE_URL".to_string()];
        let removed = vec!["hooks".to_string()];
        let h = head(&Facts {
            from: "macos/aarch64".into(),
            to: "docker:beam-x".into(),
            extras: &extras,
            env_names: &env,
            removed_settings: &removed,
        });
        assert!(
            h.starts_with("[beam] You were moved from macos/aarch64 to a sandbox (docker:beam-x).")
        );
        assert!(h.contains("except: .env."));
        assert!(h.contains("Only these env vars were copied: DATABASE_URL."));
        assert!(h.contains("call local programs: hooks."));
    }
}
