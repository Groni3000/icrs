//! Finding the game process.

use std::io;

/// PIDs of every running process whose image name matches `exe_name`
/// (case-insensitively), in the order the OS reports them.
///
/// Wine processes appear here exactly like native ones: the Win32 process list
/// inside the prefix is the one we are querying, so `IdleDragons.exe` running
/// under Proton is visible as long as we share its wineserver.
#[cfg(windows)]
pub fn find_pids(exe_name: &str) -> io::Result<Vec<u32>> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: a snapshot handle is the documented way to walk the process list;
    // every raw call below is checked and the handle is closed on every path.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..std::mem::zeroed()
        };

        // Process32FirstW fails rather than returning an empty list, so a
        // failure here is a real error, not "no processes".
        if Process32FirstW(snapshot, &mut entry) == 0 {
            let err = io::Error::last_os_error();
            CloseHandle(snapshot);
            return Err(err);
        }

        let mut pids = Vec::new();
        loop {
            if image_name(&entry.szExeFile).eq_ignore_ascii_case(exe_name) {
                pids.push(entry.th32ProcessID);
            }
            if Process32NextW(snapshot, &mut entry) == 0 {
                break; // ERROR_NO_MORE_FILES - the normal end of the walk
            }
        }

        CloseHandle(snapshot);
        Ok(pids)
    }
}

/// Decode a fixed-size, NUL-terminated UTF-16 image name.
#[cfg(windows)]
fn image_name(raw: &[u16]) -> String {
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..len])
}

#[cfg(not(windows))]
pub fn find_pids(_exe_name: &str) -> io::Result<Vec<u32>> {
    Err(io::Error::other(
        "icrs must be built for Windows (--target x86_64-pc-windows-gnu); \
         on Linux run the resulting .exe with scripts/run-in-prefix.sh",
    ))
}
