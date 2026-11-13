//! Pads: the list of pad files, switching between them, and New pad.
//!
//! The list is built in Rust and commands take an index into it, never a
//! path from the webview. Switching to a file that fails to load is refused
//! (the current pad stays), so a switch never lands on a fallback pad.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tsp_core::profile::{load_profile, to_text};
use tsp_core::session::Session;
use tsp_core::store::{self, RealFs};

use super::{Active, AppState, editing};

const EXAMPLE_V2: &str = include_str!("../../profiles/example-v2.tsp.json");

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PadEntry {
    index: usize,
    name: String,
    file: String,
    current: bool,
    /// Load error code when the file can't be opened.
    error: Option<String>,
    /// In the same folder as the current pad (so a go-to-pad button can point at it).
    sibling: bool,
}

#[derive(Default)]
pub struct PadList(pub Vec<PathBuf>);

/// `*.tsp.json` in the config dir plus the current pad, sorted by file name.
fn scan(config_dir: &Path, current: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(config_dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.to_string_lossy().ends_with(".tsp.json")).collect())
        .unwrap_or_default();
    if !out.iter().any(|p| p == current) {
        out.push(current.to_path_buf());
    }
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[tauri::command]
pub fn pads_list(app: AppHandle) -> Vec<PadEntry> {
    let state = app.state::<AppState>();
    let current = state.active.lock().unwrap().path.clone();
    let allow = state.settings.lock().unwrap().allow_system_keys;
    let paths = scan(&state.config_dir, &current);
    let entries = paths
        .iter()
        .enumerate()
        .map(|(index, p)| {
            let loaded = std::fs::read(p).map_err(|_| "profile-syntax").and_then(|b| load_profile(&b, allow).map_err(|e| e.code));
            PadEntry {
                index,
                name: loaded.as_ref().map(|l| l.profile.name.clone()).unwrap_or_else(|_| file_name(p)),
                file: file_name(p),
                current: *p == current,
                error: loaded.err().map(str::to_string),
                sibling: p.parent() == current.parent(),
            }
        })
        .collect();
    *state.pads.lock().unwrap() = PadList(paths);
    entries
}

/// Remembers the active pad in settings.json, keeping unknown keys. Never
/// writes after a failed read (a corrupt file is left for the user).
fn remember(config_dir: &Path, path: &Path) {
    let file = config_dir.join("settings.json");
    let mut value = match std::fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v @ serde_json::Value::Object(_)) => v,
            _ => return,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(_) => return,
    };
    value["lastProfile"] = serde_json::json!(path.display().to_string());
    let _ = std::fs::create_dir_all(config_dir);
    let _ = store::save(&mut RealFs, &file, &to_text(&value), None, false);
}

/// Makes `path` the active pad. In edit mode the current session is flushed
/// first and a fresh session (with fresh undo) starts on the new pad.
pub(crate) fn switch_to(app: &AppHandle, path: PathBuf) -> Result<(), String> {
    let state = app.state::<AppState>();
    let allow = state.settings.lock().unwrap().allow_system_keys;
    let bytes = std::fs::read(&path).map_err(|e| format!("profile-syntax {}: cannot read file: {e}. Fix the file, then pick it again in Pads.", path.display()))?;
    let loaded = load_profile(&bytes, allow).map_err(|e| format!("{} {}: {} Fix the file, then pick it again in Pads.", e.code, path.display(), e.message))?;
    let editing = state.editor.lock().unwrap().is_some();
    if editing {
        editing::flush_for_switch(app)?;
    }
    let warnings = loaded.warnings.iter().map(|w| format!("{} {}", w.code, w.message)).collect();
    if editing {
        let identity = store::identity(&RealFs, &path).ok();
        *state.editor.lock().unwrap() = Some(Session::begin(loaded.clone(), path.clone(), identity, allow));
    }
    *state.active.lock().unwrap() = Active { loaded, path: path.clone(), fallback: false, warnings };
    state.settings.lock().unwrap().last_profile = Some(path.clone());
    remember(&state.config_dir, &path);
    state.shared.0.lock().unwrap().queue.clear_pending();
    state.log.write(serde_json::json!({ "event": "pads-switch", "file": file_name(&path) }));
    Ok(())
}

