//! Read-only file helpers shared by `harness` and `provenance`. Both read other tools' record
//! files, where a missing file means "no facts" and a malformed one is a reportable problem.

use std::io;
use std::path::{Path, PathBuf};

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

pub(crate) fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\??\")) {
        PathBuf::from(rest)
    } else {
        p
    }
}

/// Two paths with equal ids are one file system object. `Display` is the text `usage.db` stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FileId(pub(crate) u64, pub(crate) u64);

impl std::fmt::Display for FileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x}:{:x}", self.0, self.1)
    }
}

/// (volume serial, file index) of a file or folder.
#[cfg(windows)]
pub(crate) fn file_id(path: &Path) -> io::Result<FileId> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let handle = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data that the call fills in, and the handle
    // stays open for the duration of the call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle.as_raw_handle() as HANDLE, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileId(u64::from(info.dwVolumeSerialNumber), (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
}

/// (device, inode) of a file or folder. Follows links, like the Windows open does.
#[cfg(unix)]
pub(crate) fn file_id(path: &Path) -> io::Result<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok(FileId(meta.dev(), meta.ino()))
}
