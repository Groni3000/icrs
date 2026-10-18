//! Sending keystrokes to the game.
//!
//! Keys go to the game's window as posted `WM_KEYDOWN`/`WM_KEYUP` messages,
//! not as global input. That is the whole reason this program is a Windows
//! binary living on the game's wineserver: messages land in the game's own
//! queue, so they work while the window is unfocused and they never touch
//! whatever you are actually typing in. Tools that drive X11 instead (XTEST)
//! can only type into the focused window and steal focus to do it.

use std::io;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};

/// A key as the game sees it: a virtual-key code plus the hardware scan code
/// Unity expects packed into `lParam`.
#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub name: &'static str,
    pub vk: u16,
    pub scan: u16,
}

pub const ESC: Key = Key { name: "esc", vk: 0x1B, scan: 1 };
pub const Q: Key = Key { name: "q", vk: 0x51, scan: 16 };
pub const W: Key = Key { name: "w", vk: 0x57, scan: 17 };
pub const E: Key = Key { name: "e", vk: 0x45, scan: 18 };

/// The number row. The ultimates bar labels each slot with one of these - the
/// `Hotkey` field on an `UltimatesBarItem` is that number - so this is how an
/// ultimate gets fired.
pub const D1: Key = Key { name: "1", vk: 0x31, scan: 2 };
pub const D2: Key = Key { name: "2", vk: 0x32, scan: 3 };
pub const D3: Key = Key { name: "3", vk: 0x33, scan: 4 };
pub const D4: Key = Key { name: "4", vk: 0x34, scan: 5 };
pub const D5: Key = Key { name: "5", vk: 0x35, scan: 6 };
pub const D6: Key = Key { name: "6", vk: 0x36, scan: 7 };
pub const D7: Key = Key { name: "7", vk: 0x37, scan: 8 };
pub const D8: Key = Key { name: "8", vk: 0x38, scan: 9 };
pub const D9: Key = Key { name: "9", vk: 0x39, scan: 10 };
pub const D0: Key = Key { name: "0", vk: 0x30, scan: 11 };

pub const KEYS: &[Key] =
    &[ESC, Q, W, E, D1, D2, D3, D4, D5, D6, D7, D8, D9, D0];

/// The key that fires the ultimate in a given bar slot, as the game labels it.
pub fn hotkey(number: i64) -> Option<Key> {
    KEYS.iter().copied().find(|key| key.name == number.to_string())
}

pub fn key_by_name(name: &str) -> Option<Key> {
    KEYS.iter().copied().find(|k| k.name.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
pub struct Window {
    hwnd: HWND,
}

#[cfg(windows)]
impl Window {
    /// The game's top-level visible window.
    pub fn for_process(pid: u32) -> io::Result<Self> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
        };

        struct Search {
            pid: u32,
            found: HWND,
        }

        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // SAFETY: lparam is the &mut Search we passed to EnumWindows, which
            // outlives the enumeration.
            let search = unsafe { &mut *(lparam as *mut Search) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid == search.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
                search.found = hwnd;
                return 0; // stop enumerating
            }
            1 // keep going
        }

        let mut search = Search { pid, found: std::ptr::null_mut() };
        // SAFETY: visit only touches the Search behind the pointer we pass.
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };

        if search.found.is_null() {
            return Err(io::Error::other(format!("no visible window for pid {pid}")));
        }
        Ok(Self { hwnd: search.found })
    }

    pub fn is_foreground(&self) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        // SAFETY: plain query, no arguments.
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    /// Tell the window it is active without taking focus.
    ///
    /// Unity stops consuming posted key messages once it believes it is not
    /// the active window, which under Wine happens as soon as anything outside
    /// the prefix takes X focus. Posting `WM_ACTIVATE` restores input handling
    /// and - unlike `SetForegroundWindow` - does not pull focus away from the
    /// user. `WM_SETFOCUS` alone does not work.
    pub fn nudge_active(&self) -> io::Result<()> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{WA_ACTIVE, WM_ACTIVATE};
        self.post(WM_ACTIVATE, WA_ACTIVE as WPARAM, 0)
    }

    /// Press and release one key.
    pub fn tap(&self, key: Key) -> io::Result<()> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP};

        let scan = (key.scan as LPARAM) << 16;
        self.post(WM_KEYDOWN, key.vk as WPARAM, scan)?;
        std::thread::sleep(std::time::Duration::from_millis(16));
        // 0xC0000001: transition + previous-state bits, repeat count 1.
        self.post(WM_KEYUP, key.vk as WPARAM, 0xC000_0001u32 as LPARAM | scan)
    }

    /// `PostMessage` queues and returns immediately. `SendMessage` would block
    /// until the game pumps its queue, which stalls for seconds whenever the
    /// window is not being composited.
    fn post(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> io::Result<()> {
        use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
        // SAFETY: hwnd came from EnumWindows; a dead window is reported by the
        // return value rather than being undefined behaviour.
        if unsafe { PostMessageW(self.hwnd, msg, wparam, lparam) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn hwnd(&self) -> usize {
        self.hwnd as usize
    }
}
