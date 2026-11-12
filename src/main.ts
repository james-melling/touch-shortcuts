// Render-only web layer: Rust validates the profile, computes geometry,
// runs actions and decides outcomes. This file draws keys and forwards taps.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type KeyView = {
  id: string; label: string; group: string; icon: string | null; color: string | null;
  repeat: number; disabled: string | null; combo: string | null;
  x: number; y: number; w: number; h: number; cell: [number, number, number, number];
};
type GridView = { columns: number; rows: number; left: number; top: number; cellWidth: number; rowHeight: number; gap: number };
type PadView = {
  name: string; fallback: boolean; acceptTouch: boolean; width: number; height: number;
  theme: string; warnings: string[]; keys: KeyView[];
  editing: boolean; grid: GridView; header: number; scale: number; padOpacity: number; backgroundOpacity: number;
  opaque: boolean;
};
type Selection = { kind: "none" } | { kind: "pad" } | { kind: "cell"; x: number; y: number } | { kind: "button"; id: string };
type EditorView = { editing: boolean; rev: number; selection: Selection; pad: PadView; notice: string | null };
type EditFailure = { code: string; message: string; view: EditorView | null };
type ActionResult = {
  buttonId: string; ok: boolean; code: string; message: string; sideEffects: string; sent: number; of: number;
};

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

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const keysEl = $("keys");
const statusEl = $("status");
const menuEl = $("menu");
let pad: PadView | null = null;
let editor: EditorView | null = null;
let regionFailed = false;
let lastRegion = "";
let livePreview: { padOpacity: number; backgroundOpacity: number } | null = null;
const NOT_SET = "Not set: ··· → Edit";
let lastError = "";
let penInRange = false;
let redUntilSuccess = false;

function setStatus(text: string, tone: "" | "amber" | "red") {
  statusEl.textContent = text;
  statusEl.className = `row2 ${tone}`;
}

function keyEl(k: KeyView): HTMLElement {
  const el = document.createElement("div");
  el.className = `k ${k.group}${k.disabled ? " disabled" : ""}`;
  el.dataset.id = k.id;
  Object.assign(el.style, { left: `${k.x}px`, top: `${k.y}px`, width: `${k.w}px`, height: `${k.h}px` });
  if (k.color) {
    el.style.setProperty("--fg", k.color);
    el.style.setProperty("--bd", k.color);
    el.style.setProperty("--bg", `${k.color}0f`);
  }
  if (k.icon && ICONS[k.icon]) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.innerHTML = ICONS[k.icon]; // fixed built-in set, never profile text
    el.appendChild(svg);
  }
  const mod = /^(Ctrl|Shift|Alt)\+(.+)$/.exec(k.label);
  if (mod) {
    const small = document.createElement("small");
    small.textContent = `${mod[1]}+`;
    el.appendChild(small);
  }
  const label = document.createElement("span");
  label.className = "label";
  label.textContent = mod ? mod[2] : k.label; // textContent only: labels are untrusted
  el.appendChild(label);
  if (k.repeat > 1) {
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = `×${k.repeat}`;
    el.appendChild(badge);
  }
  el.setAttribute("aria-label", k.disabled ? `${k.label} (disabled)` : k.label);
  if (k.disabled === NOT_SET) el.classList.add("none");
  const sel = editor?.selection;
  if (pad?.editing && sel?.kind === "button" && sel.id === k.id) el.classList.add("sel");
  el.addEventListener("pointerdown", (e) => {
    if (pad?.editing) {
      e.preventDefault();
      if (e.button !== 0 || !e.isPrimary || drag) return;
      const s = editor?.selection;
      if (s?.kind === "button" && s.id === k.id) startDrag(e, k, "move", el);
      else void select({ kind: "button", id: k.id });
      return;
    }
    void onKey(e, k, el);
  });
  return el;
}

