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
    #[cfg(test)]
    c.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
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
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(SNAPSHOT_SH)
        .arg("snapshot.sh")
        .arg(repo)
        .arg(refname)
        .arg(out)
        .arg(exclude.unwrap_or(""));
    #[cfg(test)]
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let text = run(&mut cmd)?;
    Snap::from_kv(&text)
}

pub fn config_get(repo: &Path, key: &str) -> String {
    run(git(repo).args(["config", "--get", key])).unwrap_or_default()
}

/// Files that are modified or untracked (and not ignored), with their sizes.
pub fn changed_files(repo: &Path) -> Result<Vec<(String, u64)>> {
    let output = git(repo)
        .args(["status", "--porcelain=v1", "-z", "-uall"])
        .output()?;
    if !output.status.success() {
        bail!("cannot inspect the Git worktree");
    }
    let raw = String::from_utf8(output.stdout).context("Git paths must be UTF-8")?;
    let mut names = Vec::new();
    let mut parts = raw.split('\0');
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        names.push(entry[3..].to_string());
        if entry[..2].contains(['R', 'C']) {
            parts.next();
        }
    }
    let mut files: Vec<(String, u64)> = names
        .iter()
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
    /// The local repository changed. The caller retains the recovery worktree.
    KeptAside,
}

/// Apply the remote work to the local repository.
///
/// `sent` is the state beam sent, `remote` is the state that came back. The objects of `remote`
/// must already be in the local repository (fetched from the bundle).
pub fn apply_back(repo: &Path, sent: &Snap, remote: &Snap, check_ref: &str) -> Result<BackOutcome> {
    let local = snapshot(repo, check_ref, None, None)?;
    let branch_ok = branch_move_ok(repo, &local, remote)?;
    // A retry after a completed apply must not replay the worktree update.
    if local.same_state(remote) {
        return Ok(BackOutcome::Applied);
    }
    if !local.same_state(sent)
        || !branch_ok
        || !ignored_obstructions(repo, &local, remote)?.is_empty()
    {
        return Ok(BackOutcome::KeptAside);
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

/// Git checkout can overwrite ignored files. Check them before touching the index.
pub fn ignored_obstructions(repo: &Path, local: &Snap, target: &Snap) -> Result<Vec<String>> {
    let read_paths = |args: &[&str]| -> Result<String> {
        let output = git(repo).args(args).output()?;
        if !output.status.success() {
            bail!("cannot inspect local ignored files");
        }
        Ok(String::from_utf8(output.stdout)?)
    };
    let added = read_paths(&[
        "diff",
        "--no-renames",
        "--name-only",
        "--diff-filter=A",
        "-z",
        &local.wt_tree,
        &target.wt_tree,
        "--",
    ])?;
    let ignored = read_paths(&[
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "-z",
    ])?;
    let mut conflicts = Vec::new();
    for p in ignored.split('\0').filter(|p| !p.is_empty()) {
        if added
            .split('\0')
            .filter(|p| !p.is_empty())
            .any(|a| a == p || a.starts_with(&format!("{p}/")) || p.starts_with(&format!("{a}/")))
        {
            conflicts.push(p.into());
        }
    }
    Ok(conflicts)
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

/// Keep an exact copy of the returned Git state in its own worktree before updating local files.
pub fn recovery_worktree(repo: &Path, destination: &Path, remote: &Snap) -> Result<()> {
    if !destination.join(".git").exists() {
        run(git(repo)
            .arg("worktree")
            .args(["add", "--detach"])
            .arg(destination)
            .arg(&remote.head))?;
    }
    run(git(destination).args(["read-tree", "--reset", "-u", &remote.wt_tree]))?;
    run(git(destination).args(["read-tree", &remote.idx_tree]))?;
    let _ = git(destination)
        .args(["update-index", "-q", "--refresh"])
        .output();
    Ok(())
}

/// Read indexed blob sizes with one Git process, including files changed only in the index.
pub fn indexed_sizes(repo: &Path) -> Result<Vec<(String, u64)>> {
    use std::io::Write;
    use std::process::Stdio;
    let indexed = run(git(repo).args(["ls-files", "--stage", "-z"]))?;
    let mut objects = std::collections::BTreeMap::new();
    for entry in indexed.split('\0').filter(|s| !s.is_empty()) {
        let (meta, name) = entry.split_once('\t').context("invalid Git index entry")?;
        let cols: Vec<_> = meta.split_whitespace().collect();
        if cols.len() != 3 {
            bail!("invalid Git index entry");
        }
        if cols[0] == "160000" {
            bail!("submodules need separate transfers. Beam does not yet copy their worktrees");
        }
        if cols[2] != "0" {
            bail!("resolve Git merge conflicts before beaming this workspace");
        }
        objects.insert(cols[1].to_string(), name.to_string());
    }
    if objects.is_empty() {
        return Ok(vec![]);
    }
    let input = objects.keys().cloned().collect::<Vec<_>>().join("\n") + "\n";
    let mut child = git(repo)
        .arg("cat-file")
        .arg("--batch-check=%(objectname) %(objectsize)")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("no Git stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Git input writer stopped"))??;
    if !output.status.success() {
        bail!(
            "cannot inspect staged files: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout)?
        .lines()
        .map(|line| {
            let (id, size) = line.split_once(' ').context("invalid Git object size")?;
            Ok((
                objects.get(id).context("unexpected Git object")?.clone(),
                size.parse()?,
            ))
        })
        .collect()
}
