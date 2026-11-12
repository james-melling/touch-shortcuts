//! Surface spike (temporary): a transparent copy of the pad with key-only
//! tap-through, and a dummy editor panel that opens without taking focus.
//! Started from ··· → "Surface spike (test)" or with `--spike`. Results are
//! appended to `<log dir>/spike-results.txt`. Delete this module once the
//! spike outcome is recorded in docs/designs/in-app-editor.md.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};

use super::{AppState, MARGIN, PadView, pad_view, work_area_logical, work_area_logical_height};
#[cfg(windows)]
use tsp_core::executor::Foreground;
#[cfg(windows)]
use tsp_win as win;

pub const PAD: &str = "spike-pad";
pub const PANEL: &str = "spike-panel";
/// Extra header height of the spike pad (two rows of 44 px test controls).
pub const HEAD_EXTRA: u32 = 92;
const PANEL_WIDTH: f64 = 350.0;

pub struct SpikeState {
    pub results: PathBuf,
    target: Mutex<(isize, String)>,
    last_fg: Mutex<String>,
}

impl SpikeState {
    pub fn new(log_dir: &std::path::Path) -> Self {
        SpikeState { results: log_dir.join("spike-results.txt"), target: Mutex::new((0, String::new())), last_fg: Mutex::new(String::new()) }
    }
}

pub fn flag_set() -> bool {
    std::env::args().any(|a| a == "--spike") || std::env::var("TSP_SPIKE").is_ok_and(|v| v == "1")
}

fn record(app: &AppHandle, line: &str) {
    let state = app.state::<SpikeState>();
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    if let Some(dir) = state.results.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&state.results) {
        let _ = writeln!(f, "t={ms} {line}");
    }
}

#[cfg(windows)]
fn hwnd_of(app: &AppHandle, label: &str) -> isize {
    app.get_webview_window(label).and_then(|w| w.hwnd().ok()).map(|h| h.0 as isize).unwrap_or(0)
}

/// Builds the spike pad and hides the normal pad. Safe to call from setup or
/// an async command, never from a sync command (WebView2 deadlock).
pub fn start(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(PAD).is_some() {
        return Ok(());
    }
    let (ww, wa) = work_area_logical(app);
    let (_, layout) = pad_view(&app.state::<AppState>().active.lock().unwrap(), (ww, wa.saturating_sub(HEAD_EXTRA)));
    let (w, h) = (layout.width as f64, (layout.height + HEAD_EXTRA) as f64);
    let main = app.get_webview_window("main").ok_or("main window missing")?;
    let m = main.primary_monitor().map_err(|e| e.to_string())?.ok_or("no monitor")?;
    let s = m.scale_factor();
    let area = m.work_area();
    let x = area.position.x as f64 / s + area.size.width as f64 / s - w - MARGIN as f64;
    let y = area.position.y as f64 / s + MARGIN as f64;

    let builder = WebviewWindowBuilder::new(app, PAD, WebviewUrl::App("spike.html".into()))
        .title("Touch Shortcuts spike")
        .decorations(false)
        .always_on_top(true)
        .focusable(false)
        .focused(false)
        .visible(false)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.transparent(true);
    let pad = builder.build().map_err(|e| e.to_string())?;
    // Size and place after creation, as place_window does (builder geometry is not DPI-reliable).
    let _ = pad.set_size(LogicalSize::new(w, h));
    let _ = pad.set_position(LogicalPosition::new(x, y));
    let _ = main.hide();
    #[cfg(windows)]
    {
        let h = pad.hwnd().map_err(|e| e.to_string())?.0 as isize;
        win::set_pad_hwnd(h);
        win::prepare_window(h, 1);
    }
    #[cfg(not(windows))]
    let _ = pad.show();
    record(app, &format!("spike-start size={w}x{h} scale={s} work_area_h={wa}"));
    Ok(())
}

