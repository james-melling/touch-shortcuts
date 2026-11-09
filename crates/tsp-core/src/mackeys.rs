//! Windows virtual-key codes → macOS virtual keycodes (`kVK_*`) and
//! modifier flags, so one profile drives both platforms.
//!
//! `Win` maps to Command. Numpad keys map to the keypad, which types digits
//! in text fields regardless of layout, like on Windows.

/// `CGEventFlags` bits.
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_ALT: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;
pub const FLAG_NUMPAD: u64 = 0x0020_0000;

/// ANSI-position keycodes for A..Z (kVK_ANSI_A..Z are not alphabetical).
const LETTERS: [u16; 26] = [
    0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, // A-M
    0x2D, 0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06, // N-Z
];
const DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
const KEYPAD: [u16; 10] = [0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5B, 0x5C];
const FKEYS: [u16; 12] = [0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F];

pub fn mac_keycode(vk: u16) -> Option<u16> {
    Some(match vk {
        0x30..=0x39 => DIGITS[(vk - 0x30) as usize],
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x60..=0x69 => KEYPAD[(vk - 0x60) as usize],
        0x70..=0x7B => FKEYS[(vk - 0x70) as usize],
        0xBE => 0x2F, // .
        0xBC => 0x2B, // ,
        0x6D => 0x4E, // keypad minus
        0x6B => 0x45, // keypad plus
        0x6E => 0x41, // keypad decimal
        0x0D => 0x24, // Return
        0x09 => 0x30, // Tab
        0x1B => 0x35, // Esc
        0x20 => 0x31, // Space
        0x08 => 0x33, // Backspace
        0x2E => 0x75, // forward Delete
        0x26 => 0x7E, // Up
        0x28 => 0x7D, // Down
        0x25 => 0x7B, // Left
        0x27 => 0x7C, // Right
        0x24 => 0x73, // Home
        0x23 => 0x77, // End
        0x21 => 0x74, // PageUp
        0x22 => 0x79, // PageDown
        0x11 => 0x3B, // Control
        0x10 => 0x38, // Shift
        0x12 => 0x3A, // Option
        0x5B => 0x37, // Command (Win)
        _ => return None,
    })
}

/// Flag bit a modifier VK sets while held (0 for non-modifiers).
pub fn modifier_flag(vk: u16) -> u64 {
    match vk {
        0x11 => FLAG_CONTROL,
        0x10 => FLAG_SHIFT,
        0x12 => FLAG_ALT,
        0x5B => FLAG_COMMAND,
        _ => 0,
    }
}

/// Extra flag macOS sets on the key itself (keypad and arrow keys).
pub fn key_flag(vk: u16) -> u64 {
    match vk {
        0x60..=0x69 | 0x6B | 0x6D | 0x6E | 0x25..=0x28 => FLAG_NUMPAD,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combo::parse_combo;

    #[test]
    fn every_grammar_key_has_a_pinned_mac_keycode() {
        let table: &[(&str, u16)] = &[
            ("0", 0x1D), ("1", 0x12), ("2", 0x13), ("3", 0x14), ("4", 0x15),
            ("5", 0x17), ("6", 0x16), ("7", 0x1A), ("8", 0x1C), ("9", 0x19),
            ("A", 0x00), ("B", 0x0B), ("C", 0x08), ("D", 0x02), ("E", 0x0E), ("F", 0x03), ("G", 0x05),
            ("H", 0x04), ("I", 0x22), ("J", 0x26), ("K", 0x28), ("L", 0x25), ("M", 0x2E), ("N", 0x2D),
            ("O", 0x1F), ("P", 0x23), ("Q", 0x0C), ("R", 0x0F), ("S", 0x01), ("T", 0x11), ("U", 0x20),
            ("V", 0x09), ("W", 0x0D), ("X", 0x07), ("Y", 0x10), ("Z", 0x06),
            (".", 0x2F), ("Comma", 0x2B), ("Minus", 0x4E), ("Plus", 0x45),
            ("Num0", 0x52), ("Num1", 0x53), ("Num2", 0x54), ("Num3", 0x55), ("Num4", 0x56),
            ("Num5", 0x57), ("Num6", 0x58), ("Num7", 0x59), ("Num8", 0x5B), ("Num9", 0x5C), ("NumDecimal", 0x41),
            ("Enter", 0x24), ("Tab", 0x30), ("Esc", 0x35), ("Space", 0x31), ("Backspace", 0x33), ("Delete", 0x75),
            ("Up", 0x7E), ("Down", 0x7D), ("Left", 0x7B), ("Right", 0x7C),
            ("Home", 0x73), ("End", 0x77), ("PageUp", 0x74), ("PageDown", 0x79),
            ("F1", 0x7A), ("F2", 0x78), ("F3", 0x63), ("F4", 0x76), ("F5", 0x60), ("F6", 0x61),
            ("F7", 0x62), ("F8", 0x64), ("F9", 0x65), ("F10", 0x6D), ("F11", 0x67), ("F12", 0x6F),
        ];
        for &(name, code) in table {
            let vk = parse_combo(name).unwrap().key.vk;
            assert_eq!(mac_keycode(vk), Some(code), "key {name} (vk {vk:#x})");
        }
    }

    #[test]
    fn modifiers_map_to_mac_modifier_keys_and_flags() {
        for (name, code, flag) in [("Ctrl", 0x3B, FLAG_CONTROL), ("Shift", 0x38, FLAG_SHIFT), ("Alt", 0x3A, FLAG_ALT), ("Win", 0x37, FLAG_COMMAND)] {
            let vk = parse_combo(&format!("{name}+A")).unwrap().modifiers[0].vk;
            assert_eq!(mac_keycode(vk), Some(code), "{name}");
            assert_eq!(modifier_flag(vk), flag, "{name}");
        }
        assert_eq!(modifier_flag(parse_combo("A").unwrap().key.vk), 0);
    }

    #[test]
    fn keypad_and_arrow_keys_carry_the_numpad_flag() {
        for name in ["Num1", "NumDecimal", "Up", "Down", "Left", "Right"] {
            assert_eq!(key_flag(parse_combo(name).unwrap().key.vk), FLAG_NUMPAD, "{name}");
        }
        assert_eq!(key_flag(parse_combo("A").unwrap().key.vk), 0);
    }

    #[test]
    fn unknown_vk_has_no_mac_keycode() {
        assert_eq!(mac_keycode(0xFF), None);
    }
}
