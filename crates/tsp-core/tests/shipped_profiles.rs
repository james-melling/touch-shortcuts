//! Every shipped profile must load with no warnings and no disabled keys,
//! and the R1 starter must send exactly the pinned key events.

use std::path::Path;
use tsp_core::combo::press_events;
use tsp_core::geometry::{self, Cells, Grid, GridSpec};
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

/// Golden: the shipped v1 R1 profile, converted to schema 2, renders exactly
/// v1's rects on the field Surface (912 px work area: rows shrink to 74 px)
/// and on a tall screen (2000 px: natural 78 px).
#[test]
fn r1_v1_profile_converts_to_identical_rects() {
    let bytes = std::fs::read(profiles_dir().join("r1-delay-gain.tsp.json")).unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let spans: Vec<u32> = raw["buttons"].as_array().unwrap().iter().map(|b| b["span"].as_u64().unwrap_or(2) as u32).collect();
    let p = load_profile(&bytes, false).unwrap().profile;
    let grid = Grid { columns: p.pad.columns, rows: p.pad.rows, cell_width: p.pad.cell_width, cell_height: p.pad.cell_height, gap: p.pad.gap, scale: p.pad.scale };
    let cells: Vec<Cells> = p.buttons.iter().map(|b| Cells { x: b.x, y: b.y, w: b.w, h: b.h }).collect();
    for (wa, row) in [(912, 74.0), (2000, 78.0)] {
        let v1 = geometry::layout(&GridSpec { grid_columns: 6, button_width: 92, button_height: 78, gap: 8, spans: &spans }, wa);
        let v2 = geometry::fit(&grid, &cells, (1920, wa), false);
        assert_eq!(v2.buttons, v1.buttons, "work area {wa}");
        assert_eq!((v2.width, v2.height), (v1.width, v1.height), "work area {wa}");
        assert_eq!(v2.row_height, row);
    }
}

#[test]
fn example_v2_profile_is_schema_2_with_no_conversion() {
    let bytes = std::fs::read(profiles_dir().join("example-v2.tsp.json")).unwrap();
    let l = load_profile(&bytes, false).unwrap();
    assert_eq!(l.source_schema, 2);
    assert_eq!(l.profile.buttons.len(), 21);
    assert!(l.normalised.is_empty());
}

/// Golden text: as the editor saves it, one label edit changes exactly one line.
#[test]
fn one_label_edit_changes_one_line_of_the_saved_example() {
    use tsp_core::edit::{Op, apply};
    use tsp_core::profile::to_text;
    let bytes = std::fs::read(profiles_dir().join("example-v2.tsp.json")).unwrap();
    let l = load_profile(&bytes, false).unwrap();
    let a = apply(&l, &Op::SetLabel { button_id: "tab".into(), label: "Next".into() }, false).unwrap();
    let (before, after) = (to_text(&l.doc), to_text(&a.loaded.doc));
    assert_eq!(before.lines().count(), after.lines().count());
    let diff: Vec<_> = before.lines().zip(after.lines()).filter(|(x, y)| x != y).collect();
    assert_eq!(diff, vec![("      \"label\": \"Tab\",", "      \"label\": \"Next\",")]);
}
