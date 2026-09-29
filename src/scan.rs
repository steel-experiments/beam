// ABOUTME: Finds text that looks like a secret in files that beam uploads.
// ABOUTME: The result is only a warning for the user. It does not block the upload.

/// (label, marker) pairs. A marker is a prefix that real secrets of that kind have.
const MARKERS: &[(&str, &str)] = &[
    ("Anthropic key", "sk-ant-"),
    ("OpenAI key", "sk-proj-"),
    ("GitHub token", "ghp_"),
    ("GitHub token", "github_pat_"),
    ("GitHub token", "gho_"),
    ("AWS access key", "AKIA"),
    ("Slack token", "xoxb-"),
    ("Slack token", "xoxp-"),
    ("private key", "-----BEGIN"),
];

/// Count the markers in `text`. Returns (label, count) for each label that has hits.
pub fn scan(text: &str) -> Vec<(&'static str, usize)> {
    let mut out: Vec<(&'static str, usize)> = vec![];
    for (label, marker) in MARKERS {
        let n = text.matches(marker).count();
        if n == 0 {
            continue;
        }
        match out.iter_mut().find(|(l, _)| l == label) {
            Some(e) => e.1 += n,
            None => out.push((label, n)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_markers_by_label() {
        let t = "token ghp_abc and github_pat_x and sk-ant-123";
        assert_eq!(scan(t), vec![("Anthropic key", 1), ("GitHub token", 2)]);
        assert!(scan("nothing to see").is_empty());
    }
}
