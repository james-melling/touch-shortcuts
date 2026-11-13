// Edit panel (docs/designs/in-app-editor.md, "Edit panel window").
// Render-only: every change is an op sent to Rust (edit_apply), which
// validates, saves and broadcasts `editor-state` to the pad and this panel.
// The panel opens without focus; only Label, Name and Record activate it,
// and committing them hands focus straight back to the target app.

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type KeyView = {
  id: string; label: string; group: string; icon: string | null; color: string | null;
  repeat: number; disabled: string | null; combo: string | null; cell: [number, number, number, number];
  page: string | null; textSize: string;
};
type PadView = {
  name: string; keys: KeyView[]; grid: { columns: number; rows: number };
  scale: number; padOpacity: number; backgroundOpacity: number; opaque: boolean;
};
type Selection = { kind: "none" } | { kind: "pad" } | { kind: "cell"; x: number; y: number } | { kind: "button"; id: string };
type EditorView = {
  editing: boolean; rev: number; selection: Selection; saveState: "saved" | "pending" | "failed" | "conflict";
  saveMessage: string | null; unsavedOps: number; canUndo: boolean; canRedo: boolean; pad: PadView;
  sameCombo: [string, number][]; mirror: string | null; target: string; path: string; notice: string | null;
};
type EditFailure = { code: string; message: string; button: string | null; view: EditorView | null };

