// SPDX-License-Identifier: GPL-3.0-or-later
// Modal overlays: keyboard shortcuts (the "?" overlay) and the self-check results.
// Native <dialog> (Safari 15.4+, Chrome 37+): focus is trapped and Esc closes it.
import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type { SelfCheckResult } from "../api/types";
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

/** Every keyboard shortcut, as handled in App.tsx. */
export const SHORTCUTS: [keys: string[], what: string][] = [
  [["Space"], "Play / pause"],
  [["←", "→"], "Back / forward 5 seconds"],
  [["[", "]"], "Key down / up one semitone"],
  [["−", "+"], "Key down / up one semitone"],
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
                {keys.map((k, i) => (
                  <span key={k}>
                    {i > 0 && (keys[0].startsWith("Ctrl") ? " + " : " / ")}
                    <kbd>{k}</kbd>
                  </span>
                ))}
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
                <p>{it.detail}</p>
              </li>
            ))}
          </ul>
          <p>
            Report saved to <code>{result.report_path}</code>
          </p>
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
