//! touch-shortcuts app shell: window, worker thread, commands, log.
//!
//! Rules (profile validation, executor, queue, geometry) live in tsp-core.
//! This file wires them to Tauri and, on Windows, to Win32 (`win.rs`).

#[cfg(windows)]
use tsp_win as win;
#[cfg(target_os = "macos")]
use tsp_mac as mac;

mod applog;
mod editing;
mod pads;

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager};
use tsp_core::combo::Combo;
use tsp_core::executor::{self, Clock, Foreground, Outcome, Run, SideEffects, Target};
#[cfg(not(any(windows, target_os = "macos")))]
use tsp_core::{combo::KeyEvent, executor::{Elevation, KeySink}};
use tsp_core::geometry::{self, Cells, Grid, MARGIN};
use tsp_core::profile::{Action, Dock, Group, Loaded, load_profile};
use tsp_core::queue::{TapQueue, TapResult};

const STARTER: &str = include_str!("../../profiles/r1-delay-gain.tsp.json");
const STARTER_FILE: &str = "r1-delay-gain.tsp.json";

// ---------- settings and launch flags ----------

#[derive(Debug, Clone, Default)]
struct Settings {
    allow_system_keys: bool,
    focus_mode: u8,
    last_profile: Option<PathBuf>,
    /// Escape hatch: render fully opaque, never use a window region.
    window_opaque: bool,
    /// Reload the pad when its file changes on disk (outside edit mode).
    watch_profile: bool,
}

fn read_settings(config_dir: &std::path::Path, warnings: &mut Vec<String>) -> Settings {
    let path = config_dir.join("settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return Settings { focus_mode: 1, watch_profile: true, ..Default::default() } };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => Settings {
            allow_system_keys: v.get("allowSystemKeys").and_then(|x| x.as_bool()).unwrap_or(false),
            focus_mode: v.get("focusMode").and_then(|x| x.as_u64()).map(|n| n.clamp(1, 3) as u8).unwrap_or(1),
            last_profile: v.get("lastProfile").and_then(|x| x.as_str()).map(PathBuf::from),
            window_opaque: v.get("window").and_then(|x| x.as_str()) == Some("opaque"),
            watch_profile: v.get("watchProfile").and_then(|x| x.as_bool()).unwrap_or(true),
        },
        Err(e) => {
            warnings.push(format!("settings-unreadable {}: {e}. Using defaults.", path.display()));
            Settings { focus_mode: 1, watch_profile: true, ..Default::default() }
        }
    }
}

/// `--focus-mode=N` beats `TSP_FOCUS_MODE` beats settings.json.
fn focus_mode_override() -> Option<u8> {
    let from_arg = std::env::args().find_map(|a| a.strip_prefix("--focus-mode=").and_then(|v| v.parse().ok()));
    from_arg.or_else(|| std::env::var("TSP_FOCUS_MODE").ok().and_then(|v| v.parse().ok())).map(|n: u8| n.clamp(1, 3))
}

// ---------- profile loading with fallback ----------

struct Active {
    loaded: Loaded,
    path: PathBuf,
    fallback: bool,
    warnings: Vec<String>,
}

fn load_active(config_dir: &std::path::Path, settings: &Settings, extra: Vec<String>) -> Active {
    let mut warnings = extra;
    let default_path = config_dir.join(STARTER_FILE);
    if !default_path.exists() {
        let _ = std::fs::create_dir_all(config_dir);
        if let Err(e) = std::fs::write(&default_path, STARTER) {
            warnings.push(format!("could not write the starter profile to {}: {e}", default_path.display()));
        }
    }
    let path = settings.last_profile.clone().unwrap_or(default_path);
    let attempt = std::fs::read(&path)
        .map_err(|e| format!("profile-syntax {}: cannot read file: {e}", path.display()))
        .and_then(|bytes| load_profile(&bytes, settings.allow_system_keys).map_err(|e| format!("{} {}: {}", e.code, path.display(), e.message)));
    match attempt {
        Ok(loaded) => {
            warnings.extend(loaded.warnings.iter().map(|w| format!("{} {}", w.code, w.message)));
            Active { loaded, path, fallback: false, warnings }
        }
        Err(msg) => {
            warnings.push(msg);
            let loaded = load_profile(STARTER.as_bytes(), settings.allow_system_keys).expect("embedded starter profile is valid (tested)");
            Active { loaded, path, fallback: true, warnings }
        }
    }
}

