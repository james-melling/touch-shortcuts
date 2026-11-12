//! Combo grammar: `Mod+Mod+Key` → Windows virtual-key codes.
//!
//! Keys are always sent as virtual-key codes (never Unicode / VK_PACKET).
//! Navigation keys carry the extended-key flag. Starter profiles use the
//! numpad digits (`Num0`-`Num9`) because number-row VKs depend on the
//! keyboard layout (AZERTY types `&` for VK `1`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub name: &'static str,
    pub vk: u16,
    pub extended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    /// Modifiers in press order (Ctrl, Shift, Alt, Win as written).
    pub modifiers: Vec<Key>,
    pub key: Key,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComboError {
    Empty,
    UnknownKey(String),
    ModifiersOnly,
    TooManyKeys,
    DuplicateModifier(&'static str),
}

/// One `INPUT` record for `SendInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u16,
    pub down: bool,
    pub extended: bool,
}

const fn k(name: &'static str, vk: u16, extended: bool) -> Key {
    Key { name, vk, extended }
}

pub const CTRL: Key = k("Ctrl", 0x11, false);
pub const SHIFT: Key = k("Shift", 0x10, false);
pub const ALT: Key = k("Alt", 0x12, false);
pub const WIN: Key = k("Win", 0x5B, true);

const VK_ESC: u16 = 0x1B;
const VK_TAB: u16 = 0x09;
const VK_SPACE: u16 = 0x20;
const VK_F4: u16 = 0x73;

const NAMED: &[Key] = &[
    k(".", 0xBE, false),
    k("Minus", 0x6D, false), // VK_SUBTRACT: layout-independent
    k("Plus", 0x6B, false),  // VK_ADD: layout-independent
    k("Comma", 0xBC, false),
    k("NumDecimal", 0x6E, false),
    k("Enter", 0x0D, false),
    k("Tab", VK_TAB, false),
    k("Esc", VK_ESC, false),
    k("Space", VK_SPACE, false),
    k("Backspace", 0x08, false),
    k("Delete", 0x2E, true),
    k("Up", 0x26, true),
    k("Down", 0x28, true),
    k("Left", 0x25, true),
    k("Right", 0x27, true),
    k("Home", 0x24, true),
    k("End", 0x23, true),
    k("PageUp", 0x21, true),
    k("PageDown", 0x22, true),
];

const ALIASES: &[(&str, &str)] = &[
    ("control", "Ctrl"),
    ("return", "Enter"),
    ("escape", "Esc"),
    ("del", "Delete"),
    ("arrowup", "Up"),
    ("arrowdown", "Down"),
    ("arrowleft", "Left"),
    ("arrowright", "Right"),
    ("-", "Minus"),
];

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const NUMS: [&str; 10] = ["Num0", "Num1", "Num2", "Num3", "Num4", "Num5", "Num6", "Num7", "Num8", "Num9"];
const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const FKEYS: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

fn modifier(token: &str) -> Option<Key> {
    [CTRL, SHIFT, ALT, WIN].into_iter().find(|m| m.name.eq_ignore_ascii_case(token))
}

fn key(token: &str) -> Option<Key> {
    let pos = |names: &[&'static str]| names.iter().position(|n| n.eq_ignore_ascii_case(token));
    if let Some(i) = pos(&DIGITS) {
        return Some(k(DIGITS[i], 0x30 + i as u16, false));
    }
    if let Some(i) = pos(&LETTERS) {
        return Some(k(LETTERS[i], 0x41 + i as u16, false));
    }
    if let Some(i) = pos(&NUMS) {
        return Some(k(NUMS[i], 0x60 + i as u16, false));
    }
    if let Some(i) = pos(&FKEYS) {
        return Some(k(FKEYS[i], 0x70 + i as u16, false));
    }
    NAMED.iter().copied().find(|n| n.name.eq_ignore_ascii_case(token))
}

fn canonical(token: &str) -> &str {
    ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(token))
        .map_or(token, |(_, name)| name)
}

