//! Profile format (`*.tsp.json`): the only parser and validator.
//!
//! Schema 2 is the working format: a real grid where each button covers a
//! rectangle. Schema 1 files load by converting their raw JSON to a schema 2
//! document first (v1-only fields dropped, positions computed exactly as v1
//! flowed them), then everything is validated as schema 2. The converted
//! document is kept in [`Loaded::doc`] with every unknown field intact, so the
//! editor can patch it and write it back without losing anything.
//!
//! Parsing goes through `serde_json::Value` first (line/column for syntax
//! errors), then each button is validated on its own so one bad button is
//! disabled instead of rejecting the whole pad. Structural errors reject the
//! profile with a `<code> <locator>: <actual vs allowed>. <fix>` message.

use serde_json::{Map, Value, json};

use crate::combo::{Combo, ComboError, is_system_combo, parse_combo};

pub const MAX_PROFILE_BYTES: usize = 256 * 1024;
pub const MAX_BUTTONS: usize = 60;
/// Where a rewritten v1 profile's `$schema` points (VS Code fetches it).
pub const V2_SCHEMA_URL: &str = "https://raw.githubusercontent.com/james-melling/touch-shortcuts/main/schema/tsp-v2.schema.json";
/// Built-in icons (the renderer has an SVG for each).
pub const ICONS: &[&str] = &["up", "down", "left", "right", "enter", "tab", "undo", "redo"];
pub const THEMES: &[&str] = &["c-low-glare", "b-flat"];
/// Background opacity below this is treated as 0 (floating mode) everywhere.
pub const FLOATING_BELOW: f64 = 0.05;

/// Inclusive bounds shared by the validator and the generated JSON Schema.
pub mod bounds {
    pub const COLUMNS: (u64, u64) = (1, 12);
    pub const ROWS: (u64, u64) = (1, 60);
    pub const CELL_WIDTH: (u64, u64) = (8, 200);
    pub const CELL_HEIGHT: (u64, u64) = (40, 200);
    pub const GAP: (u64, u64) = (0, 24);
    pub const SCALE: (f64, f64) = (0.5, 2.0);
    pub const PAD_OPACITY: (f64, f64) = (0.3, 1.0);
    pub const BACKGROUND_OPACITY: (f64, f64) = (0.0, 1.0);
    pub const REPEAT: (u64, u64) = (1, 50);
    pub const INTERVAL_MS: (u64, u64) = (10, 1000);
    pub const LABEL_CHARS: usize = 24;
    pub const NAME_CHARS: usize = 48;
    pub const ID_CHARS: usize = 32;
    // Schema 1 only.
    pub const V1_BUTTON_SIZE: (u64, u64) = (40, 200);
}

pub mod defaults {
    pub const SCALE: f64 = 1.0;
    pub const PAD_OPACITY: f64 = 1.0;
    pub const BACKGROUND_OPACITY: f64 = 0.97;
    pub const REPEAT: u64 = 1;
    pub const INTERVAL_MS: u64 = 25;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group { Num, Ent, Fine, Coarse, Hist, Custom }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dock { Right, Left }

/// Where a "go to pad" button leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTarget {
    /// The pad that was active before the last "go to pad".
    Back,
    /// A pad file in the same folder as the current pad (file name only).
    File(String),
}

/// Label text size; labels also shrink to fit their key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSize { Small, Medium, Large }

impl TextSize {
    pub fn name(self) -> &'static str {
        match self {
            TextSize::Small => "small",
            TextSize::Medium => "medium",
            TextSize::Large => "large",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Keys { combo: Combo, repeat: u32, interval_ms: u32 },
    /// `{ "type": "page", "target": "fx.tsp.json" | "back" }`: switch pads.
    Page { target: PageTarget },
    /// `{ "type": "none" }`: a button not assigned yet. Sends nothing.
    None,
    /// Unknown key name, unknown action type or blocked system key: the key
    /// renders disabled and tapping it shows `code: reason`. The raw action
    /// stays untouched in the document.
    Disabled { code: &'static str, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub id: String,
    pub label: String,
    /// Top-left cell and size in cells (w = columns, h = rows).
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub group: Group,
    /// Only built-in icons; an unknown icon is `None` here (and kept in the doc).
    pub icon: Option<String>,
    pub color: Option<String>,
    pub text_size: TextSize,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub gap: u32,
    pub scale: f64,
    pub pad_opacity: f64,
    pub background_opacity: f64,
    pub dock: Dock,
    pub theme: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub accept_touch: bool,
    pub target_apps: Option<Vec<String>>,
    pub pad: Pad,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub profile: Profile,
    pub warnings: Vec<Warning>,
    /// The schema 2 document (converted from v1 when needed), unknown fields kept.
    pub doc: Value,
    /// The schema the file was written in (1 or 2).
    pub source_schema: u32,
    /// Load normalised a value in `doc` (e.g. background opacity 0.03 → 0);
    /// the next save writes it.
    pub normalised: Vec<String>,
}

const TOP_FIELDS: &[&str] = &["$schema", "schema", "name", "acceptTouch", "targetApps", "pad", "buttons"];
const PAD_FIELDS: &[&str] = &["grid", "cellWidth", "cellHeight", "gap", "scale", "padOpacity", "backgroundOpacity", "dock", "theme"];
const GRID_FIELDS: &[&str] = &["columns", "rows"];
const BUTTON_FIELDS: &[&str] = &["id", "label", "x", "y", "w", "h", "group", "icon", "color", "textSize", "action"];
const ACTION_FIELDS: &[&str] = &["type", "combo", "repeat", "intervalMs", "target"];
/// Renamed fields that are too far from their new name for the edit-distance check.
const RENAMED: &[(&str, &str)] = &[("opacity", "padOpacity")];
const V1_PAD_FIELDS: &[&str] = &["gridColumns", "buttonWidth", "buttonHeight", "gap"];
const RELOAD: &str = "then ··· → Reload.";

fn bound(message: String) -> ProfileError {
    ProfileError { code: "profile-bound", message }
}

struct Ctx<'a> {
    warnings: &'a mut Vec<Warning>,
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

impl Ctx<'_> {
    fn warn(&mut self, code: &'static str, message: String) {
        self.warnings.push(Warning { code, message });
    }

    /// Unknown fields load silently unless they look like a misspelling of a
    /// known field (edit distance ≤ 2) or a renamed one, which is usually a
    /// mistake that would otherwise be ignored without a word.
    fn check_fields(&mut self, obj: &Map<String, Value>, known: &[&str], locator: &str) {
        for name in obj.keys() {
            if known.contains(&name.as_str()) {
                continue;
            }
            let renamed = RENAMED.iter().find(|(old, new)| old == name && known.contains(new)).map(|(_, new)| *new);
            let close = renamed.or_else(|| known.iter().copied().find(|k| edit_distance(&k.to_lowercase(), &name.to_lowercase()) <= 2));
            if let Some(close) = close {
                let at = if locator.is_empty() { String::new() } else { format!("{locator}.") };
                self.warn("profile-unknown-field", format!("{at}{name} is not a profile field; did you mean {close}? It was ignored."));
            }
        }
    }
}

fn int(obj: &Map<String, Value>, field: &str, locator: &str, default: Option<u64>, (lo, hi): (u64, u64)) -> Result<u64, ProfileError> {
    let at = format!("{locator}{field}");
    let value = match obj.get(field) {
        None => return default.ok_or_else(|| bound(format!("{at} is required. Add it, {RELOAD}"))),
        Some(v) => v,
    };
    let n = value.as_u64().ok_or_else(|| bound(format!("{at} = {value} must be a whole number ({lo}-{hi}). Fix it, {RELOAD}")))?;
    if n < lo {
        return Err(bound(format!("{at} = {n} (allowed {lo}-{hi}). Set {field} ≥ {lo}, {RELOAD}")));
    }
    if n > hi {
        return Err(bound(format!("{at} = {n} (allowed {lo}-{hi}). Set {field} ≤ {hi}, {RELOAD}")));
    }
    Ok(n)
}

/// Rounds to hundredths so repeated 0.1 steps land exactly (1.0 + 10 × 0.1 = 2.0).
pub fn snap_hundredths(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn num(obj: &Map<String, Value>, field: &str, locator: &str, default: f64, (lo, hi): (f64, f64)) -> Result<f64, ProfileError> {
    let at = format!("{locator}{field}");
    let Some(value) = obj.get(field) else { return Ok(default) };
    let n = value.as_f64().map(snap_hundredths).ok_or_else(|| bound(format!("{at} = {value} must be a number ({lo}-{hi}). Fix it, {RELOAD}")))?;
    if !(lo..=hi).contains(&n) {
        return Err(bound(format!("{at} = {n} (allowed {lo}-{hi}). Set {field} between {lo} and {hi}, {RELOAD}")));
    }
    Ok(n)
}

fn text<'v>(obj: &'v Map<String, Value>, field: &str, locator: &str, max: usize) -> Result<Option<&'v str>, ProfileError> {
    match obj.get(field) {
        None => Ok(None),
        Some(Value::String(s)) => {
            let len = s.chars().count();
            if len == 0 || len > max {
                Err(bound(format!("{locator}{field} is {len} characters (allowed 1-{max}). Shorten it, {RELOAD}")))
            } else {
                Ok(Some(s.as_str()))
            }
        }
        Some(v) => Err(bound(format!("{locator}{field} = {v} must be text. Fix it, {RELOAD}"))),
    }
}

