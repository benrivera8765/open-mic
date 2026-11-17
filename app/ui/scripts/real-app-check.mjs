// SPDX-License-Identifier: GPL-3.0-or-later
// Drives the REAL app (Tauri + Rust core + audio engine) through its WebView2 with
// Playwright over the Chrome DevTools Protocol, to check the UI against the real
// core: the library (including damaged files), loading a song, the key control, and
// the playback clock as the UI shows it (smoothness and accuracy against the
// engine's playback://clock events), plus the ticker check with WebView2's fonts.
//
// SAFETY: nothing is played aloud. The scratch Settings/settings.json selects the
// engine's null output device ("null:48000", paced in real time, no sound card) with
// master volume 0 before the app starts.
//
//   node scripts/real-app-check.mjs <path to jukeoke.exe> [song.jksong ...]
//
// Windows only (WebView2 remote debugging through WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS).
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const here = fileURLToPath(new URL(".", import.meta.url));
const exe = process.argv[2];
if (!exe) {
  console.error("usage: node scripts/real-app-check.mjs <jukeoke.exe> [songs...]");
  process.exit(2);
}
const songs = process.argv.slice(3);
if (songs.length === 0) songs.push(join(here, "..", "..", "..", "tests", "fixtures", "generated", "tiny.jksong"));

// Scratch data root in developer mode: $JK_BUILD/dev-data.
const scratch = mkdtempSync(join(process.env.JK_SCRATCH ?? tmpdir(), "jk-real-app-"));
const data = join(scratch, "dev-data");
mkdirSync(join(data, "Settings"), { recursive: true });
mkdirSync(join(data, "Library"), { recursive: true });
for (const s of songs) copyFileSync(s, join(data, "Library", basename(s)));
writeFileSync(
  join(data, "Settings", "settings.json"),
  JSON.stringify({
    ui: { skin: false, bubbles: false, motion: "system", lyrics_text_size: 56, marker_position: 0.33, colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" }, library_sort: "title", fullscreen_stage: false },
    audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} },
  }),
);

