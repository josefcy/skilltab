//! Read-only file helpers shared by `harness` and `provenance`. Both read other tools' record
//! files, where a missing file means "no facts" and a malformed one is a reportable problem.

use std::io;
use std::path::Path;

/// `Ok(None)` when the file does not exist. A UTF-8 BOM is dropped (PowerShell 5 writes one).
pub(crate) fn read_text(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            Ok(Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn read_json(path: &Path) -> Result<Option<serde_json::Value>, String> {
    match read_text(path).map_err(|e| e.to_string())? {
        None => Ok(None),
        Some(t) => serde_json::from_str(&t).map(Some).map_err(|e| format!("invalid JSON: {e}")),
    }
}
