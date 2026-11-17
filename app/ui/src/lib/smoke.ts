// SPDX-License-Identifier: GPL-3.0-or-later
// The UI's half of the Stage 0 `--smoke-test` (app/src-tauri/src/smoke.rs): once the
// first screen has really been painted, report a few measurements and let the core
// take a screenshot and exit. app_info does not say whether the app runs in that
// mode, so inside the app the report is always sent once at start-up; the core
// rejects it (harmlessly) when it was not started with --smoke-test.

/** Runs `callback` once the browser has painted the current DOM. */
function afterNextPaint(callback: () => void): void {
  requestAnimationFrame(() => setTimeout(callback, 0));
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
  void whenContentPainted(3000).then(() =>
    afterNextPaint(() => {
      const report = {
        scriptStartMs,
        firstPaintMs: paintTime("first-paint"),
        firstContentfulPaintMs: paintTime("first-contentful-paint"),
        reportSentMs: performance.now(),
        appInfoRoundTripMs,
        renderedTitle: document.querySelector("h1")?.textContent ?? "",
        userAgent: navigator.userAgent,
        viewport: { width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio },
      };
      send(report).catch(() => undefined); // not in --smoke-test mode: nothing to do
    }),
  );
}
