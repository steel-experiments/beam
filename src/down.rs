// ABOUTME: "beam down": stops the remote agent and brings the worktree and the transcript home.
// ABOUTME: Local work is never overwritten: when both sides changed, the remote work is kept aside.

use crate::claude;
use crate::git::{self, BackOutcome, Snap};
use crate::pack::{self, Entry};
use crate::remote;
use crate::state::State;
use crate::up::{step, tempdir};
use crate::util::{sha256_bytes, sha256_file};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;

pub fn load_state(path: &Path) -> Result<State> {
    let cwd = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    let root = git::toplevel(&cwd)?;
    State::load(&root)?.context("this project is not beamed. The session is local")
}

pub fn down(path: &Path, keep: bool) -> Result<()> {
    let st = load_state(path)?;
    let sb = &st.sandbox;
    let home = st.home.to_string_lossy().to_string();
    let root = st.project_root.to_string_lossy().to_string();

    sb.wake()?;
    step("agent", format!("stopping (tmux: {})", st.tmux));
    sb.exec(&remote::stop_agent(&st.tmux))?;

    let back_ref = format!("refs/beam/{}/back", st.session_id);
    let agent_paths = claude::back_paths(&st.agent_cwd);
    sb.exec(&remote::pack_back(
        &st.stage,
        &root,
        &home,
        &back_ref,
        &st.sent.wt_commit,
        &agent_paths,
    ))?;
    let data = sb.exec_bytes(&remote::cat(&format!("{}/back.tar.gz", st.stage)))?;
    step("download", crate::util::human_size(data.len() as u64));
    let entries = pack::read_entries(&data)?;

    let info = entries
        .iter()
        .find(|e| e.path == "info")
        .context("the archive has no info")?;
    let remote_snap = Snap::from_kv(&String::from_utf8_lossy(&info.data))?;
    let bundle = entries
        .iter()
        .find(|e| e.path == "repo.bundle")
        .context("the archive has no bundle")?;
    let tmp = tempdir()?;
    let bundle_path = tmp.join("back.bundle");
    std::fs::write(&bundle_path, &bundle.data)?;
    git::fetch_bundle(&st.project_root, &bundle_path, &back_ref)?;

    let short = &st.session_id[..st.session_id.len().min(8)];
    let outcome = git::apply_back(
        &st.project_root,
        &st.sent,
        &remote_snap,
        &format!("refs/beam/{}/check", st.session_id),
        &format!("beam/{short}"),
    )?;
    match &outcome {
        BackOutcome::Applied => step("worktree", "remote changes applied"),
        BackOutcome::KeptAside { branch, stash } => {
            println!(
                "! The local repository changed while the session was away. Nothing local was changed."
            );
            println!("  Remote commits:   branch {branch}");
            println!(
                "  Remote worktree:  stash@{{0}} ({})",
                &stash[..12.min(stash.len())]
            );
            println!("  To use them:      git switch {branch} && git stash apply");
        }
    }

    let agent: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.path.starts_with(".claude/"))
        .collect();
    let report = merge_agent_files(&st.home, &st.sent_files, &agent)?;
    step(
        "transcript",
        format!(
            "{} updated, {} added, {} same",
            report.updated, report.added, report.same
        ),
    );
    for c in &report.conflicts {
        println!("! {c} changed on both sides. The remote copy is at {c}.beam-remote");
    }
    let newest = agent
        .iter()
        .filter(|e| {
            e.path.starts_with(&claude::projects_rel(&st.agent_cwd)) && e.path.ends_with(".jsonl")
        })
        .filter(|e| !e.path.contains("/subagents/"))
        .max_by_key(|e| e.mtime)
        .and_then(|e| {
            Path::new(&e.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .unwrap_or(st.session_id.clone());

    if keep {
        println!(
            "! The sandbox is still there: {} (remove it yourself when you are done)",
            sb.describe()
        );
    } else {
        sb.destroy(&remote::cleanup(&st.tmux, &st.remote_paths))?;
        step("sandbox", "removed");
    }
    git::delete_refs(&st.project_root, &format!("refs/beam/{}/", st.session_id));
    st.remove()?;
    let _ = std::fs::remove_dir_all(&tmp);
    println!("✓ Session is home. Continue with: claude --resume {newest}");
    if outcome != BackOutcome::Applied || !report.conflicts.is_empty() {
        bail!("beam down finished with conflicts. See the messages above");
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub same: usize,
    pub conflicts: Vec<String>,
}

/// Copy agent files from the sandbox to `home`. A local file is replaced only when it did not
/// change since beam sent it. Otherwise the remote copy goes next to it as "<file>.beam-remote".
pub fn merge_agent_files(
    home: &Path,
    sent: &BTreeMap<String, String>,
    entries: &[&Entry],
) -> Result<MergeReport> {
    let mut r = MergeReport::default();
    for e in entries {
        if e.path.split('/').any(|c| c == ".." || c.is_empty()) {
            bail!("unsafe path in the archive: {}", e.path);
        }
        let dest = home.join(&e.path);
        let remote_hash = sha256_bytes(&e.data);
        let target = if dest.exists() {
            let local_hash = sha256_file(&dest)?;
            if local_hash == remote_hash {
                r.same += 1;
                continue;
            }
            if sent.get(&e.path) == Some(&local_hash) {
                r.updated += 1;
                dest
            } else {
                r.conflicts.push(dest.to_string_lossy().to_string());
                dest.with_file_name(format!(
                    "{}.beam-remote",
                    dest.file_name().unwrap().to_string_lossy()
                ))
            }
        } else {
            r.added += 1;
            dest
        };
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&target, &e.data)?;
        if e.mtime > 0 {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.mtime);
            let _ = std::fs::File::options()
                .write(true)
                .open(&target)
                .and_then(|f| f.set_modified(t));
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, data: &str) -> Entry {
        Entry {
            path: path.into(),
            data: data.as_bytes().to_vec(),
            mtime: 1_700_000_000,
        }
    }

    #[test]
    fn merges_without_losing_local_changes() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude/p")).unwrap();
        std::fs::write(h.join(".claude/p/s.jsonl"), "one\n").unwrap();
        std::fs::write(h.join(".claude/p/edited.jsonl"), "local edit\n").unwrap();
        std::fs::write(h.join(".claude/p/same.jsonl"), "x\n").unwrap();
        let mut sent = BTreeMap::new();
        sent.insert(".claude/p/s.jsonl".to_string(), sha256_bytes(b"one\n"));
        sent.insert(
            ".claude/p/edited.jsonl".to_string(),
            sha256_bytes(b"original\n"),
        );

        let es = [
            entry(".claude/p/s.jsonl", "one\ntwo\n"),
            entry(".claude/p/edited.jsonl", "remote edit\n"),
            entry(".claude/p/same.jsonl", "x\n"),
            entry(".claude/p/new.jsonl", "new\n"),
        ];
        let refs: Vec<&Entry> = es.iter().collect();
        let r = merge_agent_files(h, &sent, &refs).unwrap();

        assert_eq!((r.added, r.updated, r.same), (1, 1, 1));
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(
            std::fs::read_to_string(h.join(".claude/p/s.jsonl")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(
            std::fs::read_to_string(h.join(".claude/p/edited.jsonl")).unwrap(),
            "local edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(h.join(".claude/p/edited.jsonl.beam-remote")).unwrap(),
            "remote edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(h.join(".claude/p/new.jsonl")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn rejects_unsafe_paths() {
        let home = tempfile::tempdir().unwrap();
        let e = entry(".claude/../../etc/x", "no");
        assert!(merge_agent_files(home.path(), &BTreeMap::new(), &[&e]).is_err());
    }
}
