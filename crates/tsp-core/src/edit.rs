//! Edit operations on the raw profile document. Pure: raw doc in, raw doc
//! out, no file system and no clock (callers pass `now_ms`).
//!
//! Every op patches only the fields it changes, then the whole document goes
//! back through [`load_doc`], so an edit can never produce a file the loader
//! would reject. Unknown fields and disabled actions survive untouched.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::combo::{Combo, canonical_text, parse_combo, same_keys};
use crate::geometry::Cells;
use crate::layout::{self, Item, LayoutError};
use crate::profile::{Action, Button, Loaded, MAX_BUTTONS, MAX_PROFILE_BYTES, Profile, bounds, load_doc, page_target, snap_hundredths, to_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    pub code: &'static str,
    pub message: String,
    /// The button the refusal is about (selected and outlined by the UI).
    pub button: Option<String>,
}

fn refuse(code: &'static str, message: String) -> EditError {
    EditError { code, message, button: None }
}

/// Keys action as chosen in the picker or recorded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ActionSpec {
    Keys { combo: String, repeat: u32 },
    /// Go to another pad: "back" or a pad file name in the same folder.
    /// `name` (the target pad's name) is used for the auto label.
    Page { target: String, name: Option<String> },
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PadChange {
    pub name: Option<String>,
    pub columns: Option<u32>,
    pub rows: Option<u32>,
    pub cell_width: Option<u32>,
    pub cell_height: Option<u32>,
    pub gap: Option<u32>,
    pub scale: Option<f64>,
    pub pad_opacity: Option<f64>,
    pub background_opacity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Op {
    Move { button_id: String, x: u32, y: u32 },
    Resize { button_id: String, w: u32, h: u32 },
    SetLabel { button_id: String, label: String },
    SetAction { button_id: String, action: ActionSpec },
    SetColor { button_id: String, color: Option<String> },
    SetIcon { button_id: String, icon: Option<String> },
    SetGroup { button_id: String, group: String },
    /// "small" | "medium" | "large" (None = default large).
    SetTextSize { button_id: String, size: Option<String> },
    /// At a selected empty cell, or (None) at the first free cell.
    Add { at: Option<(u32, u32)> },
    Duplicate { button_id: String, mirror: bool },
    Delete { button_id: String },
    SetPad { change: PadChange },
}

impl Op {
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Move { .. } => "move",
            Op::Resize { .. } => "resize",
            Op::SetLabel { .. } => "set_label",
            Op::SetAction { .. } => "set_action",
            Op::SetColor { .. } => "set_color",
            Op::SetIcon { .. } => "set_icon",
            Op::SetGroup { .. } => "set_group",
            Op::SetTextSize { .. } => "set_text_size",
            Op::Add { .. } => "add",
            Op::Duplicate { .. } => "duplicate",
            Op::Delete { .. } => "delete",
            Op::SetPad { .. } => "set_pad",
        }
    }

    pub fn button_id(&self) -> Option<&str> {
        match self {
            Op::Move { button_id, .. }
            | Op::Resize { button_id, .. }
            | Op::SetLabel { button_id, .. }
            | Op::SetAction { button_id, .. }
            | Op::SetColor { button_id, .. }
            | Op::SetIcon { button_id, .. }
            | Op::SetGroup { button_id, .. }
            | Op::SetTextSize { button_id, .. }
            | Op::Duplicate { button_id, .. }
            | Op::Delete { button_id } => Some(button_id),
            Op::Add { .. } | Op::SetPad { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub loaded: Loaded,
    /// False when the op changed nothing (e.g. a drop on the origin): no save, no undo step.
    pub changed: bool,
    /// Button to select afterwards (new button for add/duplicate).
    pub select: Option<String>,
    /// One-line message for the panel (row added, deleted, ...).
    pub notice: Option<String>,
}

// ---------- raw document helpers ----------

fn buttons(doc: &Value) -> &[Value] {
    doc.get("buttons").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn buttons_mut(doc: &mut Value) -> &mut Vec<Value> {
    let top = doc.as_object_mut().expect("validated profile is an object");
    if !matches!(top.get("buttons"), Some(Value::Array(_))) {
        top.insert("buttons".into(), json!([]));
    }
    top.get_mut("buttons").and_then(Value::as_array_mut).expect("just inserted")
}

fn button_obj(doc: &mut Value, index: usize) -> &mut Map<String, Value> {
    buttons_mut(doc)[index].as_object_mut().expect("validated button is an object")
}

/// Sets `key`, replacing it in place if present, else inserting it right
/// after the last present key of `after` (or at the end).
fn put(obj: &mut Map<String, Value>, key: &str, value: Value, after: &[&str]) {
    if let Some(slot) = obj.get_mut(key) {
        *slot = value;
        return;
    }
    let anchor = obj.keys().rposition(|k| after.contains(&k.as_str()));
    let Some(pos) = anchor else {
        obj.insert(key.into(), value);
        return;
    };
    let old = std::mem::take(obj);
    for (i, (k, v)) in old.into_iter().enumerate() {
        obj.insert(k, v);
        if i == pos {
            obj.insert(key.into(), value.clone());
        }
    }
}

/// Writes a rect, touching only fields that change; w/h of 1 stay implicit when absent.
fn put_rect(obj: &mut Map<String, Value>, old: Cells, new: Cells) {
    if new.x != old.x || !obj.contains_key("x") {
        put(obj, "x", json!(new.x), &["label", "id"]);
    }
    if new.y != old.y || !obj.contains_key("y") {
        put(obj, "y", json!(new.y), &["x"]);
    }
    if new.w != old.w || (new.w != 1 && !obj.contains_key("w")) {
        put(obj, "w", json!(new.w), &["y", "x"]);
    }
    if new.h != old.h || (new.h != 1 && !obj.contains_key("h")) {
        put(obj, "h", json!(new.h), &["w", "y", "x"]);
    }
}

fn rect(b: &Button) -> Cells {
    Cells { x: b.x, y: b.y, w: b.w, h: b.h }
}

fn index_of(p: &Profile, id: &str) -> Result<usize, EditError> {
    p.buttons.iter().position(|b| b.id == id).ok_or_else(|| refuse("edit-stale", format!("edit-stale: no button \"{id}\" on this pad any more. Tap it again.")))
}

fn layout_error(e: LayoutError, p: &Profile) -> EditError {
    match e {
        LayoutError::OutOfGrid => refuse("layout-out-of-grid", "layout-out-of-grid: that position is outside the grid. Nothing moved.".into()),
        LayoutError::GridFull { id } => {
            let b = p.buttons.iter().find(|b| b.id == id);
            let (label, size) = b.map_or((id.clone(), String::new()), |b| (b.label.clone(), format!(" ({}×{})", b.w, b.h)));
            EditError {
                code: "layout-grid-full",
                message: format!("layout-grid-full: no room for \"{label}\"{size}. Nothing moved. Add a row, or shrink or delete a button."),
                button: Some(id),
            }
        }
    }
}

// ---------- labels ----------

fn key_display(name: &str) -> String {
    match name {
        "Up" => "↑".into(),
        "Down" => "↓".into(),
        "Left" => "←".into(),
        "Right" => "→".into(),
        "PageUp" => "PgUp".into(),
        "PageDown" => "PgDn".into(),
        "NumDecimal" => ".".into(),
        "Plus" => "+".into(),
        "Minus" => "−".into(),
        n if n.starts_with("Num") && n.len() == 4 => n[3..].into(),
        n => n.into(),
    }
}

/// Label that follows the combo until the user types one, e.g. "Ctrl+↓ ×10".
pub fn auto_label(combo: &Combo, repeat: u32) -> String {
    let text = canonical_text(combo);
    let mut parts: Vec<&str> = text.split('+').collect();
    parts.pop();
    let mut out = parts.iter().map(|m| format!("{m}+")).collect::<String>();
    out.push_str(&key_display(combo.key.name));
    if repeat > 1 {
        out.push_str(&format!(" ×{repeat}"));
    }
    out.chars().take(bounds::LABEL_CHARS).collect()
}

/// Auto label for a go-to-pad button: "→ Back", "→ <pad name>", or the file name without .tsp.json.
pub fn page_label(target: &str, name: Option<&str>) -> String {
    let shown = if target == "back" { "Back" } else { name.unwrap_or_else(|| target.trim_end_matches(".tsp.json")) };
    format!("→ {shown}").chars().take(bounds::LABEL_CHARS).collect()
}

fn raw_auto_label(action: Option<&Value>) -> Option<String> {
    let a = action?.as_object()?;
    if a.get("type")?.as_str()? == "page" {
        return Some(page_label(a.get("target")?.as_str()?, None));
    }
    if a.get("type")?.as_str()? != "keys" {
        return None;
    }
    let combo = parse_combo(a.get("combo")?.as_str()?).ok()?;
    let repeat = a.get("repeat").and_then(Value::as_u64).unwrap_or(1) as u32;
    Some(auto_label(&combo, repeat))
}

// ---------- mirror ----------

const KEY_MIRRORS: &[(&str, &str)] = &[("Up", "Down"), ("Left", "Right"), ("PageUp", "PageDown"), ("Plus", "Minus")];

fn mirror_name(name: &str) -> Option<&'static str> {
    KEY_MIRRORS.iter().find_map(|&(a, b)| if a == name { Some(b) } else if b == name { Some(a) } else { None })
}

/// The key name "Duplicate as <mirror>" would use, when the button has one.
pub fn mirror_of(b: &Button) -> Option<&'static str> {
    match &b.action {
        Action::Keys { combo, .. } => mirror_name(combo.key.name),
        _ => None,
    }
}

/// Label word pairs swapped for each mirrored key (whole words only).
fn label_pairs(key: &str) -> &'static [(&'static str, &'static str)] {
    match key {
        "Up" | "Down" => &[("Up", "Down"), ("↑", "↓")],
        "Left" | "Right" => &[("Left", "Right"), ("←", "→")],
        "PageUp" | "PageDown" => &[("PgUp", "PgDn"), ("PageUp", "PageDown")],
        _ => &[("Plus", "Minus")],
    }
}

