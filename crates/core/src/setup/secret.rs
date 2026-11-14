//! The secret patterns of `backup.ps1`. PowerShell's `-match` ignores case, so every pattern does here too.

use regex::{Regex, RegexBuilder};
use std::sync::LazyLock;

const PATTERNS: [&str; 8] = [
    "sk-[A-Za-z0-9_-]{20,}",
    "gh[pousr]_[A-Za-z0-9]{20,}",
    "github_pat_[A-Za-z0-9_]{20,}",
    "AKIA[0-9A-Z]{16}",
    "xox[abprs]-[A-Za-z0-9-]{10,}",
    r"Bearer\s+[A-Za-z0-9._~+/=-]{20,}",
    "-----BEGIN [A-Z ]*PRIVATE KEY-----",
    r#"(?i)(api[_-]?key|secret|token|passw(or)?d)["']?\s*[:=]\s*["']?[A-Za-z0-9._~+/=-]{16,}"#,
];

static COMPILED: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|p| (*p, RegexBuilder::new(p).case_insensitive(true).build().expect("a fixed pattern compiles")))
        .collect()
});

/// The first pattern the text matches. The pattern names the finding. The matched text is never returned,
/// so a plan can say what was found without printing it.
pub(crate) fn find_secret(bytes: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(bytes);
    COMPILED.iter().find(|(_, re)| re.is_match(&text)).map(|(p, _)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(n: usize) -> String {
        "a1B2".repeat(n).chars().take(n).collect()
    }

    #[test]
    fn each_of_the_eight_patterns_finds_its_secret() {
        let cases = [
            (format!("key = sk-{}", tail(24)), PATTERNS[0]),
            (format!("ghp_{}", tail(24)), PATTERNS[1]),
            (format!("github_pat_{}", tail(24)), PATTERNS[2]),
            ("AKIA0123456789ABCDEF".to_owned(), PATTERNS[3]),
            (format!("xoxb-{}", tail(12)), PATTERNS[4]),
            (format!("Authorization: Bearer {}", tail(24)), PATTERNS[5]),
            ("-----BEGIN RSA PRIVATE KEY-----\n".to_owned(), PATTERNS[6]),
            (format!("\"password\": \"{}\"", tail(20)), PATTERNS[7]),
        ];
        for (text, pattern) in cases {
            assert_eq!(find_secret(text.as_bytes()), Some(pattern), "{text}");
        }
    }

    #[test]
    fn matching_ignores_case_like_powershell() {
        assert!(find_secret(format!("SK-{}", tail(24)).as_bytes()).is_some());
        assert!(find_secret(b"akia0123456789abcdef").is_some());
        assert!(find_secret(format!("API_KEY={}", tail(20)).as_bytes()).is_some());
        assert!(find_secret(format!("BEARER {}", tail(24)).as_bytes()).is_some());
    }

    #[test]
    fn ordinary_instructions_are_not_secrets() {
        let text = "# Global\nUse short sentences.\nThe token budget is small, so keep the password rules short.\nsk-short\n";
        assert_eq!(find_secret(text.as_bytes()), None);
        assert_eq!(find_secret(&[0xff, 0xfe, 0x00]), None, "bytes that are not UTF-8 are scanned lossily");
    }
}