const NOT_SET = "Not set: ··· → Edit";
const MODS = ["Ctrl", "Shift", "Alt", "Win"] as const;
const PAGES: [string, string[]][] = [
  ["Nav", ["Left", "Up", "Down", "Right", "Home", "End", "PageUp", "PageDown"]],
  ["Edit", ["Enter", "Tab", "Esc", "Space", "Backspace", "Delete"]],
  ["Digits", ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", ".", "Comma"]],
  ["Numpad", ["Num7", "Num8", "Num9", "Num4", "Num5", "Num6", "Num1", "Num2", "Num3", "Num0", "NumDecimal", "Plus", "Minus"]],
  ["F-keys", ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"]],
  ["Letters", "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("")],
];
const SHOW: Record<string, string> = {
  Left: "←", Up: "↑", Down: "↓", Right: "→", PageUp: "PgUp", PageDown: "PgDn", Backspace: "⌫ Bksp",
  NumDecimal: "Num .", Plus: "+", Minus: "−", Comma: ",",
};
const GROUPS: [string, string][] = [["num", "Default"], ["ent", "Blue"], ["fine", "Teal"], ["coarse", "Amber"], ["hist", "Muted"]];
const ICONS: Record<string, string> = {
  up: '<path d="M12 19V5M5 12l7-7 7 7"/>',
  down: '<path d="M12 5v14M5 12l7 7 7-7"/>',
  left: '<path d="M19 12H5M12 5l-7 7 7 7"/>',
  right: '<path d="M5 12h14M12 5l7 7-7 7"/>',
  enter: '<path d="M20 5v8a3 3 0 01-3 3H5M9 12l-4 4 4 4"/>',
  tab: '<path d="M4 12h14M13 6l6 6-6 6M21 5v14"/>',
  undo: '<path d="M9 14L4 9l5-5"/><path d="M4 9h10a6 6 0 010 12h-3"/>',
  redo: '<path d="M15 14l5-5-5-5"/><path d="M20 9H10a6 6 0 000 12h3"/>',
};
const SIZES: [number, number][] = [[1, 1], [2, 1], [1, 2], [2, 2], [3, 1], [1, 3]];

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
let v: EditorView | null = null;
let tab: "button" | "pad" = "pad";
let lastKind = "none";
let lastButton: string | null = null; // for "Move <label> here"
const mods = new Set<string>();
let page = 0;
let customRepeat = 5;
let recording = 0; // timer id while Record is armed
let activated = false;
let actionMode: "keys" | "page" = "keys";
let lastButtonForMode: string | null = null;
type PadEntry = { index: number; name: string; file: string; current: boolean; error: string | null; sibling: boolean };
let padEntries: PadEntry[] = [];
let msgUntilOp = "";

// ---------- helpers ----------

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text) e.textContent = text; // textContent only: labels are untrusted
  return e;
}

function button(text: string, fn: () => void, cls = ""): HTMLButtonElement {
  const b = el("button", cls, text);
  b.type = "button";
  b.addEventListener("pointerdown", (e) => { e.preventDefault(); fn(); });
  return b;
}

function msg(text: string, tone: "" | "amber" | "red" = "") {
  msgUntilOp = text;
  $("msg").textContent = text;
  $("msg").className = `ed-msg ${tone}`;
}

function selected(): KeyView | null {
  const s = v?.selection;
  return s?.kind === "button" ? v!.pad.keys.find((k) => k.id === s.id) ?? null : null;
}

function readingOrder(): KeyView[] {
  return [...(v?.pad.keys ?? [])].sort((a, b) => a.cell[1] - b.cell[1] || a.cell[0] - b.cell[0]);
}

function parseCombo(text: string | null): { mods: string[]; key: string | null } {
  if (!text) return { mods: [], key: null };
  const parts = text.split("+");
  const key = parts.pop() ?? null;
  return { mods: parts, key };
}

// ---------- ops ----------

async function apply(op: object): Promise<boolean> {
  if (!v) return false;
  try {
    const next = await invoke<EditorView>("edit_apply", { op, baseRev: v.rev });
    msg(next.notice ?? "");
    const kind = (op as { kind: string }).kind;
    if (kind === "add" || kind === "duplicate") {
      page = 0; // "+ Add here" opens the Button tab on the Keys page
      mods.clear();
      tab = "button";
    }
    render(next);
    return true;
  } catch (err) {
    const f = err as EditFailure;
    if (f.view) render(f.view);
    msg(f.message ?? String(err), "red");
    return false;
  }
}

async function history(step: unknown) {
  if (!v) return;
  try {
    render(await invoke<EditorView>("edit_history", { step, baseRev: v.rev }));
    msg("");
  } catch (err) {
    msg((err as EditFailure).message ?? String(err), "red");
  }
}

async function select(selection: Selection) {
  render(await invoke<EditorView>("edit_select", { selection }));
}

// ---------- typing fields (Label, Name) ----------

async function activate(): Promise<boolean> {
  if (activated) return true;
  activated = await invoke<boolean>("edit_panel_activate");
  return activated;
}

async function rearm() {
  if (!activated) return;
  activated = false;
  const problem = await invoke<string | null>("edit_panel_rearm");
  if (problem) msg(problem, "amber");
}

/** A text field that activates the panel on tap and commits on Enter, blur or 600 ms idle. */
function textField(value: string, max: number, commit: (text: string) => Promise<boolean>): HTMLInputElement {
  const input = el("input");
  input.type = "text";
  input.value = value;
  input.maxLength = 48;
  input.autocomplete = "off";
  let timer = 0;
  let committed = value;
  const hint = (): boolean => {
    const t = input.value;
    const ok = t.length >= 1 && [...t].length <= max;
    input.classList.toggle("bad", !ok);
    if (!ok) msg(t.length === 0 ? "Type a label (1 character or more)" : `Labels are up to ${max} characters`, "amber");
    return ok;
  };
  const doCommit = async () => {
    clearTimeout(timer);
    if (input.value === committed || !hint()) return;
    committed = input.value;
    await commit(input.value);
  };
  input.addEventListener("pointerdown", (e) => {
    if (activated) return;
    e.preventDefault();
    void activate().then(() => input.focus());
  });
  input.addEventListener("input", () => {
    clearTimeout(timer);
    if (hint()) timer = window.setTimeout(() => void doCommit(), 600);
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void doCommit().then(() => { input.blur(); });
    }
  });
  input.addEventListener("blur", () => { void doCommit().then(rearm); });
  return input;
}

// ---------- Record ----------

function stopRecording(text = "") {
  clearTimeout(recording);
  recording = 0;
  window.removeEventListener("keydown", onRecordKey, true);
  if (text) msg(text, "amber");
  void rearm();
  renderButtonTab();
}