pub fn valid_id(id: &str) -> bool {
    (1..=bounds::ID_CHARS).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn disabled_from_combo_error(combo: &str, e: &ComboError) -> Action {
    let reason = match e {
        ComboError::UnknownKey(k) => format!("unknown key \"{k}\" in \"{combo}\""),
        ComboError::Empty => "combo is empty".to_string(),
        ComboError::ModifiersOnly => format!("\"{combo}\" has only modifiers; add a key"),
        ComboError::TooManyKeys => format!("\"{combo}\" has more than one non-modifier key"),
        ComboError::DuplicateModifier(m) => format!("\"{combo}\" repeats {m}"),
    };
    Action::Disabled { code: "key-unknown", reason }
}

fn action(obj: &Map<String, Value>, locator: &str, allow_system_keys: bool, ctx: &mut Ctx) -> Result<Action, ProfileError> {
    let Some(Value::Object(a)) = obj.get("action") else {
        return Ok(Action::Disabled { code: "action-unknown", reason: "button has no action".into() });
    };
    let alocator = format!("{locator}action.");
    let kind = a.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "keys" => {}
        "none" => return Ok(Action::None),
        "page" => {
            ctx.check_fields(a, ACTION_FIELDS, alocator.trim_end_matches('.'));
            let target = a.get("target").and_then(Value::as_str).unwrap_or("");
            return Ok(match page_target(target) {
                Some(t) => Action::Page { target: t },
                None => Action::Disabled {
                    code: "action-unknown",
                    reason: format!("go-to-pad target \"{target}\" must be \"back\" or a pad file name like \"fx.tsp.json\""),
                },
            });
        }
        _ => return Ok(Action::Disabled { code: "action-unknown", reason: format!("action type \"{kind}\" is not supported by this version") }),
    }
    ctx.check_fields(a, ACTION_FIELDS, alocator.trim_end_matches('.'));
    let repeat = int(a, "repeat", locator, Some(defaults::REPEAT), bounds::REPEAT)? as u32;
    let interval_ms = int(a, "intervalMs", locator, Some(defaults::INTERVAL_MS), bounds::INTERVAL_MS)? as u32;
    let combo_text = a.get("combo").and_then(Value::as_str).unwrap_or("");
    let combo = match parse_combo(combo_text) {
        Ok(c) => c,
        Err(e) => return Ok(disabled_from_combo_error(combo_text, &e)),
    };
    if is_system_combo(&combo) && !allow_system_keys {
        return Ok(Action::Disabled {
            code: "system-key-blocked",
            reason: format!("\"{combo_text}\" is a system key; allow it with \"allowSystemKeys\": true in settings.json (profiles cannot enable it)"),
        });
    }
    Ok(Action::Keys { combo, repeat, interval_ms })
}

/// "back", or a bare `*.tsp.json` file name (no folders, so a shared pad can
/// only point at pads next to it).
pub fn page_target(target: &str) -> Option<PageTarget> {
    if target == "back" {
        return Some(PageTarget::Back);
    }
    let ok = target.ends_with(".tsp.json")
        && (10..=80).contains(&target.len())
        && !target.starts_with('.')
        && target.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'));
    ok.then(|| PageTarget::File(target.to_string()))
}

fn group(name: Option<&str>, locator: &str, ctx: &mut Ctx) -> Group {
    match name {
        None | Some("num") => Group::Num,
        Some("ent") => Group::Ent,
        Some("fine") => Group::Fine,
        Some("coarse") => Group::Coarse,
        Some("hist") => Group::Hist,
        Some("custom") => Group::Custom,
        Some(other) => {
            ctx.warn("profile-unknown-group", format!("{locator}group \"{other}\" is unknown (num|ent|fine|coarse|hist|custom); using custom."));
            Group::Custom
        }
    }
}

fn button_locator(i: usize, b: &Map<String, Value>) -> String {
    match b.get("label").and_then(Value::as_str) {
        Some(l) => format!("buttons[{i}] \"{l}\"."),
        None => format!("buttons[{i}]."),
    }
}

