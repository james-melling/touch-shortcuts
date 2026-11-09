//! Every shipped profile must load with no warnings and no disabled keys,
//! and the R1 starter must send exactly the pinned key events.

use std::path::Path;
use tsp_core::combo::press_events;
use tsp_core::profile::{Action, load_profile};

fn profiles_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles")
}

#[test]
fn every_shipped_profile_loads_cleanly() {
    let mut count = 0;
    for entry in std::fs::read_dir(profiles_dir()).expect("profiles/ exists") {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".tsp.json") {
            continue;
        }
        count += 1;
        let loaded = load_profile(&std::fs::read(&path).unwrap(), false)
            .unwrap_or_else(|e| panic!("{}: {} {}", path.display(), e.code, e.message));
        assert!(loaded.warnings.is_empty(), "{}: {:?}", path.display(), loaded.warnings);
        for b in &loaded.profile.buttons {
            assert!(matches!(b.action, Action::Keys { .. }), "{}: {} is disabled: {:?}", path.display(), b.id, b.action);
        }
    }
    assert!(count >= 1, "no shipped profiles found");
}

/// Golden: (button id, repeat, [(vk, down, extended)...] for one press).
#[test]
fn r1_starter_sends_pinned_key_events() {
    let bytes = std::fs::read(profiles_dir().join("r1-delay-gain.tsp.json")).unwrap();
    let p = load_profile(&bytes, false).unwrap().profile;
    let got: Vec<(String, u32, Vec<(u16, bool, bool)>)> = p
        .buttons
        .iter()
        .map(|b| match &b.action {
            Action::Keys { combo, repeat, .. } => (
                b.id.clone(),
                *repeat,
                press_events(combo).iter().map(|e| (e.vk, e.down, e.extended)).collect(),
            ),
            other => panic!("{other:?}"),
        })
        .collect();
    let key = |vk: u16, ext: bool| vec![(vk, true, ext), (vk, false, ext)];
    let ctrl = |vk: u16, ext: bool| vec![(0x11, true, false), (vk, true, ext), (vk, false, ext), (0x11, false, false)];
    let want: Vec<(String, u32, Vec<(u16, bool, bool)>)> = vec![
        ("d7".into(), 1, key(0x67, false)),
        ("d8".into(), 1, key(0x68, false)),
        ("d9".into(), 1, key(0x69, false)),
        ("d4".into(), 1, key(0x64, false)),
        ("d5".into(), 1, key(0x65, false)),
        ("d6".into(), 1, key(0x66, false)),
        ("d1".into(), 1, key(0x61, false)),
        ("d2".into(), 1, key(0x62, false)),
        ("d3".into(), 1, key(0x63, false)),
        ("d0".into(), 1, key(0x60, false)),
        ("dot".into(), 1, key(0xBE, false)),
        ("enter".into(), 1, key(0x0D, false)),
        ("up".into(), 1, key(0x26, true)),
        ("down".into(), 1, key(0x28, true)),
        ("ctrl-up".into(), 1, ctrl(0x26, true)),
        ("ctrl-down".into(), 1, ctrl(0x28, true)),
        ("up10".into(), 10, key(0x26, true)),
        ("down10".into(), 10, key(0x28, true)),
        ("tab".into(), 1, key(0x09, false)),
        ("undo".into(), 1, ctrl(0x5A, false)),
        ("redo".into(), 1, ctrl(0x59, false)),
    ];
    assert_eq!(got, want);
}
