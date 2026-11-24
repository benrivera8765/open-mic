// SPDX-License-Identifier: GPL-3.0-or-later
// Modal overlays: keyboard shortcuts (the "?" overlay) and the self-check results.
// Native <dialog> (Safari 15.4+, Chrome 37+): focus is trapped and Esc closes it.
import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type { SelfCheckItem, SelfCheckResult } from "../api/types";
import { errorMessage } from "../api/client";
import { getApiSync, setOverlay } from "../state/app";
import { measureLaneCost } from "../practice/lanecost";

function Dialog({ title, onClose, children, class: cls }: { title: string; onClose: () => void; children: ComponentChildren; class?: string }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (typeof d.showModal === "function" && !d.open) d.showModal();
    else d.setAttribute("open", "");
    const close = () => onClose();
    d.addEventListener("close", close);
    return () => {
      d.removeEventListener("close", close);
      if (d.open && typeof d.close === "function") d.close();
    };
  }, []);
  return (
    <dialog ref={ref} class={`dialog ${cls ?? ""}`} aria-labelledby="dialog-title">
      <header class="dialog__header">
        <h2 id="dialog-title">{title}</h2>
        <button type="button" class="icon-button dialog__close" aria-label="Close" onClick={() => ref.current?.close()}>
          <span aria-hidden="true">✕</span>
        </button>
      </header>
      <div class="dialog__body">{children}</div>
    </dialog>
  );
}

/** In a row's keys: the keys after it do the same as the keys before it. */
export const OR = "or";

/** Every keyboard shortcut, as handled in App.tsx. */
export const SHORTCUTS: [keys: string[], what: string][] = [
  [["Space"], "Play / pause"],
  [["←", "→"], "Back / forward 5 seconds"],
  // One row for both pairs of key-change keys (two rows with the same words read as a mistake).
  [["[", "]", OR, "−", "+"], "Key down / up one semitone"],
  [["F"], "Full screen stage"],
  [["P"], "Practice mode on / off (sing and see your pitch)"],
  [["Esc"], "Leave full screen, close a window"],
  [["Ctrl / ⌘", "F"], "Search the library"],
  [["Delete"], "Library: delete the focused or current song, or the ticked ones (asks first; ⌘ + Backspace on a Mac)"],
  [["?"], "Show these shortcuts"],
  [["Space", "Enter"], "In tap-along: mark the start of the next word"],
  [["Backspace"], "In tap-along: take back the last tap"],
];

