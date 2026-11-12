//! Window helpers for the editor work: key-only window regions (tap-through),
//! no-activate panels, foreground capture/restore, and the end-session
//! subclass. First used by the Surface spike; kept for the real editor.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, RGN_OR, SetWindowRgn};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, IsWindow, MA_NOACTIVATE, SW_HIDE,
    SW_SHOWNOACTIVATE, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WM_ENDSESSION, WM_MOUSEACTIVATE,
    WM_POINTERACTIVATE, WM_QUERYENDSESSION, WS_EX_NOACTIVATE,
};

const PANEL_SUBCLASS_ID: usize = 2;
const ENDSESSION_SUBCLASS_ID: usize = 3;

static ENDSESSION_SEEN: AtomicU32 = AtomicU32::new(0);
type Handler = Box<dyn Fn() + Send + Sync>;
static ENDSESSION_HANDLER: OnceLock<Handler> = OnceLock::new();

/// Called (once per session end) when Windows logs off or shuts down. It must
/// be quick and never block; the session end is never vetoed.
pub fn set_endsession_handler(f: impl Fn() + Send + Sync + 'static) {
    let _ = ENDSESSION_HANDLER.set(Box::new(f));
}

fn hwnd(v: isize) -> HWND {
    HWND(v as *mut _)
}

/// Device-pixel rect relative to the window's client area: (left, top, right, bottom).
pub type PxRect = (i32, i32, i32, i32);

/// Restricts hit-testing and drawing to the union of `rects`. Outside the
/// region the window does not exist, so input reaches the app below.
pub fn set_region(h: isize, rects: &[PxRect]) -> Result<(), String> {
    let w = hwnd(h);
    // SetWindowRgn is in window coordinates; offset client rects by the frame.
    let (dx, dy) = unsafe {
        let mut wr = RECT::default();
        GetWindowRect(w, &mut wr).map_err(|e| format!("GetWindowRect: {e}"))?;
        let mut origin = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(w, &mut origin);
        (origin.x - wr.left, origin.y - wr.top)
    };
    unsafe {
        let rgn = CreateRectRgn(0, 0, 0, 0);
        if rgn.is_invalid() {
            return Err("CreateRectRgn failed".into());
        }
        for &(l, t, r, b) in rects {
            let part = CreateRectRgn(l + dx, t + dy, r + dx, b + dy);
            let _ = CombineRgn(Some(rgn), Some(rgn), Some(part), RGN_OR);
            let _ = DeleteObject(part.into());
        }
        // On success the system owns the region; delete it only on failure.
        if SetWindowRgn(w, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
            return Err("SetWindowRgn failed".into());
        }
    }
    Ok(())
}

/// Removes the region: the whole window rect is hit-testable again.
pub fn clear_region(h: isize) {
    unsafe {
        let _ = SetWindowRgn(hwnd(h), None, true);
    }
}

pub fn set_round_corners(h: isize, round: bool) {
    let pref = if round { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd(h), DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, std::mem::size_of_val(&pref) as u32);
    }
}

unsafe extern "system" fn panel_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    // Must never panic: an unwind out of extern "system" aborts the process.
    if msg == WM_MOUSEACTIVATE || msg == WM_POINTERACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

/// `on`: taps never activate the window (WS_EX_NOACTIVATE + MA_NOACTIVATE).
/// `off`: the window can be activated normally (for typing a label).
pub fn set_no_activate(h: isize, on: bool) {
    let w = hwnd(h);
    unsafe {
        let ex = GetWindowLongPtrW(w, GWL_EXSTYLE);
        let flag = WS_EX_NOACTIVATE.0 as isize;
        SetWindowLongPtrW(w, GWL_EXSTYLE, if on { ex | flag } else { ex & !flag });
        if on {
            let _ = SetWindowSubclass(w, Some(panel_proc), PANEL_SUBCLASS_ID, 0);
        } else {
            let _ = RemoveWindowSubclass(w, Some(panel_proc), PANEL_SUBCLASS_ID);
        }
    }
}

pub fn show_no_activate(h: isize) {
    unsafe {
        let _ = ShowWindow(hwnd(h), SW_SHOWNOACTIVATE);
    }
}

pub fn hide(h: isize) {
    unsafe {
        let _ = ShowWindow(hwnd(h), SW_HIDE);
    }
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}

pub fn is_window(h: isize) -> bool {
    h != 0 && unsafe { IsWindow(Some(hwnd(h))) }.as_bool()
}

/// Makes `h` the foreground window and waits up to 200 ms for Windows to
/// apply it. Returns whether the foreground is `h` afterwards.
pub fn activate(h: isize) -> bool {
    if !is_window(h) {
        return false;
    }
    if foreground() == h {
        return true;
    }
    unsafe {
        let _ = SetForegroundWindow(hwnd(h));
    }
    for _ in 0..40 {
        if foreground() == h {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

unsafe extern "system" fn endsession_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    // Must never panic. Pass-through only: the real editor writes unsaved edits here.
    if msg == WM_QUERYENDSESSION || (msg == WM_ENDSESSION && wp.0 != 0) {
        // First notice only; catch_unwind keeps a panic from crossing extern "system".
        if ENDSESSION_SEEN.fetch_add(1, Ordering::Relaxed) == 0 {
            if let Some(h) = ENDSESSION_HANDLER.get() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(h));
            }
        }
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

/// Installs or removes the end-session subclass (its own id, so it never
/// touches the focus-tier subclass).
pub fn set_endsession_subclass(h: isize, on: bool) {
    let w = hwnd(h);
    unsafe {
        if on {
            let _ = SetWindowSubclass(w, Some(endsession_proc), ENDSESSION_SUBCLASS_ID, 0);
        } else {
            let _ = RemoveWindowSubclass(w, Some(endsession_proc), ENDSESSION_SUBCLASS_ID);
        }
    }
}

pub fn endsession_seen() -> u32 {
    ENDSESSION_SEEN.load(Ordering::Relaxed)
}
