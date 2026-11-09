//! Profile format v1 (`*.tsp.json`): the only parser and validator.
//!
//! Parsing goes through `serde_json::Value` first (line/column for syntax
//! errors), then each button is validated on its own so one bad button is
//! disabled instead of rejecting the whole pad. Structural errors reject the
//! profile with a `<code> <locator>: <actual vs allowed>. <fix>` message.

use crate::combo::{Combo, ComboError, is_system_combo, parse_combo};

pub const MAX_PROFILE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group { Num, Ent, Fine, Coarse, Hist, Custom }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dock { Right, Left }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Keys { combo: Combo, repeat: u32, interval_ms: u32 },
    /// Unknown key name, unknown action type or blocked system key: the key
    /// renders disabled and tapping it shows `code: reason`.
    Disabled { code: &'static str, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub id: String,
    pub label: String,
    pub span: u32,
    pub group: Group,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub grid_columns: u32,
    pub button_width: u32,
    pub button_height: u32,
    pub gap: u32,
    pub dock: Dock,
    pub theme: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub profile: Profile,
    pub warnings: Vec<Warning>,
}

use serde_json::{Map, Value};

const TOP_FIELDS: &[&str] = &["$schema", "schema", "name", "acceptTouch", "targetApps", "pad", "buttons"];
const PAD_FIELDS: &[&str] = &["gridColumns", "buttonWidth", "buttonHeight", "gap", "dock", "theme"];
const BUTTON_FIELDS: &[&str] = &["id", "label", "span", "group", "icon", "color", "action"];
const ACTION_FIELDS: &[&str] = &["type", "combo", "repeat", "intervalMs"];
const THEMES: &[&str] = &["c-low-glare", "b-flat"];
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
    /// known field (edit distance ≤ 2), which is usually a typo that would
    /// otherwise be ignored without a word.
    fn check_fields(&mut self, obj: &Map<String, Value>, known: &[&str], locator: &str) {
        for name in obj.keys() {
            if known.contains(&name.as_str()) {
                continue;
            }
            if let Some(close) = known.iter().find(|k| edit_distance(&k.to_lowercase(), &name.to_lowercase()) <= 2) {
                let at = if locator.is_empty() { String::new() } else { format!("{locator}.") };
                self.warn("profile-unknown-field", format!("{at}{name} is not a profile field; did you mean {close}? It was ignored."));
            }
        }
    }
}

