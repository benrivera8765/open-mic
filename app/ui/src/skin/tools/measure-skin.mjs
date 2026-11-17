// SPDX-License-Identifier: GPL-3.0-or-later
// Performance evidence and screenshots for the jukebox skin (WORK_ORDER 4.7:
// "keep idle CPU and GPU use low, cap the frame rate, pause animation when the
// window is hidden").
//
// 1. Builds skin-demo.html with the app's own Vite config (production build) into
//    <out>/dist, and serves it from a throwaway local HTTP server.
// 2. Opens it in headless Chromium (Playwright) at 1440x900 @2x (the MacBook Air)
//    and measures, per scenario, over a fixed window:
//      - renderer main-thread busy time (CDP Performance.getMetrics TaskDuration),
//      - CPU time of the whole browser process tree (Windows: Win32_Process times),
//      - bubble frames drawn per second and draw time per frame (bubbleStats),
//      - requestAnimationFrame interval distribution (frame pacing),
//      - whether the light-drift animations are running.
//    Scenarios: bubbles on / bubbles off / motion reduce / page hidden.
// 3. Takes screenshots at 1440x900, 1280x800 (@2x) and 3840x2160 (@1x).
//
// Usage (from app/ui; SKIP_SHOTS=1 skips the screenshots):
//   node src/skin/tools/measure-skin.mjs <playwright-core dir> <out dir> [seconds] [gpu|software|both]
//   (seconds = 0: only build the demo into <out dir>/dist)
// e.g. node src/skin/tools/measure-skin.mjs "$JK_BUILD/tools/skin-playwright" "$JK_BUILD/screenshots/skin" 15
// Needs Playwright's Chromium (npx playwright-core install chromium) in the
// per-user browser cache. Not part of the app build; nothing here ships.

