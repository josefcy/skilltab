//! The only code that calls Win32 file APIs. Private to `manage`, so nothing can change the disk
//! without going through a plan.

use crate::fsutil::strip_verbatim;
use crate::model::{LinkPath, RealDir};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub(crate) const TRASH_NAME: &str = "Recycle Bin";

fn wide(path: &Path, extra_nuls: usize) -> Vec<u16> {
    let mut w: Vec<u16> = path.as_os_str().encode_wide().collect();
    w.extend(std::iter::repeat_n(0, 1 + extra_nuls));
    w
}

/// Deletes the reparse point itself. RemoveDirectoryW never follows a junction, so the target is untouched.
pub(super) fn remove_link(link: &LinkPath) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::RemoveDirectoryW;
    let path = wide(link.as_path(), 0);
    // SAFETY: `path` is a NUL-terminated UTF-16 buffer that outlives the call.
    if unsafe { RemoveDirectoryW(path.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Sends the folder to the Recycle Bin. FOF_WANTNUKEWARNING makes the shell ask before it would
/// delete permanently (a folder too large for the bin, a drive without one) instead of doing it silently.
pub(super) fn recycle(dir: &RealDir) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FO_DELETE,
        SHFILEOPSTRUCTW,
    };
    // pFrom is a list of paths, so it ends with two NULs.
    let from = wide(dir.as_path(), 1);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING) as u16,
        ..Default::default()
    };
    // SAFETY: `op` points at buffers that outlive the call, and no window handle is passed.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(io::Error::other(format!("the shell could not recycle the folder (code {code:#x})")));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(io::Error::other("recycling was cancelled"));
    }
    Ok(())
}

/// Creates `entry` as an NTFS junction to the absolute `target`. std has no API for this, and shelling out to
/// `mklink` would pass the paths through cmd's parser.
pub(super) fn create_link(entry: &Path, target: &Path) -> io::Result<()> {
    std::fs::create_dir(entry)?;
    let set = set_mount_point(entry, target);
    if set.is_err() {
        let _ = std::fs::remove_dir(entry);
    }
    set
}

fn set_mount_point(entry: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;

    let target = strip_verbatim(target.to_path_buf());
    if !target.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a junction target must be absolute"));
    }
    let print: Vec<u16> = target.as_os_str().encode_wide().collect();
    let substitute: Vec<u16> = r"\??\".encode_utf16().chain(print.iter().copied()).collect();
    let sub_bytes = (substitute.len() * 2) as u16;
    let print_bytes = (print.len() * 2) as u16;
    // Header after the tag/length/reserved fields: four u16 offsets and lengths, then both names NUL-terminated.
    let data_len = 8 + sub_bytes + 2 + print_bytes + 2;

    let mut buf: Vec<u8> = Vec::with_capacity(8 + data_len as usize);
    buf.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf.extend(data_len.to_le_bytes());
    buf.extend(0u16.to_le_bytes());
    buf.extend(0u16.to_le_bytes());
    buf.extend(sub_bytes.to_le_bytes());
    buf.extend((sub_bytes + 2).to_le_bytes());
    buf.extend(print_bytes.to_le_bytes());
    for unit in substitute.iter().chain([&0]).chain(print.iter()).chain([&0]) {
        buf.extend(unit.to_le_bytes());
    }

    let dir = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(entry)?;
    let mut returned = 0u32;
    // SAFETY: `buf` holds a complete REPARSE_DATA_BUFFER of the length passed, and the handle stays open for the call.
    let ok = unsafe {
        DeviceIoControl(
            dir.as_raw_handle() as HANDLE,
            FSCTL_SET_REPARSE_POINT,
            buf.as_ptr().cast(),
            buf.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The file names a program can have on PATH. `npx` is `npx.cmd`, which only a `.cmd` search finds.
pub(super) fn tool_names(program: &str) -> Vec<String> {
    ["exe", "cmd", "bat"].iter().map(|ext| format!("{program}.{ext}")).collect()
}

pub(super) fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// A GUI app on Windows gets the user's whole PATH, so nothing is added.
pub(super) fn extra_tool_dirs(_home: Option<&Path>) -> Vec<PathBuf> {
    Vec::new()
}
