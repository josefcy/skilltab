//! Sole owner of `<data_dir>\shelf.json` and the `<data_dir>\shelf\<id>` stash folders.
//! "Disabled" has one source of truth: an entry here.

use crate::model::{HarnessId, Held, ShelfEntry, ShelfId, ShelfReason, Timestamp};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) struct Shelf {
    data_dir: PathBuf,
    entries: Vec<ShelfEntry>,
}

const VERSION: u32 = 1;

impl Shelf {
    /// A missing file is an empty shelf. A file that cannot be read is an error, never an empty
    /// shelf: saving over it would lose the only record of where a removed link pointed.
    pub(crate) fn open(data_dir: &Path) -> io::Result<Shelf> {
        let entries = match crate::fsutil::read_text(&data_dir.join("shelf.json"))? {
            None => Vec::new(),
            Some(text) => parse(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("shelf.json: {e}")))?,
        };
        Ok(Shelf { data_dir: data_dir.to_path_buf(), entries })
    }

    pub(crate) fn entries(&self) -> &[ShelfEntry] {
        &self.entries
    }

    pub(crate) fn stash_path(&self, id: &ShelfId) -> PathBuf {
        self.data_dir.join("shelf").join(id.as_str())
    }

    /// Inserts or replaces the entry with this id, then persists before returning.
    pub(super) fn record(&mut self, entry: ShelfEntry) -> io::Result<()> {
        let mut next: Vec<ShelfEntry> = self.entries.iter().filter(|e| e.id != entry.id).cloned().collect();
        next.push(entry);
        self.save(next)
    }

    pub(super) fn forget(&mut self, id: &ShelfId) -> io::Result<()> {
        if !self.entries.iter().any(|e| &e.id == id) {
            return Ok(());
        }
        let next = self.entries.iter().filter(|e| &e.id != id).cloned().collect();
        self.save(next)
    }

    /// Write a temp file, flush it to disk, rename it over the old one. Memory changes only after that succeeds.
    fn save(&mut self, next: Vec<ShelfEntry>) -> io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let file = FileV1 { version: VERSION, entries: next.iter().map(EntryV1::from).collect() };
        let text = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
        let tmp = self.data_dir.join("shelf.json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.data_dir.join("shelf.json"))?;
        self.entries = next;
        Ok(())
    }
}

fn parse(text: &str) -> Result<Vec<ShelfEntry>, String> {
    let file: FileV1 = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("unsupported version {}, expected {VERSION}", file.version));
    }
    file.entries.into_iter().map(EntryV1::into_entry).collect()
}

#[derive(Serialize, Deserialize)]
struct FileV1 {
    version: u32,
    entries: Vec<EntryV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryV1 {
    id: String,
    harness: String,
    name: String,
    entry: PathBuf,
    held: HeldV1,
    reason: ReasonV1,
    at: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum HeldV1 {
    Link { target: PathBuf },
    Folder { stash: PathBuf },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ReasonV1 {
    Disabled,
    Removed,
}

impl From<&ShelfEntry> for EntryV1 {
    fn from(e: &ShelfEntry) -> EntryV1 {
        EntryV1 {
            id: e.id.as_str().to_owned(),
            harness: e.harness.as_str().to_owned(),
            name: e.name.clone(),
            entry: e.entry.clone(),
            held: match &e.held {
                Held::Link { target } => HeldV1::Link { target: target.clone() },
                Held::Folder { stash } => HeldV1::Folder { stash: stash.clone() },
            },
            reason: match e.reason {
                ShelfReason::Disabled => ReasonV1::Disabled,
                ShelfReason::Removed => ReasonV1::Removed,
            },
            at: e.at.to_rfc3339(),
        }
    }
}

impl EntryV1 {
    fn into_entry(self) -> Result<ShelfEntry, String> {
        let harness = HarnessId::parse(&self.harness).ok_or_else(|| format!("unknown harness {}", self.harness))?;
        let at: Timestamp = chrono::DateTime::parse_from_rfc3339(&self.at).map_err(|e| e.to_string())?.with_timezone(&chrono::Utc);
        Ok(ShelfEntry {
            id: ShelfId::new(self.id),
            harness,
            name: self.name,
            entry: self.entry,
            held: match self.held {
                HeldV1::Link { target } => Held::Link { target },
                HeldV1::Folder { stash } => Held::Folder { stash },
            },
            reason: match self.reason {
                ReasonV1::Disabled => ShelfReason::Disabled,
                ReasonV1::Removed => ShelfReason::Removed,
            },
            at,
        })
    }
}