/** Dashed empty cells for edit mode; tapping one selects it. */
function emptyCells(view: PadView): HTMLElement[] {
  const g = view.grid;
  const taken = new Set<string>();
  for (const k of view.keys) {
    const [x, y, w, h] = k.cell;
    for (let r = y; r < y + h; r++) for (let c = x; c < x + w; c++) taken.add(`${c},${r}`);
  }
  const sel = editor?.selection;
  const out: HTMLElement[] = [];
  for (let r = 0; r < g.rows; r++) {
    for (let c = 0; c < g.columns; c++) {
      if (taken.has(`${c},${r}`)) continue;
      const el = document.createElement("div");
      el.className = "cell";
      if (sel?.kind === "cell" && sel.x === c && sel.y === r) el.classList.add("sel");
      Object.assign(el.style, {
        left: `${g.left + c * (g.cellWidth + g.gap)}px`, top: `${g.top + r * (g.rowHeight + g.gap)}px`,
        width: `${g.cellWidth}px`, height: `${g.rowHeight}px`,
      });
      el.addEventListener("pointerdown", (e) => { e.preventDefault(); void select({ kind: "cell", x: c, y: r }); });
      out.push(el);
    }
  }
  return out;
}

// ---------- drag to move / resize (edit mode) ----------

type Drag = {
  id: string; mode: "move" | "resize"; pointerId: number; startX: number; startY: number;
  origin: [number, number, number, number]; grab: [number, number]; target: [number, number, number, number] | null;
  active: boolean; ghost: HTMLElement; timer: number; refused: boolean;
};
let drag: Drag | null = null;

function cellRect(x: number, y: number, w: number, h: number) {
  const g = pad!.grid;
  return {
    left: g.left + x * (g.cellWidth + g.gap), top: g.top + y * (g.rowHeight + g.gap),
    width: w * g.cellWidth + (w - 1) * g.gap, height: h * g.rowHeight + (h - 1) * g.gap,
  };
}

function cellAt(px: number, py: number): [number, number] {
  const g = pad!.grid;
  return [Math.floor((px - g.left + g.gap / 2) / (g.cellWidth + g.gap)), Math.floor((py - g.top + g.gap / 2) / (g.rowHeight + g.gap))];
}