/// Parses and validates a profile file (schema 1 or 2).
pub fn load_profile(bytes: &[u8], allow_system_keys: bool) -> Result<Loaded, ProfileError> {
    if bytes.len() > MAX_PROFILE_BYTES {
        return Err(ProfileError {
            code: "profile-too-large",
            message: format!("profile is {} bytes (allowed up to {MAX_PROFILE_BYTES}). Remove unused buttons, {RELOAD}", bytes.len()),
        });
    }
    let source = std::str::from_utf8(bytes).map_err(|e| ProfileError {
        code: "profile-not-utf8",
        message: format!("profile is not UTF-8 (invalid byte at offset {}). Save it as UTF-8, {RELOAD}", e.valid_up_to()),
    })?;
    let root: Value = serde_json::from_str(source).map_err(|e| ProfileError {
        code: "profile-syntax",
        message: format!("line {} column {}: {e}. Fix the JSON, {RELOAD}", e.line(), e.column()),
    })?;
    let Value::Object(top) = root else {
        return Err(ProfileError { code: "profile-syntax", message: format!("the profile must be a JSON object. Fix it, {RELOAD}") });
    };
    let schema = int(&top, "schema", "", None, (1, u64::MAX))?;
    match schema {
        1 => {
            let doc = convert_v1(&top)?;
            let mut loaded = load_doc(Value::Object(doc), allow_system_keys)?;
            loaded.source_schema = 1;
            Ok(loaded)
        }
        2 => load_doc(Value::Object(top), allow_system_keys),
        n => Err(ProfileError {
            code: "profile-schema-too-new",
            message: format!(
                "profile schema {n}, this build supports 1-2. Download the latest release: https://github.com/james-melling/touch-shortcuts/releases"
            ),
        }),
    }
}

/// Validates a schema 2 document (from a file or an edit) and returns the
/// typed profile. Normalised values are written back into the returned doc.
pub fn load_doc(mut doc: Value, allow_system_keys: bool) -> Result<Loaded, ProfileError> {
    let mut warnings = Vec::new();
    let mut normalised = Vec::new();
    let profile = {
        let Value::Object(top) = &mut doc else {
            return Err(ProfileError { code: "profile-syntax", message: format!("the profile must be a JSON object. Fix it, {RELOAD}") });
        };
        let mut ctx = Ctx { warnings: &mut warnings };
        validate_v2(top, allow_system_keys, &mut ctx, &mut normalised)?
    };
    Ok(Loaded { profile, warnings, doc, source_schema: 2, normalised })
}

fn validate_v2(top: &mut Map<String, Value>, allow_system_keys: bool, ctx: &mut Ctx, normalised: &mut Vec<String>) -> Result<Profile, ProfileError> {
    ctx.check_fields(top, TOP_FIELDS, "");
    let schema = int(top, "schema", "", None, (2, 2))?;
    debug_assert_eq!(schema, 2);
    let name = text(top, "name", "", bounds::NAME_CHARS)?.ok_or_else(|| bound(format!("name is required. Add it, {RELOAD}")))?.to_string();
    let accept_touch = top.get("acceptTouch").and_then(Value::as_bool).unwrap_or(true);
    let target_apps = top.get("targetApps").and_then(Value::as_array).map(|a| {
        a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
    });

    let Some(Value::Object(pad)) = top.get_mut("pad") else {
        return Err(bound(format!("pad is required. Add it, {RELOAD}")));
    };
    ctx.check_fields(pad, PAD_FIELDS, "pad");
    let Some(Value::Object(grid)) = pad.get("grid") else {
        return Err(bound(format!("pad.grid is required, e.g. \"grid\": {{ \"columns\": 4, \"rows\": 8 }}. Add it, {RELOAD}")));
    };
    ctx.check_fields(grid, GRID_FIELDS, "pad.grid");
    let columns = int(grid, "columns", "pad.grid.", None, bounds::COLUMNS)? as u32;
    let rows = int(grid, "rows", "pad.grid.", None, bounds::ROWS)? as u32;
    let cell_width = int(pad, "cellWidth", "pad.", None, bounds::CELL_WIDTH)? as u32;
    let cell_height = int(pad, "cellHeight", "pad.", None, bounds::CELL_HEIGHT)? as u32;
    let gap = int(pad, "gap", "pad.", None, bounds::GAP)? as u32;
    let scale = num(pad, "scale", "pad.", defaults::SCALE, bounds::SCALE)?;
    let pad_opacity = num(pad, "padOpacity", "pad.", defaults::PAD_OPACITY, bounds::PAD_OPACITY)?;
    let mut background_opacity = num(pad, "backgroundOpacity", "pad.", defaults::BACKGROUND_OPACITY, bounds::BACKGROUND_OPACITY)?;
    if background_opacity > 0.0 && background_opacity < FLOATING_BELOW {
        normalised.push(format!("pad.backgroundOpacity {background_opacity} → 0 (below {FLOATING_BELOW} counts as floating)"));
        background_opacity = 0.0;
        pad.insert("backgroundOpacity".into(), json!(0));
    }
    let dock = match pad.get("dock").and_then(Value::as_str) {
        None | Some("right") => Dock::Right,
        Some("left") => Dock::Left,
        Some(other) => {
            ctx.warn("profile-dock", format!("pad.dock \"{other}\" is not right or left; using right."));
            Dock::Right
        }
    };
    let theme = match pad.get("theme").and_then(Value::as_str) {
        None => THEMES[0].to_string(),
        Some(t) if THEMES.contains(&t) => t.to_string(),
        Some(other) => {
            ctx.warn("profile-theme", format!("pad.theme \"{other}\" is unknown; using {}.", THEMES[0]));
            THEMES[0].to_string()
        }
    };
    let pad = Pad { columns, rows, cell_width, cell_height, gap, scale, pad_opacity, background_opacity, dock, theme };

    let empty = Vec::new();
    let list = match top.get("buttons") {
        None => &empty,
        Some(Value::Array(a)) => a,
        Some(v) => return Err(bound(format!("buttons = {v} must be a list. Fix it, {RELOAD}"))),
    };
    if list.len() > MAX_BUTTONS {
        return Err(bound(format!("buttons has {} entries (allowed 0-{MAX_BUTTONS}). Remove some, {RELOAD}", list.len())));
    }
    let mut buttons: Vec<Button> = Vec::with_capacity(list.len());
    let mut owner: Vec<Option<usize>> = vec![None; (columns * rows) as usize];
    for (i, b) in list.iter().enumerate() {
        let Value::Object(b) = b else {
            return Err(bound(format!("buttons[{i}] must be an object. Fix it, {RELOAD}")));
        };
        let locator = button_locator(i, b);
        ctx.check_fields(b, BUTTON_FIELDS, locator.trim_end_matches('.'));
        let id = match b.get("id").and_then(Value::as_str) {
            None => return Err(bound(format!("{locator}id is required. Add a unique id like \"d7\", {RELOAD}"))),
            Some(id) if !valid_id(id) => {
                return Err(bound(format!("{locator}id \"{id}\" must be 1-32 characters of a-z, 0-9 or -. Fix it, {RELOAD}")));
            }
            Some(id) => id.to_string(),
        };
        if buttons.iter().any(|x| x.id == id) {
            return Err(bound(format!("{locator}id \"{id}\" is a duplicate id. Give each button its own id, {RELOAD}")));
        }
        let label = text(b, "label", &locator, bounds::LABEL_CHARS)?.ok_or_else(|| bound(format!("{locator}label is required. Add it, {RELOAD}")))?.to_string();
        let x = int(b, "x", &locator, None, (0, u64::from(columns - 1)))? as u32;
        let y = int(b, "y", &locator, None, (0, u64::from(rows - 1)))? as u32;
        let w = int(b, "w", &locator, Some(1), (1, u64::from(columns - x)))? as u32;
        let h = int(b, "h", &locator, Some(1), (1, u64::from(rows - y)))? as u32;
        for cy in y..y + h {
            for cx in x..x + w {
                let cell = &mut owner[(cy * columns + cx) as usize];
                if let Some(j) = *cell {
                    let other: &Button = &buttons[j];
                    return Err(ProfileError {
                        code: "profile-overlap",
                        message: format!(
                            "{locator}id \"{id}\" and buttons[{j}] id \"{}\" both cover cell ({cx}, {cy}). Move one of them, {RELOAD}",
                            other.id
                        ),
                    });
                }
                *cell = Some(i);
            }
        }
        let group = group(b.get("group").and_then(Value::as_str), &locator, ctx);
        let icon = match b.get("icon").and_then(Value::as_str) {
            None => None,
            Some(i) if ICONS.contains(&i) => Some(i.to_string()),
            Some(other) => {
                ctx.warn("profile-unknown-icon", format!("{locator}icon \"{other}\" is unknown ({}); it is not shown.", ICONS.join("|")));
                None
            }
        };
        let color = match b.get("color").and_then(Value::as_str) {
            None => None,
            Some(c) if valid_color(c) => Some(c.to_string()),
            Some(c) => return Err(bound(format!("{locator}color \"{c}\" must look like #RRGGBB. Fix it, {RELOAD}"))),
        };
        let text_size = match b.get("textSize").and_then(Value::as_str) {
            None | Some("large") => TextSize::Large,
            Some("medium") => TextSize::Medium,
            Some("small") => TextSize::Small,
            Some(other) => return Err(bound(format!("{locator}textSize \"{other}\" must be small, medium or large. Fix it, {RELOAD}"))),
        };
        let action = action(b, &locator, allow_system_keys, ctx)?;
        buttons.push(Button { id, label, x, y, w, h, group, icon, color, text_size, action });
    }
    Ok(Profile { name, accept_touch, target_apps, pad, buttons })
}