const port = 9300 + Math.floor(Math.random() * 500);
const child = spawn(exe, [], {
  env: { ...process.env, JK_BUILD: scratch, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  stdio: ["ignore", "pipe", "pipe"],
});
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

const report = { exe, scratch, checks: [], clock: null, ticker: null };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${detail}`);
};

let browser;
try {
  // Wait for the DevTools endpoint.
  for (let i = 0; ; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
      break;
    } catch (e) {
      if (i > 60) throw e;
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  const page = browser.contexts()[0].pages()[0] ?? (await browser.contexts()[0].waitForEvent("page"));
  await page.waitForSelector(".title-strip, .library__empty", { timeout: 20000 });
  await page.waitForTimeout(500);

  const lib = await page.evaluate(() => ({
    cards: document.querySelectorAll(".title-strip").length,
    damaged: [...document.querySelectorAll(".title-strip--damaged .title-strip__error")].map((e) => e.textContent),
    space: document.querySelector(".library__space")?.textContent,
    device: window.__jkDebug?.store.get().player?.device?.name ?? null,
  }));
  check("library listed", lib.cards === songs.length, `${lib.cards} cards for ${songs.length} files; ${lib.space}`);
  check("damaged files shown with their error", lib.damaged.every((t) => t && t.length > 10), JSON.stringify(lib.damaged));

  // Open the first playable song.
  await page.locator("button.title-strip").first().click();
  await page.waitForSelector(".stage__title", { timeout: 10000 });
  await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 10000 });
  const stage = await page.evaluate(() => ({ title: document.querySelector(".stage__title")?.textContent, key: document.querySelector(".key-control__label")?.textContent, device: window.__jkDebug.store.get().player?.device }));
  check("song opens on the stage", !!stage.title, `${stage.title}; key "${stage.key}"; device ${JSON.stringify(stage.device)}`);
  check("output is the silent null device", !stage.device || /null/i.test(stage.device.name), `device ${stage.device?.name}`);

  // Key control through the real core.
  await page.keyboard.press("]");
  await page.waitForTimeout(300);
  const key1 = await page.evaluate(() => ({ label: document.querySelector(".key-control__label")?.textContent, core: window.__jkDebug.store.get().player?.key_label }));
  check("key up reaches the core", /\(\+1\)/.test(key1.label ?? ""), `UI "${key1.label}", core key_label "${key1.core}"`);
  await page.keyboard.press("[");
  await page.waitForTimeout(200);

  // Real-font ticker check on the live layout.
  const v = await page.evaluate(() => window.__jkTicker.verify());
  report.ticker = v;
  check("letter under the marker (WebView2 fonts)", v.wrongLetter === 0 && v.backwards === 0 && v.tooFast === 0 && v.lookaheadMisses === 0, `${v.sungSamples} sung samples, font ${v.fontPx.toFixed(1)} px, max ${v.maxSpeedEm.toFixed(1)} em/s ${v.firstProblems.join("; ")}`);

  // Clock: play, sample the UI's interpolated clock every frame, compare with events.
  const clockStats = await page.evaluate(async () => {
    const dbg = window.__jkDebug;
    dbg.clockEvents.length = 0;
    const frames = [];
    document.querySelector(".transport__play").click();
    const t0 = performance.now();
    await new Promise((resolve) => {
      const tick = (now) => {
        frames.push({ now, pos: dbg.clock.position(now) });
        if (now - t0 < 3000) requestAnimationFrame(tick);
        else resolve();
      };
      requestAnimationFrame(tick);
    });
    const events = dbg.clockEvents.filter((e) => e.playing);
    // Error of the display against each event, evaluated just before that event re-anchored it.
    let maxErr = 0;
    let sumErr = 0;
    let n = 0;
    for (const e of events.slice(5)) {
      const before = frames.filter((f) => f.now <= e.at).pop();
      if (!before) continue;
      const predicted = before.pos + (e.at - before.now) / 1000;
      const err = Math.abs(predicted - e.position_s);
      maxErr = Math.max(maxErr, err);
      sumErr += err;
      n++;
    }
    let backwards = 0;
    let maxStep = 0;
    for (let i = 1; i < frames.length; i++) {
      const d = frames[i].pos - frames[i - 1].pos;
      if (d < -1e-9) backwards++;
      maxStep = Math.max(maxStep, d - (frames[i].now - frames[i - 1].now) / 1000);
    }
    const span = events.length > 1 ? (events[events.length - 1].at - events[0].at) / 1000 : 0;
    // Pause and measure how far the display runs past the engine's final position.
    document.querySelector(".transport__play").click();
    await new Promise((r) => setTimeout(r, 400));
    const last = dbg.clockEvents[dbg.clockEvents.length - 1];
    const shown = dbg.clock.position(performance.now());
    return {
      events: events.length,
      eventsPerSecond: span ? (events.length - 1) / span : 0,
      frames: frames.length,
      meanErrMs: n ? (sumErr / n) * 1000 : null,
      maxErrMs: maxErr * 1000,
      backwardsFrames: backwards,
      maxJumpAheadMs: maxStep * 1000,
      afterPause: { lastEventPlaying: last?.playing, engine: last?.position_s, shown, diffMs: last ? (shown - last.position_s) * 1000 : null },
    };
  });
  report.clock = clockStats;
  console.log("clock:", JSON.stringify(clockStats));
  check("clock events arrive ~30/s", clockStats.eventsPerSecond > 20, `${clockStats.eventsPerSecond.toFixed(1)} events/s`);
  check("UI clock never moves backwards while playing", clockStats.backwardsFrames === 0, `${clockStats.backwardsFrames} frames`);
  check("UI clock within 10 ms of the engine", clockStats.maxErrMs < 10, `mean ${clockStats.meanErrMs?.toFixed(2)} ms, max ${clockStats.maxErrMs.toFixed(2)} ms`);
  check("after pause the display sits on the engine position", Math.abs(clockStats.afterPause.diffMs ?? 99) < 3, `${clockStats.afterPause.diffMs?.toFixed(2)} ms`);
} catch (err) {
  check("run", false, String(err));
} finally {
  await browser?.close().catch(() => undefined);
  child.kill();
}
writeFileSync(join(scratch, "real-app-check.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 2));
console.log(`report: ${join(scratch, "real-app-check.json")}`);
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
