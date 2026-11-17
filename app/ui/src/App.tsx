// SPDX-License-Identifier: GPL-3.0-or-later
// Plain functional placeholder page (Stage 0). It proves the round trip
// UI -> Rust command -> UI and shows where the app keeps its data. The jukebox
// skin (docs/DESIGN.md) is a separate layer that will wrap real screens later.
import { useEffect, useRef, useState } from "preact/hooks";
import {
  type AppInfo,
  getAppInfo,
  inTauri,
  ping,
  sendSmokeReport,
  type SmokeReport,
} from "./api";
import { formatMs, modeLabel } from "./format";

/**
 * Runs `callback` once the browser has actually painted the current DOM:
 * requestAnimationFrame fires just before a paint, and a task queued from inside it
 * runs after that paint has been committed.
 */
function afterNextPaint(callback: () => void): void {
  requestAnimationFrame(() => setTimeout(callback, 0));
}

function paintTime(name: string): number | null {
  const entry = performance.getEntriesByType("paint").find((e) => e.name === name);
  return entry ? entry.startTime : null;
}

/**
 * Resolves once the browser reports its first contentful paint (Paint Timing API),
 * or after `timeoutMs` where that API is missing or silent. Paint entries arrive
 * only after the frame was presented, so they can lag behind requestAnimationFrame.
 */
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
      // Paint Timing not supported by this web view: rely on the timeout.
    }
    setTimeout(finish, timeoutMs);
  });
}

export function App({ scriptStartMs }: { scriptStartMs: number }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pingText, setPingText] = useState("");
  const appInfoRoundTrip = useRef(0);

  // 1. Ask Rust who we are, as soon as the page is up.
  useEffect(() => {
    if (!inTauri()) {
      setError("Not running inside the app. Start it with `npm run tauri dev` in app/ui.");
      return;
    }
    const started = performance.now();
    getAppInfo()
      .then((reply) => {
        appInfoRoundTrip.current = performance.now() - started;
        setInfo(reply);
      })
      .catch((e: unknown) => setError(`app_info failed: ${String(e)}`));
  }, []);

  // 2. In --smoke-test mode, report back once the answer has been painted.
  useEffect(() => {
    if (!info?.smokeTest) return;
    void whenContentPainted(3000).then(() => afterNextPaint(() => {
      const report: SmokeReport = {
        scriptStartMs,
        firstPaintMs: paintTime("first-paint"),
        firstContentfulPaintMs: paintTime("first-contentful-paint"),
        reportSentMs: performance.now(),
        appInfoRoundTripMs: appInfoRoundTrip.current,
        renderedTitle: document.querySelector("h1")?.textContent ?? "",
        userAgent: navigator.userAgent,
        viewport: {
          width: window.innerWidth,
          height: window.innerHeight,
          devicePixelRatio: window.devicePixelRatio,
        },
      };
      sendSmokeReport(report).catch((e: unknown) => setError(`smoke_report failed: ${String(e)}`));
    }));
  }, [info, scriptStartMs]);

  const onPing = () => {
    const started = performance.now();
    ping("hello from the UI")
      .then((reply) => {
        const roundTrip = performance.now() - started;
        setPingText(`Rust replied "${reply.echo}" in ${formatMs(roundTrip)} (app up ${reply.uptimeMs} ms)`);
      })
      .catch((e: unknown) => setPingText(`ping failed: ${String(e)}`));
  };

  return (
    <main class="page">
      <h1>{info ? info.name : "Starting..."}</h1>
      {error && <p class="error">{error}</p>}
      {info && (
        <>
          <dl class="facts">
            <dt>Version</dt>
            <dd>{info.version}</dd>
            <dt>Data mode</dt>
            <dd>{modeLabel(info.mode)}</dd>
            <dt>Why</dt>
            <dd>{info.modeReason}</dd>
            <dt>Portable root</dt>
            <dd>
              <code>{info.portableRoot ?? "(not portable)"}</code>
            </dd>
            <dt>Data folder</dt>
            <dd>
              <code>{info.dataRoot}</code>
            </dd>
            <dt>Web view data</dt>
            <dd>
              <code>{info.webviewDataDir ?? "(not kept: non-persistent web view)"}</code>
            </dd>
            <dt>System</dt>
            <dd>
              {info.os} / {info.arch}
            </dd>
            <dt>Web view</dt>
            <dd>{info.webviewVersion}</dd>
            <dt>Rust round trip</dt>
            <dd>{formatMs(appInfoRoundTrip.current)}</dd>
          </dl>
          {info.translocated && (
            <p class="error">
              macOS is running this app from a temporary copy (App Translocation), so it cannot
              see the folder it was copied to. Move it out of Downloads or use the first-launch
              steps in the manual.
            </p>
          )}
          <p>
            <button type="button" onClick={onPing}>
              Ping the Rust core
            </button>{" "}
            <span class="ping">{pingText}</span>
          </p>
          {info.smokeTest && <p class="note">Smoke test: reporting back and closing.</p>}
        </>
      )}
    </main>
  );
}
