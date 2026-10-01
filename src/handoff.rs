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

/// The next step that the user gave with `--continue`. The agent starts it without a new prompt.
pub fn continue_note(text: &str) -> String {
    format!(
        "\nThe user moved this session so that you continue to work here without them. Do not wait for a reply. Next step: {}\n",
        text.trim()
    )
}

/// The first message after `beam down`. It is one line, so the printed command can be pasted.
pub fn return_note(from: &str, kept: bool) -> String {
    format!(
        "[beam] You are back on the local machine from the sandbox ({from}). The git state (commits, staged and unstaged changes) and this conversation returned, merged with local work. {} Tools and packages that you installed in the sandbox are not here, and build output and dependencies (for example node_modules) can be different, so build and test again before you trust earlier results. $BEAM_REPORT, $BEAM_CHECK, and the sandbox beam commands are not available. Do not continue the task yet: tell the user that you are back and the state of the task, then wait for the user.",
        if kept {
            "The sandbox is kept for inspection, but its later edits do not return."
        } else {
            "The sandbox is removed."
        }
    )
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

    #[test]
    fn return_note_is_one_line_and_waits_for_the_user() {
        let note = return_note("steel:abc", false);
        assert!(!note.contains('\n'));
        assert!(
            note.starts_with(
                "[beam] You are back on the local machine from the sandbox (steel:abc)."
            )
        );
        assert!(note.contains("The sandbox is removed."));
        assert!(note.ends_with("then wait for the user."));
        assert!(return_note("docker:x", true).contains("The sandbox is kept for inspection"));
    }

    #[test]
    fn continue_note_names_the_next_step() {
        let note = continue_note(" finish the parser tests \n");
        assert!(note.contains("Do not wait for a reply."));
        assert!(note.ends_with("Next step: finish the parser tests\n"));
    }
}
