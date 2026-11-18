// SPDX-License-Identifier: GPL-3.0-or-later
// Long playback soak through the REAL app (WORK_ORDER section 9 "Smooth playback", trap 11,
// 4.10 "Sustained performance" and "Memory"): the release exe from a portable copy, skin
// and bubbles on, the Stage (lyrics ticker) visible, a song played for N minutes on the
// DEFAULT output device at master volume 0 (digital silence: the whole pipeline runs and
// the device plays zeros), a key change every K seconds through the core's player_set
// command (the command the UI's key controls call), the song looped by a seek to 0 just
// before its end, and T busy CPU threads in a separate process. Results:
// docs/benchmarks/playback.md section 11.
//
//   node scripts/real-app-soak.mjs <Jukeoke.exe> <root> <out dir> <song title> [minutes] [load threads] [key every s]
//
// Records every second: player_state (state, position, underruns, device_starved, output
// peak). Every 10 s: player_diagnostics (worker lead, longest callback / worker block) and
// the page's frame statistics (ticker frames and draw time, bubbles, rAF intervals). A
// PowerShell sampler records working set and private bytes of Jukeoke.exe and every
// descendant process (WebView2, Studio) every 2 s. Writes soak.json and memory.csv.
//
// SAFETY: master volume 0 is set in Settings/settings.json before the app starts and is
// checked during the run (output_peak_db must stay at the -120 dB floor, i.e. digital zero).
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const [exe, root, outDir, title, minutesArg, threadsArg, keyEveryArg] = process.argv.slice(2);
if (!exe || !root || !outDir || !title) {
  console.error("usage: node scripts/real-app-soak.mjs <Jukeoke.exe> <root> <out dir> <song title> [minutes=20] [load threads=14] [key every s=30]");
  process.exit(2);
}
const minutes = Number(minutesArg ?? 20);
const loadThreads = Number(threadsArg ?? 14);
const keyEvery = Number(keyEveryArg ?? 30);
const KEYS = [3, -2, 5, 0, -7, 12, -12, 2];
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify(
    {
      ui: { skin: true, bubbles: true, motion: "full", lyrics_text_size: 56, marker_position: 0.33, colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" }, library_sort: "title", fullscreen_stage: false },
      // device_id null = the system default output; master 0 = digital silence. key_mode
      // realtime: the heavier path ("auto" also picks it on this PC once measured).
      audio: { device_id: null, exclusive: false, master_volume: 0, device_offsets_ms: {}, key_mode: "realtime" },
    },
    null,
    2,
  ),
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const t0 = Date.now();
const since = () => (Date.now() - t0) / 1000;
const port = 9300 + Math.floor(Math.random() * 500);
const child = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: ["ignore", "pipe", "pipe"] });
let exited = null;
child.on("exit", (code) => (exited = { code, at_s: since() }));
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

const report = { exe, root, title, minutes, loadThreads, keyEvery, started: new Date(t0).toISOString(), errors: [], keyChanges: [], loops: 0, samples: [], pageSamples: [], diag: [] };
let browser, page, burner, sampler;
const invoke = async (cmd, args = {}) => {
  const r = await page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a).then((ok) => ({ ok }), (err) => ({ err })), [cmd, args]);
  if ("err" in r) throw new Error(typeof r.err === "string" ? r.err : JSON.stringify(r.err));
  return r.ok;
};

// Busy threads in a separate process (so this driver stays responsive): each worker spins
// on integer work and reports its count when told to stop.
const BURNER = `
const { Worker } = require("node:worker_threads");
const n = Number(process.argv[1]);
let total = 0, done = 0;
const ws = [];
for (let i = 0; i < n; i++) {
  const w = new Worker("const { parentPort } = require('node:worker_threads'); let stop = false, k = 0; parentPort.on('message', () => { stop = true; }); (function spin() { const end = Date.now() + 50; let x = 0; while (Date.now() < end) { for (let j = 0; j < 1e5; j++) x = (x * 1103515245 + 12345) | 0; k++; } if (stop) { parentPort.postMessage(k); process.exit(0); } setImmediate(spin); })();", { eval: true });
  w.on("message", (k) => { total += k; if (++done === n) { console.log(JSON.stringify({ threads: n, work_units: total })); process.exit(0); } });
  ws.push(w);
}
process.stdin.on("data", () => ws.forEach((w) => w.postMessage("stop")));
`;

const percentile = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))];