async function onRecordKey(e: KeyboardEvent) {
  e.preventDefault();
  e.stopPropagation();
  if (["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"].includes(e.code)) {
    if (e.code.startsWith("Meta")) {
      clearTimeout(recording);
      recording = window.setTimeout(() => stopRecording("Win combos: pick them with the pen"), 10000);
    }
    return;
  }
  try {
    const combo = await invoke<string>("edit_record", { code: e.code, ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, meta: e.metaKey });
    const k = selected();
    stopRecording();
    if (k) await apply({ kind: "setAction", buttonId: k.id, action: { type: "keys", combo, repeat: k.repeat } });
  } catch (err) {
    msg(String(err), "amber"); // unsupported key: stay armed
  }
}

async function startRecording() {
  if (!(await activate())) {
    msg("The panel could not take focus for recording.", "amber");
    return;
  }
  msg("Press the keys…");
  window.addEventListener("keydown", onRecordKey, true);
  recording = window.setTimeout(() => stopRecording("Record timed out: nothing changed"), 10000);
  renderButtonTab();
}

// ---------- Button tab ----------

async function refreshPads() {
  padEntries = await invoke<PadEntry[]>("pads_list");
  renderButtonTab();
}

function setKeys(key: string, repeat?: number) {
  const k = selected();
  if (!k) return;
  const combo = [...MODS.filter((m) => mods.has(m)), key].join("+");
  void apply({ kind: "setAction", buttonId: k.id, action: { type: "keys", combo, repeat: repeat ?? (k.combo ? k.repeat : 1) } });
}

function setRepeat(n: number) {
  const k = selected();
  if (!k?.combo) {
    msg("Pick a key first", "amber");
    return;
  }
  void apply({ kind: "setAction", buttonId: k.id, action: { type: "keys", combo: k.combo, repeat: n } });
}

/** A plain, always-open section (no collapsing when the selection changes). */
function section(title: string, ...children: HTMLElement[]): HTMLElement {
  const d = el("div", "ed-group");
  d.append(el("div", "ed-group-title", title), ...children);
  return d;
}

