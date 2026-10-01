// ABOUTME: Small helpers used by all modules: shell quoting, process runs, hashing.
// ABOUTME: Also parses key=value output and human sizes such as "50MB".

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Quote a string so that a POSIX shell reads it as one word.
pub fn sh_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Quote all words and join them with spaces.
pub fn sh_join<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| sh_quote(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run a command. Return stdout (trimmed). Fail with stderr when the exit code is not 0.
pub fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("cannot start {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!(
            "{} failed ({}): {}",
            describe(cmd),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run a command and return true when the exit code is 0. Output is discarded.
pub fn succeeds(cmd: &mut Command) -> bool {
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

fn describe(cmd: &Command) -> String {
    let mut s = cmd.get_program().to_string_lossy().to_string();
    for a in cmd.get_args().take(3) {
        let a = a.to_string_lossy();
        s.push(' ');
        s.push_str(if a.len() > 40 { "…" } else { &a });
    }
    s
}

pub fn sha256_bytes(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The length of the first part of `data` that ends a line and has the SHA-256 `hash`.
pub fn sha256_prefix_len(data: &[u8], hash: &str) -> Option<usize> {
    let mut hasher = Sha256::new();
    let mut start = 0;
    for (i, _) in data.iter().enumerate().filter(|(_, b)| **b == b'\n') {
        hasher.update(&data[start..=i]);
        start = i + 1;
        let digest: String = hasher
            .clone()
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if digest == hash {
            return Some(start);
        }
    }
    None
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Parse lines of "key=value".
pub fn parse_kv(s: &str) -> BTreeMap<String, String> {
    s.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Parse "50MB", "2GB", "512KB" or a plain number of bytes.
pub fn parse_size(s: &str) -> Result<u64> {
    let t = s.trim().to_ascii_uppercase();
    let (num, mult) = [
        ("GB", 1u64 << 30),
        ("MB", 1 << 20),
        ("KB", 1 << 10),
        ("B", 1),
    ]
    .iter()
    .find_map(|(suf, m)| t.strip_suffix(suf).map(|n| (n.trim().to_string(), *m)))
    .unwrap_or((t.clone(), 1));
    let n: u64 = num.parse().with_context(|| format!("bad size: {s}"))?;
    n.checked_mul(mult).context("value is too large")
}

/// Parse "90s", "30m", "4h", or a plain number of seconds.
pub fn parse_duration(s: &str) -> Result<u64> {
    let t = s.trim().to_ascii_lowercase();
    let (num, mult) = [("h", 3600u64), ("m", 60), ("s", 1)]
        .iter()
        .find_map(|(suf, m)| t.strip_suffix(suf).map(|n| (n.trim().to_string(), *m)))
        .unwrap_or((t.clone(), 1));
    let n: u64 = num.parse().with_context(|| format!("bad duration: {s}"))?;
    n.checked_mul(mult).context("value is too large")
}

pub fn human_size(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

/// A transfer size and its average speed, for example "59.5 MB at 1.0 MB/s".
/// A transfer too fast to measure shows only its size.
pub fn size_and_rate(bytes: u64, elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 0.1 {
        return human_size(bytes);
    }
    format!(
        "{} at {}/s",
        human_size(bytes),
        human_size((bytes as f64 / secs) as u64)
    )
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write a private file atomically and persist its directory entry.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("file has no parent directory")?;
    private_dir(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Reject paths that escape their base or need shell list escaping.
pub fn relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains(['\n', '\r', '\0'])
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("expected a relative path without '..': {path:?}");
    }
    Ok(())
}

/// Refuse symlinks in paths that Beam writes. Never follow them out of the destination.
pub fn safe_destination(base: &Path, rel: &str) -> Result<std::path::PathBuf> {
    relative_path(rel)?;
    let mut p = base.to_path_buf();
    for component in Path::new(rel).components() {
        p.push(component);
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("refusing to write through symlink {}", p.display());
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_len_finds_the_sent_lines() {
        let sent = sha256_bytes(b"a\nb\n");
        assert_eq!(sha256_prefix_len(b"a\nb\nc\n", &sent), Some(4));
        assert_eq!(sha256_prefix_len(b"a\nb\n", &sent), Some(4));
        assert_eq!(sha256_prefix_len(b"a\nx\nc\n", &sent), None);
        assert_eq!(sha256_prefix_len(b"a\nb", &sent), None);
    }

    #[test]
    fn size_and_rate_show_the_transfer_speed() {
        let secs = std::time::Duration::from_secs_f64;
        assert_eq!(size_and_rate(62_390_272, secs(57.7)), "59.5 MB at 1.0 MB/s");
        assert_eq!(size_and_rate(2048, secs(0.0)), "2.0 KB");
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(sh_quote("abc/def-1.2"), "abc/def-1.2");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_join(&["echo", "$HOME"]), "echo '$HOME'");
    }

    #[test]
    fn quoted_string_survives_a_real_shell() {
        let nasty = "a 'b' \"c\" $d `e` \\f\nnew line";
        let out = run(Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", sh_quote(nasty))))
        .unwrap();
        assert_eq!(out, nasty);
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("50MB").unwrap(), 50 << 20);
        assert_eq!(parse_size("1gb").unwrap(), 1 << 30);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert!(parse_size("lots").is_err());
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("4h").unwrap(), 14_400);
        assert_eq!(parse_duration("30m").unwrap(), 1_800);
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert!(parse_duration("soon").is_err());
    }

    #[test]
    fn parses_key_values() {
        let kv = parse_kv("head=abc\nbranch=\nnoise\n");
        assert_eq!(kv["head"], "abc");
        assert_eq!(kv["branch"], "");
        assert_eq!(kv.len(), 2);
    }
}