function place(el: HTMLElement, c: [number, number, number, number]) {
  const r = cellRect(...c);
  Object.assign(el.style, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px` });
}

/** The selected key's resize handle: a 44×44 hit square outside its bottom-right corner. */
function handle(view: PadView): HTMLElement[] {
  const sel = editor?.selection;
  const k = sel?.kind === "button" ? view.keys.find((x) => x.id === sel.id) : null;
  if (!k) return [];
  const h = document.createElement("div");
  h.className = "handle";
  Object.assign(h.style, { left: `${k.x + k.w}px`, top: `${k.y + k.h}px` });
  h.setAttribute("aria-label", "Resize");
  h.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.button === 0 && e.isPrimary && !drag) startDrag(e, k, "resize", h);
  });
  return [h];
}

function startDrag(e: PointerEvent, k: KeyView, mode: "move" | "resize", el: HTMLElement) {
  el.setPointerCapture(e.pointerId);
  const [cx, cy] = cellAt(e.clientX, e.clientY);
  const ghost = document.createElement("div");
  ghost.className = "ghost";
  ghost.hidden = true;
  keysEl.appendChild(ghost);
  drag = {
    id: k.id, mode, pointerId: e.pointerId, startX: e.clientX, startY: e.clientY,
    origin: k.cell, grab: [cx - k.cell[0], cy - k.cell[1]], target: k.cell, active: false, ghost, timer: 0, refused: false,
  };
}

function clearPreview() {
  keysEl.querySelectorAll(".victim").forEach((v) => v.remove());
}

function endDrag() {
  if (!drag) return;
  clearTimeout(drag.timer);
  drag.ghost.remove();
  clearPreview();
  if (drag.active) void invoke("edit_panel_visible", { visible: true });
  drag = null;
}

async function preview(d: Drag) {
  if (!d.target || drag !== d) return;
  const [x, y, w, h] = d.target;
  const op = d.mode === "move" ? { kind: "move", buttonId: d.id, x, y } : { kind: "resize", buttonId: d.id, w, h };
  try {
    const p = await invoke<{ cells: [string, [number, number, number, number]][] }>("edit_preview", { op });
    if (drag !== d) return;
    clearPreview();
    d.refused = false;
    d.ghost.classList.remove("refused");
    d.ghost.textContent = "";
    for (const [id, c] of p.cells) {
      const before = pad!.keys.find((k) => k.id === id)?.cell;
      if (id === d.id || !before || before.join() === c.join()) continue;
      const v = document.createElement("div");
      v.className = "victim";
      place(v, c);
      keysEl.appendChild(v);
    }
  } catch {
    if (drag !== d) return;
    clearPreview();
    d.refused = true;
    d.ghost.classList.add("refused");
    d.ghost.textContent = "✕ No room";
  }
}

document.addEventListener("pointermove", (e) => {
  const d = drag;
  if (!d || e.pointerId !== d.pointerId) return;
  if (!d.active) {
    if (Math.hypot(e.clientX - d.startX, e.clientY - d.startY) < 8) return;
    d.active = true;
    void invoke("edit_panel_visible", { visible: false });
  }
  const g = pad!.grid;
  const [cx, cy] = cellAt(e.clientX, e.clientY);
  const [ox, oy, ow, oh] = d.origin;
  let t: [number, number, number, number];
  if (d.mode === "move") {
    t = [cx - d.grab[0], cy - d.grab[1], ow, oh];
  } else {
    // Relative: the handle starts outside the corner, so size changes by the cells moved.
    const [sx, sy] = [ox + d.grab[0], oy + d.grab[1]];
    t = [ox, oy, Math.min(Math.max(1, ow + cx - sx), g.columns - ox), Math.min(Math.max(1, oh + cy - sy), g.rows - oy)];
  }
  const inside = t[0] >= 0 && t[1] >= 0 && t[0] + t[2] <= g.columns && t[1] + t[3] <= g.rows;
  if (!inside) {
    d.target = null;
    d.ghost.hidden = true;
    clearPreview();
    setStatus("Release to cancel", "amber");
    return;
  }
  if (d.target && d.target.join() === t.join() && !d.ghost.hidden) return;
  d.target = t;
  d.ghost.hidden = false;
  d.ghost.classList.remove("refused");
  d.ghost.textContent = "";
  place(d.ghost, t);
  clearPreview();
  setStatus(d.mode === "move" ? "Release to move" : `Release to resize to ${t[2]}×${t[3]}`, "");
  clearTimeout(d.timer);
  d.timer = window.setTimeout(() => void preview(d), 120);
});

document.addEventListener("pointerup", async (e) => {
  const d = drag;
  if (!d || e.pointerId !== d.pointerId) return;
  const t = d.target;
  const moved = d.active && t && t.join() !== d.origin.join();
  endDrag();
  if (!moved || !editor) {
    if (pad) render(pad);
    return;
  }
  const [x, y, w, h] = t!;
  const op = d.mode === "move" ? { kind: "move", buttonId: d.id, x, y } : { kind: "resize", buttonId: d.id, w, h };
  try {
    applyEditor(await invoke<EditorView>("edit_apply", { op, baseRev: editor.rev }));
  } catch (err) {
    const f = err as EditFailure;
    if (f.view) applyEditor(f.view);
    setStatus(f.message ?? String(err), "red");
  }
});

document.addEventListener("pointercancel", (e) => {
  if (drag && e.pointerId === drag.pointerId) {
    endDrag();
    if (pad) render(pad);
  }
});

async function select(selection: Selection) {
  try {
    applyEditor(await invoke<EditorView>("edit_select", { selection }));
  } catch (err) {
    setStatus(String(err), "red");
  }
}

function applyEditor(v: EditorView) {
  if (editor && v.editing && editor.editing && v.rev < editor.rev) return; // stale event
  if (drag) endDrag();
  const leaving = editor?.editing && !v.editing;
  editor = v;
  render(v.pad);
  if (leaving && !v.notice) setStatus("", "");
  if (v.notice) setStatus(v.notice, v.notice.startsWith("restore-failed") ? "amber" : "");
}

/** Effective opacities: opaque escape hatch, edit-mode preview, region fallback. */
function applyOpacity(view: PadView) {
  let pado = view.padOpacity;
  let bgo = view.backgroundOpacity;
  if (view.opaque) {
    pado = 1;
    bgo = 1;
  } else if (view.editing) {
    pado = livePreview?.padOpacity ?? 1;
    bgo = livePreview?.backgroundOpacity ?? Math.max(bgo, 0.6);
  } else if (regionFailed && bgo < 0.3) {
    bgo = 0.3;
  }
  document.documentElement.style.setProperty("--pado", String(pado));
  document.documentElement.style.setProperty("--bgo", String(bgo));
}

/** Floating mode (background 0, not editing): only keys, header and open menu take taps. */
async function updateRegion() {
  const view = pad;
  if (!view) return;
  const on = !view.opaque && !view.editing && view.backgroundOpacity === 0 && !regionFailed;
  const parts: Element[] = on ? [document.querySelector(".head")!, ...keysEl.querySelectorAll(".k")] : [];
  if (on && !menuEl.hidden) parts.push(menuEl);
  const rects = parts.map((el) => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; });
  const key = JSON.stringify([on, rects]);
  if (key === lastRegion) return;
  lastRegion = key;
  try {
    await invoke("pad_region", { rects, on });
  } catch (err) {
    regionFailed = true;
    lastRegion = "";
    applyOpacity(view);
    setStatus(`region-unavailable: ${String(err)}. Background shown at 30% until restart.`, "amber");
  }
}

function render(view: PadView) {
  pad = view;
  applyOpacity(view);
  requestAnimationFrame(() => void updateRegion());
  const head = document.querySelector(".head") as HTMLElement;
  head.style.height = `${view.header}px`;
  document.body.classList.toggle("editing", view.editing);
  $("prof").textContent = view.editing ? `EDITING · ${view.name}` : view.name + (view.fallback ? " (fallback)" : "");
  $("done").hidden = !view.editing;
  $("dot").hidden = view.warnings.length === 0;
  const keys = view.keys.map(keyEl);
  keysEl.replaceChildren(...(view.editing ? emptyCells(view) : []), ...keys, ...(view.editing ? handle(view) : []));
  if (view.editing) {
    const sel = editor?.selection;
    const s = sel?.kind === "button" ? view.keys.find((k) => k.id === sel.id) : null;
    setStatus(s ? `${s.label}${s.combo ? ` · ${s.combo}` : ""}` : sel?.kind === "cell" ? `Cell (${sel.x + 1}, ${sel.y + 1})` : "Tap a key to edit it, or an empty cell to add one", "");
  }
  if (view.fallback) setStatus("profile failed to load; using the built-in R1 pad (··· for details)", "amber");
}

function shouldFire(e: PointerEvent): boolean {
  if (e.button !== 0 || !e.isPrimary) return false; // pen barrel button, eraser, secondary contacts
  if (e.pointerType === "touch") {
    if (!pad?.acceptTouch) {
      setStatus("Pen only: enable touch in the profile", "amber");
      return false;
    }
    if (penInRange) return false; // palm resting while writing
    if (e.width > 75) return false; // ~20 mm contact: a palm, not a fingertip
  }
  return true;
}

async function onKey(e: PointerEvent, k: KeyView, el: HTMLElement) {
  e.preventDefault();
  if (!shouldFire(e)) return;
  if (k.disabled) {
    setStatus(k.disabled, "amber");
    return;
  }
  el.classList.add("pressed");
  setTimeout(() => el.classList.remove("pressed"), 120);
  if (k.repeat > 1) el.classList.add("running");
  try {
    const r = await invoke<string>("run_action", { buttonId: k.id });
    if (r === "dropped-queue-full") flash(el, "failed");
  } catch (err) {
    const m = (err as { message?: string }).message ?? String(err);
    flash(el, "failed");
    showError(m);
  }
}

function flash(el: Element, cls: "failed" | "unverified") {
  el.classList.remove(cls);
  void (el as HTMLElement).offsetWidth;
  el.classList.add(cls);
}

function showError(message: string) {
  lastError = message;
  redUntilSuccess = true;
  setStatus(message, "red");
}

function onResult(r: ActionResult) {
  const el = keysEl.querySelector(`[data-id="${CSS.escape(r.buttonId)}"]`);
  el?.classList.remove("running");
  if (r.ok && r.code === "ok") {
    if (redUntilSuccess) {
      redUntilSuccess = false;
      setStatus("", "");
    }
    return;
  }
  if (r.code === "cancelled") {
    setStatus(`cancelled ${r.sent}/${r.of}`, "");
    return;
  }
  if (r.code === "sink-unavailable") {
    if (el) flash(el, "unverified");
    setStatus("this build logs taps but cannot send keys", "amber");
    return;
  }
  if (r.code === "target-elevation-unknown") {
    if (el) flash(el, "unverified");
    setStatus(r.message, "amber");
    return;
  }
  if (el) flash(el, "failed");
  const amber = ["target-is-pad", "target-untitled", "target-not-allowed"].includes(r.code);
  if (amber) {
    lastError = r.message;
    setStatus(r.message, "amber");
  } else {
    showError(r.message);
  }
}

// ---------- menu ----------

let quitArmed = 0;
let menuTimer = 0;

function closeMenu() {
  menuEl.hidden = true;
  clearTimeout(menuTimer);
  void updateRegion();
}

/** Shows the menu only after the region includes it, so a tap on it never falls through. */
async function showMenu() {
  menuEl.style.visibility = "hidden";
  menuEl.hidden = false;
  await updateRegion();
  menuEl.style.visibility = "";
}

type PadEntry = { index: number; name: string; file: string; current: boolean; error: string | null };

/** Pads list or New pad presets, drawn inside the in-pad menu (no native popup). */
async function openSubmenu(which: "pads" | "new") {
  const rows: HTMLElement[] = [];
  const row = (label: string, fn: () => void, cls = "") => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label; // pad names are untrusted: textContent only
    if (cls) b.className = cls;
    b.addEventListener("pointerdown", (e) => { e.preventDefault(); fn(); });
    rows.push(b);
  };
  const run = async (cmd: string, args: Record<string, unknown>) => {
    closeMenu();
    try {
      const v = await invoke<EditorView>(cmd, args);
      applyEditor(v);
      if (!v.editing) render(await invoke<PadView>("get_pad"));
    } catch (err) {
      showError(String(err));
    }
  };
  if (which === "pads") {
    for (const p of await invoke<PadEntry[]>("pads_list")) {
      const label = `${p.current ? "✓ " : ""}${p.name} · ${p.file}${p.error ? ` · Can't open: ${p.error}` : ""}`;
      row(label, () => { if (!p.current) void run("pads_switch", { index: p.index }); else closeMenu(); }, p.error ? "warn" : "");
    }
  } else {
    row("R1 numpad 4×8", () => void run("pad_new", { preset: "r1Numpad" }));
    row("Blank 4×8", () => void run("pad_new", { preset: "blank4x8" }));
    row("Blank 3×10", () => void run("pad_new", { preset: "blank3x10" }));
  }
  row("‹ Back", () => openMenu());
  menuEl.replaceChildren(...rows);
  void showMenu();
  clearTimeout(menuTimer);
  menuTimer = window.setTimeout(closeMenu, 8000);
}

function openMenu() {
  const items: HTMLElement[] = [];
  for (const w of pad?.warnings ?? []) {
    const d = document.createElement("div");
    d.className = "info";
    d.textContent = w;
    items.push(d);
  }
  if (lastError) {
    const d = document.createElement("div");
    d.className = "info";
    d.textContent = `Last error: ${lastError}`;
    items.push(d);
  }
  const btn = (label: string, fn: () => void, cls = "") => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    if (cls) b.className = cls;
    b.addEventListener("pointerdown", (e) => { e.preventDefault(); fn(); });
    items.push(b);
    return b;
  };
  if (pad?.editing) {
    btn("Done", () => { closeMenu(); void done(); });
  } else if (pad?.fallback) {
    const d = document.createElement("div");
    d.className = "info";
    d.textContent = "Edit (fix the profile error first)";
    items.push(d);
  } else {
    btn("Edit", () => { closeMenu(); void beginEdit(); });
  }
  btn("Pads", () => void openSubmenu("pads"));
  btn("New pad", () => void openSubmenu("new"));
  btn("Reload profile", async () => {
    closeMenu();
    try {
      render(await invoke<PadView>("reload_profile"));
    } catch (err) {
      showError(String(err));
    }
  });
  items.push(document.createElement("hr"));
  const quit = btn(quitArmed > Date.now() ? "Tap again to quit" : "Quit", () => {
    if (quitArmed > Date.now()) {
      void invoke("quit");
    } else {
      quitArmed = Date.now() + 3000;
      quit.textContent = "Tap again to quit";
      quit.classList.add("confirm");
      setTimeout(() => { quit.textContent = "Quit"; quit.classList.remove("confirm"); }, 3000);
    }
  });
  menuEl.replaceChildren(...items);
  void showMenu();
  clearTimeout(menuTimer);
  menuTimer = window.setTimeout(closeMenu, 5000);
}

// ---------- wiring ----------

document.addEventListener("contextmenu", (e) => e.preventDefault());
document.addEventListener("pointerover", (e) => { if (e.pointerType === "pen") penInRange = true; });
document.addEventListener("pointerleave", (e) => { if (e.pointerType === "pen") penInRange = false; }, true);
document.addEventListener("pointerout", (e) => { if (e.pointerType === "pen" && !e.relatedTarget) penInRange = false; });

async function beginEdit() {
  try {
    applyEditor(await invoke<EditorView>("edit_begin"));
  } catch (err) {
    setStatus((err as EditFailure).message ?? String(err), "amber");
  }
}

async function done() {
  try {
    applyEditor(await invoke<EditorView>("edit_done", { discard: false }));
  } catch (err) {
    const f = err as EditFailure;
    if (f.view) applyEditor(f.view);
    setStatus(f.message ?? String(err), "red");
  }
}

$("done").addEventListener("pointerdown", (e) => { e.preventDefault(); void done(); });
$("grip").addEventListener("pointerdown", (e) => { e.preventDefault(); void getCurrentWindow().startDragging(); });
$("prof").addEventListener("pointerdown", (e) => {
  e.preventDefault();
  if (pad?.editing) {
    void select({ kind: "pad" });
    return;
  }
  void invoke("stop_all");
  setStatus("stopped", "");
});
$("menu-btn").addEventListener("pointerdown", (e) => {
  e.preventDefault();
  if (menuEl.hidden) openMenu(); else closeMenu();
});

void listen<ActionResult>("action-result", (e) => onResult(e.payload));
void listen<EditorView>("editor-state", (e) => applyEditor(e.payload));
void listen<{ pad: PadView; notice: string }>("pad-reloaded", (e) => {
  render(e.payload.pad);
  setStatus(e.payload.notice, e.payload.notice.startsWith("Reloaded") ? "" : "amber");
});
void listen<{ padOpacity: number; backgroundOpacity: number } | null>("pad-preview", (e) => {
  livePreview = e.payload;
  if (pad) applyOpacity(pad);
});
window.addEventListener("resize", () => { lastRegion = ""; void updateRegion(); });
let dprQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
const onDpr = () => {
  lastRegion = "";
  void updateRegion();
  dprQuery.removeEventListener("change", onDpr);
  dprQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
  dprQuery.addEventListener("change", onDpr);
};
dprQuery.addEventListener("change", onDpr);
void listen("second-instance", () => {
  const head = document.querySelector(".head") as HTMLElement;
  head.animate([{ background: "rgba(139,184,255,.25)" }, { background: "transparent" }], { duration: 1000 });
});

invoke<PadView>("get_pad").then(render).catch((err) => setStatus(`internal-error: ${String(err)}`, "red"));
