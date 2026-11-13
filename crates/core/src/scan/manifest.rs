//! SKILL.md frontmatter. Only the top-level scalars skilltab shows are read. A real YAML
//! parser would reject files (an unquoted colon in a description) that Claude Code still loads,
//! so this reader is lenient on purpose.

use crate::model::{Manifest, ManifestIssue};

pub(super) fn parse(text: &str, folder: &str) -> Manifest {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();

    let front = match frontmatter_lines(&lines) {
        Some(f) => f,
        None => {
            return Manifest {
                name: folder.to_owned(),
                description: None,
                disable_model_invocation: false,
                issues: vec![ManifestIssue::NoFrontmatter],
            };
        }
    };

    let mut name = None;
    let mut description = None;
    let mut disable = false;
    let mut i = 0;
    while i < front.len() {
        let line = front[i];
        let mut j = i + 1;
        while j < front.len() && (front[j].trim().is_empty() || front[j].starts_with([' ', '\t'])) {
            j += 1;
        }
        let key_value = line.split_once(':').filter(|(k, _)| is_key(k));
        if let Some((key, rest)) = key_value {
            let value = scalar(rest.trim(), &front[i + 1..j]);
            match key {
                "name" => name = Some(value),
                "description" => description = Some(value),
                "disable-model-invocation" => disable = value.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
        i = j;
    }

    let mut issues = Vec::new();
    let name = name.filter(|n| !n.is_empty());
    match &name {
        None => issues.push(ManifestIssue::MissingName),
        Some(n) if n != folder => issues.push(ManifestIssue::NameDiffersFromFolder { folder: folder.to_owned() }),
        Some(_) => {}
    }
    let description = description.filter(|d| !d.is_empty());
    if description.is_none() {
        issues.push(ManifestIssue::MissingDescription);
    }
    Manifest { name: name.unwrap_or_else(|| folder.to_owned()), description, disable_model_invocation: disable, issues }
}

fn frontmatter_lines<'a>(lines: &[&'a str]) -> Option<Vec<&'a str>> {
    if lines.first()?.trim_end() != "---" {
        return None;
    }
    let end = lines[1..].iter().position(|l| matches!(l.trim_end(), "---" | "..."))?;
    Some(lines[1..1 + end].to_vec())
}

fn is_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn scalar(rest: &str, continuation: &[&str]) -> String {
    let cont: Vec<&str> = continuation.iter().map(|l| l.trim()).collect();
    match rest.chars().next() {
        Some(style @ ('|' | '>')) => block(style, continuation),
        Some('"') => double_quoted(&joined(rest, &cont)),
        Some('\'') => single_quoted(&joined(rest, &cont)),
        _ => {
            let plain = joined(rest, &cont);
            match plain.find(" #") {
                Some(at) => plain[..at].trim().to_owned(),
                None => plain,
            }
        }
    }
}

fn joined(first: &str, cont: &[&str]) -> String {
    std::iter::once(first).chain(cont.iter().copied()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

fn block(style: char, lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let stripped: Vec<&str> = lines.iter().map(|l| if l.len() >= indent { &l[indent..] } else { "" }).collect();
    let text = if style == '|' {
        stripped.join("\n")
    } else {
        let mut out = String::new();
        for l in &stripped {
            if l.trim().is_empty() {
                out.push('\n');
            } else {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(l.trim_end());
            }
        }
        out
    };
    text.trim().to_owned()
}

fn double_quoted(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            },
            c => out.push(c),
        }
    }
    out
}

fn single_quoted(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s[1..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
            } else {
                break;
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_quoted_scalars() {
        let m = parse("---\nname: tdd\ndescription: \"Test: driven\\\" dev\"\ndisable-model-invocation: true\n---\nbody", "tdd");
        assert_eq!(m.name, "tdd");
        assert_eq!(m.description.as_deref(), Some("Test: driven\" dev"));
        assert!(m.disable_model_invocation);
        assert!(m.issues.is_empty());
    }

    #[test]
    fn single_quoted_with_escaped_quote_and_crlf_and_bom() {
        let m = parse("\u{feff}---\r\nname: docs\r\ndescription: 'it''s here'\r\n---\r\n", "docs");
        assert_eq!(m.description.as_deref(), Some("it's here"));
        assert!(m.issues.is_empty());
    }

    #[test]
    fn folded_block_and_multiline_plain() {
        let m = parse("---\nname: a\ndescription: >\n  one\n  two\n\n  three\nother: x\n---\n", "a");
        assert_eq!(m.description.as_deref(), Some("one two\nthree"));
        let m = parse("---\nname: a\ndescription: first\n  second # note\n---\n", "a");
        assert_eq!(m.description.as_deref(), Some("first second"));
    }

    #[test]
    fn missing_pieces_are_issues_not_failures() {
        let m = parse("no frontmatter", "folder");
        assert_eq!(m.name, "folder");
        assert_eq!(m.issues, vec![ManifestIssue::NoFrontmatter]);

        let m = parse("---\ndescription: d\n---\n", "folder");
        assert_eq!(m.name, "folder");
        assert_eq!(m.issues, vec![ManifestIssue::MissingName]);

        let m = parse("---\nname: Other\n---\n", "folder");
        assert_eq!(
            m.issues,
            vec![ManifestIssue::NameDiffersFromFolder { folder: "folder".into() }, ManifestIssue::MissingDescription]
        );
    }

    #[test]
    fn unterminated_frontmatter_counts_as_none() {
        assert_eq!(parse("---\nname: a\n", "a").issues, vec![ManifestIssue::NoFrontmatter]);
    }
}