fn match_case(word: &str, like: &str) -> String {
    if like.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()) && like.chars().any(char::is_alphabetic) {
        word.to_uppercase()
    } else if like.chars().all(|c| !c.is_alphabetic() || c.is_lowercase()) && like.chars().any(char::is_alphabetic) {
        word.to_lowercase()
    } else {
        word.to_string()
    }
}

/// Swaps the mirrored key's word pair in a typed label, whole-word and
/// case-preserving. "+" and "−"/"-" swap only when they are the whole label.
pub fn mirror_label(label: &str, key: &str) -> String {
    if matches!(key, "Plus" | "Minus") {
        match label {
            "+" => return "−".into(),
            "−" | "-" => return "+".into(),
            _ => {}
        }
    }
    let pairs = label_pairs(key);
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let swapped = pairs.iter().find_map(|&(a, b)| {
            if word.eq_ignore_ascii_case(a) {
                Some(match_case(b, word))
            } else if word.eq_ignore_ascii_case(b) {
                Some(match_case(a, word))
            } else {
                None
            }
        });
        out.push_str(&swapped.unwrap_or_else(|| word.clone()));
        word.clear();
    };
    for ch in label.chars() {
        let arrow = matches!(ch, '↑' | '↓' | '←' | '→');
        if ch.is_alphanumeric() && !arrow {
            word.push(ch);
        } else {
            flush(&mut word, &mut out);
            if arrow {
                word.push(ch);
                flush(&mut word, &mut out);
            } else {
                out.push(ch);
            }
        }
    }
    flush(&mut word, &mut out);
    out
}

fn mirror_icon(icon: &str, key: &str) -> Option<&'static str> {
    match (key, icon) {
        ("Up" | "Down", "up") => Some("down"),
        ("Up" | "Down", "down") => Some("up"),
        ("Left" | "Right", "left") => Some("right"),
        ("Left" | "Right", "right") => Some("left"),
        _ => None,
    }
}

// ---------- ids ----------

fn next_btn_id(p: &Profile) -> String {
    (1..).map(|n| format!("btn-{n}")).find(|id| p.buttons.iter().all(|b| &b.id != id)).expect("unbounded")
}