try {
  for (let i = 0; ; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
      break;
    } catch (e) {
      if (i > 120 || exited) throw new Error(`no DevTools connection (${e}); exited: ${JSON.stringify(exited)}`);
      await sleep(500);
    }
  }
  const ctx = browser.contexts()[0];
  page = ctx.pages()[0] ?? (await ctx.waitForEvent("page"));
  page.on("pageerror", (e) => report.errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && report.errors.push(m.text()));
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForSelector(".jk-strip", { timeout: 60000 });
  report.window_ready_s = since();

  // Memory sampler over the whole process tree.
  const ps1 = fileURLToPath(new URL("./soak-memory.ps1", import.meta.url));
  sampler = spawn("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps1, String(child.pid), join(outDir, "memory.csv"), String(Math.ceil(minutes * 60 + 120)), "2"], { stdio: "ignore" });

  await page.locator(".skin-nav .jk-plate", { hasText: "Library" }).click();
  await sleep(500);
  await page.locator(".jk-strip", { hasText: title }).first().click();
  await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 30000 });
  await page.locator(".skin-nav .jk-plate", { hasText: "Sing" }).click();
  await sleep(800);
  const st0 = await invoke("player_state");
  report.song = { song_id: st0.song_id, duration_s: st0.duration_s, device: st0.device, key_mode: st0.key_mode };
  console.log(`song ${st0.song_id} ${st0.duration_s.toFixed(1)} s on ${JSON.stringify(st0.device)}; key mode ${st0.key_mode}`);

  // Frame statistics in the page: every rAF interval into a 1 ms histogram.
  await page.evaluate(() => {
    const s = (window.__soak = { hist: new Array(501).fill(0), n: 0, last: performance.now(), max: 0, run: true });
    const tick = (now) => {
      const d = now - s.last;
      s.last = now;
      s.n++;
      s.hist[Math.min(500, Math.floor(d))]++;
      if (d > s.max) s.max = d;
      if (s.run) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const pageStats = () =>
    page.evaluate(() => {
      const t = window.__jkTicker;
      const b = window.__jkSkin?.bubbleStats;
      return {
        ticker: t ? { frames: t.stats.frames, totalDrawMs: t.stats.totalDrawMs, maxDrawMs: t.stats.maxDrawMs } : null,
        bubbles: b ? { frames: b.frames, drawMsTotal: b.drawMsTotal, drawMsMax: b.drawMsMax, alive: b.alive, running: b.running } : null,
        raf: { n: window.__soak.n, max: window.__soak.max, hist: window.__soak.hist.slice() },
        visibility: document.visibilityState,
        screen: { w: window.innerWidth, h: window.innerHeight, dpr: window.devicePixelRatio },
      };
    });

  burner = spawn(process.execPath, ["-e", BURNER, String(loadThreads)], { stdio: ["pipe", "pipe", "inherit"] });
  let burnerOut = "";
  burner.stdout.on("data", (d) => (burnerOut += d));
  await sleep(1000);

  await page.locator(".skin-transport__play").click();
  const p0 = await pageStats();
  const startAt = Date.now();
  const endAt = startAt + minutes * 60 * 1000;
  let nextKey = startAt + keyEvery * 1000;
  let nextDiag = startAt + 10000;
  let ki = 0;
  let lastPage = p0;
  let maxPeakDb = -Infinity;
  while (Date.now() < endAt) {
    if (exited) throw new Error(`the app exited: ${JSON.stringify(exited)}`);
    const st = await invoke("player_state");
    const pk = Math.max(...(st.output_peak_db ?? [-120]));
    if (pk > maxPeakDb) maxPeakDb = pk;
    report.samples.push({ t: (Date.now() - startAt) / 1000, state: st.state, pos: st.position_s, underruns: st.underruns, device_starved: st.device_starved, peak_db: pk });
    if (pk > -100) throw new Error(`SAFETY: output peak ${pk} dBFS (expected digital silence); stopping`);
    if (st.state === "playing" && st.position_s > st.duration_s - 3) {
      await invoke("player_seek", { position_s: 0 });
      report.loops++;
    } else if (st.state === "ended" || st.state === "paused") {
      await invoke("player_play");
      report.errors.push(`state ${st.state} at ${since().toFixed(1)} s: play pressed again`);
    }
    if (Date.now() >= nextKey) {
      const k = KEYS[ki++ % KEYS.length];
      const s = await invoke("player_set", { changes: { semitones: k } });
      report.keyChanges.push({ t: (Date.now() - startAt) / 1000, semitones: k, effective: s.semitones });
      nextKey += keyEvery * 1000;
    }
    if (Date.now() >= nextDiag) {
      const d = await invoke("player_diagnostics");
      report.diag.push({ t: (Date.now() - startAt) / 1000, ...d });
      const ps = await pageStats();
      const dt = (ps.raf.n - lastPage.raf.n);
      report.pageSamples.push({ t: (Date.now() - startAt) / 1000, tickerFrames: ps.ticker.frames - lastPage.ticker.frames, bubbleFrames: ps.bubbles ? ps.bubbles.frames - lastPage.bubbles.frames : null, rafFrames: dt, visibility: ps.visibility, alive: ps.bubbles?.alive });
      lastPage = ps;
      nextDiag += 10000;
      const last = report.samples[report.samples.length - 1];
      console.log(`${((Date.now() - startAt) / 60000).toFixed(1)} min: ${st.state} ${st.position_s.toFixed(1)} s, underruns ${last.underruns}, starved ${last.device_starved}, min lead ${d?.min_lead_ms?.toFixed(1)} ms, ticker ${(report.pageSamples.at(-1).tickerFrames / 10).toFixed(1)} fps, bubbles ${((report.pageSamples.at(-1).bubbleFrames ?? 0) / 10).toFixed(1)} fps, ${ps.visibility}`);
    }
    await sleep(1000);
  }
  const elapsed = (Date.now() - startAt) / 1000;
  const p1 = await pageStats();
  const dEnd = await invoke("player_diagnostics");
  const stEnd = await invoke("player_state");
  burner.stdin.on("error", () => undefined);
  burner.stdin.write("stop\n");
  for (let i = 0; i < 50 && !burnerOut.trim(); i++) await sleep(100);
  report.load = burnerOut.trim() ? JSON.parse(burnerOut.trim()) : null;

  // Frame statistics over the whole run.
  const hist = p1.raf.hist.map((v, i) => v - p0.raf.hist[i]);
  const n = hist.reduce((a, b) => a + b, 0);
  const cum = (q) => {
    let c = 0;
    for (let i = 0; i < hist.length; i++) {
      c += hist[i];
      if (c >= q * n) return i;
    }
    return hist.length - 1;
  };
  const median = cum(0.5) + 0.5;
  let missed = 0;
  let over = { "25ms": 0, "33ms": 0, "50ms": 0, "100ms": 0 };
  hist.forEach((v, i) => {
    const d = i + 0.5;
    if (d > 1.5 * median) missed += v * Math.max(1, Math.round(d / median) - 1);
    if (i >= 25) over["25ms"] += v;
    if (i >= 33) over["33ms"] += v;
    if (i >= 50) over["50ms"] += v;
    if (i >= 100) over["100ms"] += v;
  });
  const tickerFrames = p1.ticker.frames - p0.ticker.frames;
  // Per window, over its real length (the windows are about, not exactly, 10 s: dividing
  // by 10 reported 54.5 fps for a 9.1 s window that ran at 60 fps).
  const tickerFps = report.pageSamples.map((s, i, a) => s.tickerFrames / Math.max(1e-3, s.t - (i > 0 ? a[i - 1].t : 0))).sort((a, b) => a - b);
  report.result = {
    elapsed_s: elapsed,
    key_changes: report.keyChanges.length,
    loops: report.loops,
    underruns: stEnd.underruns,
    device_starved: stEnd.device_starved,
    max_output_peak_db: maxPeakDb,
    engine: { min_lead_ms: dEnd.min_lead_ms, max_callback_us: dEnd.max_callback_us, worker_max_block_ms: dEnd.worker_max_block_ms, worker_busy_s: dEnd.worker_busy_s, callbacks: dEnd.callbacks, underrun_frames: dEnd.underrun_frames, reader_starved: dEnd.reader_starved, shortfall_frames: dEnd.shortfall_frames, inline_constructions: dEnd.inline_constructions, deferred_blocks: dEnd.deferred_blocks, device: dEnd.device, worker_priority: dEnd.worker_priority, device_drift_ppm: dEnd.device_drift_ppm, key_mode: stEnd.key_mode },
    // The ticker draws at most 60 frames per second (whole display periods); frames
    // missing against 60 per second are counted as dropped.
    ticker: { frames: tickerFrames, dropped_vs_60fps: Math.max(0, Math.round(60 * elapsed - tickerFrames)), fps_mean: tickerFrames / elapsed, fps_per_10s_min: tickerFps[0], fps_per_10s_p50: percentile(tickerFps, 0.5), draw_ms_mean: (p1.ticker.totalDrawMs - p0.ticker.totalDrawMs) / Math.max(1, tickerFrames), draw_ms_max: p1.ticker.maxDrawMs },
    bubbles: p1.bubbles && p0.bubbles ? { fps_mean: (p1.bubbles.frames - p0.bubbles.frames) / elapsed, draw_ms_mean: (p1.bubbles.drawMsTotal - p0.bubbles.drawMsTotal) / Math.max(1, p1.bubbles.frames - p0.bubbles.frames), draw_ms_max: p1.bubbles.drawMsMax, alive: p1.bubbles.alive } : null,
    raf: { frames: n, fps_mean: n / elapsed, interval_ms_p50: median, interval_ms_p99: cum(0.99) + 0.5, interval_ms_p999: cum(0.999) + 0.5, interval_ms_max: p1.raf.max, over, missed_vsyncs: missed },
    visibility_not_visible_samples: report.pageSamples.filter((s) => s.visibility !== "visible").length,
    load: report.load,
  };
  console.log("RESULT", JSON.stringify(report.result, null, 1));
} catch (err) {
  report.errors.push(String(err && err.stack ? err.stack : err));
  console.error(err);
} finally {
  if (burner && burner.exitCode === null) {
    burner.stdin.on("error", () => undefined);
    burner.stdin.write("stop\n");
  }
  setTimeout(() => burner?.kill(), 3000);
  await browser?.close().catch(() => undefined);
  await sleep(4000); // one more memory sample with the app still running
  child.kill();
  sampler?.kill();
}
writeFileSync(join(outDir, "soak.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 1));
console.log(`report: ${join(outDir, "soak.json")}`);
process.exit(report.result && report.errors.filter((e) => !e.startsWith("state ")).length === 0 ? 0 : 1);
