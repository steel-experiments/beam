// ABOUTME: Opt-in GitHub authentication and instructions for publishing sandbox work.
use anyhow::{Result, bail};
use std::process::Command;

/// Accept only a credential-free GitHub origin. SSH origins use HTTPS in the sandbox.
pub fn github_origin(origin: &str) -> Result<String> {
    let path = origin
        .strip_prefix("https://github.com/")
        .or_else(|| origin.strip_prefix("git@github.com:"))
        .or_else(|| origin.strip_prefix("ssh://git@github.com/"));
    if let Some(path) = path {
        let path = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() == 2
            && parts.iter().all(|part| {
                !part.is_empty()
                    && *part != "."
                    && *part != ".."
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            })
        {
            return Ok(format!("https://github.com/{path}.git"));
        }
    }
    bail!("GitHub authentication needs an origin on github.com (HTTPS or SSH)")
}

/// Never include token command output in diagnostics.
pub fn github_token() -> Result<String> {
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(name)
            && !token.trim().is_empty()
        {
            return Ok(token);
        }
    }
    if let Ok(output) = Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .output()
        && output.status.success()
    {
        let token = String::from_utf8(output.stdout)?;
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }
    bail!(
        "GitHub authentication is unavailable. Set GH_TOKEN or GITHUB_TOKEN, or run `gh auth login` locally"
    )
}

pub fn handoff(transfer_id: &str) -> String {
    format!(
        "\n\nUser-authorized PR workflow:\n\
         Work on the task branch beam/{transfer_id}, which Beam creates in the sandbox.\n\
         Inspect existing local changes before editing. Do not publish unrelated or sensitive files.\n\
         Commit meaningful task changes with Conventional Commits, for example fix(parser): Handle empty input.\n\
         Follow repository commit rules. Run relevant checks and record their results.\n\
         Push completed commits regularly with git push -u origin HEAD. Verify that each push succeeds.\n\
         Use gh to open or update a draft pull request for this branch when meaningful changes exist.\n\
         Describe the change and validation. Use --body-file for multiline descriptions.\n\
         Do not merge the pull request or force-push. Do not make an empty or rushed commit just to return.\n\
         Before the sandbox stops, attempt a final push. Report failures and any remaining uncommitted work.\n\
         Beam still returns commits, uncommitted changes, and session files. A push does not back up session files.\n\
         Beam does not schedule a push before expiry. Do not rely on a final push to preserve work.\n"
    )
}

/// Publication evidence from the sandbox. Values that are not expected become "unknown".
struct Evidence<'a> {
    published: &'a str,
    dirty: &'a str,
    pr: Option<&'a str>,
}

/// The sandbox writes the report, so accept only known values before they reach the terminal.
fn evidence(text: &str) -> Evidence<'_> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .unwrap_or("unknown")
    };
    let published = value("published");
    let dirty = value("dirty");
    let pr = value("pr");
    Evidence {
        published: if ["yes", "no"].contains(&published) {
            published
        } else {
            "unknown"
        },
        dirty: if !dirty.is_empty() && dirty.bytes().all(|c| c.is_ascii_digit()) {
            dirty
        } else {
            "unknown"
        },
        pr: (pr.starts_with("https://github.com/") && !pr.chars().any(char::is_control))
            .then_some(pr),
    }
}

pub fn show_report(text: &str) {
    let report = evidence(text);
    crate::up::step(
        "publication",
        format!(
            "remote HEAD published: {}; uncommitted paths: {}",
            report.published, report.dirty
        ),
    );
    if let Some(pr) = report.pr {
        crate::up::step("pull request", pr);
    }
    if report.published != "yes" || report.dirty != "0" {
        crate::ui::warn(
            "Some work may exist only in the return package. Beam returns it without making a commit or push.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_github_origins_without_accepting_other_hosts_or_credentials() {
        for origin in [
            "https://github.com/acme/project.git",
            "git@github.com:acme/project.git",
            "ssh://git@github.com/acme/project",
        ] {
            assert_eq!(
                github_origin(origin).unwrap(),
                "https://github.com/acme/project.git"
            );
        }
        for origin in [
            "https://token@github.com/acme/project",
            "https://github.com.evil/acme/project",
            "/tmp/repo",
            "https://github.com/acme/project?token=x",
            "https://github.com/../project",
        ] {
            assert!(github_origin(origin).is_err());
        }
    }

    #[test]
    fn report_values_from_the_sandbox_cannot_carry_terminal_controls() {
        let clean = evidence("dirty=2\npublished=yes\npr=https://github.com/acme/project/pull/7\n");
        assert_eq!(clean.published, "yes");
        assert_eq!(clean.dirty, "2");
        assert_eq!(clean.pr, Some("https://github.com/acme/project/pull/7"));
        let hostile = evidence(
            "dirty=1\x1b[2J\npublished=yes\x1b]0;title\x07\npr=https://github.com/x\x1b[31m\n",
        );
        assert_eq!(hostile.published, "unknown");
        assert_eq!(hostile.dirty, "unknown");
        assert_eq!(hostile.pr, None);
        assert_eq!(evidence("").published, "unknown");
        assert_eq!(evidence("dirty=\n").dirty, "unknown");
    }

    #[test]
    fn publication_probe_checks_the_remote_tip_instead_of_cached_tracking_refs() {
        let temp = tempfile::tempdir().unwrap();
        let run = |script: &str| {
            let output = Command::new("sh")
                .args(["-c", script])
                .current_dir(temp.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GH_PROMPT_DISABLED", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(
            "git init -q --bare remote.git && git init -q -b task repo && cd repo && git config user.name Tester && git config user.email t@example.com && echo first > file && git add file && git commit -q -m first && git remote add origin ../remote.git",
        );
        let probe = format!("cd repo\n{}", include_str!("../scripts/publication.sh"));
        let unpublished = run(&probe);
        assert!(unpublished.contains("published=no"), "{unpublished}");
        run("cd repo && git push -q -u origin HEAD");
        let published = run(&probe);
        assert!(published.contains("published=yes"), "{published}");
        assert!(published.contains("dirty=0"), "{published}");
        run(
            "cd repo && echo second > file && git add file && git commit -q -m second && printf unfinished > 'new\nfile'",
        );
        let unfinished = run(&probe);
        assert!(unfinished.contains("published=no"), "{unfinished}");
        assert!(unfinished.contains("dirty=1"), "{unfinished}");
        // A remote failure gives unknown evidence without losing the dirty count.
        run("cd repo && git remote set-url origin ../missing.git");
        let failed = run(&probe);
        assert!(failed.contains("published=unknown"), "{failed}");
        assert!(failed.contains("dirty=1"), "{failed}");
    }
}
