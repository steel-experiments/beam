// ABOUTME: Saved return plans, conservative Git merges, review, and reversible applies.
use crate::{
    git::{self, Snap},
    state::{self, Phase, ProjectLock, State},
    util,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
    process::Stdio,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReturnPlan {
    pub version: u32,
    pub local: Snap,
    pub remote: Snap,
    pub target: Option<Snap>,
    pub local_paths: Vec<String>,
    pub remote_paths: Vec<String>,
    pub conflicts: Vec<String>,
}

type Tree = BTreeMap<String, String>;
fn tree(repo: &Path, id: &str) -> Result<Tree> {
    let output = git::git(repo).args(["ls-tree", "-rz", id]).output()?;
    if !output.status.success() {
        bail!("cannot read Git tree {id}");
    }
    String::from_utf8(output.stdout)?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (meta, path) = s.split_once('\t').context("invalid Git tree entry")?;
            Ok((path.to_owned(), meta.to_owned()))
        })
        .collect()
}
fn write_tree(repo: &Path, entries: &Tree) -> Result<String> {
    let dir = tempfile::tempdir()?;
    let index = dir.path().join("index");
    util::run(
        git::git(repo)
            .env("GIT_INDEX_FILE", &index)
            .args(["read-tree", "--empty"]),
    )?;
    let mut input = Vec::new();
    for (path, meta) in entries {
        input.extend_from_slice(format!("{meta}\t{path}\0").as_bytes());
    }
    let mut child = git::git(repo)
        .env("GIT_INDEX_FILE", &index)
        .args(["update-index", "-z", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("no Git input")?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Git writer stopped"))??;
    if !output.status.success() {
        bail!(
            "cannot construct merged index: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    util::run(
        git::git(repo)
            .env("GIT_INDEX_FILE", &index)
            .args(["write-tree"]),
    )
}
fn changed(base: &Tree, next: &Tree) -> BTreeSet<String> {
    base.keys()
        .chain(next.keys())
        .filter(|p| base.get(*p) != next.get(*p))
        .cloned()
        .collect()
}
fn paths(repo: &Path, base: &Snap, next: &Snap) -> Result<BTreeSet<String>> {
    let mut result = changed(&tree(repo, &base.wt_tree)?, &tree(repo, &next.wt_tree)?);
    result.extend(changed(
        &tree(repo, &base.idx_tree)?,
        &tree(repo, &next.idx_tree)?,
    ));
    Ok(result)
}

pub fn build(st: &State, remote: &Snap) -> Result<ReturnPlan> {
    let repo = &st.project_root;
    let local = git::snapshot(
        repo,
        &format!("refs/beam/{}/review-local", st.transfer_id),
        None,
        None,
    )?;
    let local_paths = paths(repo, &st.sent, &local)?;
    let remote_paths = paths(repo, &st.sent, remote)?;
    let mut conflicts = vec![];
    let mut target = if local.same_state(&st.sent) || local.same_state(remote) {
        Some(remote.clone())
    } else if remote.same_state(&st.sent) {
        Some(local.clone())
    } else if local.head != st.sent.head
        || remote.head != st.sent.head
        || local.branch != st.sent.branch
        || remote.branch != st.sent.branch
    {
        conflicts.push("Branch or commit history changed on diverging workspaces; review the saved remote worktree.".into());
        None
    } else {
        let li = tree(repo, &local.idx_tree)?;
        let lw = tree(repo, &local.wt_tree)?;
        let ri = tree(repo, &remote.idx_tree)?;
        let rw = tree(repo, &remote.wt_tree)?;
        for p in local_paths.intersection(&remote_paths) {
            if li.get(p) != ri.get(p) || lw.get(p) != rw.get(p) {
                conflicts.push(p.clone());
            }
        }
        // A file on one side must not replace a directory containing another side's work.
        for l in &local_paths {
            for r in &remote_paths {
                if l.starts_with(&format!("{r}/")) || r.starts_with(&format!("{l}/")) {
                    conflicts.push(format!("file/directory collision: {l} and {r}"));
                }
            }
        }
        if conflicts.is_empty() {
            let mut idx = li;
            let mut wt = lw;
            for p in &remote_paths {
                for (dest, source) in [(&mut idx, &ri), (&mut wt, &rw)] {
                    if let Some(value) = source.get(p) {
                        dest.insert(p.clone(), value.clone());
                    } else {
                        dest.remove(p);
                    }
                }
            }
            let idx_tree = write_tree(repo, &idx)?;
            let wt_tree = write_tree(repo, &wt)?;
            let idx_commit = util::run(
                git::git(repo)
                    .env("GIT_AUTHOR_NAME", "beam")
                    .env("GIT_AUTHOR_EMAIL", "beam@localhost")
                    .env("GIT_COMMITTER_NAME", "beam")
                    .env("GIT_COMMITTER_EMAIL", "beam@localhost")
                    .args([
                        "commit-tree",
                        "--no-gpg-sign",
                        &idx_tree,
                        "-p",
                        &local.head,
                        "-m",
                        "Beam candidate index",
                    ]),
            )?;
            let wt_commit = util::run(
                git::git(repo)
                    .env("GIT_AUTHOR_NAME", "beam")
                    .env("GIT_AUTHOR_EMAIL", "beam@localhost")
                    .env("GIT_COMMITTER_NAME", "beam")
                    .env("GIT_COMMITTER_EMAIL", "beam@localhost")
                    .args([
                        "commit-tree",
                        "--no-gpg-sign",
                        &wt_tree,
                        "-p",
                        &local.head,
                        "-m",
                        "Beam return candidate",
                        "-p",
                        &idx_commit,
                    ]),
            )?;
            util::run(git::git(repo).args([
                "update-ref",
                &format!("refs/beam/{}/candidate", st.transfer_id),
                &wt_commit,
            ]))?;
            Some(Snap {
                head: local.head.clone(),
                branch: local.branch.clone(),
                idx_tree,
                wt_tree,
                wt_commit,
            })
        } else {
            None
        }
    };
    if let Some(result) = &target {
        let obstructions = git::ignored_obstructions(repo, &local, result)?;
        if !obstructions.is_empty() {
            conflicts.extend(
                obstructions
                    .into_iter()
                    .map(|p| format!("Local ignored file would be overwritten: {p}")),
            );
            target = None;
        }
    }
    Ok(ReturnPlan {
        version: 1,
        local,
        remote: remote.clone(),
        target,
        local_paths: local_paths.into_iter().collect(),
        remote_paths: remote_paths.into_iter().collect(),
        conflicts,
    })
}

pub fn load(st: &State) -> Result<Option<ReturnPlan>> {
    let path = st.dir().join("return-plan.json");
    match std::fs::read(path) {
        Ok(bytes) => {
            let plan: ReturnPlan = serde_json::from_slice(&bytes)?;
            if plan.version != 1 {
                bail!("unsupported return plan version {}", plan.version);
            }
            Ok(Some(plan))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn candidate_path(st: &State, target: &Snap) -> std::path::PathBuf {
    let key = util::sha256_bytes(
        format!(
            "{}:{}:{}:{}",
            target.head, target.branch, target.idx_tree, target.wt_tree
        )
        .as_bytes(),
    );
    st.dir().join("candidates").join(key)
}
pub fn save(st: &State, plan: &ReturnPlan) -> Result<()> {
    if let Some(target) = &plan.target {
        let candidate = candidate_path(st, target);
        if !candidate.exists() {
            git::recovery_worktree(&st.project_root, &candidate, target)?;
        }
    }
    util::atomic_write(
        &st.dir().join("return-plan.json"),
        &serde_json::to_vec_pretty(plan)?,
    )
}
pub fn show(st: &State, plan: &ReturnPlan) {
    println!(
        "Remote work is saved locally.\n\n{} paths changed remotely\n{} paths changed locally",
        plan.remote_paths.len(),
        plan.local_paths.len()
    );
    for p in &plan.remote_paths {
        println!("  remote: {p:?}");
    }
    for p in &plan.local_paths {
        println!("  local:  {p:?}");
    }
    for p in &plan.conflicts {
        println!("! {p}");
    }
    if let Some(path) = &st.recovery {
        println!("Remote worktree: {}", path.display());
    }
    if let Some(target) = &plan.target {
        println!(
            "Proposed result (inspection copy): {}",
            candidate_path(st, target).display()
        );
    }
    if let Ok(bytes) = std::fs::read(st.dir().join("return-files.json"))
        && let Ok(files) = serde_json::from_slice::<Vec<crate::return_files::FileChange>>(&bytes)
    {
        for file in files.iter().filter(|f| f.conflict || f.before != f.after) {
            println!(
                "{} {}",
                if file.conflict {
                    "Needs review:"
                } else {
                    "Return file:"
                },
                file.path.display()
            );
        }
    }
    if plan.target.is_some() && st.phase == Phase::Downloaded {
        println!("Next: beam review --apply");
    } else {
        println!("Review the saved worktree. After manual integration: beam review --resolved");
    }
}

pub fn select(root: &Path, transfer: Option<&str>) -> Result<State> {
    let all = state::records(&crate::up::home_dir()?)?;
    all.into_iter()
        .rev()
        .find(|s| {
            s.project_root == root
                && transfer.is_none_or(|id| id == s.transfer_id)
                && s.dir().join("return-plan.json").exists()
        })
        .context("no saved return plan for this project")
}

#[allow(clippy::too_many_arguments)]
pub fn review(
    path: &Path,
    transfer: Option<&str>,
    apply: bool,
    resolved: bool,
    diff: bool,
    json: bool,
    keep: bool,
    refresh: bool,
    open: bool,
) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let _lock = ProjectLock::acquire(&crate::up::home_dir()?, &root)?;
    let st = select(&root, transfer)?;
    let plan = load(&st)?.context("return plan is missing")?;
    if refresh {
        if st.phase != Phase::Downloaded || st.dir().join("apply-started").exists() {
            bail!("only an unapplied return plan can be refreshed");
        }
        for name in ["return-plan.json", "return-files.json"] {
            match std::fs::remove_file(st.dir().join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        drop(_lock);
        return crate::down::down(&root, false, true);
    }
    if open {
        let worktree = st.recovery.as_ref().context("no saved recovery worktree")?;
        drop(_lock);
        let status =
            std::process::Command::new(std::env::var_os("SHELL").unwrap_or_else(|| "sh".into()))
                .current_dir(worktree)
                .status()?;
        if !status.success() {
            bail!("review shell exited with {status}");
        }
        return Ok(());
    }
    if apply {
        if st.phase != Phase::Downloaded {
            bail!("this return has already finished; inspect the saved worktree");
        }
        if plan.target.is_none() {
            bail!(
                "this plan needs manual integration; inspect the saved worktree and use --resolved afterward"
            );
        }
        drop(_lock);
        return crate::down::down(&root, keep, false);
    }
    if resolved {
        if st.phase == Phase::Downloaded {
            bail!("finish the return with beam down before marking recovery resolved");
        }
        util::atomic_write(
            &st.dir().join("resolved.json"),
            &serde_json::to_vec(&serde_json::json!({"resolved_at": util::now_unix()}))?,
        )?;
        println!("Recovery marked resolved. Saved copies remain available.");
        return Ok(());
    }
    if json {
        let mut value = serde_json::to_value(&plan)?;
        for (key, file) in [
            ("files", "return-files.json"),
            ("timings", "timings.json"),
            ("task", "task.json"),
        ] {
            value[key] = std::fs::read(st.dir().join(file))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(serde_json::Value::Null);
        }
        value["events"] = serde_json::to_value(crate::monitor::events(&st))?;
        value["transfer"] = st.transfer_id.clone().into();
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if diff {
        let status = git::git(&root)
            .args([
                "--no-pager",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                &st.sent.wt_tree,
                &plan.remote.wt_tree,
                "--",
            ])
            .status()?;
        if !status.success() {
            bail!("cannot show returned changes");
        }
    } else {
        show(&st, &plan);
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct Undo {
    pub before: Snap,
    pub after: Snap,
    pub completed: bool,
}
pub fn prepare_undo(st: &State, plan: &ReturnPlan, target: &Snap) -> Result<()> {
    let path = st.dir().join("undo.json");
    if path.exists() {
        return Ok(());
    }
    git::recovery_worktree(
        &st.project_root,
        &st.dir().join("before-return"),
        &plan.local,
    )?;
    util::atomic_write(
        &path,
        &serde_json::to_vec_pretty(&Undo {
            before: plan.local.clone(),
            after: target.clone(),
            completed: false,
        })?,
    )
}
pub fn undo(path: &Path, transfer: Option<&str>) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let _lock = ProjectLock::acquire(&crate::up::home_dir()?, &root)?;
    let st = select(&root, transfer)?;
    if !matches!(st.phase, Phase::Applied | Phase::Retained | Phase::Closed) {
        bail!("finish the return before undoing it");
    }
    let file = st.dir().join("undo.json");
    let mut undo: Undo = serde_json::from_slice(
        &std::fs::read(&file).context("this return did not apply Git work")?,
    )?;
    if undo.completed {
        println!("This return is already undone.");
        return Ok(());
    }
    let files = crate::return_files::undo_check(&st)?;
    if git::apply_back(
        &root,
        &undo.after,
        &undo.before,
        &format!("refs/beam/{}/undo-check", st.transfer_id),
    )? == git::BackOutcome::KeptAside
    {
        bail!(
            "local work changed after return. Both versions are saved in {}. Local work is unchanged",
            st.dir().display()
        );
    }
    crate::return_files::restore(&files)?;
    undo.completed = true;
    util::atomic_write(&file, &serde_json::to_vec_pretty(&undo)?)?;
    println!(
        "Local state restored. Returned work remains in {}.",
        st.dir().join("worktree").display()
    );
    Ok(())
}
