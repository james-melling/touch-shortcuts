# TODOS

## Pad

### Hold-to-repeat with acceleration
**What:** Holding a nudge key repeats it, speeding up the longer it is held.
**Why:** Faster coarse moves than fixed x10 keys; idea from office hours (E7).
**Pros:** One key replaces Up/Up x10 pairs. **Cons:** Harder to get exact counts; conflicts with tap-again cancel.
**Context:** Deferred by /autoplan CEO review 2026-11-08 as outside the 18-button Tuesday spec. Needs executor support for an open-ended repeat stopped by pointerup.
**Effort:** human M / CC S. **Priority:** P3. **Depends on:** serial executor (T3, T8).

### Remember pad position and size per profile
**What:** Save the pad's last position/size in the profile or settings and restore it on launch.
**Why:** Today the pad always docks right (E9).
**Pros:** Fits multi-monitor and left-handed setups. **Cons:** Off-screen restore edge cases.
**Context:** Deferred 2026-11-08. Validate against the current monitor layout before restoring.
**Effort:** human S / CC S. **Priority:** P3.

### Confirm or long-press-to-arm for coarse keys
**What:** Optional per-button `confirm` flag (or long-press to arm) for x10 keys.
**Why:** Ten presses into the wrong R1 field during a show is costly (CEO voice F11).
**Pros:** Safety on live systems. **Cons:** Slower workflow.
**Context:** Phase 2, after field feedback.
**Effort:** human S / CC S. **Priority:** P2.

## Profiles

### Per-app profile auto-switch
**What:** Switch profiles automatically when the foreground app changes (R1 → Compass).
**Why:** No menu digging mid-tune (E8).
**Pros:** Seamless multi-app tuning. **Cons:** Needs reliable app identification; surprises if wrong.
**Context:** Deferred 2026-11-08. Reuse the foreground hook from `focus_win.rs`.
**Effort:** human M / CC S. **Priority:** P2. **Depends on:** target indicator (T9).

### Compass starter profile
**What:** Ship `profiles/compass-delay-gain.tsp.json`.
**Why:** Colleagues using Meyer Compass get a working pad on first launch (E10).
**Context:** Phase 2; needs Compass's actual shortcuts confirmed on a real system.
**Effort:** human S / CC S. **Priority:** P2.

### Keyboard-layout independence for digits and "."
**What:** Make `.` and digit keys land correctly on non-US keyboard layouts.
**Why:** VK codes for `.`/digits depend on the active layout; colleagues abroad may get wrong characters.
**Context:** Found by the CEO spec review (round 3), 2026-11-08. Consider scan codes or `VkKeyScanEx` per layout.
**Effort:** human S / CC S. **Priority:** P2.

### Show actions before importing a shared profile
**What:** When opening a profile from someone else, list its combos (and any system keys) before loading.
**Why:** Shared profiles are untrusted input (CEO Section 3).
**Context:** Phase 2 editor work.
**Effort:** human S / CC S. **Priority:** P2.

## Design

### Reserve screen space (Windows AppBar mode)
**What:** Optional AppBar registration (`SHAppBarMessage` ABM_NEW/ABM_SETPOS) so a maximised R1 resizes to the left of the pad.
**Why:** The overlay covers R1's right ~318 px, including its title-bar buttons when maximised (design review J1).
**Pros:** Nothing hidden behind the pad. **Cons:** Resizes other apps; more Win32 surface.
**Context:** Deferred 2026-11-08; Tuesday uses overlay. Add as a profile/setting option.
**Effort:** human M / CC S. **Priority:** P2.

### Native acrylic background
**What:** Use `window-vibrancy` acrylic/Mica behind theme C instead of an opaque background.
**Why:** The approved mockup's blur cannot be done in CSS for a transparent window (design review J3).
**Context:** Check legibility on the Surface over R1 first. **Effort:** human S / CC S. **Priority:** P3.

### Write DESIGN.md
**What:** Document tokens, group enum, key states and header states as the design system.
**Why:** Needed before theme B, light mode and custom themes (design review Pass 5).
**Effort:** human S / CC S. **Priority:** P2.