/// `<base>-2`, `-3`, ... with the base trimmed so the suffix always survives.
pub fn duplicate_id(base: &str, taken: &[&str]) -> String {
    (2..)
        .map(|n| {
            let suffix = format!("-{n}");
            let keep = bounds::ID_CHARS - suffix.len();
            let stem: String = base.chars().take(keep).collect();
            format!("{}{suffix}", stem.trim_end_matches('-'))
        })
        .find(|id| !taken.contains(&id.as_str()))
        .expect("unbounded")
}

// ---------- apply ----------

/// Applies `op` to the current document. On success the result has passed
/// the loader; on refusal nothing changed.
pub fn apply(current: &Loaded, op: &Op, allow_system_keys: bool) -> Result<Applied, EditError> {
    let p = &current.profile;
    let mut doc = current.doc.clone();
    let mut select = op.button_id().map(str::to_string);
    let mut notice = None;
    match op {
        Op::Move { button_id, x, y } | Op::Resize { button_id, w: x, h: y } => {
            let i = index_of(p, button_id)?;
            let old = rect(&p.buttons[i]);
            let target = match op {
                Op::Move { .. } => Cells { x: *x, y: *y, ..old },
                _ => Cells { w: *x, h: *y, ..old },
            };
            if target == old {
                return Ok(Applied { loaded: current.clone(), changed: false, select, notice });
            }
            let items: Vec<Item> = p.buttons.iter().map(|b| Item { id: b.id.clone(), rect: rect(b) }).collect();
            let out = layout::place(p.pad.columns, p.pad.rows, &items, i, target).map_err(|e| layout_error(e, p))?;
            for (j, new) in out.into_iter().enumerate() {
                if new != items[j].rect {
                    put_rect(button_obj(&mut doc, j), items[j].rect, new);
                }
            }
        }
        Op::SetLabel { button_id, label } => {
            let i = index_of(p, button_id)?;
            put(button_obj(&mut doc, i), "label", json!(label), &["id"]);
        }
        Op::SetAction { button_id, action } => {
            let i = index_of(p, button_id)?;
            let obj = button_obj(&mut doc, i);
            let follows = {
                let label = obj.get("label").and_then(Value::as_str).unwrap_or("");
                label == "New" || raw_auto_label(obj.get("action")).as_deref() == Some(label)
            };
            match action {
                ActionSpec::None => {
                    obj.insert("action".into(), json!({ "type": "none" }));
                }
                ActionSpec::Page { target, name } => {
                    if page_target(target).is_none() {
                        return Err(refuse("action-unknown", format!("action-unknown: \"{target}\" isn't a pad file in this folder.")));
                    }
                    obj.insert("action".into(), json!({ "type": "page", "target": target }));
                    // Follows while the label is "New", a keys auto label, or any "→ …" page label.
                    let page_auto = obj.get("label").and_then(Value::as_str).is_some_and(|l| l.starts_with("→ "));
                    if follows || page_auto {
                        put(obj, "label", json!(page_label(target, name.as_deref())), &["id"]);
                    }
                }
                ActionSpec::Keys { combo, repeat } => {
                    let parsed = parse_combo(combo).map_err(|_| refuse("key-unknown", format!("key-unknown: \"{combo}\" is not a key this version can send.")))?;
                    let text = canonical_text(&parsed);
                    let same_type = obj.get("action").and_then(|a| a.get("type")).and_then(Value::as_str) == Some("keys");
                    if same_type {
                        let a = obj.get_mut("action").and_then(Value::as_object_mut).expect("checked");
                        put(a, "combo", json!(text), &["type"]);
                        if *repeat != 1 || a.contains_key("repeat") {
                            put(a, "repeat", json!(repeat), &["combo"]);
                        }
                    } else {
                        let mut a = Map::new();
                        a.insert("type".into(), json!("keys"));
                        a.insert("combo".into(), json!(text));
                        if *repeat != 1 {
                            a.insert("repeat".into(), json!(repeat));
                        }
                        obj.insert("action".into(), Value::Object(a));
                    }
                    if follows {
                        put(obj, "label", json!(auto_label(&parsed, *repeat)), &["id"]);
                    }
                }
            }
        }
        Op::SetColor { button_id, color } => {
            let i = index_of(p, button_id)?;
            let obj = button_obj(&mut doc, i);
            match color {
                Some(c) => put(obj, "color", json!(c), &["icon", "group", "label"]),
                None => {
                    obj.shift_remove("color");
                }
            }
        }
        Op::SetIcon { button_id, icon } => {
            let i = index_of(p, button_id)?;
            let obj = button_obj(&mut doc, i);
            match icon {
                Some(name) => put(obj, "icon", json!(name), &["label"]),
                None => {
                    obj.shift_remove("icon");
                }
            }
        }
        Op::SetTextSize { button_id, size } => {
            let i = index_of(p, button_id)?;
            let obj = button_obj(&mut doc, i);
            match size.as_deref() {
                None | Some("large") => {
                    obj.shift_remove("textSize");
                }
                Some(s) => put(obj, "textSize", json!(s), &["color", "icon", "group", "label"]),
            }
        }
        Op::SetGroup { button_id, group } => {
            let i = index_of(p, button_id)?;
            put(button_obj(&mut doc, i), "group", json!(group), &["h", "w", "y", "x", "label"]);
        }
        Op::Add { at } => {
            if p.buttons.len() >= MAX_BUTTONS {
                return Err(refuse("edit-max-buttons", format!("edit-max-buttons: this pad has {MAX_BUTTONS} buttons (the maximum). Delete one first.")));
            }
            let taken: Vec<Cells> = p.buttons.iter().map(rect).collect();
            let (w, h) = most_common_size(p);
            let fits = |r: Cells| layout::inside(&r, p.pad.columns, p.pad.rows) && taken.iter().all(|t| !overlaps(t, &r));
            let mut rows = p.pad.rows;
            let spot = match at {
                Some((x, y)) => [Cells { x: *x, y: *y, w, h }, Cells { x: *x, y: *y, w: 1, h: 1 }].into_iter().find(|r| fits(*r)),
                None => layout::first_free(p.pad.columns, p.pad.rows, &taken, w, h).or_else(|| layout::first_free(p.pad.columns, p.pad.rows, &taken, 1, 1)),
            };
            let spot = match spot {
                Some(s) => s,
                None if at.is_none() && p.pad.rows < bounds::ROWS.1 as u32 => {
                    rows += 1;
                    notice = Some(format!("No empty cell: added row {rows}"));
                    Cells { x: 0, y: p.pad.rows, w: w.min(p.pad.columns), h: 1 }
                }
                None => return Err(refuse("layout-grid-full", "layout-grid-full: no room for a new button. Nothing moved. Add a row, or shrink or delete a button.".into())),
            };
            if rows != p.pad.rows {
                set_grid(&mut doc, p.pad.columns, rows);
            }
            let id = next_btn_id(p);
            let mut obj = Map::new();
            obj.insert("id".into(), json!(id));
            obj.insert("label".into(), json!("New"));
            put_rect(&mut obj, Cells { x: u32::MAX, y: u32::MAX, w: 1, h: 1 }, spot);
            obj.insert("action".into(), json!({ "type": "none" }));
            buttons_mut(&mut doc).push(Value::Object(obj));
            select = Some(id);
        }
        Op::Duplicate { button_id, mirror } => {
            if p.buttons.len() >= MAX_BUTTONS {
                return Err(refuse("edit-max-buttons", format!("edit-max-buttons: this pad has {MAX_BUTTONS} buttons (the maximum). Delete one first.")));
            }
            let i = index_of(p, button_id)?;
            let src = &p.buttons[i];
            let taken: Vec<Cells> = p.buttons.iter().map(rect).collect();
            let spot = layout::nearest_free(p.pad.columns, p.pad.rows, &taken, rect(src)).ok_or_else(|| EditError {
                code: "layout-grid-full",
                message: format!("layout-grid-full: no room for \"{}\" ({}×{}). Nothing moved. Add a row, or shrink or delete a button.", src.label, src.w, src.h),
                button: Some(src.id.clone()),
            })?;
            let ids: Vec<&str> = p.buttons.iter().map(|b| b.id.as_str()).collect();
            let id = duplicate_id(&src.id, &ids);
            let mut obj = buttons(&doc)[i].as_object().cloned().expect("validated");
            put(&mut obj, "id", json!(id), &[]);
            put_rect(&mut obj, rect(src), spot);
            if *mirror {
                let Some(to) = mirror_of(src) else {
                    return Err(refuse("key-unknown", format!("\"{}\" has no mirror key.", src.label)));
                };
                let Action::Keys { combo, repeat, .. } = &src.action else { unreachable!("mirror_of checked") };
                let from = combo.key.name;
                let mirrored = parse_combo(&canonical_text(combo).replace(from, to)).expect("mirror keys are in the grammar");
                let label_was_auto = src.label == auto_label(combo, *repeat);
                let a = obj.get_mut("action").and_then(Value::as_object_mut).expect("keys action");
                put(a, "combo", json!(canonical_text(&mirrored)), &["type"]);
                let label = if label_was_auto { auto_label(&mirrored, *repeat) } else { mirror_label(&src.label, from) };
                put(&mut obj, "label", json!(label), &["id"]);
                if let Some(icon) = obj.get("icon").and_then(Value::as_str).and_then(|i| mirror_icon(i, from)) {
                    put(&mut obj, "icon", json!(icon), &["label"]);
                }
            }
            buttons_mut(&mut doc).push(Value::Object(obj));
            select = Some(id);
        }
        Op::Delete { button_id } => {
            let i = index_of(p, button_id)?;
            buttons_mut(&mut doc).remove(i);
            notice = Some(format!("Deleted '{}' · Undo edit", p.buttons[i].label));
            select = None;
        }
        Op::SetPad { change } => {
            let top = doc.as_object_mut().expect("validated");
            if let Some(name) = &change.name {
                top.insert("name".into(), json!(name));
            }
            let (columns, rows) = (change.columns.unwrap_or(p.pad.columns), change.rows.unwrap_or(p.pad.rows));
            if (columns, rows) != (p.pad.columns, p.pad.rows) {
                let mut outside: Vec<&Button> = p.buttons.iter().filter(|b| b.x + b.w > columns || b.y + b.h > rows).collect();
                outside.sort_by_key(|b| (b.y, b.x));
                if let Some(b) = outside.first() {
                    return Err(EditError {
                        code: "layout-button-outside-grid",
                        message: format!("layout-button-outside-grid: \"{}\" would fall outside a {columns}×{rows} grid. Move or shrink it first.", b.label),
                        button: Some(b.id.clone()),
                    });
                }
                set_grid(&mut doc, columns, rows);
            }
            let pad = doc.get_mut("pad").and_then(Value::as_object_mut).expect("validated");
            let ints = [("cellWidth", change.cell_width), ("cellHeight", change.cell_height), ("gap", change.gap)];
            for (key, v) in ints {
                if let Some(v) = v {
                    put(pad, key, json!(v), &["grid", "cellWidth", "cellHeight"]);
                }
            }
            let nums = [("scale", change.scale), ("padOpacity", change.pad_opacity), ("backgroundOpacity", change.background_opacity)];
            for (key, v) in nums {
                if let Some(v) = v {
                    put(pad, key, json!(snap_hundredths(v)), &["gap", "scale", "padOpacity"]);
                }
            }
        }
    }
    let loaded = finish(current, doc, op.button_id(), allow_system_keys)?;
    let changed = loaded.doc != current.doc;
    Ok(Applied { loaded, changed, select, notice })
}

