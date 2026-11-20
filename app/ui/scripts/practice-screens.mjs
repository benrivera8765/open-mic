// SPDX-License-Identifier: GPL-3.0-or-later
// Headless screenshots and measurements of practice mode (WO2 Stage 4), through the browser
// mock (src/api/mock.ts + mock-practice.ts), with a browser that is already installed
// (Microsoft Edge on Windows: Chromium, the WebView2 engine; or $JK_BROWSER). Run
// `npm run build` first.
//
//   node scripts/practice-screens.mjs [outDir]          (ONLY=<regex> limits the shots)
//
// Default outDir: $JK_BUILD/practice-ui/screens (outside the repository). Writes PNGs and
// report.json: page errors, horizontal overflow, the lane's whole-song check with the real
// fonts (window.__jkLane.verify) at each size, and the drawing cost of the lane and the
// lyrics together while a song plays (WO2 4.5 budget: 4 ms a frame on average).
import { createServer } from "node:http";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const here = fileURLToPath(new URL(".", import.meta.url));
const dist = join(here, "..", "dist");
const buildDir = process.env.JK_BUILD || join(here, "..", "..", "..", "..", "jukeoke-build");
const outDir = process.argv[2] ?? join(buildDir, "practice-ui", "screens");
mkdirSync(outDir, { recursive: true });
if (!existsSync(join(dist, "index.html"))) {
  console.error("dist/index.html missing: run `npm run build` first");
  process.exit(1);
}
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".woff2": "font/woff2", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json" };
const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  let path = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "");
  if (!path || path.endsWith("/") || path.endsWith("\\")) path = join(path, "index.html");
  const file = join(dist, path);
  if (!file.startsWith(dist) || !existsSync(file)) {
    res.writeHead(404).end("not found");
    return;
  }
  res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" }).end(readFileSync(file));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}/`;
const launch = process.env.JK_BROWSER ? { executablePath: process.env.JK_BROWSER } : { channel: "msedge" };
const browser = await chromium.launch({ ...launch, headless: true });
const version = browser.version();

const W1440 = { width: 1440, height: 900 };
const W1280 = { width: 1280, height: 800 };
const W4K = { width: 3840, height: 2160 };
const only = process.env.ONLY ? new RegExp(process.env.ONLY) : null;

/** Waits for the lane and lets the simulated singer sing for `ms`. */
const sing = (ms) => async (p) => {
  await p.waitForFunction(() => window.__jkLane && window.__jkLane.stats.frames > 0, null, { timeout: 10000 });
  await p.waitForTimeout(ms);
};
const result = async (p) => p.waitForSelector(".practice-result", { timeout: 40000 });
/** Changes settings in the page's store (as the Settings screen would). */
const setSettings = (patch) => async (p) =>
  p.evaluate((ch) => {
    const st = window.__jkDebug.store;
    const s = st.get().settings;
    st.set({ settings: { ...s, ui: { ...s.ui, ...(ch.ui ?? {}) }, practice: { ...s.practice, ...(ch.practice ?? {}) } } });
  }, patch);