function renderButtonTab() {
  const root = $("button-tab");
  const k = selected();
  if (!k || !v) {
    root.replaceChildren(el("p", "ed-hint", "Tap a key on the pad to edit it."));
    return;
  }
  const disabled = k.disabled && k.disabled !== NOT_SET ? k.disabled : null;
  const action = el("div", "ed-action");
  const kindRow = el("div", "ed-mods");
  kindRow.append(
    button("Keys", () => { actionMode = "keys"; renderButtonTab(); }, `chip${actionMode === "keys" ? " on" : ""}`),
    button("Go to pad", () => { actionMode = "page"; void refreshPads(); }, `chip${actionMode === "page" ? " on" : ""}`),
  );
  action.append(kindRow);
  if (actionMode === "page") {
    action.append(el("div", "ed-caption", "Go to"));
    const list = el("div", "ed-padlist");
    const goTo = (target: string, name: string | null) => void apply({ kind: "setAction", buttonId: k.id, action: { type: "page", target, name } });
    list.append(button("← Back (previous pad)", () => goTo("back", null), `wide${k.page === "back" ? " on" : ""}`));
    const siblings = padEntries.filter((p) => p.sibling && !p.current);
    for (const p of siblings) list.append(button(`${p.name} · ${p.file}`, () => goTo(p.file, p.error ? null : p.name), `wide${k.page === p.file ? " on" : ""}${p.error ? " warn" : ""}`));
    if (!siblings.length) list.append(el("p", "ed-hint", "No other pads in this folder yet: make one with Pad → New pad."));
    action.append(list);
  }
  if (disabled) {
    const r = el("div", "ed-disabled");
    r.append(el("p", "", disabled), button("Replace", () => { /* the picker below writes a fresh keys action */ msg("Pick keys below to replace it."); }));
    action.append(r);
  }
  // Modifier row
  const modRow = el("div", "ed-mods");
  for (const m of MODS) {
    const b = button(m, () => { if (mods.has(m)) mods.delete(m); else mods.add(m); renderButtonTab(); }, `chip${mods.has(m) ? " on" : ""}`);
    modRow.append(b);
  }
  // Page tabs + Record
  const pages = el("div", "ed-pages");
  PAGES.forEach(([name], i) => pages.append(button(name, () => { page = i; renderButtonTab(); }, `pg${page === i ? " on" : ""}`)));
  pages.append(recording ? button("Cancel", () => stopRecording("Record cancelled"), "pg rec on") : button("Record", () => void startRecording(), "pg rec"));
  // Key grid
  const current = parseCombo(k.combo).key;
  const grid = el("div", "ed-keys");
  for (const key of PAGES[page][1]) grid.append(button(SHOW[key] ?? key, () => setKeys(key), `key${key === current ? " on" : ""}`));
  // Repeat
  const rep = el("div", "ed-repeat");
  for (const n of [1, 4, 10]) rep.append(button(`×${n}`, () => setRepeat(n), `chip${k.combo && k.repeat === n ? " on" : ""}`));
  const custom = el("div", "ed-stepper");
  custom.append(
    button("−", () => { customRepeat = Math.max(1, customRepeat - 1); renderButtonTab(); }),
    el("span", "", `×${customRepeat}`),
    button("+", () => { customRepeat = Math.min(50, customRepeat + 1); renderButtonTab(); }),
    button("Set", () => setRepeat(customRepeat), "chip"),
  );
  rep.append(custom);
  const clear = button("Clear keys", () => void apply({ kind: "setAction", buttonId: k.id, action: { type: "none" } }), "ed-link");
  if (actionMode === "keys") action.append(modRow, pages, grid, el("div", "ed-caption", "Repeat"), rep, clear);

  // Size & position
  const [cx, cy, cw, ch] = k.cell;
  const sizes = el("div", "ed-chips");
  for (const [w, h] of SIZES) sizes.append(button(`${w}×${h}`, () => void apply({ kind: "resize", buttonId: k.id, w, h }), `chip${cw === w && ch === h ? " on" : ""}`));
  const stepper = (label: string, value: number, set: (n: number) => void) => {
    const s = el("div", "ed-stepper");
    s.append(el("span", "ed-lab", label), button("−", () => set(value - 1)), el("span", "", String(value)), button("+", () => set(value + 1)));
    return s;
  };
  const wh = el("div", "ed-row");
  wh.append(
    stepper("W", cw, (w) => { if (w >= 1) void apply({ kind: "resize", buttonId: k.id, w, h: ch }); }),
    stepper("H", ch, (h) => { if (h >= 1) void apply({ kind: "resize", buttonId: k.id, w: cw, h }); }),
  );
  const dpad = el("div", "ed-dpad");
  const mv = (dx: number, dy: number) => () => {
    const x = cx + dx, y = cy + dy;
    if (x < 0 || y < 0) return msg("layout-out-of-grid: that position is outside the grid. Nothing moved.", "red");
    void apply({ kind: "move", buttonId: k.id, x, y });
  };
  dpad.append(el("span"), button("↑", mv(0, -1)), el("span"), button("←", mv(-1, 0)), el("span", "ed-pos", `${cx + 1},${cy + 1}`), button("→", mv(1, 0)), el("span"), button("↓", mv(0, 1)), el("span"));
  const sizePos = section("Size & position", sizes, wh, dpad);

  // Look
  const label = textField(k.label, 24, (t) => apply({ kind: "setLabel", buttonId: k.id, label: t }));
  const swatches = el("div", "ed-swatches");
  for (const [g, name] of GROUPS) {
    const sw = button(name, async () => {
      if (k.color) await apply({ kind: "setColor", buttonId: k.id, color: null });
      void apply({ kind: "setGroup", buttonId: k.id, group: g });
    }, `swatch ${g}${!k.color && k.group === g ? " on" : ""}`);
    swatches.append(sw);
  }
  if (k.color) swatches.append(el("span", "swatch on custom", "Custom"));
  const icons = el("div", "ed-icons");
  icons.append(button("None", () => void apply({ kind: "setIcon", buttonId: k.id, icon: null }), `ico${k.icon ? "" : " on"}`));
  for (const [name, path] of Object.entries(ICONS)) {
    const b = button("", () => void apply({ kind: "setIcon", buttonId: k.id, icon: name }), `ico${k.icon === name ? " on" : ""}`);
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.innerHTML = path; // fixed built-in set
    b.append(svg);
    b.setAttribute("aria-label", name);
    icons.append(b);
  }
  const textSizes = el("div", "ed-chips");
  for (const [name, label] of [["small", "Small"], ["medium", "Medium"], ["large", "Large"]] as const) {
    textSizes.append(button(label, () => void apply({ kind: "setTextSize", buttonId: k.id, size: name === "large" ? null : name }), `chip${k.textSize === name ? " on" : ""}`));
  }
  const look = section("Look", el("div", "ed-caption", "Label"), label, el("div", "ed-caption", "Text size"), textSizes, el("div", "ed-caption", "Colour"), swatches, el("div", "ed-caption", "Icon"), icons);

  root.replaceChildren(action, sizePos, look);
}