export function HelpOverlay() {
  return (
    <Dialog title="Keyboard shortcuts" onClose={() => setOverlay(null)} class="help">
      <table class="help__table">
        <tbody>
          {SHORTCUTS.map(([keys, what]) => (
            <tr key={what + keys.join()}>
              <th scope="row">
                {keys.map((k, i) =>
                  k === OR ? (
                    <span key={`or-${i}`} class="help__or">
                      {" or "}
                    </span>
                  ) : (
                    <span key={k}>
                      {i > 0 && keys[i - 1] !== OR && (keys[0].startsWith("Ctrl") ? " + " : " / ")}
                      <kbd>{k}</kbd>
                    </span>
                  ),
                )}
              </th>
              <td>{what}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p class="hint">The mouse wheel over the key control also changes the key.</p>
      <p class="hint">
        Jukebox look: drag the key dial round (it changes the key when you let go), or click it and use the arrow keys, Page Up / Page Down (2 steps), Home / End; <kbd>0</kbd> or a double-click on the dial goes back to the original key. The faders take the arrow keys and the wheel too.
      </p>
    </Dialog>
  );
}

/**
 * What a passed check means, in plain words, by the check's name (selfcheck.rs and
 * practice/checks.rs name them). The core's detail lines are measurements for bug
 * reports ("max diff 0", "600 frames at 1632x389 (x1)"): they stay in the saved report
 * and under "Technical details", not in the list. A failed check shows its detail as
 * it is, because that says what to do. Null: show the detail (an unknown check, or a
 * detail that is already plain, such as "Skipped: ...").
 */
export function plainCheck(item: SelfCheckItem): string | null {
  if (!item.ok) return null;
  const d = item.detail.trim();
  if (/^(skipped|not measured)\b/i.test(d)) return null;
  switch (item.name) {
    case "System":
      return "This computer can run the app.";
    case "Web engine":
      return "The part of the system that draws the app is working.";
    case "Memory":
      return /less than 512 MB/i.test(d) ? "Memory is low: close other apps (web browsers especially) before singing, or playback may stutter." : "There is enough free memory.";
    case "Output device":
      return "Speakers or headphones are connected and ready.";
    case "Test tone":
      return /silence/i.test(d) ? "Sound reaches the speakers or headphones without dropouts (played silently)." : "The quiet test tone played without dropouts.";
    case "Bundled test song":
    case "Demo song":
      return "The test song plays back exactly as it should.";
    case "Key change":
      return "Changing the key works: the right pitch, and the song keeps its length.";
    case "Recorder":
      return "Recording the performance works.";
    case "Microphone permission":
      return "The app is allowed to use the microphone.";
    case "Microphone capture":
      return "The microphone can be heard.";
    case "Pitch tracker":
      return "The app hears which note is sung.";
    case "Practice guide":
      return "Practice data can be made from a song's lead vocal.";
    case "Practice score":
      return "Practice scores come out right.";
    case "Practice lane":
      return "This computer draws the practice notes smoothly.";
    default:
      return null;
  }
}

export function SelfCheckOverlay() {
  const [result, setResult] = useState<SelfCheckResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [silent, setSilent] = useState(false);
  const run = async (quiet = silent) => {
    setRunning(true);
    setError(null);
    setResult(null);
    try {
      // WO2 4.8: the practice lane's drawing cost in this web view goes into the report.
      const lane = await measureLaneCost().catch(() => null);
      setResult(await getApiSync().selfcheckRun(navigator.userAgent, quiet, lane));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setRunning(false);
    }
  };
  useEffect(() => void run(), []);
  return (
    <Dialog title="Self-check" onClose={() => setOverlay(null)} class="selfcheck">
      <p class="hint">Plays a very quiet test tone, renders and key-shifts the bundled test song, checks the recorder, measures how fast this computer draws the practice lane, and writes a plain-text report you can bring back.</p>
      {running && <p role="status">Running the checks… (this takes a few seconds)</p>}
      {error && <p class="note note--error">{error}</p>}
      {result && (
        <>
          <p class={`selfcheck__summary ${result.passed ? "selfcheck__summary--ok" : "selfcheck__summary--fail"}`} role="status">
            {result.passed ? "Everything passed." : "Some checks failed. The details below say what to do."}
          </p>
          <ul class="selfcheck__items">
            {result.items.map((it) => (
              <li key={it.name} class={`selfcheck__item ${it.ok ? "selfcheck__item--ok" : "selfcheck__item--fail"}`}>
                <span class="selfcheck__mark" aria-hidden="true">
                  {it.ok ? "✓" : "✗"}
                </span>
                <strong>{it.name}</strong> <span class="visually-hidden">{it.ok ? "passed" : "failed"}</span>
                <p>{plainCheck(it) ?? it.detail}</p>
              </li>
            ))}
          </ul>
          <p>
            Report saved to <code>{result.report_path}</code>
          </p>
          {/* The measurements, word for word as in the report, for whoever reads a bug report. */}
          <details class="selfcheck__details">
            <summary>Technical details</summary>
            <dl class="selfcheck__facts">
              {result.items.map((it) => (
                <div key={it.name}>
                  <dt>{it.name}</dt>
                  <dd>{it.detail}</dd>
                </div>
              ))}
            </dl>
          </details>
        </>
      )}
      <div class="row">
        <label>
          <input type="checkbox" checked={silent} onChange={(e) => setSilent((e.currentTarget as HTMLInputElement).checked)} /> Silent: play digital silence instead of the quiet test tone
        </label>
        <button type="button" class="button" disabled={running} onClick={() => void run()}>
          Run again
        </button>
      </div>
    </Dialog>
  );
}
