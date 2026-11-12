// Surface spike pad (temporary; delete with src-tauri/src/spike.rs).
// Transparent copy of the pad: background opacity buttons, key-only
// tap-through region at 0%, shadow/corner toggles and the dummy panel.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type KeyView = {
  id: string; label: string; group: string; color: string | null;
  repeat: number; disabled: string | null; x: number; y: number; w: number; h: number;
};
type SpikePad = { pad: { name: string; keys: KeyView[] }; headExtra: number; results: string };
type Status = { foreground: string; kind: string; panelOpen: boolean; target: string; endsessionSeen: number };
type ActionResult = { buttonId: string; ok: boolean; code: string };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const keysEl = $("keys");
const statusEl = $("status");

let bg = 0.97;
let tapThrough = true;
let shadow = false;
let round = true;
let endsess = false;
let taps = 0;
let ok = 0;
let lastCode = "";
let regionNote = "";
let doneNote = "";
let doneUntil = 0;
let quitArmed = 0;

function down(el: HTMLElement, fn: (e: PointerEvent) => void) {
  el.addEventListener("pointerdown", (e) => { e.preventDefault(); fn(e); });
}

function keyEl(k: KeyView, top: number): HTMLElement {
  const el = document.createElement("div");
  el.className = `k ${k.group}${k.disabled ? " disabled" : ""}`;
  Object.assign(el.style, { left: `${k.x}px`, top: `${k.y + top}px`, width: `${k.w}px`, height: `${k.h}px` });
  if (k.color) {
    el.style.setProperty("--fg", k.color);
    el.style.setProperty("--bd", k.color);
    el.style.setProperty("--bg", `${k.color}0f`);
  }
  const label = document.createElement("span");
  label.className = "label";
  label.textContent = k.label; // textContent only: labels are untrusted
  el.appendChild(label);
  if (k.repeat > 1) {
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = `×${k.repeat}`;
    el.appendChild(badge);
  }
  down(el, (e) => {
    if (e.button !== 0 || !e.isPrimary || k.disabled) return;
    taps += 1;
    el.classList.add("pressed");
    setTimeout(() => el.classList.remove("pressed"), 120);
    invoke<string>("run_action", { buttonId: k.id }).catch((err) => { lastCode = String((err as { code?: string }).code ?? err); });
  });
  return el;
}

async function updateRegion() {
  const on = bg === 0 && tapThrough;
  const rects = on
    ? [$("shead"), ...Array.from(keysEl.children)].map((el) => {
        const r = (el as HTMLElement).getBoundingClientRect();
        return [r.x, r.y, r.width, r.height];
      })
    : [];
  try {
    regionNote = await invoke<string>("spike_region", { rects, on });
  } catch (err) {
    regionNote = `region error: ${String(err)}`;
  }
}

function setBg(v: number) {
  bg = v;
  document.documentElement.style.setProperty("--bgo", String(v));
  document.querySelectorAll<HTMLButtonElement>("[data-bg]").forEach((b) => b.classList.toggle("on", Number(b.dataset.bg) === v));
  void updateRegion();
}

function setChrome() {
  $("shadow").classList.toggle("on", shadow);
  $("corners").classList.toggle("on", round);
  void invoke("spike_chrome", { shadow, round });
}

async function poll() {
  try {
    const s = await invoke<Status>("spike_status");
    const fg = document.createElement("div");
    fg.textContent = `Foreground (${s.kind}): ${s.foreground || "(untitled)"}`;
    if (s.kind !== "other") fg.className = "warn";
    const line2 = document.createElement("div");
    line2.textContent = Date.now() < doneUntil
      ? doneNote
      : `keys ${taps} tapped · ${ok} sent ok${lastCode ? ` · last ${lastCode}` : ""} · ${regionNote}`;
    statusEl.replaceChildren(fg, line2);
    $("panel").classList.toggle("on", s.panelOpen);
  } catch (err) {
    statusEl.textContent = `status error: ${String(err)}`;
  }
}

document.addEventListener("contextmenu", (e) => e.preventDefault());
down($("grip"), () => void getCurrentWindow().startDragging());
document.querySelectorAll<HTMLButtonElement>("[data-bg]").forEach((b) => down(b, () => setBg(Number(b.dataset.bg))));
down($("tap"), () => { tapThrough = !tapThrough; $("tap").classList.toggle("on", tapThrough); void updateRegion(); });
down($("shadow"), () => { shadow = !shadow; setChrome(); });
down($("corners"), () => { round = !round; setChrome(); });
down($("endsess"), () => { endsess = !endsess; $("endsess").classList.toggle("on", endsess); void invoke("spike_endsession", { on: endsess }); });
down($("panel"), () => { invoke<string>("spike_panel_open").then((m) => { regionNote = m; }).catch((err) => { regionNote = String(err); }); });
down($("quit"), () => {
  if (quitArmed > Date.now()) { void invoke("quit"); return; }
  quitArmed = Date.now() + 3000;
  $("quit").textContent = "Again";
  setTimeout(() => { $("quit").textContent = "Quit"; }, 3000);
});

void listen<ActionResult>("action-result", (e) => {
  if (e.payload.ok && e.payload.code === "ok") ok += 1; else lastCode = e.payload.code;
});
void listen<string>("spike-done", (e) => { doneNote = e.payload; doneUntil = Date.now() + 10000; });
window.addEventListener("resize", () => void updateRegion());
let dprQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
const onDpr = () => {
  void updateRegion();
  dprQuery.removeEventListener("change", onDpr);
  dprQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
  dprQuery.addEventListener("change", onDpr);
};
dprQuery.addEventListener("change", onDpr);

invoke<SpikePad>("spike_pad").then((p) => {
  keysEl.replaceChildren(...p.pad.keys.map((k) => keyEl(k, p.headExtra)));
  $("tap").classList.add("on");
  setChrome();
  setBg(0.97);
  setInterval(poll, 400);
}).catch((err) => { statusEl.textContent = `internal-error: ${String(err)}`; });