// ---------- Pad tab ----------

function renderPadTab() {
  const root = $("pad-tab");
  if (!v) return;
  const p = v.pad;
  const parts: HTMLElement[] = [];
  const s = v.selection;
  if (s.kind === "cell") {
    const cell = el("div", "ed-cell");
    cell.append(el("div", "ed-caption", `Cell (${s.x + 1}, ${s.y + 1})`));
    cell.append(button("+ Add here", () => void apply({ kind: "add", at: [s.x, s.y] }), "wide primary"));
    const prev = lastButton ? p.keys.find((k) => k.id === lastButton) : null;
    if (prev) cell.append(button(`Move ${prev.label} here`, () => void apply({ kind: "move", buttonId: prev.id, x: s.x, y: s.y }), "wide"));
    parts.push(cell);
  }
  const name = textField(p.name, 48, (t) => apply({ kind: "setPad", change: { name: t } }));
  const step = (label: string, value: string, minus: () => void, plus: () => void) => {
    const r = el("div", "ed-stepper");
    r.append(el("span", "ed-lab", label), button("−", minus), el("span", "", value), button("+", plus));
    return r;
  };
  const scale = step("Scale", p.scale.toFixed(1), () => void apply({ kind: "setPad", change: { scale: Math.max(0.5, p.scale - 0.1) } }), () => void apply({ kind: "setPad", change: { scale: Math.min(2, p.scale + 0.1) } }));
  const cols = step("Columns", String(p.grid.columns), () => void apply({ kind: "setPad", change: { columns: p.grid.columns - 1 } }), () => void apply({ kind: "setPad", change: { columns: p.grid.columns + 1 } }));
  const rows = step("Rows", String(p.grid.rows), () => void apply({ kind: "setPad", change: { rows: p.grid.rows - 1 } }), () => void apply({ kind: "setPad", change: { rows: p.grid.rows + 1 } }));
  const slider = (label: string, value: number, min: number, key: "padOpacity" | "backgroundOpacity") => {
    const wrap = el("label", "ed-slider");
    const out = el("span", "", `${Math.round(value * 100)}%`);
    const input = el("input");
    input.type = "range";
    input.min = String(min);
    input.max = "100";
    input.value = String(Math.round(value * 100));
    const valueOf = () => {
      const n = Number(input.value) / 100;
      return key === "backgroundOpacity" && n < 0.05 ? 0 : n;
    };
    input.addEventListener("input", () => {
      out.textContent = `${Math.round(valueOf() * 100)}%`;
      // Live preview on the pad while dragging (edit mode otherwise previews 100% / 60%).
      void emit("pad-preview", { padOpacity: key === "padOpacity" ? valueOf() : p.padOpacity, backgroundOpacity: key === "backgroundOpacity" ? valueOf() : p.backgroundOpacity });
    });
    input.addEventListener("change", async () => {
      const n = valueOf();
      void emit("pad-preview", null);
      if (await apply({ kind: "setPad", change: { [key]: n } })) render(await invoke<EditorView>("edit_flush"));
    });
    wrap.append(el("span", "ed-lab", label), input, out);
    return wrap;
  };
  const floating = p.backgroundOpacity === 0 && !p.opaque ? el("p", "ed-hint", "Floating: taps in gaps reach the app below. Taps pass through after Done.") : el("span");
  const padRows = el("div", "ed-padlist");
  const listPads = async () => {
    const list = await invoke<{ index: number; name: string; file: string; current: boolean; error: string | null }[]>("pads_list");
    padRows.replaceChildren(...list.map((e) => button(`${e.current ? "✓ " : ""}${e.name} · ${e.file}${e.error ? ` · Can't open: ${e.error}` : ""}`, async () => {
      if (e.current) return;
      try { render(await invoke<EditorView>("pads_switch", { index: e.index })); msg(`Switched to ${e.name}`); } catch (err) { msg(String(err), "red"); }
    }, `wide${e.error ? " warn" : ""}`)));
    padRows.scrollIntoView({ block: "nearest" });
  };
  const newPad = () => {
    const presets: [string, string][] = [["R1 numpad 4×8", "r1Numpad"], ["Blank 4×8", "blank4x8"], ["Blank 3×10", "blank3x10"]];
    padRows.replaceChildren(...presets.map(([label, preset]) => button(label, async () => {
      try { render(await invoke<EditorView>("pad_new", { preset })); msg("New pad created"); } catch (err) { msg(String(err), "red"); }
    }, "wide")));
    padRows.scrollIntoView({ block: "nearest" });
  };
  const padsRow = el("div", "ed-row");
  padsRow.append(button("Pads", () => void listPads()), button("New pad", newPad));
  const actions = el("div", "ed-row wrap");
  actions.append(
    button("+ Add button", () => void apply({ kind: "add", at: null })),
    button("Undo edit", () => void history("undo"), v.canUndo ? "" : "off"),
    button("Redo edit", () => void history("redo"), v.canRedo ? "" : "off"),
  );
  let revertArmed = false;
  const revert = button("Revert to when I tapped Edit", () => {
    if (!revertArmed) {
      revertArmed = true;
      revert.textContent = "Tap again to revert";
      revert.classList.add("danger");
      return;
    }
    void history("revert");
  }, "wide");
  parts.push(
    el("div", "ed-caption", "Name"), name,
    scale, cols, rows,
    slider("Pad opacity", p.padOpacity, 30, "padOpacity"),
    slider("Background", p.backgroundOpacity, 0, "backgroundOpacity"),
    floating, actions, revert, padsRow, padRows,
    el("p", "ed-path", v.path),
  );
  root.replaceChildren(...parts);
}

