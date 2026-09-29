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

pub fn sha256_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(sha256_bytes(&data))
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
    Ok(n * mult)
}

/// Parse "90s", "30m", "4h", or a plain number of seconds.
pub fn parse_duration(s: &str) -> Result<u64> {
    let t = s.trim().to_ascii_lowercase();
    let (num, mult) = [("h", 3600u64), ("m", 60), ("s", 1)]
        .iter()
        .find_map(|(suf, m)| t.strip_suffix(suf).map(|n| (n.trim().to_string(), *m)))
        .unwrap_or((t.clone(), 1));
    let n: u64 = num.parse().with_context(|| format!("bad duration: {s}"))?;
    Ok(n * mult)
}

pub fn human_size(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