fn int(obj: &Map<String, Value>, field: &str, locator: &str, default: Option<u64>, lo: u64, hi: u64) -> Result<u64, ProfileError> {
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

fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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
    if kind != "keys" {
        return Ok(Action::Disabled { code: "action-unknown", reason: format!("action type \"{kind}\" is not supported by this version") });
    }
    ctx.check_fields(a, ACTION_FIELDS, alocator.trim_end_matches('.'));
    let repeat = int(a, "repeat", locator, Some(1), 1, 50)? as u32;
    let interval_ms = int(a, "intervalMs", locator, Some(25), 10, 1000)? as u32;
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
    let mut warnings = Vec::new();
    let mut ctx = Ctx { warnings: &mut warnings };
    ctx.check_fields(&top, TOP_FIELDS, "");

    let schema = int(&top, "schema", "", None, 1, u64::MAX)?;
    if schema > 1 {
        return Err(ProfileError {
            code: "profile-schema-too-new",
            message: format!("profile schema {schema}, this build supports 1. Download the latest touch-shortcuts release."),
        });
    }
    let name = text(&top, "name", "", 48)?.ok_or_else(|| bound(format!("name is required. Add it, {RELOAD}")))?.to_string();
    let accept_touch = top.get("acceptTouch").and_then(Value::as_bool).unwrap_or(true);
    let target_apps = top.get("targetApps").and_then(Value::as_array).map(|a| {
        a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
    });

    let Some(Value::Object(pad)) = top.get("pad") else {
        return Err(bound(format!("pad is required. Add it, {RELOAD}")));
    };
    ctx.check_fields(pad, PAD_FIELDS, "pad");
    let grid_columns = int(pad, "gridColumns", "pad.", None, 1, 12)? as u32;
    let button_width = int(pad, "buttonWidth", "pad.", None, 40, 200)? as u32;
    let button_height = int(pad, "buttonHeight", "pad.", None, 40, 200)? as u32;
    let gap = int(pad, "gap", "pad.", None, 0, 24)? as u32;
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

    let list = top.get("buttons").and_then(Value::as_array).cloned().unwrap_or_default();
    if list.is_empty() || list.len() > 60 {
        return Err(bound(format!("buttons has {} entries (allowed 1-60). Fix the list, {RELOAD}", list.len())));
    }
    let mut buttons = Vec::with_capacity(list.len());
    for (i, b) in list.iter().enumerate() {
        let Value::Object(b) = b else {
            return Err(bound(format!("buttons[{i}] must be an object. Fix it, {RELOAD}")));
        };
        let label_raw = b.get("label").and_then(Value::as_str);
        let locator = match label_raw {
            Some(l) => format!("buttons[{i}] \"{l}\"."),
            None => format!("buttons[{i}]."),
        };
        ctx.check_fields(b, BUTTON_FIELDS, locator.trim_end_matches('.'));
        let id = match b.get("id").and_then(Value::as_str) {
            None => return Err(bound(format!("{locator}id is required. Add a unique id like \"d7\", {RELOAD}"))),
            Some(id) if !valid_id(id) => {
                return Err(bound(format!("{locator}id \"{id}\" must be 1-32 characters of a-z, 0-9 or -. Fix it, {RELOAD}")));
            }
            Some(id) => id.to_string(),
        };
        if buttons.iter().any(|x: &Button| x.id == id) {
            return Err(bound(format!("{locator}id \"{id}\" is a duplicate id. Give each button its own id, {RELOAD}")));
        }
        let label = text(b, "label", &locator, 24)?.ok_or_else(|| bound(format!("{locator}label is required. Add it, {RELOAD}")))?.to_string();
        let span = int(b, "span", &locator, Some(2), 1, u64::from(grid_columns))? as u32;
        let group = group(b.get("group").and_then(Value::as_str), &locator, &mut ctx);
        let icon = b.get("icon").and_then(Value::as_str).map(str::to_string);
        let color = match b.get("color").and_then(Value::as_str) {
            None => None,
            Some(c) if valid_color(c) => Some(c.to_string()),
            Some(c) => return Err(bound(format!("{locator}color \"{c}\" must look like #RRGGBB. Fix it, {RELOAD}"))),
        };
        let action = action(b, &locator, allow_system_keys, &mut ctx)?;
        buttons.push(Button { id, label, span, group, icon, color, action });
    }
    Ok(Loaded {
        profile: Profile { name, accept_touch, target_apps, pad: Pad { grid_columns, button_width, button_height, gap, dock, theme }, buttons },
        warnings,
    })
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

    fn with_button(button: &str) -> String {
        format!(
            r#"{{"schema":1,"name":"T","pad":{{"gridColumns":6,"buttonWidth":92,"buttonHeight":78,"gap":8}},"buttons":[{button}]}}"#
        )
    }

    fn load(text: &str) -> Result<Loaded, ProfileError> {
        load_profile(text.as_bytes(), false)
    }

    fn err(text: &str) -> ProfileError {
        load(text).expect_err("expected the profile to be rejected")
    }

    #[test]
    fn minimal_profile_loads_with_defaults() {
        let l = load(MINIMAL).unwrap();
        let p = &l.profile;
        assert_eq!(p.name, "R1 - Delay/Gain");
        assert!(p.accept_touch, "touch fires by default (final gate d5)");
        assert_eq!(p.target_apps, None);
        assert_eq!(p.pad.dock, Dock::Right);
        assert_eq!(p.pad.theme, "c-low-glare");
        assert_eq!(p.buttons[0].span, 2);
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
    fn syntax_error_reports_line_and_column() {
        let e = err("{\n  \"schema\": 1,\n  \"name\": \"x\"\n  \"pad\": {}\n}");
        assert_eq!(e.code, "profile-syntax");
        assert!(e.message.contains("line 4"), "{}", e.message);
    }

    #[test]
    fn newer_schema_is_rejected_with_upgrade_hint() {
        let e = err(&MINIMAL.replace("\"schema\": 1", "\"schema\": 2"));
        assert_eq!(e.code, "profile-schema-too-new");
        assert!(e.message.contains("schema 2"), "{}", e.message);
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
    fn empty_button_list_is_rejected() {
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
    fn combo_error_mapping_is_total() {
        // Every ComboError becomes a disabled button, never a panic.
        for e in [ComboError::Empty, ComboError::ModifiersOnly, ComboError::TooManyKeys, ComboError::DuplicateModifier("Ctrl"), ComboError::UnknownKey("x".into())] {
            let _ = disabled_from_combo_error("A", &e);
        }
    }
}
