// ABOUTME: Downloads once, saves recovery data, and applies the return without overwriting local work.
use crate::{
    claude,
    git::{self, BackOutcome, Snap},
    pack::{self, DiskEntry},
    remote,
    state::{Phase, ProjectLock, State},
    up::step,
    util,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;

pub fn load_state(path: &Path) -> Result<State> {
    let cwd = path.canonicalize()?;
    let root = git::toplevel(&cwd)?;
    State::load(&root)?.context("this project is local. Run `beam` to send it")
}

pub fn down(path: &Path, keep: bool) -> Result<()> {
    let root = git::toplevel(&path.canonicalize()?)?;
    let _lock = ProjectLock::acquire(&crate::up::home_dir()?, &root)?;
    let mut st = load_state(path)?;
    let result = return_home(&mut st, keep);
    if let Err(e) = &result {
        st.last_error = Some(format!("{e:#}"));
        let _ = st.save();
    }
    result
}

fn return_home(st: &mut State, keep: bool) -> Result<()> {
    if st.phase == Phase::Retained {
        if keep {
            println!(
                "The work is already home. The sandbox is retained: {}",
                st.describe()
            );
            return Ok(());
        }
        crate::up::cleanup(st)?;
        st.remove()?;
        println!("✓ Removed the retained sandbox. Previously returned work is unchanged.");
        return Ok(());
    }
    if matches!(
        st.phase,
        Phase::Planned | Phase::Allocating | Phase::Created | Phase::Prepared | Phase::Uploaded
    ) {
        bail!(
            "the upload is incomplete. Run `beam` to continue, or `beam kill --yes` to remove its resources"
        );
    }
    let package = st.dir().join("back.tar.gz");
    if !matches!(
        st.phase,
        Phase::Returning | Phase::Downloaded | Phase::Applied
    ) {
        st.advance(Phase::Returning)?;
    }
    if st.phase == Phase::Returning {
        let sb = st.sandbox()?;
        sb.wake()?;
        step("agent", "stopping the remote session");
        sb.exec(&remote::stop_agent(&st.tmux))?;
        sb.exec(&remote::stop_agent(&format!("{}-repair", st.tmux)))?;
        let agent_paths = if st.agent == "claude" {
            claude::back_paths(&st.agent_cwd)
        } else {
            vec![]
        };
        sb.exec(&remote::pack_back(
            &st.stage,
            &st.project_root.to_string_lossy(),
            &st.remote_home,
            &format!("refs/beam/{}/back", st.transfer_id),
            &st.sent.wt_commit,
            &agent_paths,
            &st.return_extras,
        ))?;
        sb.download(&remote::cat(&format!("{}/back.tar.gz", st.stage)), &package)?;
        step(
            "download",
            util::human_size(std::fs::metadata(&package)?.len()),
        );
        st.advance(Phase::Downloaded)?;
    }
    if st.phase == Phase::Downloaded {
        let entries = pack::extract(&package, &st.dir().join("incoming"))?;
        let info = entries
            .iter()
            .find(|e| e.path == "info")
            .context("return package has no Git snapshot")?;
        let snap = Snap::from_kv(&std::fs::read_to_string(&info.file)?)?;
        let bundle = entries
            .iter()
            .find(|e| e.path == "repo.bundle")
            .context("return package has no Git bundle")?;
        git::fetch_bundle(
            &st.project_root,
            &bundle.file,
            &format!("refs/beam/{}/back", st.transfer_id),
        )?;
        // This worktree also protects against interruption midway through applying the main worktree.
        let recovery = st.dir().join("worktree");
        if !st.dir().join("recovery.ready").exists() {
            git::recovery_worktree(&st.project_root, &recovery, &snap)?;
            util::atomic_write(&st.dir().join("recovery.ready"), b"ready")?;
        }
        st.recovery = Some(recovery.clone());
        st.save()?;
        let outcome = git::apply_back(
            &st.project_root,
            &st.sent,
            &snap,
            &format!("refs/beam/{}/check", st.transfer_id),
        )?;
        if outcome == BackOutcome::KeptAside {
            let message = format!(
                "local Git state changed; remote work is in {}",
                recovery.display()
            );
            if !st.conflicts.contains(&message) {
                st.conflicts.push(message);
            }
        } else {
            step("worktree", "remote changes applied");
        }
        let agent: Vec<_> = entries
            .iter()
            .filter(|e| e.path.starts_with(".claude/"))
            .collect();
        let report = merge_files(
            &st.home,
            &st.sent_files,
            &agent,
            "",
            &[],
            &st.dir().join("conflicts/agent"),
        )?;
        st.conflicts.extend(report.conflicts);
        let extras: Vec<_> = entries
            .iter()
            .filter(|e| e.path.starts_with("extras/"))
            .collect();
        for e in &extras {
            let rel = e.path.strip_prefix("extras/").unwrap();
            if !st
                .return_extras
                .iter()
                .any(|r| rel == r || rel.starts_with(&format!("{r}/")))
            {
                bail!("unrequested returning extra: {rel}");
            }
        }
        let report = merge_files(
            &st.project_root,
            &st.sent_extras,
            &extras,
            "extras/",
            &st.return_extras,
            &st.dir().join("conflicts/extras"),
        )?;
        st.conflicts.extend(report.conflicts);
        st.conflicts.sort();
        st.conflicts.dedup();
        st.advance(Phase::Applied)?;
    }
    // Applied is durable before cleanup. Cleanup failure never repeats the local apply.
    if st.phase == Phase::Applied {
        if keep {
            st.advance(Phase::Retained)?;
        } else {
            crate::up::cleanup(st)?;
            st.remove()?;
        }
    }
    if st.conflicts.is_empty() {
        println!(
            "✓ Session is home.{}",
            if st.agent == "claude" {
                format!(" Continue with: claude --resume {}", st.session_id)
            } else {
                String::new()
            }
        );
    } else {
        println!("Work downloaded. Local changes were preserved.");
        for c in &st.conflicts {
            println!("! {c}");
        }
        if let Some(path) = &st.recovery {
            println!(
                "Inspect remote work: cd {}",
                util::sh_quote(&path.to_string_lossy())
            );
            println!(
                "Compare files: git diff --no-index {} {}",
                util::sh_quote(&st.project_root.to_string_lossy()),
                util::sh_quote(&path.to_string_lossy())
            );
        }
    }
    if keep {
        println!(
            "Sandbox retained: {}. Use `beam attach` to inspect it or `beam kill --yes` to remove it.",
            st.describe()
        );
    }
    println!("Recovery receipt: {}", st.dir().display());
    if !st.conflicts.is_empty() {
        bail!("return finished with conflicts; see the recovery paths above");
    }
    Ok(())
}

/// Three-way merge for regular files, including local and remote deletions.
pub fn merge_files(
    base: &Path,
    sent: &BTreeMap<String, String>,
    entries: &[&DiskEntry],
    prefix: &str,
    delete_scopes: &[String],
    conflicts: &Path,
) -> Result<MergeReport> {
    use std::os::unix::fs::PermissionsExt;
    let mut report = MergeReport::default();
    let mut returned = std::collections::BTreeSet::new();
    for e in entries {
        let rel = e
            .path
            .strip_prefix(prefix)
            .context("unexpected file prefix")?;
        returned.insert(rel.to_string());
        let dest = util::safe_destination(base, rel)?;
        let remote_hash = util::sha256_file(&e.file)?;
        let local_hash = if dest.is_file() {
            Some(util::sha256_file(&dest)?)
        } else {
            None
        };
        if local_hash.as_ref() == Some(&remote_hash) {
            report.same += 1;
            continue;
        }
        let original = sent.get(rel);
        // If only local changed, preserve it (including local deletion).
        if original == Some(&remote_hash) {
            report.same += 1;
            continue;
        }
        let target = if local_hash.as_ref() == original && !dest.is_dir() {
            if original.is_some() {
                report.updated += 1;
            } else {
                report.added += 1;
            }
            dest
        } else {
            let target = util::safe_destination(conflicts, rel)?;
            report.conflicts.push(format!(
                "{rel} changed on both sides; remote copy: {}",
                target.display()
            ));
            target
        };
        let parent = target.parent().context("file has no parent")?;
        std::fs::create_dir_all(parent)?;
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::fs::copy(&e.file, tmp.path())?;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(e.mode))?;
        if e.mtime > 0 {
            tmp.as_file()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.mtime))?;
        }
        tmp.as_file().sync_all()?;
        tmp.persist(&target).map_err(|e| e.error)?;
    }
    for (rel, hash) in sent {
        if returned.contains(rel)
            || !delete_scopes
                .iter()
                .any(|scope| rel == scope || rel.starts_with(&format!("{scope}/")))
        {
            continue;
        }
        let dest = util::safe_destination(base, rel)?;
        if !dest.exists() {
            continue;
        }
        if dest.is_file() && util::sha256_file(&dest)? == *hash {
            std::fs::remove_file(dest)?;
        } else {
            report.conflicts.push(format!(
                "{rel} was deleted remotely and changed locally; local copy preserved"
            ));
        }
    }
    Ok(report)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub same: usize,
    pub conflicts: Vec<String>,
}