#[tauri::command]
pub async fn spike_start(app: AppHandle) -> Result<(), String> {
    start(&app)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpikePad {
    pad: PadView,
    head_extra: u32,
    results: String,
}

#[tauri::command]
pub fn spike_pad(app: AppHandle, state: tauri::State<AppState>, spike: tauri::State<SpikeState>) -> SpikePad {
    let (ww, wa) = work_area_logical(&app);
    let (pad, _) = pad_view(&state.active.lock().unwrap(), (ww, wa.saturating_sub(HEAD_EXTRA)));
    SpikePad { pad, head_extra: HEAD_EXTRA, results: spike.results.display().to_string() }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpikeStatus {
    foreground: String,
    kind: &'static str,
    panel_open: bool,
    target: String,
    endsession_seen: u32,
}

/// Polled by both spike windows. Foreground changes are written to the results file.
#[tauri::command]
pub fn spike_status(app: AppHandle, spike: tauri::State<SpikeState>) -> SpikeStatus {
    let panel_open = app.get_webview_window(PANEL).is_some();
    #[cfg(windows)]
    let (foreground, kind, endsession_seen) = {
        let fg = win::window::foreground();
        let t = win::WinForeground.current();
        let kind = if fg == hwnd_of(&app, PAD) {
            "pad"
        } else if panel_open && fg == hwnd_of(&app, PANEL) {
            "panel"
        } else if t.is_pad {
            "this app"
        } else {
            "other"
        };
        // While the panel is open, the return target follows the last real app
        // window in front (so opening R1 after the panel still works).
        let last = win::last_other();
        if panel_open && kind == "other" && last != 0 && spike.target.lock().unwrap().0 != last {
            let title = win::describe_raw(last).title;
            *spike.target.lock().unwrap() = (last, title.clone());
            record(&app, &format!("target -> \"{title}\""));
        }
        (t.title, kind, win::window::endsession_seen())
    };
    #[cfg(not(windows))]
    let (foreground, kind, endsession_seen) = ("(foreground tracking is Windows-only in the spike)".to_string(), "other", 0);
    let summary = format!("{kind}: {foreground}");
    let mut last = spike.last_fg.lock().unwrap();
    if *last != summary {
        *last = summary.clone();
        drop(last);
        record(&app, &format!("foreground -> {summary}"));
    }
    let target = spike.target.lock().unwrap().1.clone();
    SpikeStatus { foreground, kind, panel_open, target, endsession_seen }
}

/// Applies a key-only region from logical rects `[x, y, w, h]` (pad client
/// coordinates), or clears it when `on` is false.
#[tauri::command]
pub fn spike_region(app: AppHandle, rects: Vec<[f64; 4]>, on: bool) -> Result<String, String> {
    let pad = app.get_webview_window(PAD).ok_or("spike pad not open")?;
    let s = pad.scale_factor().map_err(|e| e.to_string())?;
    let size = pad.inner_size().map_err(|e| e.to_string())?;
    let px = tsp_core::layout::region_rects(&rects, s, (size.width, size.height));
    #[cfg(windows)]
    let result = {
        let h = hwnd_of(&app, PAD);
        if on { win::window::set_region(h, &px).map(|_| format!("region on: {} rects at scale {s}", px.len())) } else {
            win::window::clear_region(h);
            Ok("region off (full window)".to_string())
        }
    };
    #[cfg(not(windows))]
    let result: Result<String, String> = { let _ = (&px, on); Ok("regions are Windows-only in the spike".into()) };
    record(&app, &format!("region on={on} rects={} scale={s} result={result:?}", px.len()));
    result
}

#[tauri::command]
pub fn spike_chrome(app: AppHandle, shadow: bool, round: bool) {
    if let Some(pad) = app.get_webview_window(PAD) {
        let _ = pad.set_shadow(shadow);
        #[cfg(windows)]
        win::window::set_round_corners(hwnd_of(&app, PAD), round);
    }
    record(&app, &format!("chrome shadow={shadow} round={round}"));
}

#[tauri::command]
pub fn spike_endsession(app: AppHandle, on: bool) {
    #[cfg(windows)]
    win::window::set_endsession_subclass(hwnd_of(&app, PAD), on);
    record(&app, &format!("endsession-subclass on={on}"));
}

/// Opens the dummy panel beside the pad without activating it, after
/// capturing the window to return to on Done.
#[tauri::command]
pub async fn spike_panel_open(app: AppHandle) -> Result<String, String> {
    if app.get_webview_window(PANEL).is_some() {
        return Ok("panel already open".into());
    }
    #[cfg(windows)]
    {
        let fg = win::WinForeground.current();
        let h = if fg.is_pad { win::last_other() } else { fg.id as isize };
        let title = if h == 0 { String::new() } else { win::describe_raw(h).title };
        *app.state::<SpikeState>().target.lock().unwrap() = (h, title);
    }
    let pad = app.get_webview_window(PAD).ok_or("spike pad not open")?;
    let s = pad.scale_factor().map_err(|e| e.to_string())?;
    let pos = pad.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(s);
    let size = pad.inner_size().map_err(|e| e.to_string())?.to_logical::<f64>(s);
    let wa = work_area_logical_height(&app).unwrap_or(2000) as f64;
    let height = size.height.min(wa - 24.0);
    let x = (pos.x - PANEL_WIDTH - 8.0).max(0.0);
    let panel = WebviewWindowBuilder::new(&app, PANEL, WebviewUrl::App("spike-panel.html".into()))
        .title("Touch Shortcuts spike panel")
        .decorations(false)
        .always_on_top(true)
        .focused(false)
        .visible(false)
        .skip_taskbar(true)
        .resizable(false)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = panel.set_size(LogicalSize::new(PANEL_WIDTH, height));
    let _ = panel.set_position(LogicalPosition::new(x, pos.y));
    #[cfg(windows)]
    {
        let h = panel.hwnd().map_err(|e| e.to_string())?.0 as isize;
        win::window::set_no_activate(h, true);
        win::window::show_no_activate(h);
    }
    #[cfg(not(windows))]
    let _ = panel.show();
    let target = app.state::<SpikeState>().target.lock().unwrap().1.clone();
    record(&app, &format!("panel-open target=\"{target}\""));
    Ok(format!("panel open; return target: \"{target}\""))
}

/// Label tapped: make the panel activatable and bring it to the front.
#[tauri::command]
pub fn spike_panel_activate(app: AppHandle) -> bool {
    #[cfg(windows)]
    let ok = {
        let h = hwnd_of(&app, PANEL);
        win::window::set_no_activate(h, false);
        win::window::activate(h)
    };
    #[cfg(not(windows))]
    let ok = app.get_webview_window(PANEL).map(|w| w.set_focus().is_ok()).unwrap_or(false);
    record(&app, &format!("panel-activate ok={ok}"));
    ok
}

/// Label committed (Enter, or tapping away): back to no-activate and give
/// the foreground straight back to the captured target.
#[tauri::command]
pub fn spike_panel_rearm(app: AppHandle) -> String {
    let (target, title) = app.state::<SpikeState>().target.lock().unwrap().clone();
    #[cfg(windows)]
    let result = {
        win::window::set_no_activate(hwnd_of(&app, PANEL), true);
        if win::window::activate(target) && win::window::foreground() == target {
            format!("Label done: \"{title}\" is back in front.")
        } else {
            format!("Label done, but couldn't return to \"{title}\". Tap it.")
        }
    };
    #[cfg(not(windows))]
    let result = { let _ = (target, &app); format!("Label done (restore is Windows-only; target \"{title}\")") };
    record(&app, &format!("panel-rearm {result}"));
    result
}

/// Simulates the drag: hide the panel for 1.5 s, then show it without activation.
#[tauri::command]
pub async fn spike_panel_hide_show(app: AppHandle) -> String {
    #[cfg(windows)]
    {
        let h = hwnd_of(&app, PANEL);
        let before = win::describe_raw(win::window::foreground()).title;
        win::window::hide(h);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        win::window::show_no_activate(h);
        let after = win::describe_raw(win::window::foreground()).title;
        let line = format!("hide-show foreground before=\"{before}\" after=\"{after}\" same={}", before == after);
        record(&app, &line);
        line
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        "hide/show is Windows-only in the spike".into()
    }
}

/// Done: restore the captured target before closing, then verify it.
#[tauri::command]
pub async fn spike_panel_done(app: AppHandle) -> String {
    let (target, title) = app.state::<SpikeState>().target.lock().unwrap().clone();
    #[cfg(windows)]
    let result = if target == 0 || !win::window::is_window(target) {
        format!("restore-failed: the window to return to is gone (\"{title}\"). Tap it before using the pad.")
    } else if win::window::activate(target) && win::window::foreground() == target {
        format!("restored: \"{title}\" is in front. Check the caret is in the same field.")
    } else {
        format!("restore-failed: couldn't return to \"{title}\". Tap it before using the pad.")
    };
    #[cfg(not(windows))]
    let result = { let _ = target; format!("done (restore is Windows-only in the spike; target \"{title}\")") };
    if let Some(p) = app.get_webview_window(PANEL) {
        let _ = p.destroy();
    }
    record(&app, &format!("panel-done {result}"));
    let _ = app.emit_to(PAD, "spike-done", &result);
    result
}

#[tauri::command]
pub fn spike_record(app: AppHandle, item: String, result: String, note: String) {
    let clean = |s: &str| s.replace(['\n', '\r'], " ").chars().take(500).collect::<String>();
    record(&app, &format!("check item={} result={} note=\"{}\"", clean(&item), clean(&result), clean(&note)));
}
