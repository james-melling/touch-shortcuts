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
{ "focusMode": 1, "allowSystemKeys": false, "lastProfile": "C:\\path\\to\\my.tsp.json" }
```

## If keys don't land

1. Look at the header's second row: it names the code (`target-is-pad`,
   `target-elevated`, `aborted-focus-change`, ...).
2. `target-elevated`: R1 runs as administrator; start the pad as administrator.
3. Keys land in the pad or nothing happens: try `--focus-mode=2`, then `3`.
4. Open the log (path above) and check the `target` and `code` of each tap.

## Develop

```
cargo test --workspace      # core rules: combos, profiles, executor, queue, geometry
npm run tauri dev           # on macOS keys are logged, not sent (sink-unavailable)
cargo check -p tsp-win --target x86_64-pc-windows-msvc   # type-check the Win32 layer from a Mac
```

Design: [docs/designs/touch-shortcuts.md](docs/designs/touch-shortcuts.md). Licence: MIT.
