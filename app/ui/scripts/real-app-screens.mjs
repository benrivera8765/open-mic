// SPDX-License-Identifier: GPL-3.0-or-later
// Drives the REAL app (release exe, Rust core, audio engine) in the jukebox look and
// captures what its WebView2 renders: the library and the stage (a song loaded and
// playing) at several window sizes, plus checks on the way:
//   - the frame, its crest badge and the bundled fonts are there;
//   - a song opens from its title strip, plays (clock events arrive), and the key
//     dial / [ ] keys change the key through the real core;
//   - the ticker's letter-under-the-marker check passes with the skin's lyrics font;
//   - ticker frame rate and draw time while the bubbles run;
//   - no page errors, no horizontal overflow.
//
// SAFETY: nothing is played aloud. Settings/settings.json in the data root selects
// the engine's null output device ("null:48000", paced in real time, no sound card)
// and master volume 0 before the app starts.
//
//   node scripts/real-app-screens.mjs <Jukeoke.exe> <portable root> <out dir>
//
// The root must be a portable layout (<root>/Windows/Jukeoke.exe is the exe given,
// <root>/Library holds the songs). Windows only: WebView2 remote debugging through
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS, Playwright over the DevTools protocol. The
// window sizes are emulated with Emulation.setDeviceMetricsOverride, so 3840x2160
// works on a smaller monitor.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [exe, root, outDir] = process.argv.slice(2);
if (!exe || !root || !outDir) {
  console.error("usage: node scripts/real-app-screens.mjs <Jukeoke.exe> <portable root> <out dir>");
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify(
    {
      // motion "full": the drift and bubbles run whatever the OS reduce-motion setting is.
      ui: { skin: true, bubbles: true, motion: "full", lyrics_text_size: 56, marker_position: 0.33, colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" }, library_sort: "title", fullscreen_stage: false },
      audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} },
    },
    null,
    2,
  ),
);

const port = 9300 + Math.floor(Math.random() * 500);
const child = spawn(exe, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  stdio: ["ignore", "pipe", "pipe"],
});
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

