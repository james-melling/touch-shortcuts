#![cfg(windows)]
//! Windows platform layer: SendInput sink, foreground probe, focus tiers.
//!
//! Everything here is a thin shell over Win32; the rules live in tsp-core.

pub mod window;

use std::sync::atomic::{AtomicIsize, Ordering};

use tsp_core::combo::KeyEvent;
use tsp_core::executor::{Elevation, Foreground, KeySink, Target};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MAPVK_VK_TO_VSC_EX, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, MA_NOACTIVATE, PostMessageW, RegisterWindowMessageW, SW_SHOWNOACTIVATE,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WM_MOUSEACTIVATE, WM_POINTERACTIVATE,
    WS_EX_NOACTIVATE, WS_EX_TOPMOST,
};
use windows::core::{PWSTR, w};

static PAD_HWND: AtomicIsize = AtomicIsize::new(0);
static LAST_OTHER: AtomicIsize = AtomicIsize::new(0);

fn hwnd(v: isize) -> HWND {
    HWND(v as *mut _)
}

pub fn set_pad_hwnd(h: isize) {
    PAD_HWND.store(h, Ordering::SeqCst);
}

/// One `SendInput` batch. `wScan` is filled from the VK (low byte) for apps
/// that read scan codes; the extended flag comes from the key table or the
/// 0xE0 prefix MapVirtualKey reports.
pub struct SendInputSink;

impl KeySink for SendInputSink {
    fn send(&mut self, events: &[KeyEvent]) -> usize {
        let inputs: Vec<INPUT> = events
            .iter()
            .map(|e| {
                let sc = unsafe { MapVirtualKeyW(u32::from(e.vk), MAPVK_VK_TO_VSC_EX) };
                let mut flags = KEYBD_EVENT_FLAGS(0);
                if e.extended || (sc & 0xFF00) == 0xE000 {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                if !e.down {
                    flags |= KEYEVENTF_KEYUP;
                }
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT { wVk: VIRTUAL_KEY(e.vk), wScan: (sc & 0xFF) as u16, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                    },
                }
            })
            .collect();
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) as usize }
    }
}

fn window_title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn token_elevated(process: HANDLE) -> Option<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let res = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        res.ok()?;
        Some(elev.TokenIsElevated != 0)
    }
}

pub fn pad_elevated() -> bool {
    token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

/// UIPI blocks posting to a more elevated window with ERROR_ACCESS_DENIED.
fn uipi_blocked(h: HWND) -> bool {
    unsafe {
        let msg = RegisterWindowMessageW(w!("tsp-probe"));
        match PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0)) {
            Ok(()) => false,
            Err(_) => GetLastError() == ERROR_ACCESS_DENIED,
        }
    }
}

fn process_info(h: HWND) -> (u32, String, Elevation) {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    let Ok(proc) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        let e = if uipi_blocked(h) { Elevation::Yes } else { Elevation::Unknown };
        return (pid, String::new(), e);
    };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let name = match unsafe { QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) } {
        Ok(()) => {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            full.rsplit(['\\', '/']).next().unwrap_or("").to_string()
        }
        Err(_) => String::new(),
    };
    let elevation = match token_elevated(proc) {
        Some(true) => Elevation::Yes,
        Some(false) => Elevation::No,
        None if uipi_blocked(h) => Elevation::Yes,
        None => Elevation::Unknown,
    };
    unsafe {
        let _ = CloseHandle(proc);
    }
    (pid, name, elevation)
}

const SHELL_CLASSES: &[&str] = &["Shell_TrayWnd", "Progman", "WorkerW", "Windows.UI.Core.CoreWindow"];

pub fn describe(h: HWND) -> Target {
    let (pid, process, elevation) = process_info(h);
    let pad = PAD_HWND.load(Ordering::SeqCst);
    let is_pad = h.0 as isize == pad || pid == unsafe { GetCurrentProcessId() };
    Target { id: h.0 as u64, title: window_title(h), process, is_pad, elevation }
}

/// Describes a window by raw handle (for callers without the `windows` crate).
pub fn describe_raw(h: isize) -> Target {
    describe(hwnd(h))
}

pub struct WinForeground;

impl Foreground for WinForeground {
    fn current(&mut self) -> Target {
        let h = unsafe { GetForegroundWindow() };
        let t = describe(h);
        if !t.is_pad && !t.title.is_empty() && !SHELL_CLASSES.contains(&class_name(h).as_str()) {
            LAST_OTHER.store(h.0 as isize, Ordering::SeqCst);
        }
        t
    }
}

/// The last non-pad, non-shell foreground window seen (0 if none yet).
pub fn last_other() -> isize {
    LAST_OTHER.load(Ordering::SeqCst)
}

/// Tier 3: if the pad took the foreground, give it back to the last real
/// window and wait up to 100 ms for Windows to apply it.
pub fn restore_last_other() -> bool {
    let last = LAST_OTHER.load(Ordering::SeqCst);
    if last == 0 {
        return false;
    }
    unsafe {
        let _ = SetForegroundWindow(hwnd(last));
    }
    for _ in 0..20 {
        if unsafe { GetForegroundWindow() }.0 as isize == last {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

unsafe extern "system" fn no_activate_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    // Must never panic: an unwind out of extern "system" aborts the process.
    if msg == WM_MOUSEACTIVATE || msg == WM_POINTERACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

/// Applies the focus tier to the pad's top-level window and shows it
/// without activating it.
pub fn prepare_window(h: isize, tier: u8) {
    let w = hwnd(h);
    unsafe {
        if tier >= 2 {
            let ex = GetWindowLongPtrW(w, GWL_EXSTYLE);
            SetWindowLongPtrW(w, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOPMOST.0 as isize);
            let _ = SetWindowSubclass(w, Some(no_activate_proc), 1, 0);
        }
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(w, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, std::mem::size_of_val(&pref) as u32);
        let _ = ShowWindow(w, SW_SHOWNOACTIVATE);
    }
}
