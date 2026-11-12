// Render-only web layer: Rust validates the profile, computes geometry,
// runs actions and decides outcomes. This file draws keys and forwards taps.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type KeyView = {
  id: string; label: string; group: string; icon: string | null; color: string | null;
  repeat: number; disabled: string | null; x: number; y: number; w: number; h: number;
};
type PadView = {
  name: string; fallback: boolean; acceptTouch: boolean; width: number; height: number;
  theme: string; warnings: string[]; keys: KeyView[];
};
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
  el.addEventListener("pointerdown", (e) => onKey(e, k, el));
  return el;
}

function render(view: PadView) {
  pad = view;
  $("prof").textContent = view.name + (view.fallback ? " (fallback)" : "");
  $("dot").hidden = view.warnings.length === 0;
  keysEl.replaceChildren(...view.keys.map(keyEl));
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
  btn("Reload profile", async () => { closeMenu(); render(await invoke<PadView>("reload_profile")); });
  btn("Surface spike (test)", () => { closeMenu(); invoke("spike_start").catch((err) => showError(`spike: ${String(err)}`)); });
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
  menuEl.hidden = false;
  clearTimeout(menuTimer);
  menuTimer = window.setTimeout(closeMenu, 5000);
}

// ---------- wiring ----------

document.addEventListener("contextmenu", (e) => e.preventDefault());
document.addEventListener("pointerover", (e) => { if (e.pointerType === "pen") penInRange = true; });
document.addEventListener("pointerleave", (e) => { if (e.pointerType === "pen") penInRange = false; }, true);
document.addEventListener("pointerout", (e) => { if (e.pointerType === "pen" && !e.relatedTarget) penInRange = false; });

$("grip").addEventListener("pointerdown", (e) => { e.preventDefault(); void getCurrentWindow().startDragging(); });
$("prof").addEventListener("pointerdown", (e) => {
  e.preventDefault();
  void invoke("stop_all");
  setStatus("stopped", "");
});
$("menu-btn").addEventListener("pointerdown", (e) => {
  e.preventDefault();
  if (menuEl.hidden) openMenu(); else closeMenu();
});

void listen<ActionResult>("action-result", (e) => onResult(e.payload));
void listen("second-instance", () => {
  const head = document.querySelector(".head") as HTMLElement;
  head.animate([{ background: "rgba(139,184,255,.25)" }, { background: "transparent" }], { duration: 1000 });
});

invoke<PadView>("get_pad").then(render).catch((err) => setStatus(`internal-error: ${String(err)}`, "red"));
