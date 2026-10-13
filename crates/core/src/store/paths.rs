use crate::sha256_hex;
use std::path::PathBuf;

const MAX_LEN: usize = 80;
const INVALID: &str = "<>:\"/\\|?*";
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Deterministic, Windows-safe directory/file component derived from `name`.
pub fn safe_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || INVALID.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let stem = s
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    // A leading "." is escaped too: names like ".account"/".folder" are reserved for the archive's
    // sidecar files (and dot names are hidden on Unix). The hash suffix keeps it collision-free.
    if s.is_empty() || s.starts_with('.') || RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    let truncated = s.chars().count() > MAX_LEN;
    if s == name && !truncated {
        return s;
    }
    let suffix = format!("-{}", &sha256_hex(name.as_bytes())[..8]);
    let keep = MAX_LEN - suffix.len();
    let mut base: String = s.chars().take(keep).collect();
    while base.ends_with('.') || base.ends_with(' ') {
        base.pop();
    }
    format!("{base}{suffix}")
}

/// Relative path (under the archive root) of the `.eml` for a message.
pub fn eml_relpath(account: &str, folder: &str, sha256: &str) -> PathBuf {
    let prefix: String = sha256.chars().take(2).collect();
    PathBuf::from("mail")
        .join(safe_component(account))
        .join(safe_component(folder))
        .join(prefix)
        .join(format!("{sha256}.eml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_component_replaces_windows_invalid_chars() {
        let a = safe_component("INBOX/a:b?c*");
        assert!(!a.contains(['/', ':', '?', '*']));
        assert_eq!(a, safe_component("INBOX/a:b?c*"));
    }

    #[test]
    fn safe_component_distinguishes_colliding_names() {
        assert_ne!(safe_component("a:b"), safe_component("a?b"));
    }

    #[test]
    fn safe_component_handles_edge_cases() {
        assert_eq!(safe_component("INBOX"), "INBOX");
        for bad in [
            "..",
            ".",
            "",
            "CON",
            "nul.txt",
            "trail. ",
            "a\u{1}b",
            &"x".repeat(200),
        ] {
            let s = safe_component(bad);
            assert!(!s.is_empty() && s != ".." && s != ".", "{bad:?} -> {s:?}");
            assert!(s.chars().count() <= 80);
            assert!(!s.ends_with('.') && !s.ends_with(' '));
            assert!(!s
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)));
            let stem = s.split('.').next().unwrap().to_ascii_uppercase();
            assert!(!RESERVED.contains(&stem.as_str()), "{s}");
        }
    }

    #[test]
    fn safe_component_never_starts_with_a_dot() {
        for name in [
            ".account",
            ".folder",
            ".account.tmp",
            ".Trash",
            ".x.",
            ". a",
        ] {
            let s = safe_component(name);
            assert!(!s.starts_with('.'), "{name:?} -> {s:?}");
            assert_eq!(s, safe_component(name), "deterministic");
            assert!(!s.chars().any(|c| "<>:\"/\\|?*".contains(c)));
        }
        assert_ne!(safe_component(".account"), safe_component("_.account"));
        assert_ne!(safe_component(".account"), safe_component(".account.tmp"));
    }

    #[test]
    fn eml_relpath_layout() {
        let p = eml_relpath("me", "INBOX", "abcdef");
        assert_eq!(
            p,
            PathBuf::from("mail")
                .join("me")
                .join("INBOX")
                .join("ab")
                .join("abcdef.eml")
        );
    }
}