## Developer experience

### Double-click to open a profile (file association)
**What:** Register `.tsp.json` in the NSIS installer and forward argv from a second instance to the running pad's `load_profile`.
**Why:** "Opens a profile file sent by message" (Phase 2 success criterion) has no mechanism; single-instance currently drops the file (DX review G5).
**Effort:** human S / CC S. **Priority:** P2.

### tsp-validate CLI for profile pull requests
**What:** A console bin that validates profiles, prints errors in the contract format to stderr and exits non-zero; run in CI on every profile PR.
**Why:** The community profile library needs automated checks (DX review D5).
**Effort:** human S / CC S. **Priority:** P2.

### Choose the monitor
**What:** `pad.monitor` ("primary" or index) or dock to the monitor under R1's window.
**Why:** FOH Surfaces often drive an external display (DX review E2).
**Effort:** human S / CC S. **Priority:** P3.

### Portable mode
**What:** A `portable` marker next to the exe keeps config and logs beside it.
**Why:** USB-stick crews; answers RadialMenu's "portable config unreliable" complaint (DX review E5).
**Effort:** human S / CC S. **Priority:** P3.

### "keys" shorthand in profiles
**What:** Accept `"keys": "Ctrl+Z"` as sugar for `{type:"keys", combo:…}`.
**Why:** Less typing for hand authors (DX review A7).
**Effort:** human S / CC S. **Priority:** P3.

## Focus

### Tier 2b input overlay window
**What:** A native input-only `WS_EX_NOACTIVATE` layered window over the keys, hit-tested in Rust from the profile geometry, sending visual state to the web layer.
**Why:** WebView2's input window is cross-process; if tier 2 fails, this avoids tier 3's foreground restore (flash, caret loss, latency) (Eng finding 4).
**Context:** Only if the spike shows tier 2 failing. Compare against the native-shell option from the spike decision rule.
**Effort:** human M / CC S. **Priority:** P2. **Depends on:** Rust-computed geometry.

## Completed

## Editor

### "Swap only" moves (no shove)
**What:** A Pad-tab toggle so a move or resize only swaps with an equal-size button and never shoves others.
**Why:** Every move currently runs the shove rule; the only escape from an unwanted shove is Undo edit (DX review X-3, 2026-11-11).
**Pros:** Predictable layouts for users who dislike reflow. **Cons:** A second movement rule to test and explain.
**Context:** Deferred by /autoplan DX phase 2026-11-11 as a taste decision. Lives in `tsp-core::layout` as a flag on `place`.
**Effort:** human S / CC S. **Priority:** P3. **Depends on:** the in-app editor (v0.2).

### Pre-release Surface check with R1 (blocks v0.2.0-pre)
**What:** On a pen Surface with R1: panel pen controls never take focus; drag hide/show keeps R1 in front; a label commit returns focus to R1; the gate (Edit → edits → Label → Done: R1 in front, caret in the same field); floating mode taps in gaps reach R1.
**Why:** The 2026-11-12 spike passed for the window but never exercised these (no pen Surface since). Design doc Build order step 6.
**Effort:** 20 min on site. **Priority:** P1. **Depends on:** a Windows build of the editor.

### macOS floating tap-through
**What:** At background 0% on macOS, taps in the gaps should reach the app below (transparent-pixel pass-through, or toggling `ignoresMouseEvents` from the cursor position).
**Why:** The Windows region is in; on macOS the floating pad still catches taps in its gaps.
**Effort:** human M / CC S. **Priority:** P3 (Windows first).

### Editor polish left from the design
**What:** "Show file" (opener reveal) and "Save as new pad" on save failure; panel moves up while the Windows touch keyboard (InputPane) is docked; a Blank preset highlights its first empty cell; Windows CI smoke test that opens and closes the panel.
**Why:** In the design doc but not needed for the core loop; each is small.
**Effort:** human M / CC S each. **Priority:** P2.
