# Touch Shortcuts

A floating pen pad that sends keystrokes to d&b R1 and Meyer Compass on a
Windows tablet, without taking focus away from them. A modern replacement for
RadialMenu. Profiles are one JSON file you can share.

Status: pre-release, field test on a real system tune. **Keep RadialMenu
installed as your fallback.** Provided as-is: always check which field a key
lands in before relying on it during a show.

## Run it on the Surface (Tuesday build)

1. Install [rustup](https://rustup.rs), the MSVC Build Tools (C++ workload) and Node 22.
2. `npm ci`
3. `npm run tauri build` (or `npm run tauri dev` while testing)
4. Run the exe from `target/release/`. If R1 runs as administrator, run the pad as administrator too.

Or download the `touch-shortcuts-windows` installer from the latest
[windows-build](../../actions/workflows/windows-build.yml) run.

**Smart App Control:** if it is On, this unsigned build cannot run. Keep
RadialMenu until the signed release. Do not turn Smart App Control off.
SmartScreen may warn on first run: More info → Run anyway.

### Focus modes (the spike)

All three are built in; pick one at launch:

```
touch-shortcuts.exe --focus-mode=1   # window is non-focusable (default)
touch-shortcuts.exe --focus-mode=2   # + WS_EX_NOACTIVATE and MA_NOACTIVATE subclass
touch-shortcuts.exe --focus-mode=3   # + give the foreground back before each send
```

`TSP_FOCUS_MODE=2` works too, or `"focusMode": 2` in `settings.json`.

## Files

- Profile and `settings.json`: `%APPDATA%\audio.melling.touchshortcuts\`
  (first run writes the R1 starter there; edit it, then ··· → Reload)
- Log: `%LOCALAPPDATA%\audio.melling.touchshortcuts\logs\actions.log`
  (one JSON line per tap: target window title, process, outcome). Logs contain
  window titles; check them before sharing. Crash files land next to it.

`settings.json` (all optional):

```json
{ "focusMode": 1, "allowSystemKeys": false, "lastProfile": "C:\\path\\to\\my.tsp.json",
  "window": "opaque", "watchProfile": true }
```

- `"window": "opaque"` (or `--window=opaque`, or `TSP_WINDOW=opaque`) draws the
  pad fully opaque and turns floating mode off, if transparency ever misbehaves.
- `"watchProfile": false` stops the pad reloading when its file changes on disk.

## Edit a pad

1. Tap **··· → Edit**. The pad stays where it is; a panel opens beside it.
   Keys don't send while editing.
2. Tap a key to change it: pick keys with the pen (or **Record** from a
   keyboard), set ×1/×4/×10, size, label, colour and icon. Drag the selected key
   to move it (keys in the way are shoved aside, like home-screen icons); drag
   its corner handle to resize. Tap an empty cell, then **+ Add here**.
3. Tap **Done**. Changes are saved as you go, and focus goes back to R1.

The **Pad** tab sets the name, grid size, scale, pad opacity and background
opacity (0% = floating buttons: taps in the gaps reach R1). **··· → Pads**
switches pad files; **··· → New pad** starts from a preset. Undo edit / Redo
edit undo editor changes (the pad's own Undo/Redo keys send Ctrl+Z / Ctrl+Y to R1).

**Edited pads need v0.2 or later.** The first edit of an older pad saves it in
format v2 and keeps the original as `<name>.v1.bak`.

### Restore a v1 backup

Quit the pad, delete (or rename) `<name>.tsp.json`, rename
`<name>.tsp.json.v1.bak` back to `<name>.tsp.json`, and start the pad. A second
backup is never written over the first: later ones are `<name>.tsp.json.v1.1.bak`, and so on.

## Profile format v2

Hand-editing is fine: VS Code validates against
[`schema/tsp-v2.schema.json`](schema/tsp-v2.schema.json) via the file's
`$schema`. Example: [`profiles/example-v2.tsp.json`](profiles/example-v2.tsp.json).
Sizes are w×h = columns × rows.

| Field | Type | Bounds | Default |
|---|---|---|---|
| `schema` | number | 2 | required |
| `name` | text | 1-48 chars | required |
| `acceptTouch` | bool | | `true` |
| `targetApps` | list of process names | | any app |
| `pad.grid.columns` / `pad.grid.rows` | whole number | 1-12 / 1-60 | required |
| `pad.cellWidth` / `pad.cellHeight` | px at scale 1 | 8-200 / 40-200 | required |
| `pad.gap` | px | 0-24 | required |
| `pad.scale` | number | 0.5-2.0 (hundredths) | 1.0 |
| `pad.padOpacity` | number | 0.3-1.0 | 1.0 |
| `pad.backgroundOpacity` | number | 0-1 (below 0.05 = 0, floating) | 0.97 |
| `pad.dock` / `pad.theme` | `right`/`left`, `c-low-glare` | | `right`, `c-low-glare` |
| `buttons[]` | list | 0-60 | `[]` |
| `buttons[].id` | text | a-z 0-9 -, 1-32, unique | required |
| `buttons[].label` | text | 1-24 chars | required |
| `buttons[].x` / `.y` | cell | inside the grid | required |
| `buttons[].w` / `.h` | cells | inside the grid, no overlaps | 1 |
| `buttons[].group` | `num` `ent` `fine` `coarse` `hist` `custom` | | `num` |
| `buttons[].icon` | `up` `down` `left` `right` `enter` `tab` `undo` `redo` | | none |
| `buttons[].color` | `#RRGGBB` | | group colour |
| `buttons[].action` | `{ "type": "keys", "combo", "repeat" 1-50, "intervalMs" 10-1000 }` or `{ "type": "none" }` | | required |

Unknown fields are kept when the editor saves. Format v1 files still load.

## If keys don't land

1. Look at the header's second row: it names the code (`target-is-pad`,
   `target-elevated`, `aborted-focus-change`, ...).
