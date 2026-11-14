//! Policy for the setup plans. Pure: it turns the input the `setup` module read into steps, notes and blockers.

use super::{Blocker, Note, NoteKind, Plan, Step, Was};
use crate::model::ContentHash;
use crate::setup::{find_secret, ExportInput};
use std::path::PathBuf;
use std::sync::Arc;

impl Plan {
    fn note(&mut self, kind: NoteKind, text: String) {
        self.notes.push(Note { kind, text });
    }
}

/// Writes `manifest.json` and the allowlisted config files into the setup folder, and deletes the copies of
/// files that left the machine. Anything that is already as wanted becomes a Present note with no step.
/// An unreadable input or a secret blocks the whole export, so a half-read machine never reaches the folder.
pub(super) fn plan_export(i: &ExportInput, p: &mut Plan) {
    for (name, kind) in &i.unrebuildable {
        p.note(
            NoteKind::Attention,
            format!("Marketplace {name} has a source of type \"{kind}\", which restore cannot rebuild. It is left out of manifest.json."),
        );
    }
    for problem in &i.problems {
        p.block(Blocker::Unsupported { why: problem.why.clone(), remedy: problem.remedy.clone() });
    }
    for file in &i.sources {
        if let Some(pattern) = find_secret(&file.bytes) {
            p.block(Blocker::Unsupported {
                why: format!("{} looks like it holds a secret. It matches the pattern {pattern}.", file.label()),
                remedy: "Remove the secret from that file, then plan the export again.".to_owned(),
            });
        }
    }
    if !p.blockers.is_empty() {
        return;
    }

    let manifest: Arc<[u8]> = i.manifest.render().into_bytes().into();
    put(p, i.dir.manifest_path(), manifest, i.on_disk_manifest.as_deref(), "manifest.json");
    for file in &i.sources {
        let have = i.in_repo.iter().find(|r| r.same_file(file)).map(|r| &*r.bytes);
        put(p, file.repo_path(&i.dir), file.bytes.clone(), have, &file.label());
    }
    for gone in i.in_repo.iter().filter(|r| !i.sources.iter().any(|s| s.same_file(r))) {
        p.steps.push(Step::Delete { path: gone.repo_path(&i.dir), was: ContentHash::of(&gone.bytes) });
    }
}

/// One file the export wants in the setup folder, against what is there now.
fn put(p: &mut Plan, to: PathBuf, bytes: Arc<[u8]>, have: Option<&[u8]>, what: &str) {
    match have {
        Some(old) if old == &*bytes => p.note(NoteKind::Present, format!("{what} is already up to date in the setup folder.")),
        Some(old) => p.steps.push(Step::Write { to, bytes, was: Was::Hash(ContentHash::of(old)) }),
        None => p.steps.push(Step::Write { to, bytes, was: Was::Absent }),
    }
}