// ---------- view model sent to the web layer ----------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct KeyView {
    id: String,
    label: String,
    group: &'static str,
    icon: Option<String>,
    color: Option<String>,
    repeat: u32,
    disabled: Option<String>,
    /// Canonical combo text for keys actions (the panel shows and edits it).
    combo: Option<String>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// Position in grid cells.
    cell: [u32; 4],
}

/// Grid geometry for edit mode: cell (c, r) is at
/// (left + c × (cellWidth + gap), top + r × (rowHeight + gap)).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GridView {
    columns: u32,
    rows: u32,
    left: f64,
    top: f64,
    cell_width: f64,
    row_height: f64,
    gap: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PadView {
    name: String,
    fallback: bool,
    accept_touch: bool,
    width: u32,
    height: u32,
    theme: String,
    warnings: Vec<String>,
    keys: Vec<KeyView>,
    editing: bool,
    grid: GridView,
    header: f64,
    scale: f64,
    pad_opacity: f64,
    background_opacity: f64,
    /// Opaque escape hatch: render at full opacity, no floating mode.
    opaque: bool,
}

fn group_name(g: &Group) -> &'static str {
    match g {
        Group::Num => "num",
        Group::Ent => "ent",
        Group::Fine => "fine",
        Group::Coarse => "coarse",
        Group::Hist => "hist",
        Group::Custom => "custom",
    }
}

const NOT_SET: &str = "Not set: ··· → Edit";

fn pad_view(active: &Active, work_area: (u32, u32), edit: bool) -> (PadView, geometry::Fit) {
    pad_view_with(active, work_area, edit, OPAQUE.load(std::sync::atomic::Ordering::Relaxed))
}

/// Set once at startup from settings / `--window=opaque` / `TSP_WINDOW=opaque`.
static OPAQUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn pad_view_with(active: &Active, work_area: (u32, u32), edit: bool, opaque: bool) -> (PadView, geometry::Fit) {
    let p = &active.loaded.profile;
    let grid = Grid { columns: p.pad.columns, rows: p.pad.rows, cell_width: p.pad.cell_width, cell_height: p.pad.cell_height, gap: p.pad.gap, scale: p.pad.scale };
    let cells: Vec<Cells> = p.buttons.iter().map(|b| Cells { x: b.x, y: b.y, w: b.w, h: b.h }).collect();
    let layout = geometry::fit(&grid, &cells, work_area, edit);
    let mut warnings = active.warnings.clone();
    if let Some(w) = &layout.warning {
        warnings.push(w.clone());
    }
    let keys = p
        .buttons
        .iter()
        .zip(&layout.buttons)
        .map(|(b, r)| KeyView {
            id: b.id.clone(),
            label: b.label.clone(),
            group: group_name(&b.group),
            icon: b.icon.clone(),
            color: b.color.clone(),
            repeat: match &b.action { Action::Keys { repeat, .. } => *repeat, Action::None | Action::Disabled { .. } => 1 },
            disabled: match &b.action {
                Action::Disabled { code, reason } => Some(format!("{code}: {reason}")),
                Action::None => Some(NOT_SET.to_string()),
                Action::Keys { .. } => None,
            },
            combo: match &b.action { Action::Keys { combo, .. } => Some(tsp_core::combo::canonical_text(combo)), _ => None },
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            cell: [b.x, b.y, b.w, b.h],
        })
        .collect();
    let grid = GridView {
        columns: p.pad.columns,
        rows: p.pad.rows,
        left: layout.grid_left,
        top: layout.grid_top,
        cell_width: layout.cell_width,
        row_height: layout.row_height,
        gap: layout.gap,
    };
    (
        PadView {
            name: p.name.clone(),
            fallback: active.fallback,
            accept_touch: p.accept_touch,
            width: layout.width,
            height: layout.height,
            theme: p.pad.theme.clone(),
            warnings,
            keys,
            editing: edit,
            grid,
            header: layout.header,
            scale: p.pad.scale,
            pad_opacity: p.pad.pad_opacity,
            background_opacity: p.pad.background_opacity,
            opaque,
        },
        layout,
    )
}