// ---------- shell ----------

function setTab(t: "button" | "pad") {
  tab = t;
  $("tab-button").classList.toggle("on", t === "button");
  $("tab-pad").classList.toggle("on", t === "pad");
  $("button-tab").hidden = t !== "button";
  $("pad-tab").hidden = t !== "pad";
  $("foot-button").hidden = t !== "button" || !selected();
}

function render(next: EditorView) {
  if (v && next.rev < v.rev && next.editing) return; // stale event
  const kindChanged = next.selection.kind !== lastKind;
  v = next;
  lastKind = next.selection.kind;
  if (kindChanged) setTab(next.selection.kind === "button" ? "button" : "pad");
  const k = selected();
  if (k) lastButton = k.id;
  if (k && k.id !== lastButtonForMode) {
    lastButtonForMode = k.id;
    actionMode = k.page ? "page" : "keys";
    if (actionMode === "page") void refreshPads();
  }
  if (k && kindChanged) {
    const c = parseCombo(k.combo);
    mods.clear();
    c.mods.forEach((m) => mods.add(m));
    const p = PAGES.findIndex(([, keys]) => c.key !== null && keys.includes(c.key));
    if (p >= 0) page = p;
  }
  const order = readingOrder();
  const idx = k ? order.findIndex((o) => o.id === k.id) : -1;
  $("sel").textContent = k ? `${k.label} · ${idx + 1} of ${order.length}` : next.selection.kind === "cell" ? "Empty cell" : `${order.length} keys`;
  $("combo").textContent = k
    ? k.page ? `Goes to ${k.page === "back" ? "the previous pad" : k.page}` : k.combo ? `${k.combo}${k.repeat > 1 ? ` ×${k.repeat}, every 25 ms` : ""}` : k.disabled ?? ""
    : "Tap a key to edit it, or an empty cell to add one";
  const same = next.sameCombo;
  $("same").hidden = same.length === 0;
  if (same.length) $("same").textContent = `Same keys as '${same[0][0]}' (row ${same[0][1]})${same.length > 1 ? ` (+${same.length - 1} more)` : ""}`;
  // Conflict and failed-save states
  const conflict = next.saveState === "conflict";
  $("conflict").hidden = !conflict;
  $("load").textContent = `Load file (discard ${next.unsavedOps} edits)`;
  $("title-text").textContent = conflict ? "EDITING · paused: file changed on disk" : `EDITING · ${next.pad.name}`;
  $("unsaved").hidden = next.saveState !== "failed";
  if (next.saveState === "failed" && next.saveMessage) msg(next.saveMessage, "red");
  $("saved").textContent = { saved: "Saved", pending: "Saving…", failed: "Not saved", conflict: "Paused" }[next.saveState];
  const mirror = $("mirror");
  mirror.hidden = !next.mirror;
  if (next.mirror) mirror.textContent = `Duplicate as ${SHOW[next.mirror] ?? next.mirror}`;
  if (!recording) renderButtonTab();
  renderPadTab();
  setTab(tab);
  if (!next.editing) void getCurrentWindow().close();
}