fn overlaps(a: &Cells, b: &Cells) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn set_grid(doc: &mut Value, columns: u32, rows: u32) {
    let pad = doc.get_mut("pad").and_then(Value::as_object_mut).expect("validated");
    let grid = pad.entry("grid").or_insert_with(|| json!({}));
    if let Some(g) = grid.as_object_mut() {
        put(g, "columns", json!(columns), &[]);
        put(g, "rows", json!(rows), &["columns"]);
    }
}

/// Most common button size on the pad (ties: the one seen first); 1×1 when empty.
fn most_common_size(p: &Profile) -> (u32, u32) {
    let mut counts: Vec<((u32, u32), usize)> = Vec::new();
    for b in &p.buttons {
        match counts.iter_mut().find(|(s, _)| *s == (b.w, b.h)) {
            Some((_, n)) => *n += 1,
            None => counts.push(((b.w, b.h), 1)),
        }
    }
    counts.iter().rev().max_by_key(|(_, n)| *n).map_or((1, 1), |(s, _)| *s)
}

/// Validates the edited document and applies the edit-only refusals.
fn finish(before: &Loaded, doc: Value, edited: Option<&str>, allow_system_keys: bool) -> Result<Loaded, EditError> {
    let text = to_text(&doc);
    if text.len() > MAX_PROFILE_BYTES {
        return Err(refuse("profile-too-large", format!("profile-too-large: the pad would be {} bytes (allowed up to {MAX_PROFILE_BYTES}). Delete a button first.", text.len())));
    }
    let after = load_doc(doc, allow_system_keys).map_err(|e| refuse(e.code, format!("{}: {}", e.code, e.message)))?;
    if let Some(id) = edited {
        let was_disabled = before.profile.buttons.iter().find(|b| b.id == id).is_some_and(|b| matches!(b.action, Action::Disabled { .. }));
        if let Some(Action::Disabled { code, reason }) = after.profile.buttons.iter().find(|b| b.id == id).map(|b| &b.action)
            && !was_disabled
        {
            return Err(EditError { code, message: format!("{code}: {reason}"), button: Some(id.to_string()) });
        }
    }
    if after.warnings.len() > before.warnings.len() {
        let w = after.warnings.iter().find(|w| !before.warnings.contains(w)).unwrap_or(&after.warnings[0]);
        return Err(refuse(w.code, format!("{}: {}", w.code, w.message)));
    }
    Ok(after)
}

