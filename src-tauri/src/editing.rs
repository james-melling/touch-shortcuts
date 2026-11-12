//! Edit mode: the app side of `tsp_core::session`.
//!
//! One `Session` lives behind `AppState::editor`; it is never held across a
//! window call. Commands that create or close windows are async (a sync
//! command building a WebView2 window deadlocks). Every change is emitted as
//! `editor-state` to both windows, with `rev` so stale events are dropped.

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};
use tsp_core::edit::{EditError, Op, mirror_of, same_combo};
use tsp_core::session::{SaveState, Selection, Session};
use tsp_core::store::{self, RealFs};

use super::{AppState, PadView, pad_view, place_window, work_area_logical};
#[cfg(windows)]
use tsp_core::executor::Foreground;
#[cfg(windows)]
use tsp_win as win;

pub const PANEL: &str = "editor";
const PANEL_WIDTH: f64 = 350.0;

/// Window to give the foreground back to on Done (and after a label commit).
#[derive(Default, Clone)]
pub struct ReturnTarget {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub hwnd: isize,
    pub title: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EditorView {
    editing: bool,
    rev: u64,
    selection: Selection,
    /// saved | pending | failed | conflict
    save_state: &'static str,
    save_message: Option<String>,
    unsaved_ops: u32,
    can_undo: bool,
    can_redo: bool,
    pad: PadView,
    /// Other buttons with the same keys as the selected one: (label, row).
    same_combo: Vec<(String, u32)>,
    /// Key name for "Duplicate as <mirror>".
    mirror: Option<&'static str>,
    target: String,
    path: String,
    notice: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EditFailure {
    code: String,
    pub message: String,
    button: Option<String>,
    view: Option<EditorView>,
}

fn failure(e: EditError) -> EditFailure {
    EditFailure { code: e.code.into(), message: e.message, button: e.button, view: None }
}

fn plain(code: &str, message: String) -> EditFailure {
    EditFailure { code: code.into(), message, button: None, view: None }
}

fn now_ms(state: &AppState) -> u64 {
    state.started.elapsed().as_millis() as u64
}

/// Builds the view from the session (if editing) and the active pad.
fn view(app: &AppHandle, notice: Option<String>) -> EditorView {
    let state = app.state::<AppState>();
    let editor = state.editor.lock().unwrap();
    let active = state.active.lock().unwrap();
    let editing = editor.is_some();
    let (pad, _) = pad_view(&active, work_area_logical(app), editing);
    let target = state.return_target.lock().unwrap().title.clone();
    let Some(s) = editor.as_ref() else {
        return EditorView {
            editing,
            rev: 0,
            selection: Selection::None,
            save_state: "saved",
            save_message: None,
            unsaved_ops: 0,
            can_undo: false,
            can_redo: false,
            pad,
            same_combo: Vec::new(),
            mirror: None,
            target,
            path: active.path.display().to_string(),
            notice,
        };
    };
    let selected = match &s.selection {
        Selection::Button { id } => s.loaded.profile.buttons.iter().find(|b| &b.id == id),
        _ => None,
    };
    let (save_state, save_message) = match &s.save_state {
        SaveState::Saved => ("saved", None),
        SaveState::Pending => ("pending", None),
        SaveState::Failed(m) => ("failed", Some(m.clone())),
        SaveState::Conflict => ("conflict", None),
    };
    EditorView {
        editing,
        rev: s.rev,
        selection: s.selection.clone(),
        save_state,
        save_message,
        unsaved_ops: s.unsaved_ops(),
        can_undo: s.history.can_undo(),
        can_redo: s.history.can_redo(),
        pad,
        same_combo: selected.map(|b| same_combo(&s.loaded.profile, &b.id)).unwrap_or_default(),
        mirror: selected.and_then(mirror_of),
        target,
        path: s.path.display().to_string(),
        notice,
    }
}

/// Copies the session's pad into the active pad, re-places the window and
/// tells both windows.
fn publish(app: &AppHandle, notice: Option<String>) -> EditorView {
    {
        let state = app.state::<AppState>();
        let editor = state.editor.lock().unwrap();
        if let Some(s) = editor.as_ref() {
            state.active.lock().unwrap().loaded = s.loaded.clone();
        }
    }
    let state = app.state::<AppState>();
    let editing = state.editor.lock().unwrap().is_some();
    let layout = pad_view(&state.active.lock().unwrap(), work_area_logical(app), editing).1;
    place_window(app, &layout);
    if let Some(panel) = app.get_webview_window(PANEL) {
        let wa_h = f64::from(work_area_logical(app).1);
        let _ = panel.set_size(LogicalSize::new(PANEL_WIDTH, f64::from(layout.height).min(wa_h - 24.0)));
    }
    let v = view(app, notice);
    let _ = app.emit("editor-state", &v);
    v
}

/// Saves now; returns the save problem (if any) for the panel's message slot.
fn save_now(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let mut editor = state.editor.lock().unwrap();
    let s = editor.as_mut()?;
    if !s.dirty() {
        return None;
    }
    let started = std::time::Instant::now();
    let result = s.save(&mut RealFs);
    let bytes = tsp_core::profile::to_text(&s.loaded.doc).len();
    let path = s.path.clone();
    drop(editor);
    state.log.write(serde_json::json!({
        "event": "save", "bytes": bytes, "ms": started.elapsed().as_millis() as u64,
        "result": result.as_ref().map(|_| "ok").unwrap_or_else(|e| e.code()),
    }));
    result.err().map(|e| e.message(&path))
}

fn capture_target(app: &AppHandle) -> ReturnTarget {
    #[cfg(windows)]
    {
        let _ = app;
        let fg = win::WinForeground.current();
        let hwnd = if fg.is_pad { win::last_other() } else { fg.id as isize };
        let title = if hwnd == 0 { String::new() } else { win::describe_raw(hwnd).title };
        ReturnTarget { hwnd, title }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        ReturnTarget::default()
    }
}

/// Gives the foreground back to the captured target and checks it took.
fn restore_target(app: &AppHandle) -> Result<(), String> {
    let t = app.state::<AppState>().return_target.lock().unwrap().clone();
    #[cfg(windows)]
    {
        if win::window::activate(t.hwnd) && win::window::foreground() == t.hwnd {
            return Ok(());
        }
        Err(format!("restore-failed: couldn't return to \"{}\". Tap it before using the pad.", t.title))
    }
    #[cfg(not(windows))]
    {
        let _ = t;
        Ok(())
    }
}

fn open_panel(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(PANEL).is_some() {
        return Ok(());
    }
    let pad = app.get_webview_window("main").ok_or("pad window missing")?;
    let s = pad.scale_factor().map_err(|e| e.to_string())?;
    let pos = pad.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(s);
    let size = pad.inner_size().map_err(|e| e.to_string())?.to_logical::<f64>(s);
    let (wa_w, wa_h) = work_area_logical(app);
    let height = size.height.min(f64::from(wa_h) - 24.0);
    // Beside the pad on the side with room, preferring the side away from screen centre.
    let left_room = pos.x;
    let right_room = f64::from(wa_w) - (pos.x + size.width);
    let pad_on_right = pos.x + size.width / 2.0 > f64::from(wa_w) / 2.0;
    let x = if (pad_on_right && left_room >= PANEL_WIDTH + 8.0) || right_room < PANEL_WIDTH + 8.0 {
        (pos.x - PANEL_WIDTH - 8.0).max(0.0)
    } else {
        pos.x + size.width + 8.0
    };
    let panel = WebviewWindowBuilder::new(app, PANEL, WebviewUrl::App("editor.html".into()))
        .title("Touch Shortcuts editor")
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
    Ok(())
}

// ---------- commands ----------

#[tauri::command]
pub async fn edit_begin(app: AppHandle) -> Result<EditorView, EditFailure> {
    let state = app.state::<AppState>();
    if state.editor.lock().unwrap().is_some() {
        return Ok(view(&app, None));
    }
    let (loaded, path, fallback) = {
        let a = state.active.lock().unwrap();
        (a.loaded.clone(), a.path.clone(), a.fallback)
    };
    if fallback {
        return Err(plain("edit-active", "Edit (fix the profile error first)".into()));
    }
    // Stop everything and gate sends in one critical section.
    {
        let mut shared = state.shared.0.lock().unwrap();
        shared.editing = true;
        shared.queue.stop_all();
    }
    *state.return_target.lock().unwrap() = capture_target(&app);
    let identity = store::identity(&RealFs, &path).ok();
    let settings = state.settings.lock().unwrap().clone();
    *state.editor.lock().unwrap() = Some(Session::begin(loaded, path, identity, settings.allow_system_keys));
    let target = state.return_target.lock().unwrap().title.clone();
    // Place the pad at its edit-mode size first, so the panel goes beside it.
    publish(&app, None);
    let opened = open_panel(&app);
    state.log.write(serde_json::json!({ "event": "edit-begin", "target": target, "result": opened.as_ref().map(|_| "ok").unwrap_or("failed"), "error": opened.as_ref().err() }));
    if let Err(e) = opened {
        *state.editor.lock().unwrap() = None;
        state.shared.0.lock().unwrap().editing = false;
        publish(&app, None);
        return Err(plain("editor-open-failed", format!("editor-open-failed: {e}. See actions.log.")));
    }
    Ok(publish(&app, None))
}

#[tauri::command]
pub fn get_editor_state(app: AppHandle) -> EditorView {
    view(&app, None)
}

#[tauri::command]
pub fn edit_apply(app: AppHandle, op: Op, base_rev: u64) -> Result<EditorView, EditFailure> {
    let state = app.state::<AppState>();
    let result = {
        let mut editor = state.editor.lock().unwrap();
        let Some(s) = editor.as_mut() else { return Err(plain("edit-stale", "edit-stale: not editing.".into())) };
        s.apply(&op, base_rev, now_ms(&state))
    };
    state.log.write(serde_json::json!({
        "event": "edit-op", "kind": op.kind(), "buttonId": op.button_id(),
        "result": result.as_ref().map(|_| "ok").unwrap_or_else(|e| e.code),
    }));
    match result {
        Ok(r) => {
            let mut notice = r.notice;
            // Slider-driven pad changes are saved by the panel's debounce (edit_flush).
            if r.changed
                && !matches!(op, Op::SetPad { .. })
                && let Some(problem) = save_now(&app)
            {
                notice = Some(problem);
            }
            Ok(publish(&app, notice))
        }
        Err(e) => {
            let mut f = failure(e);
            f.view = Some(publish(&app, None));
            Err(f)
        }
    }
}

/// Saves pending edits (debounced slider changes, Retry, before Done/Quit/Reload).
#[tauri::command]
pub fn edit_flush(app: AppHandle) -> EditorView {
    let notice = save_now(&app);
    publish(&app, notice)
}

#[tauri::command]
pub fn edit_select(app: AppHandle, selection: Selection) -> EditorView {
    if let Some(s) = app.state::<AppState>().editor.lock().unwrap().as_mut() {
        s.select(selection);
    }
    publish(&app, None)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryStep {
    Undo,
    Redo,
    Revert,
    UndoDelete(String),
}

#[tauri::command]
pub fn edit_history(app: AppHandle, step: HistoryStep, base_rev: u64) -> Result<EditorView, EditFailure> {
    let state = app.state::<AppState>();
    let result = {
        let mut editor = state.editor.lock().unwrap();
        let Some(s) = editor.as_mut() else { return Err(plain("edit-stale", "edit-stale: not editing.".into())) };
        match &step {
            HistoryStep::Undo => s.undo(base_rev),
            HistoryStep::Redo => s.redo(base_rev),
            HistoryStep::Revert => s.revert(base_rev, now_ms(&state)),
            HistoryStep::UndoDelete(id) => s.undo_delete(id, base_rev),
        }
    };
    match result {
        Ok(true) => {
            let notice = save_now(&app);
            Ok(publish(&app, notice))
        }
        Ok(false) => Ok(view(&app, None)),
        Err(e) => Err(failure(e)),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictChoice {
    KeepMine,
    LoadFile,
}

#[tauri::command]
pub fn edit_conflict(app: AppHandle, choice: ConflictChoice) -> Result<EditorView, EditFailure> {
    let state = app.state::<AppState>();
    let result = {
        let mut editor = state.editor.lock().unwrap();
        let Some(s) = editor.as_mut() else { return Err(plain("edit-stale", "edit-stale: not editing.".into())) };
        let path = s.path.clone();
        match choice {
            ConflictChoice::KeepMine => s.keep_mine(&mut RealFs).map_err(|e| plain(e.code(), e.message(&path))),
            ConflictChoice::LoadFile => s.load_file(&mut RealFs).map_err(failure),
        }
    };
    state.log.write(serde_json::json!({ "event": "conflict", "choice": match choice { ConflictChoice::KeepMine => "keep", ConflictChoice::LoadFile => "load" }, "ok": result.is_ok() }));
    match result {
        Ok(()) => Ok(publish(&app, None)),
        Err(mut f) => {
            f.view = Some(publish(&app, None));
            Err(f)
        }
    }
}

/// Label tapped or Record started: the panel may take focus.
#[tauri::command]
pub fn edit_panel_activate(app: AppHandle) -> bool {
    #[cfg(windows)]
    {
        let Some(p) = app.get_webview_window(PANEL) else { return false };
        let Ok(h) = p.hwnd() else { return false };
        let h = h.0 as isize;
        win::window::set_no_activate(h, false);
        win::window::activate(h)
    }
    #[cfg(not(windows))]
    app.get_webview_window(PANEL).is_some_and(|p| p.set_focus().is_ok())
}

/// Label committed or Record ended: back to no-activate, focus back to the target.
#[tauri::command]
pub fn edit_panel_rearm(app: AppHandle) -> Option<String> {
    #[cfg(windows)]
    if let Some(Ok(h)) = app.get_webview_window(PANEL).map(|p| p.hwnd()) {
        win::window::set_no_activate(h.0 as isize, true);
    }
    restore_target(&app).err()
}

/// Done: refuse while a conflict or failed save is unresolved (the panel
/// shows the choice), else restore the target, close the panel, leave edit mode.
#[tauri::command]
pub async fn edit_done(app: AppHandle, discard: bool) -> Result<EditorView, EditFailure> {
    let state = app.state::<AppState>();
    if !discard {
        let problem = save_now(&app);
        let blocked = state.editor.lock().unwrap().as_ref().map(|s| (s.save_state.clone(), s.unsaved_ops()));
        match blocked {
            Some((SaveState::Conflict, _)) => {
                return Err(EditFailure { code: "profile-changed-on-disk".into(), message: "The pad file changed on disk.".into(), button: None, view: Some(publish(&app, None)) });
            }
            Some((SaveState::Failed(m), n)) if n > 0 => {
                return Err(EditFailure { code: "profile-save-failed".into(), message: problem.unwrap_or(m), button: None, view: Some(publish(&app, None)) });
            }
            _ => {}
        }
    }
    let restored = restore_target(&app);
    if let Some(p) = app.get_webview_window(PANEL) {
        let _ = p.destroy();
    }
    let session = state.editor.lock().unwrap().take();
    if discard {
        // Unsaved edits dropped: the active pad goes back to what is on disk.
        if let Some(s) = &session {
            if let Ok(bytes) = std::fs::read(&s.path) {
                let allow = state.settings.lock().unwrap().allow_system_keys;
                if let Ok(l) = tsp_core::profile::load_profile(&bytes, allow) {
                    state.active.lock().unwrap().loaded = l;
                }
            }
        }
    }
    state.shared.0.lock().unwrap().editing = false;
    state.log.write(serde_json::json!({
        "event": "edit-done",
        "ops": session.as_ref().map(|s| s.rev).unwrap_or(0),
        "saved": session.as_ref().is_some_and(|s| !s.dirty()),
        "restore": if restored.is_ok() { "ok" } else { "failed" },
    }));
    Ok(publish(&app, restored.err()))
}

/// Record from keyboard: a keydown → canonical combo text, or the message to show.
#[tauri::command]
pub fn edit_record(code: String, ctrl: bool, shift: bool, alt: bool, meta: bool) -> Result<String, String> {
    tsp_core::combo::record(&code, ctrl, shift, alt, meta)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// Every button's cells after the op: (id, [x, y, w, h]).
    cells: Vec<(String, [u32; 4])>,
}

/// Read-only: where every button would land if `op` (move/resize) were applied.
/// Same code path as `edit_apply`, nothing saved or recorded.
#[tauri::command]
pub fn edit_preview(app: AppHandle, op: Op) -> Result<Preview, EditFailure> {
    let state = app.state::<AppState>();
    let editor = state.editor.lock().unwrap();
    let Some(s) = editor.as_ref() else { return Err(plain("edit-stale", "edit-stale: not editing.".into())) };
    let allow = state.settings.lock().map(|x| x.allow_system_keys).unwrap_or(false);
    let applied = tsp_core::edit::apply(&s.loaded, &op, allow).map_err(failure)?;
    Ok(Preview { cells: applied.loaded.profile.buttons.iter().map(|b| (b.id.clone(), [b.x, b.y, b.w, b.h])).collect() })
}

/// Hides the panel while a drag is in progress, then shows it without activation.
#[tauri::command]
pub fn edit_panel_visible(app: AppHandle, visible: bool) {
    let Some(p) = app.get_webview_window(PANEL) else { return };
    #[cfg(windows)]
    if let Ok(h) = p.hwnd() {
        let h = h.0 as isize;
        if visible { win::window::show_no_activate(h) } else { win::window::hide(h) }
        return;
    }
    let _ = if visible { p.show() } else { p.hide() };
}

/// Windows is logging off or shutting down: never veto, never block. If the
/// session holds unsaved edits, write them beside the pad as
/// `<name>.unsaved-<unix time>.tsp.json` (falling back to the config dir).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn write_unsaved(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(editor) = state.editor.try_lock() else { return };
    let Some(s) = editor.as_ref().filter(|s| s.dirty()) else { return };
    let text = tsp_core::profile::to_text(&s.loaded.doc);
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let stem = s.path.file_name().map(|n| n.to_string_lossy().trim_end_matches(".tsp.json").to_string()).unwrap_or_else(|| "pad".into());
    let name = format!("{stem}.unsaved-{secs}.tsp.json");
    let beside = s.path.with_file_name(&name);
    if std::fs::write(&beside, &text).is_err() {
        let _ = std::fs::write(state.config_dir.join(&name), &text);
    }
}

/// Re-broadcasts the current state (after a pad switch or new pad).
pub fn refresh(app: &AppHandle, notice: Option<String>) -> EditorView {
    publish(app, notice)
}

/// Before leaving the current pad in edit mode: save, and refuse while a
/// conflict or failed save would lose edits.
pub fn flush_for_switch(app: &AppHandle) -> Result<(), String> {
    let problem = save_now(app);
    let state = app.state::<AppState>();
    let editor = state.editor.lock().unwrap();
    match editor.as_ref().map(|s| (&s.save_state, s.unsaved_ops())) {
        Some((SaveState::Conflict, _)) => Err("The pad file changed on disk: choose Keep my edits or Load file before switching.".into()),
        Some((SaveState::Failed(m), n)) if n > 0 => Err(problem.unwrap_or_else(|| m.clone())),
        _ => Ok(()),
    }
}

/// ··· → Reload while editing: save first, then load the file into the session.
pub fn reload_in_edit(app: &AppHandle) -> Result<PadView, String> {
    flush_for_switch(app)?;
    let state = app.state::<AppState>();
    let result = state.editor.lock().unwrap().as_mut().map(|s| s.load_file(&mut RealFs));
    if let Some(Err(e)) = result {
        return Err(e.message);
    }
    Ok(publish(app, None).pad)
}
