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
/// so a plan can say what was found without printing it. The bytes are read as UTF-8 and also as
/// `ReadAllText` reads them, which honours a byte order mark, so a UTF-16 file cannot hide a secret.
pub(crate) fn find_secret(bytes: &[u8]) -> Option<&'static str> {
    let views = [Some(String::from_utf8_lossy(bytes).into_owned()), bom_text(bytes)];
    COMPILED.iter().find(|(_, re)| views.iter().flatten().any(|text| re.is_match(text))).map(|(p, _)| *p)
}

/// The text of bytes that start with a byte order mark, decoded as that mark says. None without one.
fn bom_text(bytes: &[u8]) -> Option<String> {
    let utf32 = |body: &[u8], join: fn([u8; 4]) -> u32| -> String {
        body.as_chunks::<4>().0.iter().map(|c| char::from_u32(join(*c)).unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    };
    let utf16 = |body: &[u8], join: fn([u8; 2]) -> u16| -> String {
        let units = body.as_chunks::<2>().0.iter().map(|c| join(*c));
        char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    };
    match bytes {
        [0xff, 0xfe, 0, 0, body @ ..] => Some(utf32(body, u32::from_le_bytes)),
        [0xff, 0xfe, body @ ..] => Some(utf16(body, u16::from_le_bytes)),
        [0xfe, 0xff, body @ ..] => Some(utf16(body, u16::from_be_bytes)),
        [0, 0, 0xfe, 0xff, body @ ..] => Some(utf32(body, u32::from_be_bytes)),
        [0xef, 0xbb, 0xbf, body @ ..] => Some(String::from_utf8_lossy(body).into_owned()),
        _ => None,
    }
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

    fn encode(text: &str, bom: &[u8], unit: impl Fn(char) -> Vec<u8>) -> Vec<u8> {
        let mut out = bom.to_vec();
        out.extend(text.chars().flat_map(unit));
        out
    }

    fn utf16(c: char, big: bool) -> Vec<u8> {
        let mut buf = [0u16; 2];
        c.encode_utf16(&mut buf).iter().flat_map(|u| if big { u.to_be_bytes() } else { u.to_le_bytes() }).collect()
    }

    #[test]
    fn a_secret_in_a_file_with_a_byte_order_mark_is_found_as_read_all_text_would_read_it() {
        let text = format!("token: ghp_{}
", tail(24));
        let files = [
            ("utf-8", [&[0xef, 0xbb, 0xbf][..], text.as_bytes()].concat()),
            ("utf-16 le", encode(&text, &[0xff, 0xfe], |c| utf16(c, false))),
            ("utf-16 be", encode(&text, &[0xfe, 0xff], |c| utf16(c, true))),
            ("utf-32 le", encode(&text, &[0xff, 0xfe, 0, 0], |c| u32::from(c).to_le_bytes().to_vec())),
            ("utf-32 be", encode(&text, &[0, 0, 0xfe, 0xff], |c| u32::from(c).to_be_bytes().to_vec())),
        ];
        for (name, bytes) in files {
            assert_eq!(find_secret(&bytes), Some(PATTERNS[1]), "{name}");
        }
        let plain = encode("# Rules
Use short sentences.
", &[0xff, 0xfe], |c| utf16(c, false));
        assert_eq!(find_secret(&plain), None);
    }

    #[test]
    fn ordinary_instructions_are_not_secrets() {
        let text = "# Global\nUse short sentences.\nThe token budget is small, so keep the password rules short.\nsk-short\n";
        assert_eq!(find_secret(text.as_bytes()), None);
        assert_eq!(find_secret(&[0xff, 0xfe, 0x00]), None, "bytes that are not UTF-8 are scanned lossily");
    }
}