#[tauri::command]
pub async fn pads_switch(app: AppHandle, index: usize) -> Result<editing::EditorView, String> {
    let path = app.state::<AppState>().pads.lock().unwrap().0.get(index).cloned().ok_or("That pad is no longer in the list. Open Pads again.")?;
    switch_to(&app, path)?;
    Ok(editing::refresh(&app, None))
}

#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    R1Numpad,
    Blank4x8,
    Blank3x10,
}

fn preset_doc(preset: Preset, name: &str) -> serde_json::Value {
    let mut doc = match preset {
        Preset::R1Numpad => serde_json::from_str(EXAMPLE_V2).expect("shipped example is valid JSON (tested)"),
        Preset::Blank4x8 => serde_json::json!({ "schema": 2, "name": "", "pad": { "grid": { "columns": 4, "rows": 8 }, "cellWidth": 69, "cellHeight": 76, "gap": 8 }, "buttons": [] }),
        Preset::Blank3x10 => serde_json::json!({ "schema": 2, "name": "", "pad": { "grid": { "columns": 3, "rows": 10 }, "cellWidth": 92, "cellHeight": 76, "gap": 8 }, "buttons": [] }),
    };
    doc["name"] = serde_json::json!(name);
    doc
}

/// Writes `pad-N.tsp.json` (smallest unused N) from a preset, makes it the
/// active pad and enters edit mode on it. Never overwrites a file.
#[tauri::command]
pub async fn pad_new(app: AppHandle, preset: Preset) -> Result<editing::EditorView, String> {
    let state = app.state::<AppState>();
    let dir = state.config_dir.clone();
    let n = (1..).find(|n| !dir.join(format!("pad-{n}.tsp.json")).exists()).expect("unbounded");
    let path = dir.join(format!("pad-{n}.tsp.json"));
    let text = to_text(&preset_doc(preset, &format!("New pad {n}")));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()))
        .map_err(|e| format!("pad-create-failed: couldn't write {} ({e}). Current pad unchanged.", path.display()))?;
    state.log.write(serde_json::json!({ "event": "pad-new", "file": file_name(&path) }));
    switch_to(&app, path)?;
    if state.editor.lock().unwrap().is_none() {
        return editing::edit_begin(app.clone()).await.map_err(|f| f.message);
    }
    Ok(editing::refresh(&app, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_a_valid_pad() {
        for p in [Preset::R1Numpad, Preset::Blank4x8, Preset::Blank3x10] {
            let doc = preset_doc(p, "New pad 1");
            let l = load_profile(to_text(&doc).as_bytes(), false).unwrap();
            assert_eq!(l.profile.name, "New pad 1");
            assert!(l.warnings.is_empty());
        }
    }

    #[test]
    fn scan_includes_the_current_pad_and_sorts() {
        let dir = std::env::temp_dir().join(format!("tsp-pads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.tsp.json"), "{}").unwrap();
        std::fs::write(dir.join("A.tsp.json"), "{}").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let elsewhere = PathBuf::from("/elsewhere/c.tsp.json");
        let got: Vec<String> = scan(&dir, &elsewhere).iter().map(|p| file_name(p)).collect();
        assert_eq!(got, ["A.tsp.json", "b.tsp.json", "c.tsp.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remember_keeps_unknown_keys_and_never_writes_a_corrupt_file() {
        let dir = std::env::temp_dir().join(format!("tsp-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"allowSystemKeys":true,"future":1}"#).unwrap();
        remember(&dir, Path::new("/x/pad-1.tsp.json"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!((v["future"].clone(), v["allowSystemKeys"].clone(), v["lastProfile"].clone()), (serde_json::json!(1), serde_json::json!(true), serde_json::json!("/x/pad-1.tsp.json")));
        std::fs::write(dir.join("settings.json"), "{ broken").unwrap();
        remember(&dir, Path::new("/y.tsp.json"));
        assert_eq!(std::fs::read_to_string(dir.join("settings.json")).unwrap(), "{ broken");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
