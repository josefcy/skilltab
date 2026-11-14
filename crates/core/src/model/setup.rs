//! The setup manifest: what a machine needs to get its skills back. Plain data with no I/O.
//! A manifest comes from a cloned repo, so every string that later becomes a command argument
//! or a path segment is validated once, here, and carried as a type that cannot hold a bad one.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// `backup.ps1` sorts with `StringComparer.Ordinal`, which compares UTF-16 code units. That differs
/// from UTF-8 byte order for characters above U+FFFF, so the order is spelled out.
fn ordinal(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// One command argument. Not empty, no control characters, and no leading dash, so it can never read as a flag.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Arg(String);

impl Arg {
    pub(crate) fn parse(s: &str) -> Result<Arg, String> {
        if s.is_empty() {
            return Err("is empty".to_owned());
        }
        if s.chars().any(char::is_control) {
            return Err(format!("\"{}\" holds a control character", s.escape_debug()));
        }
        if s.starts_with('-') {
            return Err(format!("\"{s}\" starts with a dash, so a command would read it as an option"));
        }
        Ok(Arg(s.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A folder name that is safe to join onto a path: an `Arg` with no separator and not `.` or `..`.
/// A colon is out too, because on Windows `C:x` joins as a path on another drive.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FolderName(String);

impl FolderName {
    pub(crate) fn parse(s: &str) -> Result<FolderName, String> {
        Arg::parse(s)?;
        if s == "." || s == ".." || s.contains(['/', '\\', ':']) {
            return Err(format!("\"{s}\" is not a plain folder name"));
        }
        Ok(FolderName(s.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! ordinal_order {
    ($t:ty) => {
        impl Ord for $t {
            fn cmp(&self, other: &Self) -> Ordering {
                ordinal(&self.0, &other.0)
            }
        }
        impl PartialOrd for $t {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
    };
}
ordinal_order!(Arg);
ordinal_order!(FolderName);

/// The only two marketplace sources the format can rebuild.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarketplaceSource {
    Github { repo: Arg },
    Git { url: Arg },
}

/// The jstack `manifest.json`. Sorted and unique by type, so rendering needs no sort call and
/// parse then render gives the same value back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupManifest {
    pub marketplaces: BTreeMap<Arg, MarketplaceSource>,
    /// "name@marketplace". `claudePlugins` on the wire, because only Claude Code contributes plugins.
    pub plugins: BTreeSet<Arg>,
    /// Skills CLI source to the skill names it installs.
    pub skill_sources: BTreeMap<Arg, BTreeSet<FolderName>>,
    /// Folder names directly under `<setup folder>/skills`.
    pub own_skills: BTreeSet<FolderName>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arg_cannot_read_as_a_flag_or_hold_control_characters() {
        assert!(Arg::parse("acme/skills").is_ok());
        assert!(Arg::parse("https://example.test/a b").is_ok());
        for bad in ["", "-y", "--yes", "a\nb", "a\u{7f}"] {
            assert!(Arg::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_folder_name_cannot_leave_its_folder() {
        assert!(FolderName::parse("my-skill").is_ok());
        for bad in ["", ".", "..", "a/b", r"a\b", "C:x", "-x"] {
            assert!(FolderName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_sort_by_utf16_code_unit_like_powershell_ordinal() {
        let mut names = [FolderName::parse("\u{1F600}").unwrap(), FolderName::parse("\u{FF5E}").unwrap(), FolderName::parse("Z").unwrap()];
        names.sort();
        let sorted: Vec<&str> = names.iter().map(FolderName::as_str).collect();
        assert_eq!(sorted, ["Z", "\u{1F600}", "\u{FF5E}"], "a surrogate pair sorts before U+FF5E");
    }
}
