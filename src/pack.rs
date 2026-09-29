// ABOUTME: Builds the snapshot archive (tar.gz) that beam uploads to the sandbox.
// ABOUTME: Also reads the archive that comes back from "beam down".

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Archive {
    builder: tar::Builder<GzEncoder<File>>,
    pub path: PathBuf,
}

impl Archive {
    pub fn create(path: &Path) -> Result<Archive> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let mut builder = tar::Builder::new(GzEncoder::new(f, Compression::default()));
        builder.mode(tar::HeaderMode::Complete);
        Ok(Archive {
            builder,
            path: path.to_path_buf(),
        })
    }

    /// Add a file or a full directory from disk.
    pub fn add_path(&mut self, name: &str, src: &Path) -> Result<()> {
        let meta = std::fs::symlink_metadata(src)
            .with_context(|| format!("cannot read {}", src.display()))?;
        if meta.is_dir() {
            self.builder.append_dir_all(name, src)
        } else {
            self.builder.append_path_with_name(src, name)
        }
        .with_context(|| format!("cannot add {} to the snapshot", src.display()))
    }

    pub fn add_bytes(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(crate::util::now_unix());
        h.set_cksum();
        self.builder.append_data(&mut h, name, data)?;
        Ok(())
    }

    pub fn finish(self) -> Result<u64> {
        let enc = self.builder.into_inner()?;
        enc.finish()?;
        Ok(std::fs::metadata(&self.path)?.len())
    }
}

pub struct DiskEntry {
    pub path: String,
    pub file: PathBuf,
    pub mtime: u64,
    pub mode: u32,
}

/// Extract regular files to a private staging directory, one file at a time.
/// All other entry types are rejected except directories. No archive symlink is followed.
pub fn extract(gz: &Path, directory: &Path) -> Result<Vec<DiskEntry>> {
    crate::util::private_dir(directory)?;
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(gz)?));
    let mut result = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry
            .path()?
            .to_str()
            .context("archive paths must be UTF-8")?
            .to_string();
        let path = path
            .strip_prefix("./")
            .unwrap_or(&path)
            .trim_end_matches('/')
            .to_string();
        crate::util::relative_path(&path)?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            anyhow::bail!("unsupported archive entry: {path}");
        }
        if !seen.insert(path.clone()) {
            anyhow::bail!("duplicate archive entry: {path}");
        }
        if path != "info"
            && path != "repo.bundle"
            && !path.starts_with(".claude/")
            && !path.starts_with("extras/")
        {
            anyhow::bail!("unexpected archive entry: {path}");
        }
        let destination = crate::util::safe_destination(directory, &path)?;
        let parent = destination.parent().context("archive path has no parent")?;
        crate::util::private_dir(parent)?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::copy(&mut entry, &mut tmp)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&destination).map_err(|e| e.error)?;
        result.push(DiskEntry {
            path,
            file: destination,
            mtime: entry.header().mtime()?,
            mode: entry.header().mode()? & 0o777,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_archive_rejects_links_and_duplicate_paths() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("bad.tar.gz");
        let mut ar = Archive::create(&path).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_mode(0o777);
        h.set_cksum();
        ar.builder
            .append_link(&mut h, "extras/link", "/tmp")
            .unwrap();
        ar.finish().unwrap();
        assert!(extract(&path, &d.path().join("one")).is_err());
        let mut ar = Archive::create(&path).unwrap();
        ar.add_bytes("info", b"first").unwrap();
        ar.add_bytes("info", b"second").unwrap();
        ar.finish().unwrap();
        assert!(extract(&path, &d.path().join("two")).is_err());
    }

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/sub")).unwrap();
        std::fs::write(d.path().join("src/sub/a.txt"), "A").unwrap();
        let out = d.path().join("x.tar.gz");
        let mut a = Archive::create(&out).unwrap();
        a.add_path(".claude/dir", &d.path().join("src")).unwrap();
        a.add_bytes("info", b"hello").unwrap();
        assert!(a.finish().unwrap() > 0);

        let entries = extract(&out, &d.path().join("incoming")).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, vec![".claude/dir/sub/a.txt", "info"]);
        assert_eq!(std::fs::read(&entries[1].file).unwrap(), b"hello");
    }
}