const report = { exe, root, checks: [], sizes: [], ticker: [], perf: null, errors: [] };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${detail}`);
};
const SIZES = [
  { width: 1440, height: 900 },
  { width: 1280, height: 800 },
  { width: 3840, height: 2160 },
];

let browser;
try {
  for (let i = 0; ; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
      break;
    } catch (e) {
      if (i > 60) throw e;
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  const ctx = browser.contexts()[0];
  const page = ctx.pages()[0] ?? (await ctx.waitForEvent("page"));
  page.on("pageerror", (e) => report.errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && report.errors.push(m.text()));
  await page.waitForSelector(".jk-strip, .library__empty", { timeout: 30000 });
  await page.waitForTimeout(800);

  const first = await page.evaluate(async () => ({
    skin: document.documentElement.dataset.skin,
    motion: document.querySelector(".jk-frame")?.getAttribute("data-jk-motion"),
    strips: document.querySelectorAll(".jk-strip").length,
    damaged: [...document.querySelectorAll(".library__strip--damaged .jk-strip-note")].map((e) => e.textContent),
    badge: document.querySelector(".jk-frame-crest .jk-badge")?.getAttribute("aria-label") ?? null,
    bubbles: !!document.querySelector(".jk-bubbles"),
    fonts: {
      lyrics: await document.fonts.load('700 64px "Atkinson Hyperlegible Next"').then((f) => f.length > 0),
      display: document.fonts.check('500 16px "Oswald"'),
      badge: document.fonts.check('400 16px "Pacifico"'),
    },
    device: window.__jkDebug?.store.get().player?.device ?? null,
    space: document.querySelector(".library__space")?.textContent,
    // The skin pauses its animation while the window is minimised; that needs the
    // window's is_minimized getter, which core:default must allow.
    minimisedQuery: await window.__TAURI_INTERNALS__
      .invoke("plugin:window|is_minimized", { label: window.__TAURI_INTERNALS__.metadata.currentWindow.label })
      .then((v) => `ok: ${v}`, (e) => `refused: ${e}`),
  }));
  report.first = first;
  check("jukebox look on", first.skin === "on" && first.motion === "full", `data-skin=${first.skin}, frame motion=${first.motion}`);
  check("crest badge shows the app name", !!first.badge, `badge "${first.badge}"`);
  check("bubbles canvas present", first.bubbles, String(first.bubbles));
  check("bundled fonts loaded", first.fonts.lyrics && first.fonts.display && first.fonts.badge, JSON.stringify(first.fonts));
  check("library listed as title strips", first.strips > 0, `${first.strips} strips; damaged: ${JSON.stringify(first.damaged)}; ${first.space}`);
  check("output is the silent null device", !first.device || /null/i.test(first.device.name), `device ${JSON.stringify(first.device)}`);
  check("window minimised state is readable (skin pause)", first.minimisedQuery === "ok: false", first.minimisedQuery);

  const shot = async (name) => {
    const file = join(outDir, `${name}.png`);
    await page.screenshot({ path: file });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    report.sizes.push({ name, file, overflow });
    console.log(`  saved ${file}${overflow > 0 ? ` (horizontal overflow ${overflow}px)` : ""}`);
    if (overflow > 0) check(`${name}: no horizontal overflow`, false, `${overflow}px`);
  };

  let songOpened = false;
  for (const vp of SIZES) {
    const tag = `${vp.width}x${vp.height}`;
    await page.setViewportSize(vp);
    // Library.
    await page.locator(".skin-nav .jk-plate", { hasText: "Library" }).click();
    await page.waitForSelector(".jk-strip");
    await page.waitForTimeout(1200); // covers, frame art for the new size
    await shot(`library-${tag}`);

    // Stage: open the demo song (first time from its strip), play from just before a line.
    if (!songOpened) {
      await page.locator(".jk-strip", { hasText: /twinkle/i }).first().click();
      await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 15000 });
      songOpened = true;
      // The key saved in the song file (a previous run may have left one): start from 0.
      const saved = await page.evaluate(() => ({ label: document.querySelector(".jk-keydial-label")?.textContent, semis: window.__jkDebug.store.get().songSettings?.semitones }));
      await page.locator(".jk-dial").focus();
      await page.keyboard.press("0");
      await page.waitForTimeout(600);
      await page.evaluate(() => document.activeElement?.blur());
      const k0 = await page.evaluate(() => ({ label: document.querySelector(".jk-keydial-label")?.textContent, core: window.__jkDebug.store.get().player?.key_label, semis: window.__jkDebug.store.get().songSettings?.semitones }));
      // Key through the real core: two steps up with the app-wide shortcut, one back with the dial.
      await page.keyboard.press("]");
      await page.keyboard.press("]");
      await page.waitForTimeout(400);
      const k2 = await page.evaluate(() => ({ label: document.querySelector(".jk-keydial-label")?.textContent, core: window.__jkDebug.store.get().player?.key_label, now: document.querySelector(".jk-dial")?.getAttribute("aria-valuenow") }));
      await page.locator(".jk-dial").focus();
      await page.keyboard.press("ArrowDown");
      await page.waitForTimeout(600); // the dial commits 160 ms after the last step
      const k1 = await page.evaluate(() => ({ label: document.querySelector(".jk-keydial-label")?.textContent, core: window.__jkDebug.store.get().player?.key_label, semis: window.__jkDebug.store.get().songSettings?.semitones }));
      check(
        "key shortcuts and the dial reach the core",
        k0.semis === 0 && /original/.test(k0.core ?? "") && /\(\+2\)/.test(k2.core ?? "") && k1.semis === 1 && /\(\+1\)/.test(k1.core ?? ""),
        `saved in the song file at start: ${saved.semis} ("${saved.label}"); dial 0: core "${k0.core}"; after ] ]: UI "${k2.label}", core "${k2.core}"; after dial down: UI "${k1.label}", core "${k1.core}"`,
      );
      await page.evaluate(() => document.activeElement?.blur()); // leave the dial
    } else {
      await page.locator(".skin-nav .jk-plate", { hasText: "Sing" }).click();
      await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 15000 });
    }
    // Seek just before the third line (10 s in the demo song) and play 1.3 s.
    const played = await page.evaluate(async () => {
      const dbg = window.__jkDebug;
      const input = document.querySelector("#seek");
      input.value = "9.2";
      input.dispatchEvent(new Event("change", { bubbles: true }));
      await new Promise((r) => setTimeout(r, 300));
      dbg.clockEvents.length = 0;
      document.querySelector(".skin-transport__play").click();
      await new Promise((r) => setTimeout(r, 1300));
      const ev = dbg.clockEvents.filter((e) => e.playing);
      return { state: dbg.store.get().player?.state, events: ev.length, from: ev[0]?.position_s, to: ev[ev.length - 1]?.position_s };
    });
    check(`${tag}: song plays (clock from the engine)`, played.state === "playing" && played.events > 20, `state ${played.state}, ${played.events} clock events, ${played.from?.toFixed(2)} -> ${played.to?.toFixed(2)} s`);
    await shot(`stage-${tag}`);

    if (vp.width === 1440) {
      // Ticker cost while the bubbles and the light drift run (5 s of playback).
      report.perf = await page.evaluate(async () => {
        const t = window.__jkTicker;
        const f0 = t.stats.frames;
        const d0 = t.stats.totalDrawMs;
        const b = window.__jkSkin?.bubbleStats;
        const b0 = b ? { frames: b.frames, ms: b.drawMsTotal } : null;
        t.stats.maxDrawMs = 0;
        const intervals = [];
        let last = performance.now();
        let run = true;
        const tick = (now) => {
          intervals.push(now - last);
          last = now;
          if (run) requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
        await new Promise((r) => setTimeout(r, 5000));
        run = false;
        intervals.sort((a, b) => a - b);
        const frames = t.stats.frames - f0;
        return {
          seconds: 5,
          tickerFps: frames / 5,
          avgDrawMs: (t.stats.totalDrawMs - d0) / Math.max(1, frames),
          maxDrawMs: t.stats.maxDrawMs,
          rafP50: intervals[Math.floor(intervals.length * 0.5)],
          rafP99: intervals[Math.floor(intervals.length * 0.99)],
          rafMax: intervals[intervals.length - 1],
          bubbles: b && b0 ? { fps: (b.frames - b0.frames) / 5, avgDrawMs: (b.drawMsTotal - b0.ms) / Math.max(1, b.frames - b0.frames), maxDrawMs: b.drawMsMax, alive: b.alive, running: b.running } : null,
          underruns: window.__jkDebug.store.get().player?.underruns ?? null,
        };
      });
      console.log("  perf:", JSON.stringify(report.perf));
      check("ticker keeps its frame rate with bubbles on", report.perf.tickerFps > 50 && report.perf.avgDrawMs < 4, `${report.perf.tickerFps.toFixed(1)} fps, draw avg ${report.perf.avgDrawMs.toFixed(3)} ms max ${report.perf.maxDrawMs.toFixed(2)} ms, rAF p50 ${report.perf.rafP50.toFixed(1)} ms p99 ${report.perf.rafP99.toFixed(1)} ms max ${report.perf.rafMax.toFixed(1)} ms`);
    }
    // Pause (the play plate) and run the letter-under-the-marker check on the live layout.
    await page.locator(".skin-transport__play").click();
    await page.waitForTimeout(300);
    const v = await page.evaluate(() => ({ ...window.__jkTicker.verify(), font: getComputedStyle(document.querySelector(".ticker")).getPropertyValue("--ticker-font").trim() }));
    report.ticker.push({ viewport: tag, ...v });
    check(
      `${tag}: letter under the marker (skin font, WebView2)`,
      v.wrongLetter === 0 && v.backwards === 0 && v.tooFast === 0 && v.lookaheadMisses === 0,
      `${v.sungSamples} sung samples, font ${v.fontPx.toFixed(1)} px (x${v.fontScale.toFixed(2)}) ${v.font.split(",")[0]}, min lookahead ${v.minLookaheadS.toFixed(2)} s ${v.firstProblems.join("; ")}`,
    );
  }
  check("no page errors", report.errors.length === 0, report.errors.slice(0, 5).join(" | ") || "none");
} catch (err) {
  check("run", false, String(err && err.stack ? err.stack : err));
} finally {
  await browser?.close().catch(() => undefined);
  child.kill();
}
writeFileSync(join(outDir, "real-app-screens.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 2));
console.log(`report: ${join(outDir, "real-app-screens.json")}`);
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
