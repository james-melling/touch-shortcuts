// Surface spike panel (temporary; delete with src-tauri/src/spike.rs).
// Opens without activation; only Label/Note taps make it take focus.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

type Status = { foreground: string; kind: string; panelOpen: boolean; target: string; endsessionSeen: number };
type SpikePad = { results: string };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const msg = (t: string) => { $("msg").textContent = t; };
let activated = false;

const CHECKS: [string, string][] = [
  ["bg0-pen", "BG 0% with Tap-thru on: 10 pen taps in the gaps between keys all reach R1."],
  ["bg0-finger", "Same with 10 finger taps in the gaps."],
  ["bg0-keys", "BG 0%: 20 taps on keys all send (R1's value changes every time)."],
  ["bg0-outline", "BG 0%, Shadow off, Corners off: nothing visible around the invisible part of the pad."],
  ["bg0-shadow", "Now Shadow on: no outline or shadow appears around the invisible rect (Fail if one does)."],
  ["focus-transparent", "Tapping keys on this transparent pad: R1's title bar stays active and the caret stays in its field."],
  ["focus-endsess", "EndSess on, tap keys again: R1 still keeps focus and caret."],
  ["panel-pen", "Panel open: tapping the chips above leaves R1 in front (top line says 'other: …R1…')."],
  ["panel-hide", "Hide 1.5 s: the foreground doesn't change."],
  ["label-focus", "Tap Label: the panel takes focus and you can write in it."],
  ["label-kb", "The touch keyboard or handwriting panel appears on the FIRST Label tap (Fail if you had to tap twice)."],
  ["label-visible", "With the touch keyboard docked, the Label field is still visible."],
  ["gate", "GATE: caret in an R1 field → pad Panel → chips → Label → write → Done: R1 is in front with the caret in the same field."],
  ["elgato", "Optional: Elgato Virtual Stream Deck keeps R1 focused when tapped (note what you saw)."],
];

function down(el: HTMLElement, fn: (e: PointerEvent) => void) {
  el.addEventListener("pointerdown", (e) => { e.preventDefault(); fn(e); });
}

/** First tap on a text field activates the panel, then focuses the field. */
function typing(el: HTMLInputElement | HTMLTextAreaElement) {
  el.addEventListener("pointerdown", (e) => {
    if (activated) return; // window is active now: let the tap focus natively
    e.preventDefault();
    invoke<boolean>("spike_panel_activate").then((okay) => {
      activated = okay;
      el.focus();
      msg(okay ? "Panel activated for typing." : "Panel could not take focus (see results file).");
    });
  });
}

function renderChecks() {
  const rows = CHECKS.map(([id, text]) => {
    const row = document.createElement("div");
    row.className = "check";
    const p = document.createElement("p");
    p.textContent = text;
    if (id === "gate") p.className = "gate";
    const pass = document.createElement("button");
    pass.type = "button"; pass.className = "pass"; pass.textContent = "Pass";
    const fail = document.createElement("button");
    fail.type = "button"; fail.className = "fail"; fail.textContent = "Fail";
    const mark = (result: string, btn: HTMLElement) => {
      const note = ($("note") as HTMLTextAreaElement).value.trim();
      void invoke("spike_record", { item: id, result, note });
      pass.classList.toggle("on", btn === pass);
      fail.classList.toggle("on", btn === fail);
      ($("note") as HTMLTextAreaElement).value = "";
      msg(`Recorded ${id}: ${result}${note ? " (with note)" : ""}`);
    };
    down(pass, () => mark("pass", pass));
    down(fail, () => mark("fail", fail));
    row.append(p, pass, fail);
    return row;
  });
  $("checks").replaceChildren(...rows);
}

async function poll() {
  try {
    const s = await invoke<Status>("spike_status");
    $("fg").textContent = `Foreground: ${s.kind}: ${s.foreground || "(untitled)"} · Done returns to: ${s.target || "(none captured)"}`;
    $("fg").className = `pfg ${s.kind === "panel" ? "panel" : ""}`;
  } catch (err) {
    $("fg").textContent = `status error: ${String(err)}`;
  }
}

document.addEventListener("contextmenu", (e) => e.preventDefault());
down($("ptitle"), () => void getCurrentWindow().startDragging());
document.querySelectorAll<HTMLButtonElement>(".chip").forEach((c) => down(c, () => c.classList.toggle("on")));
down($("hideshow"), () => { msg("Hiding…"); invoke<string>("spike_panel_hide_show").then(msg); });
typing($("label") as HTMLInputElement);
typing($("note") as HTMLTextAreaElement);
down($("done"), () => { msg("Returning…"); void invoke("spike_panel_done"); });

renderChecks();
invoke<SpikePad>("spike_pad").then((p) => { $("path").textContent = `Results: ${p.results}`; });
setInterval(poll, 400);
void poll();