pub fn parse_combo(text: &str) -> Result<Combo, ComboError> {
    if text.trim().is_empty() {
        return Err(ComboError::Empty);
    }
    let mut modifiers: Vec<Key> = Vec::new();
    let mut main: Option<Key> = None;
    // "-" is a key on its own; any other text splits on '+'.
    let tokens: Vec<&str> = if text.trim() == "-" { vec!["-"] } else { text.split('+').map(str::trim).collect() };
    for raw in tokens {
        let token = canonical(raw);
        if let Some(m) = modifier(token) {
            if modifiers.iter().any(|x| x.vk == m.vk) {
                return Err(ComboError::DuplicateModifier(m.name));
            }
            modifiers.push(m);
        } else if let Some(kk) = key(token) {
            if main.is_some() {
                return Err(ComboError::TooManyKeys);
            }
            main = Some(kk);
        } else {
            return Err(ComboError::UnknownKey(raw.to_string()));
        }
    }
    main.map(|key| Combo { modifiers, key }).ok_or(ComboError::ModifiersOnly)
}

/// Canonical text: modifiers in Ctrl, Shift, Alt, Win order, then the key.
pub fn canonical_text(combo: &Combo) -> String {
    let mut out = String::new();
    for m in [CTRL, SHIFT, ALT, WIN] {
        if combo.modifiers.iter().any(|x| x.vk == m.vk) {
            out.push_str(m.name);
            out.push('+');
        }
    }
    out.push_str(combo.key.name);
    out
}

/// Same keys regardless of modifier order or aliases.
pub fn same_keys(a: &Combo, b: &Combo) -> bool {
    a.key.vk == b.key.vk && a.modifiers.len() == b.modifiers.len() && a.modifiers.iter().all(|m| b.modifiers.iter().any(|x| x.vk == m.vk))
}

/// Record from keyboard: maps a `KeyboardEvent.code` to a grammar key name.
/// Digit0-9 and Period are the number-row keys; codes with no grammar key
/// return `None` (the panel says "<key> isn't supported: pick it with the pen").
pub fn key_from_code(code: &str) -> Option<&'static str> {
    if let Some(l) = code.strip_prefix("Key") {
        return LETTERS.iter().copied().find(|x| *x == l);
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return DIGITS.iter().copied().find(|x| *x == d);
    }
    if let Some(d) = code.strip_prefix("Numpad") {
        if let Some(i) = DIGITS.iter().position(|x| *x == d) {
            return Some(NUMS[i]);
        }
        return match d {
            "Decimal" => Some("NumDecimal"),
            "Add" => Some("Plus"),
            "Subtract" => Some("Minus"),
            "Enter" => Some("Enter"),
            _ => None,
        };
    }
    if code.starts_with('F') && code.len() <= 3 {
        return FKEYS.iter().copied().find(|x| *x == code);
    }
    Some(match code {
        "Period" => ".",
        "Comma" => "Comma",
        "Enter" => "Enter",
        "Tab" => "Tab",
        "Escape" => "Esc",
        "Space" => "Space",
        "Backspace" => "Backspace",
        "Delete" => "Delete",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "Home" => "Home",
        "End" => "End",
        "PageUp" => "PageUp",
        "PageDown" => "PageDown",
        _ => return None,
    })
}

/// Modifier codes are held, never recorded as the key.
pub fn is_modifier_code(code: &str) -> bool {
    matches!(code, "ControlLeft" | "ControlRight" | "ShiftLeft" | "ShiftRight" | "AltLeft" | "AltRight" | "MetaLeft" | "MetaRight" | "OSLeft" | "OSRight")
}

/// Turns a recorded keydown into canonical combo text, or the message to show.
pub fn record(code: &str, ctrl: bool, shift: bool, alt: bool, meta: bool) -> Result<String, String> {
    if meta {
        return Err("Win combos: pick them with the pen".into());
    }
    let Some(key) = key_from_code(code) else {
        let shown = code.strip_prefix("Key").or_else(|| code.strip_prefix("Digit")).unwrap_or(code);
        return Err(format!("{shown} isn't supported: pick it with the pen"));
    };
    let mut text = String::new();
    for (on, m) in [(ctrl, CTRL), (shift, SHIFT), (alt, ALT)] {
        if on {
            text.push_str(m.name);
            text.push('+');
        }
    }
    text.push_str(key);
    parse_combo(&text).map(|c| canonical_text(&c)).map_err(|_| format!("{code} isn't supported: pick it with the pen"))
}

