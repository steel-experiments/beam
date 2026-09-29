// ABOUTME: Plans and restores returned extras and agent files without overwriting later edits.
use crate::{pack::DiskEntry, state::State, util};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    hash: String,
    mode: u32,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct FileChange {
    pub path: PathBuf,
    pub before: Option<Fingerprint>,
    pub after: Option<Fingerprint>,
    pub backup: Option<PathBuf>,
    pub conflict: bool,
}
pub fn fingerprint(path: &Path) -> Result<Option<Fingerprint>> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => Ok(Some(Fingerprint {
            hash: util::sha256_file(path)?,
            mode: m.permissions().mode() & 0o777,
        })),
        Ok(_) => bail!("return path is not a regular file: {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn collect(
    st: &State,
    base: &Path,
    sent: &BTreeMap<String, String>,
    entries: &[&DiskEntry],
    prefix: &str,
    scopes: &[String],
    out: &mut Vec<FileChange>,
) -> Result<()> {
    let mut names: BTreeMap<String, Option<&DiskEntry>> = entries
        .iter()
        .map(|e| {
            Ok((
                e.path
                    .strip_prefix(prefix)
                    .context("invalid return prefix")?
                    .to_owned(),
                Some(*e),
            ))
        })
        .collect::<Result<_>>()?;
    for name in sent.keys().filter(|p| {
        scopes
            .iter()
            .any(|s| *p == s || p.starts_with(&format!("{s}/")))
    }) {
        names.entry(name.clone()).or_insert(None);
    }
    for (name, entry) in names {
        if prefix == "extras/"
            && !scopes
                .iter()
                .any(|s| name == *s || name.starts_with(&format!("{s}/")))
        {
            bail!("unrequested returning extra: {name}");
        }
        let path = util::safe_destination(base, &name)?;
        let before = fingerprint(&path)?;
        let remote = entry
            .map(|e| {
                Ok::<_, anyhow::Error>(Fingerprint {
                    hash: util::sha256_file(&e.file)?,
                    mode: e.mode & 0o777,
                })
            })
            .transpose()?;
        let local_hash = before.as_ref().map(|f| &f.hash);
        let remote_hash = remote.as_ref().map(|f| &f.hash);
        let original = sent.get(&name);
        let conflict =
            local_hash != remote_hash && remote_hash != original && local_hash != original;
        let after = if local_hash == remote_hash || remote_hash == original || conflict {
            before.clone()
        } else {
            remote
        };
        let backup = if before != after && before.is_some() {
            let backup = st.dir().join("before-files").join(out.len().to_string());
            std::fs::create_dir_all(backup.parent().unwrap())?;
            std::fs::copy(&path, &backup)?;
            std::fs::File::open(&backup)?.sync_all()?;
            Some(backup)
        } else {
            None
        };
        out.push(FileChange {
            path,
            before,
            after,
            backup,
            conflict,
        });
    }
    Ok(())
}
pub fn prepare(st: &State, entries: &[DiskEntry]) -> Result<Vec<FileChange>> {
    let path = st.dir().join("return-files.json");
    if path.exists() {
        return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    let scopes = crate::agent::get(&st.agent)?.return_paths(&st.agent_cwd);
    let mut files = vec![];
    collect(
        st,
        &st.home,
        &st.sent_files,
        &entries
            .iter()
            .filter(|e| crate::agent::contains_path(&scopes, &e.path))
            .collect::<Vec<_>>(),
        "",
        &[],
        &mut files,
    )?;
    collect(
        st,
        &st.project_root,
        &st.sent_extras,
        &entries
            .iter()
            .filter(|e| e.path.starts_with("extras/"))
            .collect::<Vec<_>>(),
        "extras/",
        &st.return_extras,
        &mut files,
    )?;
    util::atomic_write(&path, &serde_json::to_vec_pretty(&files)?)?;
    Ok(files)
}
pub fn check(files: &[FileChange], applied: bool) -> Result<()> {
    for f in files {
        // Recheck parents too: an editor may have replaced a directory with a link.
        let parent = f.path.parent().context("file has no parent")?;
        if parent.exists() && parent.canonicalize()? != parent {
            bail!("return path parent changed: {}", parent.display());
        }
        let current = fingerprint(&f.path)?;
        if current != f.before && (!applied || current != f.after) {
            bail!(
                "{} changed after review. Local work is preserved; use beam review --refresh before applying",
                f.path.display()
            );
        }
    }
    Ok(())
}
pub fn undo_check(st: &State) -> Result<Vec<FileChange>> {
    let path = st.dir().join("return-files.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    let files: Vec<FileChange> = serde_json::from_slice(&std::fs::read(path)?)?;
    check(&files, true)?;
    Ok(files)
}
pub fn restore(files: &[FileChange]) -> Result<()> {
    for f in files.iter().filter(|f| f.before != f.after) {
        if fingerprint(&f.path)? == f.before {
            continue;
        }
        if let (Some(before), Some(backup)) = (&f.before, &f.backup) {
            let bytes = std::fs::read(backup)?;
            if util::sha256_bytes(&bytes) != before.hash {
                bail!("undo backup failed verification: {}", backup.display());
            }
            util::atomic_write(&f.path, &bytes)?;
            std::fs::set_permissions(&f.path, std::fs::Permissions::from_mode(before.mode))?;
        } else {
            std::fs::remove_file(&f.path)?;
        }
    }
    Ok(())
}
