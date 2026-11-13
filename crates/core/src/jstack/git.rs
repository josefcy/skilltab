//! The only place that starts `git`. Every call is read-only except `fetch`, and each one is a
//! separate argument vector, never a shell string.

use super::JstackError;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What one `git status --porcelain=v2 --branch` call says about the repo.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Status {
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<(String, u32, u32)>,
    /// Repo-relative, forward-slash paths. Renames list both ends.
    pub dirty: Vec<String>,
}

pub(super) fn run(root: &Path, args: &[&str]) -> Result<String, JstackError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        // A status must never take index.lock, and a fetch must fail instead of waiting for a password.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| git_error(args, format!("could not run git: {e}")))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if stderr.contains("not a git repository") {
        return Err(JstackError::NotAGitRepo(root.to_path_buf()));
    }
    Err(git_error(args, stderr))
}

/// Like `run`, but a non-zero exit is an answer ("not set"), not a failure.
pub(super) fn run_optional(root: &Path, args: &[&str]) -> Result<Option<String>, JstackError> {
    match run(root, args) {
        Ok(text) => Ok(Some(text.trim().to_owned()).filter(|t| !t.is_empty())),
        Err(JstackError::Git { message, .. }) if message.is_empty() => Ok(None),
        Err(e) => Err(e),
    }
}

fn git_error(args: &[&str], message: String) -> JstackError {
    JstackError::Git { command: format!("git {}", args.join(" ")), message }
}

/// `-z` output is NUL-separated and never quotes paths.
pub(super) fn parse_status(text: &str) -> Status {
    let mut st = Status::default();
    let mut upstream_name = None;
    let mut ahead_behind = None;
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, value) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" if value != "(initial)" => st.head = Some(value.to_owned()),
                "branch.head" if value != "(detached)" => st.branch = Some(value.to_owned()),
                "branch.upstream" => upstream_name = Some(value.to_owned()),
                "branch.ab" => ahead_behind = parse_ab(value),
                _ => {}
            }
            continue;
        }
        let (kind, rest) = rec.split_at(1);
        // 1 XY sub mH mI mW hH hI <path>
        // 2 XY sub mH mI mW hH hI Xscore <path> NUL <orig>
        // u XY sub m1 m2 m3 mW h1 h2 h3 <path>
        let path = match kind {
            "1" => rest.trim_start().splitn(8, ' ').nth(7),
            "2" => rest.trim_start().splitn(9, ' ').nth(8),
            "u" => rest.trim_start().splitn(10, ' ').nth(9),
            "?" => Some(rest.trim_start()),
            _ => None,
        };
        if let Some(p) = path {
            st.dirty.push(p.to_owned());
        }
        if kind == "2" {
            if let Some(orig) = records.next() {
                st.dirty.push(orig.to_owned());
            }
        }
    }
    if let (Some(name), Some((ahead, behind))) = (upstream_name, ahead_behind) {
        st.upstream = Some((name, ahead, behind));
    }
    st
}

fn parse_ab(value: &str) -> Option<(u32, u32)> {
    let (a, b) = value.split_once(' ')?;
    Some((a.strip_prefix('+')?.parse().ok()?, b.strip_prefix('-')?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_v2_reads_branch_headers_and_every_kind_of_dirty_entry() {
        let text = [
            "# branch.oid 0123abc",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +1 -2",
            "1 .M N... 100644 100644 100644 aaa bbb skills/unslop/SKILL.md",
            "2 R. N... 100644 100644 100644 aaa bbb R100 skills/new/SKILL.md",
            "skills/old/SKILL.md",
            "? skills/fresh/notes with space.md",
            "",
        ]
        .join("\0");
        let st = parse_status(&text);
        assert_eq!(st.head.as_deref(), Some("0123abc"));
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream, Some(("origin/main".into(), 1, 2)));
        assert_eq!(
            st.dirty,
            ["skills/unslop/SKILL.md", "skills/new/SKILL.md", "skills/old/SKILL.md", "skills/fresh/notes with space.md"]
        );
    }

    #[test]
    fn detached_head_without_upstream_has_no_branch_and_no_upstream() {
        let st = parse_status("# branch.oid 0123abc\0# branch.head (detached)\0");
        assert_eq!(st.branch, None);
        assert_eq!(st.upstream, None);
    }

    #[test]
    fn upstream_whose_remote_branch_is_gone_is_unknown() {
        let st = parse_status("# branch.oid 0123abc\0# branch.head main\0# branch.upstream origin/main\0");
        assert_eq!(st.upstream, None);
    }
}
