// ABOUTME: Git side of beam on the local machine: snapshots, checks, and applying work that comes back.
// ABOUTME: The snapshot logic is one shell script (scripts/snapshot.sh) that runs the same way on both sides.

use crate::util::{parse_kv, run, succeeds};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SNAPSHOT_SH: &str = include_str!("../scripts/snapshot.sh");

/// The git state of a worktree, as the snapshot script reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snap {
    pub head: String,
    pub branch: String,
    pub idx_tree: String,
    pub wt_tree: String,
    pub wt_commit: String,
}

impl Snap {
    pub fn from_kv(text: &str) -> Result<Snap> {
        let kv = parse_kv(text);
        let get = |k: &str| {
            kv.get(k)
                .cloned()
                .with_context(|| format!("snapshot output has no {k}: {text}"))
        };
        Ok(Snap {
            head: get("head")?,
            branch: get("branch")?,
            idx_tree: get("idx_tree")?,
            wt_tree: get("wt_tree")?,
            wt_commit: get("wt_commit")?,
        })
    }

    /// True when the worktree, the index, and HEAD are the same in both snapshots.
    pub fn same_state(&self, other: &Snap) -> bool {
        self.head == other.head
            && self.branch == other.branch
            && self.idx_tree == other.idx_tree
            && self.wt_tree == other.wt_tree
    }
}

pub fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir);
    c
}

pub fn toplevel(dir: &Path) -> Result<PathBuf> {
    let out = run(git(dir).args(["rev-parse", "--show-toplevel"]))
        .with_context(|| format!("{} is not in a git repository", dir.display()))?;
    Ok(PathBuf::from(out))
}

/// Run the snapshot script on a local repository.
pub fn snapshot(
    repo: &Path,
    refname: &str,
    bundle: Option<&Path>,
    exclude: Option<&str>,
) -> Result<Snap> {
    let out = bundle
        .map(|b| b.to_string_lossy().to_string())
        .unwrap_or("-".into());
    let text = run(Command::new("sh")
        .arg("-c")
        .arg(SNAPSHOT_SH)
        .arg("snapshot.sh")
        .arg(repo)
        .arg(refname)
        .arg(out)
        .arg(exclude.unwrap_or("")))?;
    Snap::from_kv(&text)
}

pub fn config_get(repo: &Path, key: &str) -> String {
    run(git(repo).args(["config", "--get", key])).unwrap_or_default()
}

/// Files that are modified or untracked (and not ignored), with their sizes.
pub fn changed_files(repo: &Path) -> Result<Vec<(String, u64)>> {
    let out = run(git(repo).args(["ls-files", "-z", "-m", "-o", "--exclude-standard"]))?;
    let mut files: Vec<(String, u64)> = out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|f| {
            let size = std::fs::symlink_metadata(repo.join(f))
                .map(|m| m.len())
                .unwrap_or(0);
            (f.to_string(), size)
        })
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Commits in HEAD that are not in the upstream branch. None when there is no upstream.
pub fn unpushed_count(repo: &Path) -> Option<usize> {
    run(git(repo).args(["rev-list", "--count", "@{upstream}..HEAD"]))
        .ok()
        .and_then(|s| s.parse().ok())
}

/// Make sure git ignores the .beam/ state directory in this repository.
pub fn exclude_beam_dir(repo: &Path) -> Result<()> {
    let rel = run(git(repo).args(["rev-parse", "--git-path", "info/exclude"]))?;
    let path = if Path::new(&rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        repo.join(rel)
    };
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|l| l.trim() == "/.beam/") {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let sep = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&path, format!("{current}{sep}/.beam/\n"))?;
    Ok(())
}

pub fn delete_refs(repo: &Path, prefix: &str) {
    if let Ok(out) = run(git(repo).args(["for-each-ref", "--format=%(refname)", prefix])) {
        for r in out.lines() {
            let _ = run(git(repo).args(["update-ref", "-d", r]));
        }
    }
}

/// What happened when beam applied the work that came back.
#[derive(Debug, PartialEq, Eq)]
pub enum BackOutcome {
    Applied,
    /// The local repository changed while the session was away. The remote work is kept aside.
    KeptAside {
        branch: String,
        stash: String,
    },
}

/// Apply the remote work to the local repository.
///
/// `sent` is the state beam sent, `remote` is the state that came back. The objects of `remote`
/// must already be in the local repository (fetched from the bundle).
pub fn apply_back(
    repo: &Path,
    sent: &Snap,
    remote: &Snap,
    check_ref: &str,
    aside_branch: &str,
) -> Result<BackOutcome> {
    let local = snapshot(repo, check_ref, None, None)?;
    let branch_ok = branch_move_ok(repo, &local, remote)?;
    if !local.same_state(sent) || !branch_ok {
        run(git(repo).args(["branch", "-f", aside_branch, &remote.head]))?;
        run(git(repo).args([
            "stash",
            "store",
            "-m",
            &format!("beam: remote worktree ({aside_branch})"),
            &remote.wt_commit,
        ]))?;
        return Ok(BackOutcome::KeptAside {
            branch: aside_branch.into(),
            stash: remote.wt_commit.clone(),
        });
    }

    // The index gets the full local worktree (with untracked files), so that a two-way merge
    // moves the worktree to the remote tree. This also removes files that the remote deleted.
    run(git(repo).args(["read-tree", &local.wt_tree]))?;
    let _ = git(repo).args(["update-index", "-q", "--refresh"]).output();
    if let Err(e) = run(git(repo).args(["read-tree", "-m", "-u", &local.wt_tree, &remote.wt_tree]))
    {
        let _ = run(git(repo).args(["read-tree", &local.idx_tree]));
        return Err(e.context("cannot update the worktree"));
    }

    if remote.branch.is_empty() {
        run(git(repo).args(["update-ref", "--no-deref", "HEAD", &remote.head]))?;
    } else {
        let r = format!("refs/heads/{}", remote.branch);
        run(git(repo).args(["update-ref", &r, &remote.head]))?;
        run(git(repo).args(["symbolic-ref", "HEAD", &r]))?;
    }
    run(git(repo).args(["read-tree", &remote.idx_tree]))?;
    let _ = git(repo).args(["update-index", "-q", "--refresh"]).output();
    Ok(BackOutcome::Applied)
}

/// A branch that the remote moved must still point to an ancestor of the remote HEAD locally.
fn branch_move_ok(repo: &Path, local: &Snap, remote: &Snap) -> Result<bool> {
    if remote.branch.is_empty() || remote.branch == local.branch {
        return Ok(true);
    }
    let r = format!("refs/heads/{}", remote.branch);
    if !succeeds(git(repo).args(["rev-parse", "-q", "--verify", &r])) {
        return Ok(true);
    }
    Ok(succeeds(git(repo).args([
        "merge-base",
        "--is-ancestor",
        &r,
        &remote.head,
    ])))
}

pub fn fetch_bundle(repo: &Path, bundle: &Path, refname: &str) -> Result<()> {
    run(git(repo)
        .arg("fetch")
        .arg("-q")
        .arg(bundle)
        .arg(format!("{refname}:{refname}")))
    .map(|_| ())
    .map_err(|e| {
        if bundle.exists() {
            e
        } else {
            e.context("bundle is missing")
        }
    })
}

pub fn ensure_has_commits(repo: &Path) -> Result<()> {
    if !succeeds(git(repo).args(["rev-parse", "-q", "--verify", "HEAD"])) {
        bail!("the repository has no commits. Make a first commit, then beam");
    }
    Ok(())
}