async function done(discard = false) {
  try {
    await invoke("edit_done", { discard });
  } catch (err) {
    const f = err as EditFailure;
    if (f.view) render(f.view);
    msg(f.code === "profile-changed-on-disk" ? "The pad file changed on disk: choose below before closing." : `Unsaved edits: ${f.message}`, "red");
  }
}

document.addEventListener("contextmenu", (e) => e.preventDefault());
$("title").addEventListener("pointerdown", (e) => {
  if ((e.target as HTMLElement).id === "done") return;
  e.preventDefault();
  void getCurrentWindow().startDragging();
});
$("done").addEventListener("pointerdown", (e) => { e.preventDefault(); void done(); });
$("tab-button").addEventListener("pointerdown", (e) => { e.preventDefault(); setTab("button"); });
$("tab-pad").addEventListener("pointerdown", (e) => { e.preventDefault(); setTab("pad"); });
const step = (d: number) => () => {
  const order = readingOrder();
  if (!order.length) return;
  const k = selected();
  const i = k ? order.findIndex((o) => o.id === k.id) : -1;
  void select({ kind: "button", id: order[(i + d + order.length) % order.length].id });
};
$("prev").addEventListener("pointerdown", (e) => { e.preventDefault(); step(-1)(); });
$("next").addEventListener("pointerdown", (e) => { e.preventDefault(); step(1)(); });
$("dup").addEventListener("pointerdown", (e) => { e.preventDefault(); const k = selected(); if (k) void apply({ kind: "duplicate", buttonId: k.id, mirror: false }); });
$("mirror").addEventListener("pointerdown", (e) => { e.preventDefault(); const k = selected(); if (k) void apply({ kind: "duplicate", buttonId: k.id, mirror: true }); });
$("del").addEventListener("pointerdown", async (e) => {
  e.preventDefault();
  const k = selected();
  if (!k || !(await apply({ kind: "delete", buttonId: k.id }))) return;
  const id = k.id;
  const undo = button(`Deleted '${k.label}' · Undo edit`, () => void history({ undoDelete: id }), "ed-link");
  $("msg").replaceChildren(undo);
  setTimeout(() => { if ($("msg").contains(undo)) msg(""); }, 5000);
});
$("keep").addEventListener("pointerdown", async (e) => {
  e.preventDefault();
  try { render(await invoke<EditorView>("edit_conflict", { choice: "keepMine" })); msg("Your edits were written to the file."); } catch (err) { msg((err as EditFailure).message, "red"); }
});
$("load").addEventListener("pointerdown", async (e) => {
  e.preventDefault();
  try { render(await invoke<EditorView>("edit_conflict", { choice: "loadFile" })); msg("Loaded the file from disk."); } catch (err) { const f = err as EditFailure; if (f.view) render(f.view); msg(f.message, "red"); }
});
$("retry").addEventListener("pointerdown", async (e) => { e.preventDefault(); render(await invoke<EditorView>("edit_flush")); });
$("discard").addEventListener("pointerdown", (e) => { e.preventDefault(); void done(true); });
window.addEventListener("keydown", (e) => {
  if (recording || !(e.ctrlKey && !e.shiftKey && !e.altKey)) return;
  if (e.key === "z") { e.preventDefault(); void history("undo"); }
  if (e.key === "y") { e.preventDefault(); void history("redo"); }
});

void listen<EditorView>("editor-state", (e) => render(e.payload));
invoke<EditorView>("get_editor_state").then((s) => { lastKind = ""; render(s); });
void msgUntilOp;