2. `target-elevated`: R1 runs as administrator; start the pad as administrator.
3. Keys land in the pad or nothing happens: try `--focus-mode=2`, then `3`.
4. Open the log (path above) and check the `target` and `code` of each tap.
5. Floating pad (background 0%) but taps in the gaps don't reach R1: check the
   log for `"event":"region"` with `"mode":"fallback"`. The pad then shows its
   background at 30% (`region-unavailable`). `--window=opaque` turns floating off.
6. `profile-save-failed` while editing: your edits are kept in memory. Make
   the file writable and tap Retry, or Discard edits and close.

## Error codes

| Code | Meaning |
|---|---|
| <a id="profile-syntax"></a>`profile-syntax` | The file isn't valid JSON (line and column given). |
| <a id="profile-bound"></a>`profile-bound` | A value is missing or out of range (field and allowed range given). |
| <a id="profile-overlap"></a>`profile-overlap` | Two buttons cover the same cell. |
| <a id="profile-schema-too-new"></a>`profile-schema-too-new` | Made by a newer touch-shortcuts; update from Releases. |
| <a id="profile-too-large"></a>`profile-too-large` | Over 256 KB. |
| <a id="profile-not-utf8"></a>`profile-not-utf8` | Save the file as UTF-8. |
| <a id="profile-unknown-field"></a>`profile-unknown-field` | Warning: a field looks misspelt (or renamed: `opacity` is now `padOpacity`). |
| <a id="profile-unknown-group"></a>`profile-unknown-group` | Warning: unknown group, shown as `custom`. |
| <a id="profile-unknown-icon"></a>`profile-unknown-icon` | Warning: unknown icon, not shown. |
| <a id="profile-dock"></a>`profile-dock` / <a id="profile-theme"></a>`profile-theme` | Warning: unknown value, default used. |
| <a id="profile-save-failed"></a>`profile-save-failed` | Couldn't save; edits kept in memory. |
| <a id="profile-backup-failed"></a>`profile-backup-failed` | Couldn't write the `.v1.bak`; nothing changed. |
| <a id="profile-changed-on-disk"></a>`profile-changed-on-disk` | The file was edited elsewhere while editing: Keep my edits or Load file. |
| <a id="pad-create-failed"></a>`pad-create-failed` | New pad couldn't be written. |
| <a id="key-unknown"></a>`key-unknown` | A combo uses a key this version can't send. |
| <a id="action-unknown"></a>`action-unknown` | Action type from a newer version, or not set yet. |
| <a id="system-key-blocked"></a>`system-key-blocked` | Win/Alt+F4-style combos need `"allowSystemKeys": true` in settings.json. |
| <a id="unknown-button"></a>`unknown-button` | Internal: a tap for a button that no longer exists. |
| <a id="target-is-pad"></a>`target-is-pad` | The pad itself was in front; tap the R1 field first. |
| <a id="target-untitled"></a>`target-untitled` | No titled window in front. |
| <a id="target-not-allowed"></a>`target-not-allowed` | The app in front isn't in the profile's `targetApps`. |
| <a id="target-elevated"></a>`target-elevated` | R1 runs as administrator; run the pad as administrator. |
| <a id="target-elevation-unknown"></a>`target-elevation-unknown` | Sent, but couldn't confirm the target isn't elevated. |
| <a id="aborted-focus-change"></a>`aborted-focus-change` | Focus moved mid-repeat; n of N presses were sent. |
| <a id="send-incomplete"></a>`send-incomplete` | Windows accepted only part of a press; check the field. |
| <a id="dropped-queue-full"></a>`dropped-queue-full` | Too many taps queued; the extra tap was dropped. |
| <a id="sink-unavailable"></a>`sink-unavailable` | This build can't send keys on this OS. |
| <a id="sink-permission"></a>`sink-permission` | macOS: allow the app under Accessibility. |
| <a id="worker-stopped"></a>`worker-stopped` / <a id="internal-error"></a>`internal-error` | Quit and relaunch; see the crash file. |
| <a id="log-unwritable"></a>`log-unwritable` / <a id="settings-unreadable"></a>`settings-unreadable` | Check the folder; defaults used. |
| <a id="edit-active"></a>`edit-active` | Editing: tap Done to use the pad. |
| <a id="edit-stale"></a>`edit-stale` | The pad changed under that tap; try again. |
| <a id="edit-max-buttons"></a>`edit-max-buttons` | 60 buttons is the maximum. |
| <a id="layout-grid-full"></a>`layout-grid-full` | No room to shove a key; add a row, or shrink or delete one. |
| <a id="layout-out-of-grid"></a>`layout-out-of-grid` | That position is outside the grid. |
| <a id="layout-button-outside-grid"></a>`layout-button-outside-grid` | Shrinking the grid would strand a key; move it first. |
| <a id="restore-failed"></a>`restore-failed` | Couldn't give focus back to R1 after Done; tap it. |
| <a id="editor-open-failed"></a>`editor-open-failed` | The edit panel couldn't open; try ··· → Edit again. |
| <a id="region-unavailable"></a>`region-unavailable` | Floating tap-through unavailable; background shown at 30%. |

