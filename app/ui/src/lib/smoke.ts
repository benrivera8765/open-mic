// SPDX-License-Identifier: GPL-3.0-or-later
// The UI's half of the Stage 0 `--smoke-test` (app/src-tauri/src/smoke.rs): once the
// first screen has really been painted, report a few measurements and let the core
// take a screenshot and exit. app_info does not say whether the app runs in that
// mode, so inside the app the report is always sent once at start-up; the core
// rejects it (harmlessly) when it was not started with --smoke-test.
//
// Progress notes (D-279): the macOS CI run's UI never reported and left no trace of how
// far it got. `smokeProgress` sends each start-up step and any script error to the core,
// which keeps them in smoke-test.json (`ui_progress`) and names the last one when the UI
// does not report in time. Outside --smoke-test the first note is rejected and no more
// are sent.

type Send = (step: string) => Promise<void>;

/** At most this many notes per start-up (a script error in a loop must not flood the core). */
const PROGRESS_MAX = 40;
let progressSend: Send | null = null;
let progressSent = 0;

/** Sends one start-up note to the core (in --smoke-test mode only; see above). */
export function smokeProgress(step: string): void {
  if (!progressSend || progressSent >= PROGRESS_MAX) return;
  progressSent += 1;
  progressSend(step).catch(() => {
    progressSend = null; // not in --smoke-test mode: nothing to do
  });
}

/** Starts sending notes, script errors and unhandled rejections included. Once per start-up. */
export function startSmokeProgress(send: Send): void {
  if (progressSend) return;
  progressSend = send;
  window.addEventListener("error", (e) => smokeProgress(`script error: ${e.message} (${e.filename}:${e.lineno}:${e.colno})`));
  window.addEventListener("unhandledrejection", (e) => {
    const r: unknown = e.reason;
    smokeProgress(`unhandled rejection: ${r instanceof Error ? `${r.name}: ${r.message}` : String(r)}`);
  });
}

function pageState(): string {
  return `visibility ${document.visibilityState}, focus ${document.hasFocus()}`;
}

/** Runs `callback` once the browser has painted the current DOM. */
function afterNextPaint(callback: () => void): void {
  let framed = false;
  // A frame that never comes is reported (not worked around): the lyrics ticker and the
  // practice lane draw on animation frames too.
  const silent = setTimeout(() => {
    if (!framed) smokeProgress(`ui: no animation frame 5 s after asking (${pageState()})`);
  }, 5000);
  requestAnimationFrame(() => {
    framed = true;
    clearTimeout(silent);
    smokeProgress("ui: animation frame ran");
    setTimeout(callback, 0);
  });
}

function paintTime(name: string): number | null {
  const entry = performance.getEntriesByType("paint").find((e) => e.name === name);
  return entry ? entry.startTime : null;
}

/** Resolves at the first contentful paint, or after `timeoutMs` where Paint Timing is missing. */
function whenContentPainted(timeoutMs: number): Promise<void> {
  return new Promise((resolve) => {
    if (paintTime("first-contentful-paint") !== null) {
      resolve();
      return;
    }
    let observer: PerformanceObserver | undefined;
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      observer?.disconnect();
      resolve();
    };
    try {
      observer = new PerformanceObserver((list) => {
        if (list.getEntries().some((e) => e.name === "first-contentful-paint")) finish();
      });
      observer.observe({ type: "paint", buffered: true });
    } catch {
      // Paint Timing not supported: rely on the timeout.
    }
    setTimeout(finish, timeoutMs);
  });
}

/** Sends the smoke report (camelCase, as smoke.rs `UiReport` expects). */
export function reportSmoke(send: (report: unknown) => Promise<void>, scriptStartMs: number, appInfoRoundTripMs: number): void {
  void whenContentPainted(3000).then(() => {
    const fcp = paintTime("first-contentful-paint");
    smokeProgress(`ui: ${fcp === null ? "no first-contentful-paint entry after 3 s" : `first contentful paint at ${Math.round(fcp)} ms`} (${pageState()})`);
    afterNextPaint(() => {
      const report = {
        scriptStartMs,
        firstPaintMs: paintTime("first-paint"),
        firstContentfulPaintMs: fcp ?? paintTime("first-contentful-paint"),
        reportSentMs: performance.now(),
        appInfoRoundTripMs,
        renderedTitle: document.querySelector("h1")?.textContent ?? "",
        userAgent: navigator.userAgent,
        viewport: { width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio },
      };
      smokeProgress("ui: sending the report");
      send(report).catch((err: unknown) => {
        // Outside --smoke-test this is the expected rejection; in it, the reason is the clue.
        smokeProgress(`ui: smoke_report was refused: ${err instanceof Error ? err.message : JSON.stringify(err)}`);
      });
    });
  });
}