// ---------- worker ----------

#[derive(Clone)]
struct Job {
    button_id: String,
    label: String,
    combo_text: String,
    combo: Combo,
    repeat: u32,
    interval_ms: u32,
    target_apps: Option<Vec<String>>,
}

struct Shared {
    queue: TapQueue<Job>,
    quit: bool,
    /// Edit mode: taps select instead of sending. Checked under this lock in
    /// the same critical section as `queue.tap`, and set together with stop-all.
    editing: bool,
}

struct AppState {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    active: Mutex<Active>,
    started: Instant,
    config_dir: PathBuf,
    settings: Mutex<Settings>,
    focus_mode: u8,
    /// The edit session while in edit mode. Never held across window calls.
    editor: Mutex<Option<tsp_core::session::Session>>,
    return_target: Mutex<editing::ReturnTarget>,
    pads: Mutex<pads::PadList>,
    log: applog::Log,
}

const HAS_SINK: bool = cfg!(any(windows, target_os = "macos"));
const SINK_NAME: &str = if cfg!(windows) { "SendInput" } else if cfg!(target_os = "macos") { "CGEventPostToPid" } else { "sink-unavailable" };

struct RealClock(Instant);
impl Clock for RealClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
    fn sleep_until_ms(&mut self, t: u64) {
        let now = self.now_ms();
        if t > now {
            std::thread::sleep(std::time::Duration::from_millis(t - now));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct LogOnlySink;
#[cfg(not(any(windows, target_os = "macos")))]
impl KeySink for LogOnlySink {
    fn send(&mut self, events: &[KeyEvent]) -> usize {
        events.len() // logged as sink-unavailable by the worker
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct DevForeground;
#[cfg(not(any(windows, target_os = "macos")))]
impl Foreground for DevForeground {
    fn current(&mut self) -> Target {
        Target { id: 1, title: "(dev build without a key sink)".into(), process: "dev".into(), is_pad: false, elevation: Elevation::No }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ActionResult {
    button_id: String,
    ok: bool,
    code: String,
    message: String,
    side_effects: &'static str,
    sent: u32,
    of: u32,
}

fn describe_outcome(o: &Outcome, target: &Target, job: &Job) -> ActionResult {
    let base = |ok: bool, code: &str, message: String, side_effects: &'static str, sent: u32, of: u32| ActionResult {
        button_id: job.button_id.clone(),
        ok,
        code: code.into(),
        message,
        side_effects,
        sent,
        of,
    };
    match o {
        Outcome::Ok { sent, unverified: false } => base(true, "ok", String::new(), "none", *sent, *sent),
        Outcome::Ok { sent, unverified: true } => base(
            true,
            "target-elevation-unknown",
            format!("target-elevation-unknown: sent to \"{}\" but could not confirm it is not elevated", target.title),
            "unknown",
            *sent,
            *sent,
        ),
        Outcome::Refused { code } => {
            let msg = match *code {
                "target-is-pad" => "target-is-pad: nothing sent. Tap the R1 field first, then retry.".to_string(),
                "target-untitled" => "target-untitled: nothing sent. Tap the R1 field first, then retry.".to_string(),
                "target-not-allowed" => format!("target-not-allowed: nothing sent; \"{}\" ({}) is not in this profile's targetApps.", target.title, target.process),
                "target-elevated" => "target-elevated: keys will be dropped. Quit, then start the pad with Run as administrator.".to_string(),
                "sink-permission" => "sink-permission: nothing sent. Allow this app under System Settings → Privacy & Security → Accessibility, then quit and relaunch the pad.".to_string(),
                other => format!("{other}: nothing sent."),
            };
            base(false, code, msg, "none", 0, job.repeat)
        }
        Outcome::Aborted { sent, of, side_effects } => {
            let se = match side_effects { SideEffects::None => "none", SideEffects::Partial => "partial", SideEffects::Unknown => "unknown" };
            base(false, "aborted-focus-change", format!("aborted-focus-change: {sent}/{of} sent (focus moved). Value changed by {sent}; check before retrying."), se, *sent, *of)
        }
        Outcome::Cancelled { sent, of } => base(true, "cancelled", format!("cancelled {sent}/{of}"), "partial", *sent, *of),
        Outcome::SendIncomplete { sent, of } => base(
            false,
            "send-incomplete",
            "send-incomplete: outcome unknown, modifiers released; check the R1 field before retrying".into(),
            "unknown",
            *sent,
            *of,
        ),
    }
}

fn spawn_worker(app: AppHandle, shared: Arc<(Mutex<Shared>, Condvar)>, started: Instant, focus_mode: u8, pad_elevated: bool, log: applog::Log) {
    std::thread::spawn(move || {
        #[cfg(windows)]
        let (mut sink, mut fg) = (win::SendInputSink, win::WinForeground);
        #[cfg(target_os = "macos")]
        let (mut sink, mut fg) = (mac::MacSink, mac::MacForeground);
        #[cfg(not(any(windows, target_os = "macos")))]
        let (mut sink, mut fg) = (LogOnlySink, DevForeground);
        let mut clock = RealClock(started);
        loop {
            let job = {
                let (lock, cv) = &*shared;
                let mut s = lock.lock().unwrap();
                loop {
                    if s.quit {
                        return;
                    }
                    let now = clock.now_ms();
                    for id in std::mem::take(&mut s.queue.stale_dropped) {
                        log.write(serde_json::json!({ "event": "dropped-stale", "buttonId": id }));
                    }
                    if let Some(p) = s.queue.start_next(now) {
                        break p.action;
                    }
                    s = cv.wait(s).unwrap();
                }
            };
            #[cfg(windows)]
            if focus_mode == 3 && fg.current().is_pad {
                win::restore_last_other();
            }
            let _ = focus_mode;
            let target = fg.current();
            let run = Run { combo: &job.combo, repeat: job.repeat, interval_ms: job.interval_ms, target_apps: job.target_apps.as_deref(), pad_elevated };
            let cancel = || shared.0.lock().map(|s| s.queue.cancel_requested || s.quit).unwrap_or(true);
            let t_start = clock.now_ms();
            #[cfg(target_os = "macos")]
            let permitted = mac::can_post();
            #[cfg(not(target_os = "macos"))]
            let permitted = true;
            let outcome = if permitted {
                executor::execute(&run, &mut sink, &mut fg, &mut clock, &cancel)
            } else {
                Outcome::Refused { code: "sink-permission" }
            };
            let mut result = describe_outcome(&outcome, &target, &job);
            if !HAS_SINK && result.ok && result.code == "ok" {
                // Never report success for keys that were not sent.
                result = ActionResult {
                    ok: false,
                    code: "sink-unavailable".into(),
                    message: format!("sink-unavailable: {} not sent; this macOS build logs taps only (keys are sent on Windows)", job.combo_text),
                    side_effects: "none",
                    sent: 0,
                    ..result
                };
            }
            log.write(serde_json::json!({
                "event": "action",
                "buttonId": job.button_id,
                "label": job.label,
                "combo": job.combo_text,
                "repeat": job.repeat,
                "target": { "title": target.title, "process": target.process, "isPad": target.is_pad, "elevated": format!("{:?}", target.elevation) },
                "code": result.code,
                "ok": result.ok,
                "sent": result.sent,
                "of": result.of,
                "sideEffects": result.side_effects,
                "elapsedMs": clock.now_ms() - t_start,
                "sink": SINK_NAME,
            }));
            let _ = app.emit("action-result", result);
            shared.0.lock().unwrap().queue.finish();
        }
    });
}

// ---------- commands ----------

#[derive(Serialize)]
struct AppError {
    code: String,
    message: String,
}

fn editing(state: &AppState) -> bool {
    state.shared.0.lock().map(|s| s.editing).unwrap_or(false)
}

#[tauri::command]
fn get_pad(state: tauri::State<AppState>, app: AppHandle) -> PadView {
    pad_view(&state.active.lock().unwrap(), work_area_logical(&app), editing(&state)).0
}

#[tauri::command]
fn run_action(button_id: String, state: tauri::State<AppState>) -> Result<String, AppError> {
    let job = {
        let active = state.active.lock().unwrap();
        let p = &active.loaded.profile;
        let b = p.buttons.iter().find(|b| b.id == button_id).ok_or_else(|| AppError { code: "unknown-button".into(), message: format!("no button \"{button_id}\" in the loaded profile") })?;
        match &b.action {
            Action::Disabled { code, reason } => return Err(AppError { code: (*code).into(), message: format!("{code}: {reason}") }),
            Action::None => return Err(AppError { code: "action-unknown".into(), message: NOT_SET.into() }),
            Action::Keys { combo, repeat, interval_ms } => Job {
                button_id: b.id.clone(),
                label: b.label.clone(),
                combo_text: format!("{}{}", combo.modifiers.iter().map(|m| format!("{}+", m.name)).collect::<String>(), combo.key.name),
                combo: combo.clone(),
                repeat: *repeat,
                interval_ms: *interval_ms,
                target_apps: p.target_apps.clone(),
            },
        }
    };
    let (lock, cv) = &*state.shared;
    let now = state.started.elapsed().as_millis() as u64;
    let result = {
        let mut shared = lock.lock().unwrap();
        if shared.editing {
            return Err(AppError { code: "edit-active".into(), message: "edit-active: nothing sent. Tap Done to use the pad.".into() });
        }
        shared.queue.tap(&button_id, job, now)
    };
    cv.notify_one();
    Ok(match result {
        TapResult::Queued => "queued",
        TapResult::Dropped => "dropped-queue-full",
        TapResult::CancelRunning => "cancelled",
    }
    .into())
}

#[tauri::command]
fn stop_all(state: tauri::State<AppState>) {
    state.shared.0.lock().unwrap().queue.stop_all();
}

#[tauri::command]
fn reload_profile(state: tauri::State<AppState>, app: AppHandle) -> Result<PadView, String> {
    if state.editor.lock().unwrap().is_some() {
        return editing::reload_in_edit(&app);
    }
    let settings = state.settings.lock().unwrap().clone();
    let fresh = load_active(&state.config_dir, &settings, Vec::new());
    let mut active = state.active.lock().unwrap();
    if fresh.fallback && !active.fallback {
        // A failed reload keeps the last valid profile and reports the error.
        active.warnings = fresh.warnings;
    } else {
        *active = fresh;
    }
    state.shared.0.lock().unwrap().queue.clear_pending();
    let (view, layout) = pad_view(&active, work_area_logical(&app), false);
    drop(active);
    place_window(&app, &layout);
    Ok(view)
}

/// Floating mode: hit-test only the keys, header and open menu (Windows).
/// `rects` are logical `[x, y, w, h]` in pad client coordinates; `on: false`
/// restores the full window. Shadow and rounded corners are off while on.
#[tauri::command]
fn pad_region(app: AppHandle, state: tauri::State<AppState>, rects: Vec<[f64; 4]>, on: bool) -> Result<(), String> {
    let w = app.get_webview_window("main").ok_or("pad window missing")?;
    let scale = w.scale_factor().map_err(|e| e.to_string())?;
    let size = w.inner_size().map_err(|e| e.to_string())?;
    let px = tsp_core::layout::region_rects(&rects, scale, (size.width, size.height));
    let _ = w.set_shadow(!on);
    #[cfg(windows)]
    let result = {
        let h = w.hwnd().map_err(|e| e.to_string())?.0 as isize;
        win::window::set_round_corners(h, !on);
        if on { win::window::set_region(h, &px) } else { win::window::clear_region(h); Ok(()) }
    };
    #[cfg(not(windows))]
    let result: Result<(), String> = Ok(());
    state.log.write(serde_json::json!({
        "event": "region", "mode": if !on { "full" } else if result.is_ok() { "keys" } else { "fallback" },
        "rects": px.len(), "scaleFactor": scale, "error": result.as_ref().err(),
    }));
    if result.is_err() {
        let _ = w.set_shadow(true);
    }
    result
}

/// Reloads the pad when its file changes on disk, outside edit mode, with
/// the same rules as Reload (a broken file keeps the last valid pad). Polls
/// (mtime, size, hash) and waits for the file to be stable for 500 ms.
fn spawn_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        use tsp_core::store::{RealFs, identity};
        let mut seen: Option<(PathBuf, Option<tsp_core::store::Identity>)> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(700));
            let state = app.state::<AppState>();
            if state.shared.0.lock().map(|s| s.quit).unwrap_or(true) {
                return;
            }
            let watch = state.settings.lock().unwrap().watch_profile;
            if !watch || editing(&state) || state.active.lock().unwrap().fallback {
                seen = None; // own writes in edit mode never trigger a reload
                continue;
            }
            let path = state.active.lock().unwrap().path.clone();
            let now = identity(&RealFs, &path).ok();
            match &seen {
                Some((p, id)) if *p == path && *id == now => continue,
                Some((p, _)) if *p == path => {}
                _ => {
                    seen = Some((path, now));
                    continue;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            let settled = identity(&RealFs, &path).ok();
            if settled != now || editing(&state) {
                continue;
            }
            seen = Some((path.clone(), settled));
            let allow = state.settings.lock().unwrap().allow_system_keys;
            let result = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| tsp_core::profile::load_profile(&b, allow).map_err(|e| format!("{} {}: {}", e.code, path.display(), e.message)));
            state.shared.0.lock().unwrap().queue.stop_all();
            let notice = {
                let mut active = state.active.lock().unwrap();
                match result {
                    Ok(loaded) => {
                        active.warnings = loaded.warnings.iter().map(|w| format!("{} {}", w.code, w.message)).collect();
                        active.loaded = loaded;
                        "Reloaded from disk".to_string()
                    }
                    Err(e) => format!("Pad file changed but can't be loaded; keeping the last good pad. {e}"),
                }
            };
            state.log.write(serde_json::json!({ "event": "watcher-reload", "ok": notice == "Reloaded from disk" }));
            let (view, layout) = pad_view(&state.active.lock().unwrap(), work_area_logical(&app), false);
            place_window(&app, &layout);
            let _ = app.emit("pad-reloaded", serde_json::json!({ "pad": view, "notice": notice }));
        }
    });
}

#[tauri::command]
fn quit(state: tauri::State<AppState>, app: AppHandle) {
    {
        let mut s = state.shared.0.lock().unwrap();
        s.queue.stop_all();
        s.quit = true;
    }
    state.shared.1.notify_all();
    app.exit(0);
}

#[tauri::command]
fn focus_mode(state: tauri::State<AppState>) -> u8 {
    state.focus_mode
}

// ---------- window placement ----------

/// Primary monitor work area in logical px (falls back to a large screen).
fn work_area_logical(app: &AppHandle) -> (u32, u32) {
    (|| {
        let w = app.get_webview_window("main")?;
        let m = w.primary_monitor().ok()??;
        let scale = m.scale_factor();
        let size = m.work_area().size;
        Some(((size.width as f64 / scale) as u32, (size.height as f64 / scale) as u32))
    })()
    .unwrap_or((2560, 2000))
}

fn place_window(app: &AppHandle, layout: &geometry::Fit) {
    let Some(w) = app.get_webview_window("main") else { return };
    let Ok(Some(m)) = w.primary_monitor() else { return };
    let scale = m.scale_factor();
    let wa = m.work_area();
    let (wx, wy) = (wa.position.x as f64 / scale, wa.position.y as f64 / scale);
    let ww = wa.size.width as f64 / scale;
    let dock_left = app.state::<AppState>().active.lock().unwrap().loaded.profile.pad.dock == Dock::Left;
    let x = if dock_left { wx + MARGIN as f64 } else { wx + ww - layout.width as f64 - MARGIN as f64 };
    let _ = w.set_size(LogicalSize::new(layout.width, layout.height));
    let _ = w.set_position(LogicalPosition::new(x, wy + MARGIN as f64));
}

fn install_panic_hook(log_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = std::fs::create_dir_all(&log_dir);
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(log_dir.join(format!("crash-{ts}.txt")), format!("{info}\n\n{bt}"));
        previous(info);
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = app.emit("second-instance", ());
        }))
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let log_dir = app.path().app_log_dir()?;
            install_panic_hook(log_dir.clone());
            let mut extra = Vec::new();
            let mut settings = read_settings(&config_dir, &mut extra);
            if let Some(m) = focus_mode_override() {
                settings.focus_mode = m;
            }
            if std::env::args().any(|a| a == "--window=opaque") || std::env::var("TSP_WINDOW").is_ok_and(|v| v == "opaque") {
                settings.window_opaque = true;
            }
            if !HAS_SINK {
                extra.push("sink-unavailable: this build logs taps but does not send keystrokes (Windows and macOS send them).".into());
            }
            #[cfg(target_os = "macos")]
            {
                mac::start_tracker();
                if !mac::request_post_access() {
                    extra.push("sink-permission: keys cannot be sent until this app is allowed under System Settings → Privacy & Security → Accessibility. Allow it, then quit and relaunch the pad.".into());
                }
            }
            OPAQUE.store(settings.window_opaque, std::sync::atomic::Ordering::Relaxed);
            let active = load_active(&config_dir, &settings, extra);
            let started = Instant::now();
            #[cfg(windows)]
            let pad_elevated = win::pad_elevated();
            #[cfg(not(windows))]
            let pad_elevated = false;
            let log = applog::Log::start(log_dir.join("actions.log"));
            log.write(serde_json::json!({
                "event": "startup",
                "version": env!("CARGO_PKG_VERSION"),
                "focusMode": settings.focus_mode,
                "padElevated": pad_elevated,
                "profile": active.path.display().to_string(),
                "fallback": active.fallback,
                "warnings": active.warnings,
            }));
            let shared = Arc::new((Mutex::new(Shared { queue: TapQueue::default(), quit: false, editing: false }), Condvar::new()));
            let focus_mode = settings.focus_mode;
            app.manage(AppState {
                shared: shared.clone(),
                active: Mutex::new(active),
                started,
                config_dir,
                settings: Mutex::new(settings),
                focus_mode,
                editor: Mutex::new(None),
                return_target: Mutex::new(editing::ReturnTarget::default()),
                pads: Mutex::new(pads::PadList::default()),
                log: log.clone(),
            });
            spawn_worker(handle.clone(), shared, started, focus_mode, pad_elevated, log);
            spawn_watcher(handle.clone());

            let layout = pad_view(&handle.state::<AppState>().active.lock().unwrap(), work_area_logical(&handle), false).1;
            place_window(&handle, &layout);
            #[cfg(windows)]
            if let Some(w) = handle.get_webview_window("main") {
                if let Ok(h) = w.hwnd() {
                    win::set_pad_hwnd(h.0 as isize);
                    win::prepare_window(h.0 as isize, focus_mode);
                    let app_for_end = handle.clone();
                    win::window::set_endsession_handler(move || editing::write_unsaved(&app_for_end));
                    win::window::set_endsession_subclass(h.0 as isize, true);
                }
            }
            #[cfg(not(windows))]
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.show();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_pad,
            run_action,
            stop_all,
            reload_profile,
            quit,
            focus_mode,
            pad_region,
            editing::edit_begin,
            editing::get_editor_state,
            editing::edit_apply,
            editing::edit_flush,
            editing::edit_select,
            editing::edit_history,
            editing::edit_conflict,
            editing::edit_panel_activate,
            editing::edit_panel_rearm,
            editing::edit_done,
            editing::edit_record,
            editing::edit_preview,
            editing::edit_panel_visible,
            pads::pads_list,
            pads::pads_switch,
            pads::pad_new,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