## macOS

Keys go to the app that was in front before you tapped the pad (posted to
that app with `CGEventPostToPid`; if tapping brought the pad forward, the app
is re-activated first). The same profiles work: `Win` means Command, numpad
keys map to the keypad.

- macOS asks for permission once. Allow it under System Settings → Privacy &
  Security → Accessibility, then quit and relaunch the pad. Without it every
  tap shows `sink-permission`. In `npm run tauri dev` the permission belongs
  to the terminal app that launched it.
- Mac apps often use Command where Windows uses Ctrl (Undo is `Win+Z` on a
  Mac profile, not `Ctrl+Z`).
- Diagnostic: `cargo run -p tsp-mac --example send -- "Num7" 3` types into
  whatever app is in front 2 s later.

## Develop

```
cargo test --workspace      # core rules: combos, profiles, layout, edits, saving, executor, queue, geometry
TSP_UPDATE_SCHEMA=1 cargo test -p tsp-core --test schema   # regenerate schema/tsp-v2.schema.json
npm run tauri dev           # Windows and macOS send keys; other platforms log taps (sink-unavailable)
cargo check -p tsp-win --target x86_64-pc-windows-msvc   # type-check the Win32 layer from a Mac
```

Design: [docs/designs/touch-shortcuts.md](docs/designs/touch-shortcuts.md) and
[docs/designs/in-app-editor.md](docs/designs/in-app-editor.md). Licence: MIT.