import { execFileSync } from "node:child_process";
import { createReadStream, existsSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const uiRoot = resolve(here, "..", "..", "..");
const [pwDir, outArg, secondsArg, modeArg] = process.argv.slice(2);
if (!pwDir || !outArg) {
  console.error("usage: node measure-skin.mjs <playwright-core dir> <out dir> [seconds]");
  process.exit(2);
}
const outDir = resolve(outArg);
const SECONDS = Number(secondsArg ?? 15);
// gpu: full Chromium in new headless mode, which composites on the real GPU (like
// WebView2 does). software: headless shell with SwiftShader (no GPU: worst case).
const MODES = modeArg === "gpu" || modeArg === "software" ? [modeArg] : ["gpu", "software"];
const launchOptions = (mode) => ({ headless: true, channel: mode === "gpu" ? "chromium" : undefined });
mkdirSync(outDir, { recursive: true });

const require = createRequire(join(resolve(pwDir), "package.json"));
const { chromium } = require("playwright-core");

// ---------------------------------------------------------------- build
const distDir = join(outDir, "dist");
{
  const { build } = await import("vite");
  await build({
    root: uiRoot,
    configFile: join(uiRoot, "vite.config.ts"),
    logLevel: "warn",
    build: {
      outDir: distDir,
      emptyOutDir: true,
      rollupOptions: { input: { demo: join(uiRoot, "skin-demo.html") } },
    },
  });
}
if (SECONDS === 0) {
  // Build only (e.g. to run scripts/check-dist-syntax.mjs and check-dist-css.mjs on it).
  console.log(distDir);
  process.exit(0);
}

// ---------------------------------------------------------------- static server
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".woff2": "font/woff2", ".svg": "image/svg+xml", ".png": "image/png" };
const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://x");
  const file = join(distDir, decodeURIComponent(url.pathname));
  if (!file.startsWith(distDir) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}/skin-demo.html`;

// ---------------------------------------------------------------- helpers
/** CPU seconds (user + kernel) of a process and all its descendants (Windows). */
function treeCpuSeconds(rootPid) {
  if (process.platform !== "win32") return null;
  const json = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,UserModeTime,KernelModeTime,CommandLine | ConvertTo-Json -Compress"],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  const rows = JSON.parse(json);
  const children = new Map();
  for (const r of rows) {
    if (!children.has(r.ParentProcessId)) children.set(r.ParentProcessId, []);
    children.get(r.ParentProcessId).push(r);
  }
  const byPid = new Map(rows.map((r) => [r.ProcessId, r]));
  let total = 0;
  let count = 0;
  /** CPU seconds per Chromium process type (browser, renderer, gpu-process, utility). */
  const byType = {};
  const stack = [byPid.get(rootPid)].filter(Boolean);
  while (stack.length) {
    const r = stack.pop();
    const t = Number(r.UserModeTime) + Number(r.KernelModeTime);
    total += t;
    count++;
    const type = /--type=([a-z-]+)/.exec(r.CommandLine ?? "")?.[1] ?? "browser";
    byType[type] = (byType[type] ?? 0) + t / 1e7;
    for (const c of children.get(r.ProcessId) ?? []) stack.push(c);
  }
  return { seconds: total / 1e7, processes: count, byType };
}

function metric(metrics, name) {
  return metrics.metrics.find((m) => m.name === name)?.value ?? 0;
}

async function frameStats(page, ms) {
  return page.evaluate(async (ms) => {
    const gaps = [];
    let last = performance.now();
    const end = last + ms;
    await new Promise((done) => {
      const tick = (t) => {
        gaps.push(t - last);
        last = t;
        if (t < end) requestAnimationFrame(tick);
        else done();
      };
      requestAnimationFrame(tick);
    });
    gaps.shift();
    gaps.sort((a, b) => a - b);
    const q = (p) => gaps[Math.min(gaps.length - 1, Math.floor(p * gaps.length))];
    return { frames: gaps.length, p50: q(0.5), p95: q(0.95), p99: q(0.99), max: gaps[gaps.length - 1] };
  }, ms);
}

async function animationsState(page) {
  return page.evaluate(() => {
    const anims = document.getAnimations().filter((a) => a.effect && a.effect.target && a.effect.target.hasAttribute("data-jk-drift"));
    return { drift: anims.length, running: anims.filter((a) => a.playState === "running").length, paused: anims.filter((a) => a.playState === "paused").length };
  });
}

async function measureScenario(mode, name, query, { hide = false } = {}) {
  // hide = "minimize": a real window, minimised. Needs a headed Chromium, parked
  //   off screen (nobody sees it; native occlusion tracking is off so it counts as
  //   visible until it is minimised). Playwright's focus emulation keeps
  //   document.visibilityState at "visible" there, but the browser stops frames.
  // hide = "simulate": headless; dispatches a visibilitychange to "hidden", which is
  //   what WKWebView / WebView2 deliver when the host hides the view. Tests the
  //   skin's own handler.
  const options = hide === "minimize"
    ? {
        headless: false,
        channel: "chromium",
        args: [
          "--window-position=-32000,-32000",
          "--window-size=1440,900",
          "--disable-features=CalculateNativeWinOcclusion",
          ...(mode === "software" ? ["--disable-gpu"] : []),
        ],
      }
    : launchOptions(mode);
  const srv = await chromium.launchServer(options);
  const pid = srv.process().pid;
  const browser = await chromium.connect(srv.wsEndpoint());
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}?hud=0&${query}`, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  const renderer = await page.evaluate(() => {
    const gl = document.createElement("canvas").getContext("webgl");
    const ext = gl && gl.getExtension("WEBGL_debug_renderer_info");
    return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : "unknown";
  });
  await page.waitForTimeout(3000); // settle: fonts, first rasterisation, prewarmed bubbles

  let hiddenHow = null;
  let visibleBeforeHide = null;
  if (hide) {
    visibleBeforeHide = await page.evaluate(() => document.visibilityState);
    if (hide === "minimize") {
      const cdp = await context.newCDPSession(page);
      const { windowId } = await cdp.send("Browser.getWindowForTarget");
      await cdp.send("Browser.setWindowBounds", { windowId, bounds: { windowState: "minimized" } });
      const { bounds } = await cdp.send("Browser.getWindowBounds", { windowId });
      hiddenHow = `real window minimised (window state: ${bounds.windowState})`;
    } else {
      await page.evaluate(() => {
        Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
        Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
        document.dispatchEvent(new Event("visibilitychange"));
      });
      hiddenHow = "visibilitychange to hidden (dispatched in the page)";
    }
    await page.waitForTimeout(1000);
  }

  const cdp = await context.newCDPSession(page);
  await cdp.send("Performance.enable");
  // Each counter is divided by its own time base: page clock for the bubble
  // counters, the metrics' Timestamp for main-thread time, and the midpoint of the
  // (slow, ~0.5 s) process-time queries for the process CPU.
  const stats0 = await page.evaluate(() => ({ ...window.__jkSkin.bubbleStats, t: performance.now() }));
  const m0 = await cdp.send("Performance.getMetrics");
  const a0 = Date.now();
  const c0 = treeCpuSeconds(pid);
  const t0 = (a0 + Date.now()) / 2;
  await new Promise((r) => setTimeout(r, SECONDS * 1000));
  const a1 = Date.now();
  const c1 = treeCpuSeconds(pid);
  const t1 = (a1 + Date.now()) / 2;
  const m1 = await cdp.send("Performance.getMetrics");
  const stats1 = await page.evaluate(() => ({ ...window.__jkSkin.bubbleStats, t: performance.now() }));
  const wall = (t1 - t0) / 1000;
  const metricWall = metric(m1, "Timestamp") - metric(m0, "Timestamp");
  const pageWall = (stats1.t - stats0.t) / 1000;

  const anim = await animationsState(page);
  const visibility = await page.evaluate(() => document.visibilityState);
  const frames = hide ? null : await frameStats(page, 5000);
  const bubbleFrames = stats1.frames - stats0.frames;
  const result = {
    mode,
    scenario: name,
    query,
    renderer,
    visibility,
    hiddenHow,
    visibleBeforeHide,
    seconds: wall,
    mainThreadBusyPct: (100 * (metric(m1, "TaskDuration") - metric(m0, "TaskDuration"))) / metricWall,
    scriptPct: (100 * (metric(m1, "ScriptDuration") - metric(m0, "ScriptDuration"))) / metricWall,
    stylePlusLayoutPct:
      (100 * (metric(m1, "RecalcStyleDuration") - metric(m0, "RecalcStyleDuration") + metric(m1, "LayoutDuration") - metric(m0, "LayoutDuration"))) / metricWall,
    layoutsPerSecond: (metric(m1, "LayoutCount") - metric(m0, "LayoutCount")) / metricWall,
    browserTreeCpuPctOfOneCore: c0 && c1 ? (100 * (c1.seconds - c0.seconds)) / wall : null,
    browserProcesses: c1?.processes ?? null,
    cpuPctByProcessType:
      c0 && c1 ? Object.fromEntries(Object.keys(c1.byType).map((k) => [k, (100 * (c1.byType[k] - (c0.byType[k] ?? 0))) / wall])) : null,
    bubbleFps: bubbleFrames / pageWall,
    bubbleDrawMsAvg: bubbleFrames > 0 ? (stats1.drawMsTotal - stats0.drawMsTotal) / bubbleFrames : 0,
    bubbleDrawMsMax: stats1.drawMsMax,
    bubblesAlive: stats1.alive,
    driftAnimations: anim,
    rafIntervalsMs: frames,
    jsHeapMB: metric(m1, "JSHeapUsedSize") / 1048576,
    errors,
  };
  await browser.close();
  await srv.close();
  return result;
}