#[cfg(test)]
mod merge_file_tests {
    use super::*;
    fn incoming(dir: &Path, name: &str, content: &str) -> DiskEntry {
        let file = dir.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content).unwrap();
        DiskEntry {
            path: format!("extras/{name}"),
            file,
            mtime: 0,
            mode: 0o600,
        }
    }
    #[test]
    fn merge_handles_both_deletions_conflicts_and_retries() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("local");
        let input = d.path().join("incoming");
        std::fs::create_dir(&base).unwrap();
        let mut sent = BTreeMap::new();
        for name in [
            "changed",
            "deleted-local",
            "deleted-remote",
            "conflict",
            "local-only",
        ] {
            std::fs::write(base.join(name), "old").unwrap();
            sent.insert(name.into(), util::sha256_bytes(b"old"));
        }
        std::fs::remove_file(base.join("deleted-local")).unwrap();
        std::fs::write(base.join("conflict"), "local").unwrap();
        std::fs::write(base.join("local-only"), "local").unwrap();
        let es = [
            incoming(&input, "changed", "remote"),
            incoming(&input, "deleted-local", "old"),
            incoming(&input, "conflict", "remote"),
            incoming(&input, "local-only", "old"),
        ];
        let refs: Vec<_> = es.iter().collect();
        let scopes: Vec<_> = sent.keys().cloned().collect();
        let conflicts = d.path().join("conflicts");
        for _ in 0..2 {
            let report = merge_files(&base, &sent, &refs, "extras/", &scopes, &conflicts).unwrap();
            assert_eq!(report.conflicts.len(), 1);
            assert_eq!(
                std::fs::read_to_string(base.join("changed")).unwrap(),
                "remote"
            );
            assert_eq!(
                std::fs::read_to_string(base.join("conflict")).unwrap(),
                "local"
            );
            assert_eq!(
                std::fs::read_to_string(base.join("local-only")).unwrap(),
                "local"
            );
            assert_eq!(
                std::fs::read_to_string(conflicts.join("conflict")).unwrap(),
                "remote"
            );
            assert!(!base.join("deleted-local").exists());
            assert!(!base.join("deleted-remote").exists());
        }
    }
    #[test]
    fn destination_symlink_is_never_followed() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base");
        std::fs::create_dir(&base).unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "safe").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("file")).unwrap();
        let entry = incoming(&d.path().join("incoming"), "file", "unsafe");
        assert!(
            merge_files(
                &base,
                &BTreeMap::new(),
                &[&entry],
                "extras/",
                &[],
                &d.path().join("conflicts")
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "safe");
    }
}
