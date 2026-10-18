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

    pub fn read_u8(&self, address: u64) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_into(address, &mut buf)?;
        Ok(buf[0])
    }

    pub fn read_f32(&self, address: u64) -> io::Result<f32> {
        let mut buf = [0u8; 4];
        self.read_into(address, &mut buf)?;
        Ok(f32::from_le_bytes(buf))
    }

    pub fn read_f64(&self, address: u64) -> io::Result<f64> {
        let mut buf = [0u8; 8];
        self.read_into(address, &mut buf)?;
        Ok(f64::from_le_bytes(buf))
    }

    pub fn read_i64(&self, address: u64) -> io::Result<i64> {
        let mut buf = [0u8; 8];
        self.read_into(address, &mut buf)?;
        Ok(i64::from_le_bytes(buf))
    }

    /// Read the game's 16-byte `Quad`: mantissa, then binary exponent.
    ///
    /// The value is `(mantissa + 2^63) * 2^exponent`. The bias makes the
    /// mantissa unsigned in effect, which is how Script Hub reads it too.
    pub fn read_quad(&self, address: u64) -> io::Result<Quad> {
        Ok(Quad {
            mantissa: self.read_i64(address)?,
            exponent: self.read_i64(address + 8)?,
        })
    }

    /// Read a Mono string through the pointer stored at `address`.
    ///
    /// Layout: the object header is `0x10` bytes, then a 4-byte length (in
    /// UTF-16 code units), then the characters. An implausible length means we
    /// are not looking at a string object, which is worth saying out loud
    /// rather than returning the mojibake it would decode to.
    pub fn read_mono_string(&self, address: u64) -> io::Result<String> {
        const MAX_UNITS: i32 = 4096;

        let object = self.read_u64(address)?;
        if object == 0 {
            return Err(io::Error::other("null string pointer"));
        }
        let units = self.read_i32(object + 0x10)?;
        if !(0..=MAX_UNITS).contains(&units) {
            return Err(io::Error::other(format!(
                "string at {object:#x} claims length {units} - not a Mono string"
            )));
        }
        let mut bytes = vec![0u8; units as usize * 2];
        self.read_into(object + 0x14, &mut bytes)?;
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&wide))
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

    /// Walk the chain recording every hop, stopping at the first failure.
    ///
    /// `resolve` names the step that broke; this shows the addresses either
    /// side of it, which is what distinguishes "the static address moved"
    /// (dies at hop 0 or 1) from "a field moved in this build" (dies deep,
    /// after plausible pointers).
    pub fn trace(&self, base: u64, offsets: &[u64]) -> Vec<Hop> {
        let mut hops = Vec::with_capacity(offsets.len());
        let mut addr = base;
        for &offset in offsets {
            let read = self.read_u64(addr);
            let hop = Hop {
                at: addr,
                offset,
                value: read.as_ref().ok().copied(),
            };
            let broken = hop.value.is_none_or(|value| value == 0);
            hops.push(hop);
            if broken {
                break;
            }
            addr = hops.last().unwrap().value.unwrap() + offset;
        }
        hops
    }
}

/// The game's big-number type, kept symbolic because the value it represents
/// routinely exceeds `f64`'s range at deep zones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub mantissa: i64,
    pub exponent: i64,
}

impl Quad {
    /// Base-10 logarithm of the value, which stays finite however large it is.
    pub fn log10(&self) -> f64 {
        const BIAS: f64 = 9_223_372_036_854_775_808.0; // 2^63
        (self.mantissa as f64 + BIAS).log10() + self.exponent as f64 * std::f64::consts::LN_2 / std::f64::consts::LN_10
    }
}

impl std::fmt::Display for Quad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let decimated = self.log10();
        if !decimated.is_finite() {
            return write!(f, "{}*2^{}", self.mantissa, self.exponent);
        }
        if decimated <= 4.0 {
            const BIAS: f64 = 9_223_372_036_854_775_808.0;
            let value = (self.mantissa as f64 + BIAS) * 2f64.powf(self.exponent as f64);
            return write!(f, "{value:.2}");
        }
        let exponent = decimated.floor();
        let significand = 10f64.powf(decimated - exponent);
        write!(f, "{significand:.2}e{exponent:.0}")
    }
}

/// One dereference in a pointer walk: what was read, and from where.
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub struct Hop {
    /// The address dereferenced.
    pub at: u64,
    /// The offset added to the result.
    pub offset: u64,
    /// The pointer read, or `None` if the address was not readable.
    pub value: Option<u64>,
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