/// Other buttons sending the same keys (parsed modifier set + key, and
/// repeat): (label, 1-based row) in reading order.
pub fn same_combo(p: &Profile, id: &str) -> Vec<(String, u32)> {
    let Some(Button { action: Action::Keys { combo, repeat, .. }, .. }) = p.buttons.iter().find(|b| b.id == id) else {
        return Vec::new();
    };
    let mut hits: Vec<&Button> = p
        .buttons
        .iter()
        .filter(|b| b.id != id)
        .filter(|b| matches!(&b.action, Action::Keys { combo: c, repeat: r, .. } if same_keys(c, combo) && r == repeat))
        .collect();
    hits.sort_by_key(|b| (b.y, b.x));
    hits.into_iter().map(|b| (b.label.clone(), b.y + 1)).collect()
}

// ---------- undo / redo ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Label,
    Delete,
    Other,
}

#[derive(Debug, Clone)]
struct Step {
    doc: Value,
    kind: StepKind,
    button: Option<String>,
    at_ms: u64,
}

/// Snapshot undo/redo for one pad. Clear it whenever the active pad changes.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    pub const CAP: usize = 100;
    /// Label commits on the same button within this window form one step.
    pub const LABEL_COALESCE_MS: u64 = 2000;

    /// Records the document as it was before a successful, changing op.
    pub fn record(&mut self, before: Value, kind: StepKind, button: Option<&str>, now_ms: u64) {
        self.redo.clear();
        if kind == StepKind::Label
            && let Some(last) = self.undo.last_mut()
            && last.kind == StepKind::Label
            && last.button.as_deref() == button
            && now_ms.saturating_sub(last.at_ms) <= Self::LABEL_COALESCE_MS
        {
            last.at_ms = now_ms;
            return;
        }
        self.undo.push(Step { doc: before, kind, button: button.map(str::to_string), at_ms: now_ms });
        if self.undo.len() > Self::CAP {
            self.undo.remove(0);
        }
    }

    /// Returns the document to restore, saving `current` for redo.
    pub fn undo(&mut self, current: Value) -> Option<Value> {
        let step = self.undo.pop()?;
        self.redo.push(Step { doc: current, kind: StepKind::Other, button: None, at_ms: step.at_ms });
        Some(step.doc)
    }

    pub fn redo(&mut self, current: Value) -> Option<Value> {
        let step = self.redo.pop()?;
        self.undo.push(Step { doc: current, kind: StepKind::Other, button: None, at_ms: step.at_ms });
        Some(step.doc)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The delete notice's Undo acts only while that delete is the latest step.
    pub fn last_is_delete_of(&self, id: &str) -> bool {
        self.undo.last().is_some_and(|s| s.kind == StepKind::Delete && s.button.as_deref() == Some(id))
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

pub fn step_kind(op: &Op) -> StepKind {
    match op {
        Op::SetLabel { .. } => StepKind::Label,
        Op::Delete { .. } => StepKind::Delete,
        _ => StepKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{PageTarget, load_profile};

    const PAD: &str = r#"{
  "schema": 2,
  "name": "T",
  "future": {"keep": true},
  "pad": {"grid": {"columns": 4, "rows": 8}, "cellWidth": 69, "cellHeight": 76, "gap": 8, "padExtra": 1},
  "buttons": [
    {"id": "a", "label": "A", "x": 0, "y": 0, "action": {"type": "keys", "combo": "A"}},
    {"id": "b", "label": "B", "x": 1, "y": 0, "action": {"type": "keys", "combo": "B", "intervalMs": 40, "later": 1}},
    {"id": "up", "label": "Up ×10", "icon": "up", "x": 0, "y": 1, "w": 2, "action": {"type": "keys", "combo": "Up", "repeat": 10}},
    {"id": "run", "label": "Run", "x": 3, "y": 7, "action": {"type": "keys", "combo": "Win+R"}},
    {"id": "q", "label": "Go", "x": 2, "y": 7, "action": {"type": "qlab", "cue": "1"}}
  ]
}"#;

    fn pad() -> Loaded {
        load_profile(PAD.as_bytes(), false).unwrap()
    }

    fn ok(l: &Loaded, op: Op) -> Applied {
        apply(l, &op, false).unwrap_or_else(|e| panic!("{op:?}: {e:?}"))
    }

    fn btn<'a>(a: &'a Applied, id: &str) -> &'a Button {
        a.loaded.profile.buttons.iter().find(|b| b.id == id).unwrap()
    }

    #[test]
    fn move_patches_only_rects_and_keeps_everything_else() {
        let l = pad();
        let a = ok(&l, Op::Move { button_id: "a".into(), x: 1, y: 0 });
        assert!(a.changed);
        assert_eq!((btn(&a, "a").x, btn(&a, "b").x), (1, 0), "swap");
        assert_eq!(a.loaded.doc["future"], json!({"keep": true}));
        assert_eq!(a.loaded.doc["pad"]["padExtra"], 1);
        assert_eq!(a.loaded.doc["buttons"][1]["action"], json!({"type": "keys", "combo": "B", "intervalMs": 40, "later": 1}));
        assert_eq!(a.loaded.doc["buttons"][3]["action"], json!({"type": "keys", "combo": "Win+R"}), "blocked system key survives");
        assert_eq!(a.loaded.doc["buttons"][4]["action"], json!({"type": "qlab", "cue": "1"}), "unknown action survives");
    }

    #[test]
    fn drop_on_origin_is_unchanged() {
        let l = pad();
        let a = ok(&l, Op::Move { button_id: "a".into(), x: 0, y: 0 });
        assert!(!a.changed);
    }

    #[test]
    fn label_edit_changes_exactly_one_line_of_text() {
        let l = pad();
        let a = ok(&l, Op::SetLabel { button_id: "b".into(), label: "Bee".into() });
        let (before, after) = (to_text(&l.doc), to_text(&a.loaded.doc));
        let diff: Vec<(&str, &str)> = before.lines().zip(after.lines()).filter(|(x, y)| x != y).collect();
        assert_eq!(before.lines().count(), after.lines().count());
        assert_eq!(diff.len(), 1, "{diff:?}");
        assert!(diff[0].1.contains("\"Bee\""));
    }

    #[test]
    fn invalid_label_is_refused_and_nothing_changes() {
        let l = pad();
        let e = apply(&l, &Op::SetLabel { button_id: "a".into(), label: "x".repeat(25) }, false).unwrap_err();
        assert_eq!(e.code, "profile-bound");
    }

    #[test]
    fn resize_shoves_and_out_of_grid_is_refused() {
        let l = pad();
        let a = ok(&l, Op::Resize { button_id: "a".into(), w: 2, h: 1 });
        // b's only distance-1 fit is to the right: straight below is the 2×1 Up key.
        assert_eq!((btn(&a, "a").w, btn(&a, "b").x, btn(&a, "b").y), (2, 2, 0));
        let e = apply(&l, &Op::Resize { button_id: "a".into(), w: 5, h: 1 }, false).unwrap_err();
        assert_eq!(e.code, "layout-out-of-grid");
        assert_eq!(a.loaded.doc["buttons"][0].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["id", "label", "x", "y", "w", "action"]);
    }

    #[test]
    fn set_action_patches_keys_and_label_follows_auto_labels_only() {
        let l = pad();
        // "Up ×10" is not "↑ ×10" (the auto label), so it was typed: keep it.
        let a = ok(&l, Op::SetAction { button_id: "up".into(), action: ActionSpec::Keys { combo: "control+down".into(), repeat: 4 } });
        assert_eq!(a.loaded.doc["buttons"][2]["action"], json!({"type": "keys", "combo": "Ctrl+Down", "repeat": 4}));
        assert_eq!(btn(&a, "up").label, "Up ×10");
        // A new button's label follows.
        let added = ok(&l, Op::Add { at: Some((3, 0)) });
        let id = added.select.clone().unwrap();
        let set = ok(&added.loaded, Op::SetAction { button_id: id.clone(), action: ActionSpec::Keys { combo: "Ctrl+Down".into(), repeat: 10 } });
        assert_eq!(btn(&set, &id).label, "Ctrl+↓ ×10");
        // ... and keeps following while it equals the auto label.
        let again = ok(&set.loaded, Op::SetAction { button_id: id.clone(), action: ActionSpec::Keys { combo: "Num7".into(), repeat: 1 } });
        assert_eq!(btn(&again, &id).label, "7");
        assert!(again.loaded.doc["buttons"][5]["action"].get("repeat").is_some(), "existing repeat field is patched, not removed");
    }

    #[test]
    fn replacing_a_disabled_action_writes_a_fresh_keys_action() {
        let l = pad();
        let a = ok(&l, Op::SetAction { button_id: "q".into(), action: ActionSpec::Keys { combo: "F5".into(), repeat: 1 } });
        assert_eq!(a.loaded.doc["buttons"][4]["action"], json!({"type": "keys", "combo": "F5"}));
        let c = ok(&l, Op::SetAction { button_id: "a".into(), action: ActionSpec::None });
        assert_eq!(btn(&c, "a").action, Action::None);
    }

    #[test]
    fn an_edit_that_disables_the_button_is_refused_but_old_disabled_ones_stay_editable() {
        let l = pad();
        let e = apply(&l, &Op::SetAction { button_id: "a".into(), action: ActionSpec::Keys { combo: "Win+E".into(), repeat: 1 } }, false).unwrap_err();
        assert_eq!((e.code, e.button.as_deref()), ("system-key-blocked", Some("a")));
        let a = ok(&l, Op::SetLabel { button_id: "run".into(), label: "Run box".into() });
        assert_eq!(a.loaded.doc["buttons"][3]["action"], json!({"type": "keys", "combo": "Win+R"}));
        assert_eq!(btn(&a, "run").label, "Run box");
    }

    #[test]
    fn an_edit_that_adds_a_warning_is_refused() {
        let l = pad();
        let e = apply(&l, &Op::SetIcon { button_id: "a".into(), icon: Some("rocket".into()) }, false).unwrap_err();
        assert_eq!(e.code, "profile-unknown-icon");
        let a = ok(&l, Op::SetIcon { button_id: "up".into(), icon: None });
        assert!(a.loaded.doc["buttons"][2].get("icon").is_none());
    }

    #[test]
    fn add_places_a_none_button_with_the_most_common_size() {
        let l = pad();
        let a = ok(&l, Op::Add { at: None });
        let b = btn(&a, "btn-1");
        assert_eq!((b.x, b.y, b.w, b.h, &b.label), (2, 0, 1, 1, &"New".to_string()));
        assert_eq!(b.action, Action::None);
        assert_eq!(a.select.as_deref(), Some("btn-1"));
        let at = ok(&l, Op::Add { at: Some((3, 3)) });
        assert_eq!((btn(&at, "btn-1").x, btn(&at, "btn-1").y), (3, 3));
    }

    #[test]
    fn add_on_the_shipped_r1_pad_grows_row_11_with_a_2x1_button() {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../profiles/r1-delay-gain.tsp.json")).unwrap();
        let l = load_profile(&bytes, false).unwrap();
        assert_eq!(l.profile.pad.rows, 10);
        let a = ok(&l, Op::Add { at: None });
        let b = btn(&a, "btn-1");
        assert_eq!((b.x, b.y, b.w, b.h), (0, 10, 2, 1));
        assert_eq!(a.loaded.profile.pad.rows, 11);
        assert_eq!(a.notice.as_deref(), Some("No empty cell: added row 11"));
    }

    #[test]
    fn add_refuses_past_60_buttons() {
        let buttons: Vec<String> = (0..60).map(|i| format!(r#"{{"id":"b{i}","label":"{i}","x":{},"y":{},"action":{{"type":"none"}}}}"#, i % 6, i / 6)).collect();
        let text = format!(r#"{{"schema":2,"name":"T","pad":{{"grid":{{"columns":6,"rows":12}},"cellWidth":40,"cellHeight":40,"gap":4}},"buttons":[{}]}}"#, buttons.join(","));
        let l = load_profile(text.as_bytes(), false).unwrap();
        assert_eq!(apply(&l, &Op::Add { at: None }, false).unwrap_err().code, "edit-max-buttons");
    }

    #[test]
    fn duplicate_copies_everything_but_id_and_position() {
        let l = pad();
        let a = ok(&l, Op::Duplicate { button_id: "b".into(), mirror: false });
        let d = &a.loaded.doc["buttons"][5];
        assert_eq!(d["id"], "b-2");
        assert_eq!(d["action"], json!({"type": "keys", "combo": "B", "intervalMs": 40, "later": 1}));
        assert_eq!((d["x"].clone(), d["y"].clone()), (json!(2), json!(0)), "nearest free fit");
        let again = ok(&a.loaded, Op::Duplicate { button_id: "b".into(), mirror: false });
        assert_eq!(again.select.as_deref(), Some("b-3"));
    }

    #[test]
    fn duplicate_ids_keep_the_suffix_at_32_characters() {
        let base = "a".repeat(32);
        let id = duplicate_id(&base, &[&base]);
        assert_eq!(id.len(), 32);
        assert!(id.ends_with("-2"));
        let taken = [base.as_str(), id.as_str()];
        assert!(duplicate_id(&base, &taken).ends_with("-3"));
    }

    #[test]
    fn mirror_duplicate_swaps_key_label_word_and_matching_icon() {
        let l = pad();
        let a = ok(&l, Op::Duplicate { button_id: "up".into(), mirror: true });
        let d = &a.loaded.doc["buttons"][5];
        assert_eq!(d["action"]["combo"], "Down");
        assert_eq!(d["action"]["repeat"], 10);
        assert_eq!(d["label"], "Down ×10");
        assert_eq!(d["icon"], "down");
        assert!(apply(&l, &Op::Duplicate { button_id: "a".into(), mirror: true }, false).is_err());
    }

    #[test]
    fn mirror_labels() {
        assert_eq!(mirror_label("Ctrl+Up", "Up"), "Ctrl+Down");
        assert_eq!(mirror_label("Up ×10", "Up"), "Down ×10");
        assert_eq!(mirror_label("Upper", "Up"), "Upper");
        assert_eq!(mirror_label("UP", "Up"), "DOWN");
        assert_eq!(mirror_label("nudge up", "Up"), "nudge down");
        assert_eq!(mirror_label("↑ ×4", "Up"), "↓ ×4");
        assert_eq!(mirror_label("Left", "Up"), "Left", "only the mirrored key's pair");
        assert_eq!(mirror_label("+", "Plus"), "−");
        assert_eq!(mirror_label("Gain +", "Plus"), "Gain +", "+ only as the whole label");
        assert_eq!(mirror_label("PgUp", "PageUp"), "PgDn");
    }

    #[test]
    fn mirror_icons_swap_only_the_matching_pair() {
        assert_eq!(mirror_icon("up", "Up"), Some("down"));
        assert_eq!(mirror_icon("up", "Left"), None, "a Left key with an up icon keeps it");
        assert_eq!(mirror_icon("left", "Right"), Some("right"));
    }

    #[test]
    fn delete_leaves_a_notice_and_allows_an_empty_pad() {
        let l = load_profile(r#"{"schema":2,"name":"T","pad":{"grid":{"columns":1,"rows":1},"cellWidth":40,"cellHeight":40,"gap":0},"buttons":[{"id":"a","label":"A","x":0,"y":0,"action":{"type":"none"}}]}"#.as_bytes(), false).unwrap();
        let a = ok(&l, Op::Delete { button_id: "a".into() });
        assert!(a.loaded.profile.buttons.is_empty());
        assert_eq!(a.notice.as_deref(), Some("Deleted 'A' · Undo edit"));
    }

    #[test]
    fn set_pad_changes_grid_scale_and_refuses_a_shrink_that_strands_a_button() {
        let l = pad();
        let e = apply(&l, &Op::SetPad { change: PadChange { rows: Some(7), ..Default::default() } }, false).unwrap_err();
        assert_eq!((e.code, e.button.as_deref()), ("layout-button-outside-grid", Some("q")), "first in reading order");
        let a = ok(&l, Op::SetPad { change: PadChange { columns: Some(5), scale: Some(1.0 + 0.1 + 0.1 + 0.1), background_opacity: Some(0.02), ..Default::default() } });
        assert_eq!(a.loaded.profile.pad.columns, 5);
        assert_eq!(a.loaded.profile.pad.scale, 1.3);
        assert_eq!(a.loaded.profile.pad.background_opacity, 0.0);
        assert_eq!(a.loaded.doc["pad"]["backgroundOpacity"], 0);
        let mut s = 1.0;
        for _ in 0..10 {
            s = snap_hundredths(s + 0.1);
        }
        assert_eq!(s, 2.0);
    }

    #[test]
    fn stale_button_is_refused() {
        assert_eq!(apply(&pad(), &Op::Delete { button_id: "nope".into() }, false).unwrap_err().code, "edit-stale");
    }

    #[test]
    fn same_combo_compares_parsed_keys_and_repeat() {
        let text = r#"{"schema":2,"name":"T","pad":{"grid":{"columns":4,"rows":4},"cellWidth":40,"cellHeight":40,"gap":0},"buttons":[
          {"id":"a","label":"A","x":0,"y":0,"action":{"type":"keys","combo":"Shift+Ctrl+Up"}},
          {"id":"b","label":"B","x":0,"y":2,"action":{"type":"keys","combo":"control+shift+arrowup"}},
          {"id":"c","label":"C","x":1,"y":1,"action":{"type":"keys","combo":"Ctrl+Shift+Up"}},
          {"id":"d","label":"D","x":2,"y":0,"action":{"type":"keys","combo":"Ctrl+Shift+Up","repeat":2}}]}"#;
        let p = load_profile(text.as_bytes(), false).unwrap().profile;
        assert_eq!(same_combo(&p, "a"), vec![("C".to_string(), 2), ("B".to_string(), 3)]);
    }

    #[test]
    fn auto_labels() {
        let l = |c: &str, r| auto_label(&parse_combo(c).unwrap(), r);
        assert_eq!(l("Ctrl+Down", 10), "Ctrl+↓ ×10");
        assert_eq!(l("Shift+Ctrl+PageUp", 1), "Ctrl+Shift+PgUp");
        assert_eq!(l("Num7", 1), "7");
        assert_eq!(l("Enter", 1), "Enter");
    }

    #[test]
    fn ops_deserialize_from_the_webview_shape() {
        let op: Op = serde_json::from_str(r#"{"kind":"setAction","buttonId":"a","action":{"type":"keys","combo":"Up","repeat":4}}"#).unwrap();
        assert_eq!(op, Op::SetAction { button_id: "a".into(), action: ActionSpec::Keys { combo: "Up".into(), repeat: 4 } });
        let op: Op = serde_json::from_str(r#"{"kind":"setPad","change":{"backgroundOpacity":0}}"#).unwrap();
        assert!(matches!(op, Op::SetPad { change: PadChange { background_opacity: Some(_), .. } }));
        let op: Op = serde_json::from_str(r#"{"kind":"add","at":[1,2]}"#).unwrap();
        assert_eq!(op, Op::Add { at: Some((1, 2)) });
    }

    // ---------- history ----------

    #[test]
    fn label_commits_within_2s_on_one_button_coalesce() {
        let mut h = History::default();
        h.record(json!(1), StepKind::Label, Some("a"), 0);
        h.record(json!(2), StepKind::Label, Some("a"), 1500);
        h.record(json!(3), StepKind::Label, Some("a"), 3000);
        assert_eq!(h.undo(json!(4)), Some(json!(1)), "all three commits are one step");
        assert!(!h.can_undo());
        h.record(json!(5), StepKind::Label, Some("a"), 10_000);
        h.record(json!(6), StepKind::Label, Some("b"), 10_100);
        assert_eq!(h.undo(json!(7)), Some(json!(6)));
    }

    #[test]
    fn undo_redo_round_trip_and_new_edit_clears_redo() {
        let mut h = History::default();
        h.record(json!("v0"), StepKind::Other, None, 0);
        assert_eq!(h.undo(json!("v1")), Some(json!("v0")));
        assert_eq!(h.redo(json!("v0")), Some(json!("v1")));
        assert_eq!(h.undo(json!("v1")), Some(json!("v0")));
        h.record(json!("v0"), StepKind::Other, None, 1);
        assert!(!h.can_redo());
    }

    #[test]
    fn history_is_capped_at_100_and_tracks_the_latest_delete() {
        let mut h = History::default();
        for i in 0..150 {
            h.record(json!(i), StepKind::Other, None, i);
        }
        let mut n = 0;
        while h.undo(json!(null)).is_some() {
            n += 1;
        }
        assert_eq!(n, 100);
        h.clear();
        h.record(json!(0), StepKind::Delete, Some("a"), 0);
        assert!(h.last_is_delete_of("a"));
        h.record(json!(1), StepKind::Other, None, 1);
        assert!(!h.last_is_delete_of("a"), "a later op disarms the notice's undo");
    }

    #[test]
    fn undo_after_a_pad_switch_cannot_write_the_old_pad() {
        let mut h = History::default();
        h.record(json!({"pad": "one"}), StepKind::Other, None, 0);
        h.clear(); // the app clears history whenever the active pad changes
        assert_eq!(h.undo(json!({"pad": "two"})), None);
    }

    #[test]
    fn page_actions_and_text_size_edit() {
        let l = pad();
        let added = ok(&l, Op::Add { at: Some((3, 0)) });
        let id = added.select.clone().unwrap();
        let a = ok(&added.loaded, Op::SetAction { button_id: id.clone(), action: ActionSpec::Page { target: "fx.tsp.json".into(), name: None } });
        assert_eq!(btn(&a, &id).action, Action::Page { target: PageTarget::File("fx.tsp.json".into()) });
        assert_eq!(btn(&a, &id).label, "→ fx");
        let named = ok(&a.loaded, Op::SetAction { button_id: id.clone(), action: ActionSpec::Page { target: "fx.tsp.json".into(), name: Some("FX sends".into()) } });
        assert_eq!(btn(&named, &id).label, "→ FX sends");
        let b = ok(&named.loaded, Op::SetAction { button_id: id.clone(), action: ActionSpec::Page { target: "back".into(), name: None } });
        assert_eq!(btn(&b, &id).label, "→ Back", "label follows while it is a page label");
        assert!(apply(&l, &Op::SetAction { button_id: "a".into(), action: ActionSpec::Page { target: "../x.tsp.json".into(), name: None } }, false).is_err());
        let small = ok(&l, Op::SetTextSize { button_id: "a".into(), size: Some("small".into()) });
        assert_eq!(small.loaded.doc["buttons"][0]["textSize"], "small");
        let back = ok(&small.loaded, Op::SetTextSize { button_id: "a".into(), size: None });
        assert!(back.loaded.doc["buttons"][0].get("textSize").is_none());
    }
}