/// Schema 1 → schema 2, on the raw JSON. Validates the v1-only fields with
/// v1's messages, places buttons exactly as v1 flowed them (reading order,
/// wrapping when a span does not fit), drops `gridColumns`, `buttonWidth`,
/// `buttonHeight` and `span`, and keeps every other field where it was.
fn convert_v1(top: &Map<String, Value>) -> Result<Map<String, Value>, ProfileError> {
    let Some(Value::Object(pad)) = top.get("pad") else {
        return Err(bound(format!("pad is required. Add it, {RELOAD}")));
    };
    let grid_columns = int(pad, "gridColumns", "pad.", None, bounds::COLUMNS)?;
    let button_width = int(pad, "buttonWidth", "pad.", None, bounds::V1_BUTTON_SIZE)?;
    let button_height = int(pad, "buttonHeight", "pad.", None, bounds::V1_BUTTON_SIZE)?;
    let gap = int(pad, "gap", "pad.", None, bounds::GAP)?;
    let list = top.get("buttons").and_then(Value::as_array).cloned().unwrap_or_default();
    if list.is_empty() || list.len() > MAX_BUTTONS {
        return Err(bound(format!("buttons has {} entries (allowed 1-{MAX_BUTTONS}). Fix the list, {RELOAD}", list.len())));
    }

    // Flow in half-key columns, as v1 geometry did.
    let (mut row, mut used) = (0u64, 0u64);
    let mut buttons = Vec::with_capacity(list.len());
    for (i, b) in list.iter().enumerate() {
        let Value::Object(b) = b else {
            return Err(bound(format!("buttons[{i}] must be an object. Fix it, {RELOAD}")));
        };
        let locator = button_locator(i, b);
        let span = int(b, "span", &locator, Some(2), (1, grid_columns))?;
        if used + span > grid_columns {
            row += 1;
            used = 0;
        }
        let mut out = Map::new();
        let mut placed = false;
        for (k, v) in b {
            if k == "span" {
                continue;
            }
            out.insert(k.clone(), v.clone());
            if k == "label" {
                insert_rect(&mut out, used, row, span);
                placed = true;
            }
        }
        if !placed {
            insert_rect(&mut out, used, row, span);
        }
        buttons.push(Value::Object(out));
        used += span;
    }
    let rows = row + 1;

    let mut new_pad = Map::new();
    new_pad.insert("grid".into(), json!({ "columns": grid_columns, "rows": rows }));
    new_pad.insert("cellWidth".into(), json!((button_width.saturating_sub(gap)) / 2));
    new_pad.insert("cellHeight".into(), json!(button_height));
    new_pad.insert("gap".into(), json!(gap));
    for (k, v) in pad {
        if !V1_PAD_FIELDS.contains(&k.as_str()) {
            new_pad.insert(k.clone(), v.clone());
        }
    }

    let mut out = Map::new();
    for (k, v) in top {
        let v = match k.as_str() {
            "schema" => json!(2),
            "$schema" if v.as_str().is_some_and(|s| s.contains("tsp-v1")) => json!(V2_SCHEMA_URL),
            "pad" => Value::Object(new_pad.clone()),
            "buttons" => Value::Array(buttons.clone()),
            _ => v.clone(),
        };
        out.insert(k.clone(), v);
    }
    Ok(out)
}

fn insert_rect(out: &mut Map<String, Value>, x: u64, y: u64, w: u64) {
    out.insert("x".into(), json!(x));
    out.insert("y".into(), json!(y));
    out.insert("w".into(), json!(w));
    out.insert("h".into(), json!(1));
}

