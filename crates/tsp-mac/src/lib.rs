//! macOS layer: posts key events to the app that was in front before the pad,
//! and tracks that app. Needs the "post events" (Accessibility) permission.
//!
//! The pad window cannot become key (Tauri `focusable: false`), but clicking
//! it may still activate the pad app. So the target is "the frontmost app,
//! or the last other app if the pad is frontmost", keys go to that app's pid,
//! and if the pad is in front the target is re-activated first (a Cocoa app
//! only routes key events to its key window while it is active).
#![cfg(target_os = "macos")]

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use core_graphics::event::{CGEvent, CGEventFlags};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use tsp_core::combo::KeyEvent;
use tsp_core::executor::{Elevation, Foreground, KeySink, Target};
use tsp_core::mackeys::{key_flag, mac_keycode, modifier_flag};

/// Last app other than the pad seen in front (0 = none yet).
static LAST_OTHER: AtomicI32 = AtomicI32::new(0);
/// App the most recent foreground check resolved to; the sink posts here.
static TARGET: AtomicI32 = AtomicI32::new(0);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

fn pad_pid() -> i32 {
    std::process::id() as i32
}

/// True if this process may post keyboard events.
pub fn can_post() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

/// Ask for the permission (macOS shows its prompt once, then points to
/// System Settings → Privacy & Security → Accessibility). Returns the
/// current state.
pub fn request_post_access() -> bool {
    can_post() || unsafe { CGRequestPostEventAccess() }
}

fn frontmost() -> Option<(i32, String)> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        Some((app.processIdentifier(), app.localizedName().map(|s| s.to_string()).unwrap_or_default()))
    })
}

fn running_name(pid: i32) -> Option<String> {
    autoreleasepool(|_| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
        if app.isTerminated() {
            return None;
        }
        Some(app.localizedName().map(|s| s.to_string()).unwrap_or_default())
    })
}

fn note_frontmost() -> Option<(i32, String)> {
    let f = frontmost();
    if let Some((pid, _)) = &f {
        if *pid > 0 && *pid != pad_pid() {
            LAST_OTHER.store(*pid, Ordering::Relaxed);
        }
    }
    f
}

/// Poll the frontmost app so the pad knows where keys should go once it
/// is itself in front.
pub fn start_tracker() {
    std::thread::spawn(|| loop {
        note_frontmost();
        std::thread::sleep(Duration::from_millis(150));
    });
}

fn target(pid: i32, name: String) -> Target {
    TARGET.store(pid, Ordering::Relaxed);
    Target { id: pid as u64, title: name.clone(), process: name, is_pad: false, elevation: Elevation::No }
}

pub struct MacForeground;

impl Foreground for MacForeground {
    fn current(&mut self) -> Target {
        let me = pad_pid();
        if let Some((pid, name)) = note_frontmost() {
            if pid > 0 && pid != me {
                return target(pid, name);
            }
        }
        let other = LAST_OTHER.load(Ordering::Relaxed);
        if other > 0 {
            if let Some(name) = running_name(other) {
                return target(other, name);
            }
            LAST_OTHER.store(0, Ordering::Relaxed);
        }
        TARGET.store(0, Ordering::Relaxed);
        Target { id: me as u64, title: "Touch Shortcuts".into(), process: "touch-shortcuts".into(), is_pad: true, elevation: Elevation::No }
    }
}

/// If the pad is in front, hand activation back to `pid` and wait (up to
/// 300 ms) until it is frontmost. Returns false if it never got there.
fn ensure_active(pid: i32) -> bool {
    let front = frontmost().map(|f| f.0);
    if front == Some(pid) {
        return true;
    }
    if front != Some(pad_pid()) {
        return false; // a third app took over: let the executor's check abort
    }
    let asked = autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .map(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
            .unwrap_or(false)
    });
    if !asked {
        return false;
    }
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        if frontmost().map(|f| f.0) == Some(pid) {
            std::thread::sleep(Duration::from_millis(15)); // let its key window settle
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

pub struct MacSink;

impl KeySink for MacSink {
    /// Posts the batch to the current target's pid. Returns how many events
    /// were posted; anything short makes the executor release what it pressed.
    fn send(&mut self, events: &[KeyEvent]) -> usize {
        let pid = TARGET.load(Ordering::Relaxed);
        if pid <= 0 || !can_post() || !ensure_active(pid) {
            return 0;
        }
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) else { return 0 };
        let mut held: u64 = 0;
        for (i, e) in events.iter().enumerate() {
            let Some(code) = mac_keycode(e.vk) else { return i };
            let m = modifier_flag(e.vk);
            if m != 0 {
                if e.down { held |= m } else { held &= !m }
            }
            let Ok(ev) = CGEvent::new_keyboard_event(source.clone(), code, e.down) else { return i };
            ev.set_flags(CGEventFlags::from_bits_retain(held | key_flag(e.vk)));
            ev.post_to_pid(pid);
        }
        events.len()
    }
}