async function screenshots() {
  const browser = await chromium.launch(launchOptions("gpu"));
  const shots = [];
  const sizes = [
    { w: 1440, h: 900, dsf: 2 },
    { w: 1280, h: 800, dsf: 2 },
    { w: 3840, h: 2160, dsf: 1 },
  ];
  for (const { w, h, dsf } of sizes) {
    for (const view of ["library", "stage", "gallery"]) {
      const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: dsf });
      await page.goto(`${base}?hud=0&motion=full&view=${view}`, { waitUntil: "load" });
      await page.evaluate(() => document.fonts.ready);
      await page.waitForTimeout(2500);
      const file = join(outDir, `skin-${w}x${h}-${view}.png`);
      await page.screenshot({ path: file });
      shots.push(file);
      await page.close();
    }
  }
  // Reduced motion (static light, no bubbles) at the MacBook size.
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
  await page.goto(`${base}?hud=0&motion=reduce&view=stage`, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(1500);
  const file = join(outDir, "skin-1440x900-stage-reduce-motion.png");
  await page.screenshot({ path: file });
  shots.push(file);
  await browser.close();
  return shots;
}

const results = [];
try {
  for (const mode of MODES) {
    results.push(await measureScenario(mode, "bubbles on, motion full (stage)", "view=stage&motion=full&bubbles=1"));
    results.push(await measureScenario(mode, "bubbles off, motion full (stage)", "view=stage&motion=full&bubbles=0"));
    results.push(await measureScenario(mode, "motion reduce (stage)", "view=stage&motion=reduce"));
    results.push(await measureScenario(mode, "window minimised, bubbles on (stage)", "view=stage&motion=full&bubbles=1", { hide: "minimize" }));
    results.push(await measureScenario(mode, "visibility hidden, bubbles on (stage)", "view=stage&motion=full&bubbles=1", { hide: "simulate" }));
  }
  const shots = process.env.SKIP_SHOTS ? [] : await screenshots();
  const report = { when: new Date().toISOString(), seconds: SECONDS, viewport: "1440x900 @2x", results, screenshots: shots };
  writeFileSync(join(outDir, "measure.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
} finally {
  server.close();
}