/// Combos that can take over the machine. Allowed only by the local
/// `settings.json` (`allowSystemKeys`), never by a profile.
pub fn is_system_combo(combo: &Combo) -> bool {
    let has = |m: Key| combo.modifiers.iter().any(|x| x.vk == m.vk);
    let only = |ms: &[Key]| combo.modifiers.len() == ms.len() && ms.iter().all(|m| has(*m));
    let key = combo.key.vk;
    has(WIN)
        || (only(&[CTRL]) && key == VK_ESC)
        || (only(&[ALT]) && matches!(key, VK_F4 | VK_TAB | VK_ESC | VK_SPACE))
        || (only(&[CTRL, SHIFT]) && key == VK_ESC)
}

/// One press as a single `SendInput` batch: modifiers down, key down, key up,
/// modifiers up in reverse order.
pub fn press_events(combo: &Combo) -> Vec<KeyEvent> {
    let ev = |kk: &Key, down| KeyEvent { vk: kk.vk, down, extended: kk.extended };
    let mut out: Vec<KeyEvent> = combo.modifiers.iter().map(|m| ev(m, true)).collect();
    out.push(ev(&combo.key, true));
    out.push(ev(&combo.key, false));
    out.extend(combo.modifiers.iter().rev().map(|m| ev(m, false)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk(text: &str) -> (Vec<u16>, u16, bool) {
        let c = parse_combo(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        (c.modifiers.iter().map(|m| m.vk).collect(), c.key.vk, c.key.extended)
    }

    #[test]
    fn single_keys_map_to_pinned_vks() {
        // Pins every grammar key so a table edit can't silently change output.
        let table: &[(&str, u16, bool)] = &[
            ("0", 0x30, false), ("9", 0x39, false), ("A", 0x41, false), ("Z", 0x5A, false),
            (".", 0xBE, false), ("Minus", 0x6D, false), ("Plus", 0x6B, false), ("Comma", 0xBC, false),
            ("Num0", 0x60, false), ("Num7", 0x67, false), ("Num9", 0x69, false), ("NumDecimal", 0x6E, false),
            ("Enter", 0x0D, false), ("Tab", 0x09, false), ("Esc", 0x1B, false), ("Space", 0x20, false),
            ("Backspace", 0x08, false), ("Delete", 0x2E, true),
            ("Up", 0x26, true), ("Down", 0x28, true), ("Left", 0x25, true), ("Right", 0x27, true),
            ("Home", 0x24, true), ("End", 0x23, true), ("PageUp", 0x21, true), ("PageDown", 0x22, true),
            ("F1", 0x70, false), ("F12", 0x7B, false),
        ];
        for &(text, code, ext) in table {
            assert_eq!(vk(text), (vec![], code, ext), "key {text}");
        }
    }

    #[test]
    fn modifiers_parse_in_written_order() {
        assert_eq!(vk("Ctrl+Down"), (vec![0x11], 0x28, true));
        assert_eq!(vk("Shift+Tab"), (vec![0x10], 0x09, false));
        assert_eq!(vk("Ctrl+Shift+Z"), (vec![0x11, 0x10], 0x5A, false));
        assert_eq!(vk("Win+R"), (vec![0x5B], 0x52, false));
    }

    #[test]
    fn case_whitespace_and_aliases_are_accepted() {
        assert_eq!(vk("ctrl + z"), (vec![0x11], 0x5A, false));
        assert_eq!(vk("Control+Return"), (vec![0x11], 0x0D, false));
        assert_eq!(vk("escape"), (vec![], 0x1B, false));
        assert_eq!(vk("Del"), (vec![], 0x2E, true));
        assert_eq!(vk("ArrowUp"), (vec![], 0x26, true));
        assert_eq!(vk("-"), (vec![], 0x6D, false));
        assert_eq!(vk("num1"), (vec![], 0x61, false));
    }

    #[test]
    fn invalid_combos_are_rejected() {
        assert_eq!(parse_combo(""), Err(ComboError::Empty));
        assert_eq!(parse_combo("   "), Err(ComboError::Empty));
        assert_eq!(parse_combo("Ctrl+Foo"), Err(ComboError::UnknownKey("Foo".into())));
        assert_eq!(parse_combo("Ctrl+Shift"), Err(ComboError::ModifiersOnly));
        assert_eq!(parse_combo("Ctrl"), Err(ComboError::ModifiersOnly));
        assert_eq!(parse_combo("A+B"), Err(ComboError::TooManyKeys));
        assert_eq!(parse_combo("Ctrl+Ctrl+A"), Err(ComboError::DuplicateModifier("Ctrl")));
        assert_eq!(parse_combo("Ctrl+"), Err(ComboError::UnknownKey("".into())));
    }

    #[test]
    fn system_combos_are_flagged() {
        for text in ["Win+R", "Win+D", "Ctrl+Esc", "Alt+F4", "Alt+Tab", "Alt+Esc", "Alt+Space", "Ctrl+Shift+Esc"] {
            assert!(is_system_combo(&parse_combo(text).unwrap()), "{text} should be a system combo");
        }
        for text in ["Ctrl+Z", "Ctrl+Up", "Alt+F", "Shift+Tab", "Esc", "Num7"] {
            assert!(!is_system_combo(&parse_combo(text).unwrap()), "{text} should not be a system combo");
        }
    }

    #[test]
    fn one_press_is_mods_down_key_down_key_up_mods_up_reversed() {
        let c = parse_combo("Ctrl+Shift+Down").unwrap();
        let ev = press_events(&c);
        let shape: Vec<(u16, bool, bool)> = ev.iter().map(|e| (e.vk, e.down, e.extended)).collect();
        assert_eq!(
            shape,
            vec![
                (0x11, true, false),
                (0x10, true, false),
                (0x28, true, true),
                (0x28, false, true),
                (0x10, false, false),
                (0x11, false, false),
            ]
        );
    }

    #[test]
    fn plain_key_press_is_two_events() {
        let ev = press_events(&parse_combo("Num1").unwrap());
        assert_eq!(ev, vec![
            KeyEvent { vk: 0x61, down: true, extended: false },
            KeyEvent { vk: 0x61, down: false, extended: false },
        ]);
    }

    #[test]
    fn parser_never_panics_on_odd_input() {
        for text in ["+", "++", "+A", "A+", "Ctrl++", "🎛️", "Ctrl+🎛️", "\0", "a b", &"x".repeat(10_000)] {
            let _ = parse_combo(text);
        }
    }

    #[test]
    fn canonical_text_orders_modifiers_and_resolves_aliases() {
        let c = parse_combo("shift + control + arrowup").unwrap();
        assert_eq!(canonical_text(&c), "Ctrl+Shift+Up");
        assert!(same_keys(&c, &parse_combo("Ctrl+Shift+Up").unwrap()));
        assert!(!same_keys(&c, &parse_combo("Ctrl+Up").unwrap()));
    }

    #[test]
    fn keyboard_codes_map_to_the_grammar() {
        let table: &[(&str, &str)] = &[
            ("KeyA", "A"), ("KeyZ", "Z"), ("Digit0", "0"), ("Digit9", "9"), ("Period", "."), ("Comma", "Comma"),
            ("Numpad0", "Num0"), ("Numpad7", "Num7"), ("NumpadDecimal", "NumDecimal"), ("NumpadAdd", "Plus"),
            ("NumpadSubtract", "Minus"), ("NumpadEnter", "Enter"), ("Enter", "Enter"), ("Tab", "Tab"),
            ("Escape", "Esc"), ("Space", "Space"), ("Backspace", "Backspace"), ("Delete", "Delete"),
            ("ArrowUp", "Up"), ("ArrowDown", "Down"), ("ArrowLeft", "Left"), ("ArrowRight", "Right"),
            ("Home", "Home"), ("End", "End"), ("PageUp", "PageUp"), ("PageDown", "PageDown"), ("F1", "F1"), ("F12", "F12"),
        ];
        for &(code, name) in table {
            assert_eq!(key_from_code(code), Some(name), "{code}");
            assert!(parse_combo(name).is_ok(), "{name} must be in the grammar");
        }
        for code in ["Equal", "BracketLeft", "Insert", "F13", "NumpadMultiply", "IntlBackslash", "ControlLeft"] {
            assert_eq!(key_from_code(code), None, "{code}");
        }
    }

    #[test]
    fn record_builds_canonical_combos_and_explains_refusals() {
        assert_eq!(record("ArrowDown", true, false, false, false).unwrap(), "Ctrl+Down");
        assert_eq!(record("KeyZ", true, true, true, false).unwrap(), "Ctrl+Shift+Alt+Z");
        assert_eq!(record("Escape", false, false, false, false).unwrap(), "Esc");
        assert_eq!(record("Equal", false, false, false, false).unwrap_err(), "Equal isn't supported: pick it with the pen");
        assert_eq!(record("KeyR", false, false, false, true).unwrap_err(), "Win combos: pick them with the pen");
        assert!(is_modifier_code("ShiftRight") && !is_modifier_code("KeyA"));
    }
}