const shots = [
  ["practice-1440x900", "?song=0&at=11.5&play=1&practice=1", W1440, sing(2600)],
  ["practice-1280x800", "?song=0&at=11.5&play=1&practice=1", W1280, sing(2600)],
  ["practice-3840x2160", "?song=0&at=11.5&play=1&practice=1", W4K, sing(2600)],
  ["practice-1440x900-retina", "?song=0&at=11.5&play=1&practice=1", { ...W1440, dpr: 2 }, sing(2600)],
  ["practice-piano-right-1440x900", "?song=0&at=30&play=1&practice=1", W1440, async (p) => {
    await sing(200)(p);
    await setSettings({ practice: { piano_side: "right" } })(p);
    await p.waitForTimeout(2400);
  }],
  ["practice-row-tuning-spoken-1440x900", "?song=2&at=27&play=1&practice=1", W1440, sing(2400)],
  ["practice-octave-down-1440x900", "?song=0&at=14&play=1&practice=1&singeroctave=-1", W1440, sing(9400)],
  ["practice-key-down-3-1440x900", "?song=0&at=11.5&play=1&practice=1", W1440, async (p) => {
    await sing(900)(p);
    for (let k = 0; k < 5; k++) await p.keyboard.press("[");
    await p.waitForTimeout(1600);
  }],
  ["practice-break-countdown-1440x900", "?song=0&at=37&play=1&practice=1", W1440, sing(800)],
  ["practice-watch-only-no-mic-1440x900", "?song=0&at=11.5&play=1&practice=1&mic=none", W1440, sing(1500)],
  ["practice-permission-macos-1440x900", "?platform=macos&song=0&at=11.5&practice=1", W1440, async (p) => p.waitForSelector(".practice-panel")],
  ["practice-denied-windows-privacy-1440x900", "?song=0&at=11.5&practice=1&micprivacy=off", W1440, sing(800)],
  ["practice-picker-1440x900", "?song=0&at=11.5&practice=1&picker=1", W1440, async (p) => {
    await p.waitForSelector(".practice-picker__channel");
    await p.waitForTimeout(400);
  }],
  ["practice-music-in-input-1440x900", "?song=0&at=11.5&play=1&practice=1&music=present", W1440, async (p) => p.waitForSelector(".practice-panel__box--warn", { timeout: 10000 })],
  ["practice-preparing-1440x900", "?song=1&at=8&practice=1", W1440, async (p) => p.waitForSelector(".practice-progress")],
  ["practice-not-ready-1440x900", "?song=1&at=8&practice=1&guidefail=1", W1440, async (p) => p.waitForSelector(".practice__add-data", { timeout: 10000 })],
  ["practice-simple-reduce-1440x900", "?song=0&at=11.5&play=1&practice=1", W1440, async (p) => {
    await sing(400)(p);
    await setSettings({ ui: { motion: "reduce" }, practice: { simple_lane: true } })(p);
    await p.waitForTimeout(1800);
  }],
  ["practice-lane-only-1440x900", "?song=0&at=11.5&play=1&practice=1", W1440, async (p) => {
    await sing(200)(p);
    await setSettings({ practice: { lyrics_below: false } })(p);
    await p.waitForTimeout(2200);
  }],
  ["practice-result-1440x900", "?song=0&at=0&play=1&practice=1&speed=12", W1440, result],
  ["practice-result-1280x800", "?song=0&at=0&play=1&practice=1&speed=12", W1280, result],
  ["practice-result-3840x2160", "?song=0&at=0&play=1&practice=1&speed=12", W4K, result],
  ["practice-result-music-1440x900", "?song=0&at=0&play=1&practice=1&speed=12&music=marginal", W1440, async (p) => {
    await p.waitForSelector(".practice-panel__box--warn", { timeout: 15000 });
    await p.getByRole("button", { name: "Got it" }).click();
    await result(p);
  }],
  ["practice-plain-1440x900", "?skin=0&song=0&at=11.5&play=1&practice=1", W1440, sing(2600)],
];