/// JSON Schema (draft-07) for schema 2, built from the same bounds as the
/// validator. Written to `schema/tsp-v2.schema.json` (kept in sync by a test).
/// Unknown fields are allowed, as in the loader.
pub fn v2_json_schema() -> Value {
    use bounds::*;
    let int = |(lo, hi): (u64, u64), d: &str| json!({ "type": "integer", "minimum": lo, "maximum": hi, "description": d });
    let num = |(lo, hi): (f64, f64), default: f64, d: &str| json!({ "type": "number", "minimum": lo, "maximum": hi, "default": default, "description": d });
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": V2_SCHEMA_URL,
        "title": "touch-shortcuts profile (schema 2)",
        "description": "A pad of buttons on a grid. Sizes are w×h = columns × rows. Generated from the app's validator; do not edit by hand.",
        "type": "object",
        "required": ["schema", "name", "pad"],
        "properties": {
            "$schema": { "type": "string" },
            "schema": { "const": 2, "description": "Profile format version." },
            "name": { "type": "string", "minLength": 1, "maxLength": NAME_CHARS, "description": "Shown in the pad header." },
            "acceptTouch": { "type": "boolean", "default": true, "description": "false = pen and mouse only; finger taps send nothing." },
            "targetApps": { "type": "array", "items": { "type": "string" }, "description": "Process names the pad may type into, e.g. [\"R1.exe\"]. Omit for any app." },
            "pad": {
                "type": "object",
                "required": ["grid", "cellWidth", "cellHeight", "gap"],
                "properties": {
                    "grid": {
                        "type": "object",
                        "required": ["columns", "rows"],
                        "properties": { "columns": int(COLUMNS, "Grid columns."), "rows": int(ROWS, "Grid rows.") }
                    },
                    "cellWidth": int(CELL_WIDTH, "Cell width in px at scale 1."),
                    "cellHeight": int(CELL_HEIGHT, "Cell height in px at scale 1 (rows may shrink to fit the screen, never below 56 px)."),
                    "gap": int(GAP, "Space between cells in px at scale 1."),
                    "scale": num(SCALE, defaults::SCALE, "Multiplies every size; snapped to hundredths."),
                    "padOpacity": num(PAD_OPACITY, defaults::PAD_OPACITY, "Fades the slab and keys (not the header)."),
                    "backgroundOpacity": num(BACKGROUND_OPACITY, defaults::BACKGROUND_OPACITY, "Slab alpha. Below 0.05 counts as 0: floating buttons, taps in the gaps reach the app below."),
                    "dock": { "enum": ["right", "left"], "default": "right" },
                    "theme": { "enum": THEMES, "default": THEMES[0] }
                }
            },
            "buttons": {
                "type": "array",
                "maxItems": MAX_BUTTONS,
                "items": {
                    "type": "object",
                    "required": ["id", "label", "x", "y", "action"],
                    "properties": {
                        "id": { "type": "string", "pattern": "^[a-z0-9-]{1,32}$", "description": "Unique id: a-z, 0-9 and -." },
                        "label": { "type": "string", "minLength": 1, "maxLength": LABEL_CHARS },
                        "x": int((0, COLUMNS.1 - 1), "Left column (0-based)."),
                        "y": int((0, ROWS.1 - 1), "Top row (0-based)."),
                        "w": { "type": "integer", "minimum": 1, "maximum": COLUMNS.1, "default": 1, "description": "Width in columns." },
                        "h": { "type": "integer", "minimum": 1, "maximum": ROWS.1, "default": 1, "description": "Height in rows." },
                        "group": { "enum": ["num", "ent", "fine", "coarse", "hist", "custom"], "default": "num", "description": "Colour group." },
                        "icon": { "enum": ICONS },
                        "color": { "type": "string", "pattern": "^#[0-9a-fA-F]{6}$", "description": "Overrides the group colour." },
                        "textSize": { "enum": ["small", "medium", "large"], "default": "large", "description": "Label size; labels also shrink to fit the key." },
                        "action": {
                            "type": "object",
                            "required": ["type"],
                            "properties": {
                                "type": { "enum": ["keys", "page", "none"], "description": "keys = send a key combo; page = go to another pad; none = not set yet." },
                                "target": { "type": "string", "description": "For page: \"back\" or a pad file in the same folder, e.g. \"fx.tsp.json\"." },
                                "combo": { "type": "string", "description": "Mod+Mod+Key, e.g. \"Ctrl+Down\", \"Num7\", \"Enter\"." },
                                "repeat": int(REPEAT, "Presses per tap."),
                                "intervalMs": int(INTERVAL_MS, "Start-to-start time between presses.")
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Profile text as written by the editor: 2-space pretty print, trailing newline.
pub fn to_text(doc: &Value) -> String {
    let mut s = serde_json::to_string_pretty(doc).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
      "schema": 1,
      "name": "R1 - Delay/Gain",
      "pad": { "gridColumns": 6, "buttonWidth": 92, "buttonHeight": 78, "gap": 8 },
      "buttons": [
        { "id": "d7", "label": "7", "group": "num", "action": { "type": "keys", "combo": "Num7" } },
        { "id": "down10", "label": "Down", "icon": "down", "span": 3, "group": "coarse",
          "action": { "type": "keys", "combo": "Down", "repeat": 10, "intervalMs": 25 } }
      ]
    }"#;

    const V2: &str = r#"{
      "schema": 2,
      "name": "R1 numpad 4x8",
      "pad": { "grid": { "columns": 4, "rows": 8 }, "cellWidth": 69, "cellHeight": 76, "gap": 8 },
      "buttons": [
        { "id": "d7", "label": "7", "x": 0, "y": 0, "group": "num", "action": { "type": "keys", "combo": "Num7" } },
        { "id": "enter", "label": "Enter", "icon": "enter", "x": 0, "y": 4, "w": 2, "h": 2, "group": "ent",
          "action": { "type": "keys", "combo": "Enter" } }
      ]
    }"#;

    fn with_button(button: &str) -> String {
        format!(
            r#"{{"schema":1,"name":"T","pad":{{"gridColumns":6,"buttonWidth":92,"buttonHeight":78,"gap":8}},"buttons":[{button}]}}"#
        )
    }

    fn v2_with(buttons: &str) -> String {
        format!(r#"{{"schema":2,"name":"T","pad":{{"grid":{{"columns":4,"rows":8}},"cellWidth":69,"cellHeight":76,"gap":8}},"buttons":[{buttons}]}}"#)
    }

    fn load(text: &str) -> Result<Loaded, ProfileError> {
        load_profile(text.as_bytes(), false)
    }

    fn err(text: &str) -> ProfileError {
        load(text).expect_err("expected the profile to be rejected")
    }

    #[test]
    fn minimal_v1_profile_loads_with_defaults() {
        let l = load(MINIMAL).unwrap();
        let p = &l.profile;
        assert_eq!(l.source_schema, 1);
        assert_eq!(p.name, "R1 - Delay/Gain");
        assert!(p.accept_touch, "touch fires by default (final gate d5)");
        assert_eq!(p.target_apps, None);
        assert_eq!(p.pad.dock, Dock::Right);
        assert_eq!(p.pad.theme, "c-low-glare");
        assert_eq!((p.pad.scale, p.pad.pad_opacity, p.pad.background_opacity), (1.0, 1.0, 0.97));
        assert_eq!((p.buttons[0].x, p.buttons[0].y, p.buttons[0].w, p.buttons[0].h), (0, 0, 2, 1));
        match &p.buttons[0].action {
            Action::Keys { combo, repeat, interval_ms } => {
                assert_eq!(combo.key.vk, 0x67);
                assert_eq!((*repeat, *interval_ms), (1, 25));
            }
            other => panic!("expected keys action, got {other:?}"),
        }
        match &p.buttons[1].action {
            Action::Keys { repeat, .. } => assert_eq!(*repeat, 10),
            other => panic!("{other:?}"),
        }
        assert_eq!(p.buttons[1].group, Group::Coarse);
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    }

    #[test]
    fn v1_converts_to_the_grid_v1_would_have_drawn() {
        let p = load(MINIMAL).unwrap().profile;
        assert_eq!((p.pad.columns, p.pad.rows, p.pad.cell_width, p.pad.cell_height, p.pad.gap), (6, 1, 42, 78, 8));
        // Span 3 after a span-2 key fits in the 6 half-columns: same row.
        assert_eq!((p.buttons[1].x, p.buttons[1].y, p.buttons[1].w), (2, 0, 3));
    }

    #[test]
    fn v1_conversion_wraps_like_v1_flow() {
        let l = load(&with_button(
            r#"{"id":"a","label":"A","action":{"type":"keys","combo":"A"}},
               {"id":"b","label":"B","action":{"type":"keys","combo":"B"}},
               {"id":"c","label":"C","span":4,"action":{"type":"keys","combo":"C"}}"#,
        ))
        .unwrap();
        let c = &l.profile.buttons[2];
        assert_eq!((c.x, c.y, c.w), (0, 1, 4));
        assert_eq!(l.profile.pad.rows, 2);
    }

    #[test]
    fn v1_doc_drops_v1_fields_keeps_unknowns_and_field_order() {
        let text = r#"{"$schema":"https://x/tsp-v1.schema.json","schema":1,"name":"T","futureTop":1,
          "pad":{"gridColumns":6,"buttonWidth":92,"buttonHeight":78,"gap":8,"dock":"left","padExtra":"keep"},
          "buttons":[{"id":"a","label":"A","span":3,"extra":{"n":1},"action":{"type":"keys","combo":"A","later":true}}]}"#;
        let l = load(text).unwrap();
        let doc = &l.doc;
        assert_eq!(doc["$schema"], V2_SCHEMA_URL);
        assert_eq!(doc["schema"], 2);
        assert_eq!(doc["futureTop"], 1);
        assert_eq!(doc["pad"]["grid"], json!({"columns": 6, "rows": 1}));
        assert_eq!(doc["pad"]["padExtra"], "keep");
        assert_eq!(doc["pad"]["dock"], "left");
        for gone in ["gridColumns", "buttonWidth", "buttonHeight"] {
            assert!(doc["pad"].get(gone).is_none(), "{gone} kept");
        }
        let b = &doc["buttons"][0];
        assert!(b.get("span").is_none());
        assert_eq!(b["extra"], json!({"n": 1}));
        assert_eq!(b["action"]["later"], true);
        let keys: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["id", "label", "x", "y", "w", "h", "extra", "action"]);
        let top_keys: Vec<&str> = doc.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(top_keys, ["$schema", "schema", "name", "futureTop", "pad", "buttons"]);
    }

    #[test]
    fn other_schema_values_are_kept_on_conversion() {
        let l = load(&MINIMAL.replace("\"schema\": 1,", "\"$schema\": \"./mine.json\", \"schema\": 1,")).unwrap();
        assert_eq!(l.doc["$schema"], "./mine.json");
    }

    #[test]
    fn v1_conversion_extremes() {
        // buttonWidth 40 with gap 24 → 8 px cells.
        let p = load(&MINIMAL.replace("\"buttonWidth\": 92", "\"buttonWidth\": 40").replace("\"gap\": 8", "\"gap\": 24")).unwrap().profile;
        assert_eq!(p.pad.cell_width, 8);
        // gridColumns 1 with 60 buttons → 60 rows.
        let buttons: Vec<String> = (0..60).map(|i| format!(r#"{{"id":"b{i}","label":"{i}","span":1,"action":{{"type":"keys","combo":"A"}}}}"#)).collect();
        let text = format!(
            r#"{{"schema":1,"name":"T","pad":{{"gridColumns":1,"buttonWidth":92,"buttonHeight":78,"gap":8}},"buttons":[{}]}}"#,
            buttons.join(",")
        );
        let p = load(&text).unwrap().profile;
        assert_eq!((p.pad.columns, p.pad.rows), (1, 60));
        assert_eq!(p.buttons[59].y, 59);
    }

    #[test]
    fn v2_profile_loads() {
        let l = load(V2).unwrap();
        assert_eq!(l.source_schema, 2);
        let e = &l.profile.buttons[1];
        assert_eq!((e.x, e.y, e.w, e.h), (0, 4, 2, 2));
        assert_eq!(e.icon.as_deref(), Some("enter"));
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert!(l.normalised.is_empty());
    }

    #[test]
    fn v2_accepts_zero_buttons_but_v1_does_not() {
        assert!(load(&v2_with("")).unwrap().profile.buttons.is_empty());
        let no_list = r#"{"schema":2,"name":"T","pad":{"grid":{"columns":4,"rows":8},"cellWidth":69,"cellHeight":76,"gap":8}}"#;
        assert!(load(no_list).unwrap().profile.buttons.is_empty());
        assert_eq!(err(&with_button("")).code, "profile-bound");
    }

    #[test]
    fn v2_buttons_must_fit_inside_the_grid() {
        let e = err(&v2_with(r#"{"id":"a","label":"A","x":3,"y":0,"w":2,"action":{"type":"none"}}"#));
        assert_eq!(e.code, "profile-bound");
        assert!(e.message.contains("w = 2 (allowed 1-1)"), "{}", e.message);
        let e = err(&v2_with(r#"{"id":"a","label":"A","x":4,"y":0,"action":{"type":"none"}}"#));
        assert!(e.message.contains("x = 4 (allowed 0-3)"), "{}", e.message);
        let e = err(&v2_with(r#"{"id":"a","label":"A","x":0,"y":7,"h":2,"action":{"type":"none"}}"#));
        assert!(e.message.contains("h = 2 (allowed 1-1)"), "{}", e.message);
    }

    #[test]
    fn v2_overlap_names_both_ids_and_the_cell() {
        let e = err(&v2_with(
            r#"{"id":"a","label":"A","x":0,"y":0,"w":2,"h":2,"action":{"type":"none"}},
               {"id":"b","label":"B","x":1,"y":1,"action":{"type":"none"}}"#,
        ));
        assert_eq!(e.code, "profile-overlap");
        assert!(e.message.contains("\"b\"") && e.message.contains("\"a\"") && e.message.contains("(1, 1)"), "{}", e.message);
    }

    #[test]
    fn none_action_loads_as_unassigned() {
        let l = load(&v2_with(r#"{"id":"n","label":"New","x":0,"y":0,"action":{"type":"none"}}"#)).unwrap();
        assert_eq!(l.profile.buttons[0].action, Action::None);
    }

    #[test]
    fn scale_snaps_and_opacity_bounds_apply() {
        let text = V2.replace("\"gap\": 8 }", "\"gap\": 8, \"scale\": 1.2999999 }");
        assert_eq!(load(&text).unwrap().profile.pad.scale, 1.3);
        let e = err(&V2.replace("\"gap\": 8 }", "\"gap\": 8, \"padOpacity\": 0.2 }"));
        assert!(e.message.contains("pad.padOpacity = 0.2 (allowed 0.3-1)"), "{}", e.message);
        let e = err(&V2.replace("\"gap\": 8 }", "\"gap\": 8, \"scale\": 2.5 }"));
        assert!(e.message.contains("pad.scale"), "{}", e.message);
    }

    #[test]
    fn tiny_background_opacity_normalises_to_zero_in_profile_and_doc() {
        let l = load(&V2.replace("\"gap\": 8 }", "\"gap\": 8, \"backgroundOpacity\": 0.03 }")).unwrap();
        assert_eq!(l.profile.pad.background_opacity, 0.0);
        assert_eq!(l.doc["pad"]["backgroundOpacity"], 0);
        assert_eq!(l.normalised.len(), 1);
        let l = load(&V2.replace("\"gap\": 8 }", "\"gap\": 8, \"backgroundOpacity\": 0.05 }")).unwrap();
        assert_eq!(l.profile.pad.background_opacity, 0.05);
        assert!(l.normalised.is_empty());
    }

    #[test]
    fn old_opacity_name_raises_the_near_miss_warning() {
        let l = load(&V2.replace("\"gap\": 8 }", "\"gap\": 8, \"opacity\": 0.5 }")).unwrap();
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        assert!(l.warnings[0].message.contains("padOpacity"), "{}", l.warnings[0].message);
        assert_eq!(l.profile.pad.pad_opacity, 1.0);
    }

    #[test]
    fn unknown_icon_warns_and_is_not_rendered() {
        let l = load(&v2_with(r#"{"id":"a","label":"A","x":0,"y":0,"icon":"constructor","action":{"type":"none"}}"#)).unwrap();
        assert_eq!(l.profile.buttons[0].icon, None);
        assert_eq!(l.warnings[0].code, "profile-unknown-icon");
        assert_eq!(l.doc["buttons"][0]["icon"], "constructor");
    }

    #[test]
    fn syntax_error_reports_line_and_column() {
        let e = err("{\n  \"schema\": 1,\n  \"name\": \"x\"\n  \"pad\": {}\n}");
        assert_eq!(e.code, "profile-syntax");
        assert!(e.message.contains("line 4"), "{}", e.message);
    }

    #[test]
    fn newer_schema_is_rejected_with_upgrade_hint() {
        let e = err(&MINIMAL.replace("\"schema\": 1", "\"schema\": 3"));
        assert_eq!(e.code, "profile-schema-too-new");
        assert!(e.message.contains("schema 3") && e.message.contains("releases"), "{}", e.message);
    }

    #[test]
    fn bound_error_names_button_field_value_and_range() {
        let e = err(&with_button(r#"{"id":"down","label":"Down","action":{"type":"keys","combo":"Down","repeat":80}}"#));
        assert_eq!(e.code, "profile-bound");
        assert_eq!(e.message, r#"buttons[0] "Down".repeat = 80 (allowed 1-50). Set repeat ≤ 50, then ··· → Reload."#);
    }

    #[test]
    fn interval_and_pad_bounds_are_enforced() {
        let e = err(&with_button(r#"{"id":"a","label":"A","action":{"type":"keys","combo":"A","intervalMs":5}}"#));
        assert!(e.message.contains("intervalMs = 5 (allowed 10-1000)"), "{}", e.message);
        let e = err(&MINIMAL.replace("\"gridColumns\": 6", "\"gridColumns\": 13"));
        assert!(e.message.contains("pad.gridColumns = 13 (allowed 1-12)"), "{}", e.message);
        let e = err(&MINIMAL.replace("\"span\": 3", "\"span\": 7"));
        assert!(e.message.contains("span = 7 (allowed 1-6)"), "{}", e.message);
    }

    #[test]
    fn unknown_key_disables_only_that_button() {
        let l = load(&with_button(r#"{"id":"x","label":"X","action":{"type":"keys","combo":"Ctrl+F13"}}"#)).unwrap();
        match &l.profile.buttons[0].action {
            Action::Disabled { code, reason } => {
                assert_eq!(*code, "key-unknown");
                assert!(reason.contains("F13"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_action_type_disables_only_that_button() {
        let l = load(&with_button(r#"{"id":"q","label":"Go","action":{"type":"qlab","cue":"1"}}"#)).unwrap();
        assert!(matches!(&l.profile.buttons[0].action, Action::Disabled { code: "action-unknown", .. }));
    }

    #[test]
    fn system_keys_are_blocked_unless_allowed_locally() {
        let text = with_button(r#"{"id":"r","label":"Run","action":{"type":"keys","combo":"Win+R"}}"#);
        let blocked = load_profile(text.as_bytes(), false).unwrap();
        assert!(matches!(&blocked.profile.buttons[0].action, Action::Disabled { code: "system-key-blocked", .. }));
        let allowed = load_profile(text.as_bytes(), true).unwrap();
        assert!(matches!(&allowed.profile.buttons[0].action, Action::Keys { .. }));
    }

    #[test]
    fn profile_level_allow_system_keys_is_ignored() {
        let text = format!(
            r#"{{"schema":1,"name":"T","allowSystemKeys":true,"pad":{{"gridColumns":6,"buttonWidth":92,"buttonHeight":78,"gap":8}},"buttons":[{}]}}"#,
            r#"{"id":"r","label":"Run","action":{"type":"keys","combo":"Win+R"}}"#
        );
        let l = load_profile(text.as_bytes(), false).unwrap();
        assert!(matches!(&l.profile.buttons[0].action, Action::Disabled { code: "system-key-blocked", .. }));
    }

    #[test]
    fn ids_must_be_present_unique_and_well_formed() {
        assert_eq!(err(&with_button(r#"{"label":"A","action":{"type":"keys","combo":"A"}}"#)).code, "profile-bound");
        let dup = with_button(r#"{"id":"a","label":"A","action":{"type":"keys","combo":"A"}},{"id":"a","label":"B","action":{"type":"keys","combo":"B"}}"#);
        assert!(err(&dup).message.contains("duplicate id"), "dup");
        assert!(err(&with_button(r#"{"id":"Bad Id","label":"A","action":{"type":"keys","combo":"A"}}"#)).message.contains("id"));
    }

    #[test]
    fn near_miss_field_warns_but_schema_key_is_silent() {
        let l = load(&with_button(r#"{"id":"a","label":"A","action":{"type":"keys","combo":"A","intervalMS":40}}"#)).unwrap();
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        assert!(l.warnings[0].message.contains("intervalMS") && l.warnings[0].message.contains("intervalMs"));
        let l = load(&MINIMAL.replace("\"schema\": 1,", "\"$schema\": \"./tsp-v1.schema.json\", \"schema\": 1, \"futureThing\": true,")).unwrap();
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    }

    #[test]
    fn unknown_group_renders_custom_with_warning() {
        let l = load(&with_button(r#"{"id":"a","label":"A","group":"coars","action":{"type":"keys","combo":"A"}}"#)).unwrap();
        assert_eq!(l.profile.buttons[0].group, Group::Custom);
        assert_eq!(l.warnings.len(), 1);
    }

    #[test]
    fn dock_left_is_accepted_and_other_values_warn() {
        let l = load(&MINIMAL.replace("\"gap\": 8", "\"gap\": 8, \"dock\": \"left\"")).unwrap();
        assert_eq!(l.profile.pad.dock, Dock::Left);
        let l = load(&MINIMAL.replace("\"gap\": 8", "\"gap\": 8, \"dock\": \"top\"")).unwrap();
        assert_eq!(l.profile.pad.dock, Dock::Right);
        assert_eq!(l.warnings.len(), 1);
        assert_eq!(l.doc["pad"]["dock"], "top", "display fallback is not written back");
    }

    #[test]
    fn bad_colour_and_long_label_are_rejected() {
        assert!(err(&with_button(r#"{"id":"a","label":"A","color":"red","action":{"type":"keys","combo":"A"}}"#)).message.contains("color"));
        let long = "x".repeat(25);
        assert!(err(&with_button(&format!(r#"{{"id":"a","label":"{long}","action":{{"type":"keys","combo":"A"}}}}"#))).message.contains("label"));
    }

    #[test]
    fn oversized_and_non_utf8_files_are_rejected() {
        let big = vec![b' '; MAX_PROFILE_BYTES + 1];
        assert_eq!(load_profile(&big, false).unwrap_err().code, "profile-too-large");
        assert_eq!(load_profile(&[0xff, 0xfe, 0x00], false).unwrap_err().code, "profile-not-utf8");
    }

    #[test]
    fn empty_v1_button_list_is_rejected() {
        assert_eq!(err(&MINIMAL.replace(
            &MINIMAL[MINIMAL.find("\"buttons\"").unwrap()..MINIMAL.rfind('}').unwrap()],
            "\"buttons\": []\n")).code, "profile-bound");
    }

    #[test]
    fn target_apps_are_read() {
        let l = load(&MINIMAL.replace("\"schema\": 1,", "\"schema\": 1, \"targetApps\": [\"R1.exe\"],")).unwrap();
        assert_eq!(l.profile.target_apps, Some(vec!["R1.exe".to_string()]));
    }

    #[test]
    fn disabled_actions_keep_their_raw_json() {
        let l = load(&v2_with(
            r#"{"id":"r","label":"Run","x":0,"y":0,"action":{"type":"keys","combo":"Win+R"}},
               {"id":"q","label":"Go","x":1,"y":0,"action":{"type":"qlab","cue":"1","workspace":"Show"}}"#,
        ))
        .unwrap();
        assert_eq!(l.doc["buttons"][0]["action"], json!({"type": "keys", "combo": "Win+R"}));
        assert_eq!(l.doc["buttons"][1]["action"], json!({"type": "qlab", "cue": "1", "workspace": "Show"}));
    }

    #[test]
    fn text_output_is_two_space_pretty_with_trailing_newline() {
        let t = to_text(&json!({"a": {"b": 1}}));
        assert_eq!(t, "{\n  \"a\": {\n    \"b\": 1\n  }\n}\n");
    }

    #[test]
    fn combo_error_mapping_is_total() {
        // Every ComboError becomes a disabled button, never a panic.
        for e in [ComboError::Empty, ComboError::ModifiersOnly, ComboError::TooManyKeys, ComboError::DuplicateModifier("Ctrl"), ComboError::UnknownKey("x".into())] {
            let _ = disabled_from_combo_error("A", &e);
        }
    }

    #[test]
    fn page_actions_and_text_size_load() {
        let l = load(&v2_with(
            r#"{"id":"go","label":"FX","x":0,"y":0,"textSize":"small","action":{"type":"page","target":"fx pad.tsp.json"}},
               {"id":"bk","label":"Back","x":1,"y":0,"action":{"type":"page","target":"back"}},
               {"id":"bad","label":"Bad","x":2,"y":0,"action":{"type":"page","target":"../secret.tsp.json"}}"#,
        ))
        .unwrap();
        let b = &l.profile.buttons;
        assert_eq!(b[0].action, Action::Page { target: PageTarget::File("fx pad.tsp.json".into()) });
        assert_eq!(b[0].text_size, TextSize::Small);
        assert_eq!(b[1].action, Action::Page { target: PageTarget::Back });
        assert_eq!(b[1].text_size, TextSize::Large);
        assert!(matches!(b[2].action, Action::Disabled { code: "action-unknown", .. }), "no folders in targets");
        assert!(page_target("C:\\x.tsp.json").is_none());
        assert!(page_target("notes.txt").is_none());
        let e = err(&v2_with(r#"{"id":"a","label":"A","x":0,"y":0,"textSize":"huge","action":{"type":"none"}}"#));
        assert!(e.message.contains("textSize"), "{}", e.message);
    }
}
