//! Reading another process's memory.
//!
//! On Linux this works because we run inside the game's Proton container and
//! therefore share its wineserver: to Wine we are simply another Win32
//! process, so `ReadProcessMemory` behaves exactly as it does on Windows.

use std::io;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

/// An open handle to the game, with read access to its address space.
#[cfg(windows)]
pub struct Process {
    handle: HANDLE,
    pid: u32,
}

#[cfg(windows)]
impl Process {
    pub fn open(pid: u32) -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        };
        // SAFETY: a plain Win32 call; the returned handle is checked and is
        // closed in Drop.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, pid })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Fill `buf` from `address`. A short read is an error: a partially filled
    /// buffer is indistinguishable from real data once it reaches a caller.
    pub fn read_into(&self, address: u64, buf: &mut [u8]) -> io::Result<()> {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

        let mut read = 0usize;
        // SAFETY: buf is a valid, writable slice of exactly `buf.len()` bytes,
        // which is the count handed to the call.
        let ok = unsafe {
            ReadProcessMemory(
                self.handle,
                address as *const core::ffi::c_void,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut read,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if read != buf.len() {
            return Err(io::Error::other(format!(
                "short read at {address:#x}: got {read} of {} bytes",
                buf.len()
            )));
        }
        Ok(())
    }

    pub fn read_u64(&self, address: u64) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        self.read_into(address, &mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    pub fn read_i32(&self, address: u64) -> io::Result<i32> {
        let mut buf = [0u8; 4];
        self.read_into(address, &mut buf)?;
        Ok(i32::from_le_bytes(buf))
    }

    /// Load address of a module (e.g. `mono-2.0-bdwgc.dll`) in this process.
    pub fn module_base(&self, module_name: &str) -> io::Result<u64> {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
            TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
        };

        // SAFETY: snapshot handle is validated and closed on every path.
        unsafe {
            let snapshot =
                CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, self.pid);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }

            let mut entry = MODULEENTRY32W {
                dwSize: size_of::<MODULEENTRY32W>() as u32,
                ..zeroed()
            };
            if Module32FirstW(snapshot, &mut entry) == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(snapshot);
                return Err(err);
            }

            loop {
                if wide_to_string(&entry.szModule).eq_ignore_ascii_case(module_name) {
                    let base = entry.modBaseAddr as u64;
                    CloseHandle(snapshot);
                    return Ok(base);
                }
                if Module32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
            CloseHandle(snapshot);
        }
        Err(io::Error::other(format!("module not found: {module_name}")))
    }

    /// Walk a pointer chain and return the address of the final value.
    ///
    /// Each step dereferences the current address and adds the next offset -
    /// the same rule as Script Hub's `classMemory.pointer()`, which is where
    /// the published offsets come from:
    ///
    /// ```text
    /// addr = base
    /// for off in offsets:  addr = [addr] + off
    /// ```
    ///
    /// A null dereference is reported with the step that produced it, because
    /// "which link went stale after the update" is the question we will be
    /// asking every time the game patches.
    pub fn resolve(&self, base: u64, offsets: &[u64]) -> io::Result<u64> {
        let mut addr = base;
        for (step, &offset) in offsets.iter().enumerate() {
            let next = self.read_u64(addr).map_err(|err| {
                io::Error::other(format!("step {step} (+{offset:#x}) at {addr:#x}: {err}"))
            })?;
            if next == 0 {
                return Err(io::Error::other(format!(
                    "null pointer at step {step} (+{offset:#x}) - chain is stale or \
                     the game has not loaded this structure yet"
                )));
            }
            addr = next + offset;
        }
        Ok(addr)
    }
}

#[cfg(windows)]
impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: handle came from OpenProcess and is closed exactly once.
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(windows)]
fn wide_to_string(raw: &[u16]) -> String {
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..len])
}