const report = { browser: `msedge/chromium ${version}`, shots: [], problems: [], verify: [], cost: [] };
for (const [name, query, vp, steps] of shots) {
  if (only && !only.test(name)) continue;
  const ctx = await browser.newContext({ viewport: { width: vp.width, height: vp.height }, deviceScaleFactor: vp.dpr ?? 1, reducedMotion: "no-preference" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto(base + query);
  await page.waitForSelector(".app-main, .app--fatal", { timeout: 10000 });
  await page.waitForTimeout(500);
  try {
    if (steps) await steps(page);
  } catch (e) {
    errors.push(`steps: ${e}`);
  }
  await page.waitForTimeout(150);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  const lane = await page.evaluate(() => (window.__jkLane && document.querySelector(".practice-lane") ? window.__jkLane.layout() : null));
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  report.shots.push({ name, file, viewport: vp, errors, horizontalOverflowPx: overflow, lane });
  if (errors.length) report.problems.push(`${name}: ${errors.join(" | ")}`);
  if (overflow > 0) report.problems.push(`${name}: horizontal overflow ${overflow}px`);
  console.log(
    `${name.padEnd(44)} ${errors.length ? "ERRORS " + errors.join(" | ") : "ok"}${lane ? ` lane ${lane.width}x${lane.height} rows ${lane.rows.toFixed(1)} rowH ${lane.rowH.toFixed(1)} bar ${lane.barH.toFixed(1)} head ${lane.headH.toFixed(1)}` : ""}`,
  );
  await ctx.close();
}

// --- the lane's whole-song check with Edge's own fonts, at each size and key ---------------
if (!only || only.test("verify")) {
  for (const vp of [W1280, W1440, W4K]) {
    const ctx = await browser.newContext({ viewport: vp, deviceScaleFactor: 1 });
    const page = await ctx.newPage();
    await page.goto(base + "?song=0&at=1&practice=1");
    await page.waitForFunction(() => window.__jkLane && window.__jkLane.stats.frames > 0, null, { timeout: 10000 });
    for (const sc of [
      { key: 0, octave: 0, spelling: "sharps" },
      { key: -5, octave: 0, spelling: "sharps" },
      { key: 3, octave: 0, spelling: "sharps" },
      { key: -3, octave: -1, spelling: "sharps" },
    ]) {
      const r = await page.evaluate((s) => window.__jkLane.verify(s), sc);
      const bad = r.barErrors + r.nameErrors + r.wordErrors + r.targetErrors + r.markerErrors + r.cueErrors + r.wordOverlaps + r.outOfView;
      report.verify.push({ viewport: `${vp.width}x${vp.height}`, ...sc, ...r });
      console.log(
        `verify ${vp.width}x${vp.height} key ${sc.key} oct ${sc.octave}: bars ${r.barsChecked} (worst ${r.worstBarPx.toFixed(2)} px), words ${r.wordsChecked}, keys ${r.targetChecked}, marker ${r.markerChecked}; errors ${bad}; lookahead ${r.lookaheadS.toFixed(2)} s, now at ${(r.nowFraction * 100).toFixed(0)} %, rows ${r.rows.toFixed(1)}`,
      );
      if (bad) report.problems.push(`verify ${vp.width}x${vp.height} ${JSON.stringify(sc)}: ${r.firstErrors.join("; ")}`);
    }
    await ctx.close();
  }
}

// --- drawing cost: lane + lyrics while the song plays with the simulated singer ------------
// Three measures per size (docs/benchmarks/practice-screen.md):
//   - the frame loops' own time (lane + lyrics: frame function and drawing commands);
//   - the page's whole main-thread busy time per frame while it plays (CDP Performance
//     metrics, TaskDuration: script, style, layout, paint and the canvases' flush), with
//     practice on and, as the baseline, the ordinary singing screen with practice off;
//   - the lane alone rasterised on the CPU inside the timed span (lanecost.ts).
// The software rows run a second browser with the GPU switched off (Skia CPU raster for
// everything), the worst case for a machine whose web view draws canvases on the CPU.
if (!only || only.test("cost")) {
  const soft = await chromium.launch({ ...launch, headless: true, args: ["--disable-gpu", "--disable-gpu-compositing", "--disable-accelerated-2d-canvas"] });
  for (const [label, vp, dpr, simple, practice, b] of [
    ["1440x900", W1440, 1, false, true, browser],
    ["1440x900 practice off", W1440, 1, false, false, browser],
    ["1440x900@2", W1440, 2, false, true, browser],
    ["1280x800", W1280, 1, false, true, browser],
    ["3840x2160", W4K, 1, false, true, browser],
    ["1440x900 simple lane", W1440, 1, true, true, browser],
    ["1440x900 software", W1440, 1, false, true, soft],
    ["1440x900 software practice off", W1440, 1, false, false, soft],
    ["1440x900@2 software", W1440, 2, false, true, soft],
    ["1440x900@2 software simple", W1440, 2, true, true, soft],
  ]) {
    const ctx = await b.newContext({ viewport: vp, deviceScaleFactor: dpr });
    const page = await ctx.newPage();
    await page.goto(base + `?song=0&at=9&play=1${practice ? "&practice=1" : ""}`);
    await page.waitForFunction((pr) => (!pr || (window.__jkLane && window.__jkLane.stats.frames > 0)) && window.__jkTicker, practice, { timeout: 10000 });
    if (simple) await setSettings({ practice: { simple_lane: true } })(page);
    await page.waitForTimeout(600);
    const cdp = await ctx.newCDPSession(page);
    await cdp.send("Performance.enable");
    const busy = async () => (await cdp.send("Performance.getMetrics")).metrics.find((m) => m.name === "TaskDuration").value;
    const busy0 = await busy();
    const r = await page.evaluate(async (pr) => {
      const lane = pr ? window.__jkLane.stats : { frames: 0, totalDrawMs: 0, maxDrawMs: 0 };
      const lyr = window.__jkTicker.stats;
      const b = { lf: lane.frames, ld: lane.totalDrawMs, yf: lyr.frames, yd: lyr.totalDrawMs };
      lane.maxDrawMs = 0;
      const intervals = [];
      let last = performance.now();
      let run = true;
      const tick = (now) => {
        intervals.push(now - last);
        last = now;
        if (run) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      await new Promise((res) => setTimeout(res, 8000));
      run = false;
      const lf = lane.frames - b.lf;
      const yf = lyr.frames - b.yf;
      const frames = intervals.length;
      intervals.sort((x, y) => x - y);
      return {
        displayFrames: frames,
        laneFrames: lf,
        laneAvgMs: (lane.totalDrawMs - b.ld) / Math.max(1, lf),
        laneMaxMs: lane.maxDrawMs,
        lyricsFrames: yf,
        lyricsAvgMs: (lyr.totalDrawMs - b.yd) / Math.max(1, yf),
        // The drawing loops' own rate (capped at 60) and the browser's display rate.
        fps: (pr ? lf : yf) / 8,
        displayFps: frames / 8,
        medianIntervalMs: intervals[Math.floor(intervals.length / 2)],
        over25: intervals.filter((x) => x > 25).length,
      };
    }, practice);
    const busy1 = await busy();
    // Per frame of the drawing loops (capped at 60 fps), which is what the 4 ms budget is
    // about. Headless Chromium runs requestAnimationFrame at about 170 Hz with the GPU on,
    // so dividing by the display's frames would understate the cost about 2.9 times.
    r.mainThreadMsPerFrame = ((busy1 - busy0) * 1000) / Math.max(1, practice ? r.laneFrames : r.lyricsFrames);
    r.mainThreadMsPerDisplayFrame = ((busy1 - busy0) * 1000) / Math.max(1, r.displayFrames);
    if (practice) {
      Object.assign(
        r,
        await page.evaluate(() => ({
          bench: window.__jkLane.bench(20, 600, 1 / 60),
          benchSimple: window.__jkLane.bench(20, 600, 1 / 60, true),
          // Each frame rasterised on the CPU inside the timed span (lanecost.ts): an upper bound.
          benchRaster: window.__jkLane.bench(20, 600, 1 / 60, undefined, true),
          benchRasterSimple: window.__jkLane.bench(20, 600, 1 / 60, true, true),
        })),
      );
    }
    const both = r.laneAvgMs + r.lyricsAvgMs;
    report.cost.push({ label, ...r, bothAvgMs: both });
    console.log(
      `cost ${label.padEnd(32)} loops: lane ${r.laneAvgMs.toFixed(2)} + lyrics ${r.lyricsAvgMs.toFixed(2)} = ${both.toFixed(2)} ms; main thread ${r.mainThreadMsPerFrame.toFixed(2)} ms/frame; loops ${r.fps.toFixed(1)} fps (display ${r.displayFps.toFixed(0)}), ${r.over25} display frames > 25 ms` +
        (practice ? `; lane CPU-rasterised avg ${r.benchRaster.avg.toFixed(2)} p95 ${r.benchRaster.p95.toFixed(2)}, simple ${r.benchRasterSimple.avg.toFixed(2)}` : ""),
    );
    if (both > 4) report.problems.push(`cost ${label}: ${both.toFixed(2)} ms a frame (budget 4)`);
    // The budget applies at the WO2 window sizes (1440x900, 1280x800); 4K and Retina are reported.
    if (practice && r.benchRaster.avg > 4 && dpr === 1 && !label.startsWith("3840")) report.problems.push(`cost ${label}: the lane alone takes ${r.benchRaster.avg.toFixed(2)} ms a frame rasterised on the CPU (budget 4 with the lyrics)`);
    await ctx.close();
  }
  await soft.close();
}

writeFileSync(join(outDir, "report.json"), JSON.stringify(report, null, 2));
await browser.close();
server.close();
console.log(report.problems.length ? `PROBLEMS:\n${report.problems.join("\n")}` : "OK: no problems");
process.exit(report.problems.length ? 1 : 0);
