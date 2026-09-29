// ABOUTME: Builds the snapshot archive (tar.gz) that beam uploads to the sandbox.
// ABOUTME: Also reads the archive that comes back from "beam down".

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct Archive {
    builder: tar::Builder<GzEncoder<File>>,
    pub path: PathBuf,
}

impl Archive {
    pub fn create(path: &Path) -> Result<Archive> {
        let f = File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
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

/// A regular file from an archive.
pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
    pub mtime: u64,
}

pub fn read_entries(gz: &[u8]) -> Result<Vec<Entry>> {
    let mut ar = tar::Archive::new(GzDecoder::new(gz));
    let mut out = vec![];
    for e in ar.entries().context("bad archive")? {
        let mut e = e?;
        if !e.header().entry_type().is_file() {
            continue;
        }
        let path = e
            .path()?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();
        let mtime = e.header().mtime().unwrap_or(0);
        let mut data = vec![];
        e.read_to_end(&mut data)?;
        out.push(Entry { path, data, mtime });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/sub")).unwrap();
        std::fs::write(d.path().join("src/sub/a.txt"), "A").unwrap();
        let out = d.path().join("x.tar.gz");
        let mut a = Archive::create(&out).unwrap();
        a.add_path("home/dir", &d.path().join("src")).unwrap();
        a.add_bytes("info", b"hello").unwrap();
        assert!(a.finish().unwrap() > 0);

        let entries = read_entries(&std::fs::read(&out).unwrap()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, vec!["home/dir/sub/a.txt", "info"]);
        assert_eq!(entries[1].data, b"hello");
    }
}
