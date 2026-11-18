// SPDX-License-Identifier: GPL-3.0-or-later
// Headless screenshots of the main screens, driven through the browser mock
// (src/api/mock.ts), plus a short ticker performance and layout check. The mock
// starts with the jukebox skin on (the app default); the "plain-*" shots use ?skin=0.
//
// Uses playwright-core with a browser that is already installed (nothing is
// downloaded): Microsoft Edge on Windows (Chromium, the same engine as WebView2), or
// any Chromium given in $JK_BROWSER. Run `npm run build` first.
//
//   node scripts/screenshots.mjs [outDir]
//
// Default outDir: $JK_BUILD/ui-screenshots (outside the repository). Writes one PNG
// per shot and report.json; exits 1 when a page logs an error, a layout overflows
// horizontally, or the ticker misses its frame budget.
import { createServer } from "node:http";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const here = fileURLToPath(new URL(".", import.meta.url));
const dist = join(here, "..", "dist");
const buildDir = process.env.JK_BUILD || join(here, "..", "..", "..", "..", "jukeoke-build");
const outDir = process.argv[2] ?? join(buildDir, "ui-screenshots");
mkdirSync(outDir, { recursive: true });
if (!existsSync(join(dist, "index.html"))) {
  console.error("dist/index.html missing: run `npm run build` first");
  process.exit(1);
}

// --- a tiny static server for dist/ ------------------------------------------
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

// --- browser -------------------------------------------------------------------
const launch = process.env.JK_BROWSER ? { executablePath: process.env.JK_BROWSER } : { channel: "msedge" };
const browser = await chromium.launch({ ...launch, headless: true });
const version = browser.version();

/** Screens to capture: [name, query, viewport, extra steps]. */
const W1440 = { width: 1440, height: 900 };
const W1280 = { width: 1280, height: 800 };
const W4K = { width: 3840, height: 2160 };
const shots = [
  ["library-1440x900", "", W1440],
  ["library-1280x800", "", W1280],
  ["library-3840x2160", "", W4K],
  ["stage-1440x900", "?song=0&at=10.4", W1440],
  ["stage-1280x800", "?song=0&at=10.4", W1280],
  ["stage-3840x2160", "?song=0&at=10.4", W4K],
  ["stage-1440x900-retina", "?song=0&at=10.4", { ...W1440, dpr: 2 }],
  ["stage-break-countdown-1440x900", "?song=0&at=40", W1440],
  // The page display (default since D-218): a lead-in bar before a line, a line change.
  ["stage-lead-in-1440x900", "?song=0&at=79", W1440],
  ["stage-line-change-1440x900", "?song=0&at=12.5", W1440],
  // The scrolling ticker (settings.ui.lyrics_display "scroll").
  ["stage-scroll-1440x900", "?lyricsdisplay=scroll&song=0&at=10.4", W1440],
  ["stage-scroll-break-countdown-1440x900", "?lyricsdisplay=scroll&song=0&at=37.5", W1440],
  ["stage-intro-countdown-1440x900", "?song=0&at=2", W1440],
  ["stage-accents-1440x900", "?song=1&at=11", W1440],
  ["stage-fullscreen-1440x900", "?song=0&at=10.4&fullscreen=1", W1440],
  ["stage-fullscreen-1280x800", "?song=0&at=10.4&fullscreen=1", W1280],
  ["stage-mixer-1440x900", "?song=0&at=10.4", W1440, async (p) => p.getByRole("button", { name: "Mixer" }).click()],
  ["sync-editor-1440x900", "?song=0&at=10.4&screen=sync", W1440],
  ["recorder-windows-1440x900", "?screen=recorder", W1440],
  ["recorder-recording-1440x900", "?screen=recorder", W1440, async (p) => { await p.getByRole("button", { name: "Start recording" }).click(); await p.waitForTimeout(1200); }],
  ["recorder-macos-permission-1440x900", "?platform=macos&screen=recorder", W1440],
  ["studio-1440x900", "?screen=studio", W1440, async (p) => { await p.getByRole("button", { name: "Choose files…" }).click(); await p.waitForTimeout(300); }],
  ["studio-queue-1440x900", "?screen=studio", W1440, async (p) => {
    await p.getByRole("button", { name: "Choose files…" }).click();
    await p.getByRole("button", { name: "Add to queue" }).click();
    await p.waitForTimeout(1500);
  }],
  ["studio-unavailable-1440x900", "?studio=off&screen=studio", W1440],
  ["studio-macos-1440x900", "?platform=macos&screen=studio", W1440],
  ["settings-1440x900", "?screen=settings", W1440],
  ["help-overlay-1440x900", "?overlay=help", W1440],
  ["selfcheck-1440x900", "?overlay=selfcheck", W1440, async (p) => p.waitForSelector(".selfcheck__summary", { timeout: 5000 })],
  ["library-empty-1280x800", "?empty=1", W1280],
  // Deleting songs (the owner's request of 18 Nov 2026, D-230): a strip's trash plate,
  // the question, Select, Deleted songs, and the second question before "for good".
  ["library-delete-plate-1440x900", "", W1440, async (p) => p.locator(".library__strip").nth(0).hover()],
  ["library-delete-confirm-1440x900", "", W1440, async (p) => {
    await p.locator(".library__strip").nth(0).hover();
    await p.locator(".library__strip").nth(0).locator(".library__trash").click();
    await p.waitForSelector("dialog.confirm");
  }],
  ["library-select-1440x900", "", W1440, async (p) => {
    await p.locator(".library__select").click();
    for (const i of [0, 3]) await p.locator(".library__strip .jk-strip").nth(i).click();
  }],
  ["library-deleted-songs-1440x900", "", W1440, async (p) => {
    await p.locator(".library__select").click();
    for (const i of [1, 2, 3]) await p.locator(".library__strip .jk-strip").nth(i).click();
    await p.locator(".library__delete-ticked").click();
    await p.locator(".confirm__ok").click();
    await p.locator(".library__show-deleted").click();
    await p.waitForSelector(".deleted-song");
  }],
  ["library-purge-confirm-1440x900", "", W1440, async (p) => {
    await p.locator(".library__strip").nth(1).locator(".library__trash").click({ force: true });
    await p.locator(".confirm__ok").click();
    await p.locator(".library__show-deleted").click();
    await p.locator(".deleted-song__purge").first().click();
    await p.waitForSelector("dialog.confirm");
  }],
  // The plain look (settings.ui.skin off); everything above runs with the jukebox skin.
  ["plain-library-1440x900", "?skin=0", W1440],
  ["plain-stage-1440x900", "?skin=0&song=0&at=10.4", W1440],
  ["plain-stage-fullscreen-1280x800", "?skin=0&song=0&at=10.4&fullscreen=1", W1280],
  ["plain-settings-1440x900", "?skin=0&screen=settings", W1440],
  ["plain-library-delete-confirm-1440x900", "?skin=0", W1440, async (p) => {
    await p.locator(".title-strip__row").nth(0).locator(".library__trash").click();
    await p.waitForSelector("dialog.confirm");
  }],
];

const report = { browser: `msedge/chromium ${version}`, shots: [], problems: [], ticker: null };

for (const [name, query, vp, steps] of shots) {
  const ctx = await browser.newContext({ viewport: { width: vp.width, height: vp.height }, deviceScaleFactor: vp.dpr ?? 1, reducedMotion: "no-preference" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto(base + query);
  await page.waitForSelector(".app-main, .app--fatal", { timeout: 10000 });
  await page.waitForTimeout(500); // covers, fonts, the mock's simulated latency
  if (steps) await steps(page);
  await page.waitForTimeout(200);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  const shot = { name, file, viewport: vp, errors, horizontalOverflowPx: overflow };
  report.shots.push(shot);
  if (errors.length) report.problems.push(`${name}: page errors: ${errors.join(" | ")}`);
  if (overflow > 0) report.problems.push(`${name}: horizontal overflow ${overflow}px`);
  console.log(`${name.padEnd(40)} ${errors.length ? "ERRORS" : "ok"}${overflow > 0 ? ` overflow ${overflow}px` : ""}`);
  await ctx.close();
}

// --- the layout's own check with real fonts (Edge's text measurement), both displays ---
report.realFontTicker = [];
for (const display of ["page", "scroll"]) for (const vp of [W1280, W1440, W4K]) {
  for (const song of [0, 1, 2]) {
    const ctx = await browser.newContext({ viewport: vp, deviceScaleFactor: 1 });
    const page = await ctx.newPage();
    await page.goto(base + `?lyricsdisplay=${display}&song=${song}&at=1`);
    await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 10000 });
    const r = await page.evaluate(() => {
      const v = window.__jkTicker.verify();
      return { ...v, title: document.querySelector(".stage__title")?.textContent };
    });
    const row = { display, viewport: `${vp.width}x${vp.height}`, ...r };
    report.realFontTicker.push(row);
    console.log(
      `real fonts ${display.padEnd(6)} ${row.viewport.padEnd(10)} ${String(row.title).padEnd(30)} sung samples ${row.sungSamples}, wrong letter ${row.wrongLetter}, backwards ${row.backwards}, too fast ${row.tooFast}, lookahead misses ${row.lookaheadMisses}, min lookahead ${row.minLookaheadS.toFixed(2)} s, font ${row.fontPx.toFixed(1)} px (x${row.fontScale.toFixed(2)}), max ${row.maxSpeedEm.toFixed(1)} em/s`,
    );
    if (row.wrongLetter || row.backwards || row.tooFast || row.lookaheadMisses || row.overflowFrames || row.hiddenFrames || row.leadInProblems || row.switchProblems) report.problems.push(`real-font ${display} check failed for ${row.title} at ${row.viewport}: ${row.firstProblems.join("; ")}`);
    await ctx.close();
  }
}

// --- ticker performance: play 6 s through a line and a line change at 1440x900@2 ------
{
  const ctx = await browser.newContext({ viewport: W1440, deviceScaleFactor: 2 });
  const page = await ctx.newPage();
  await page.goto(base + "?song=0&at=8&play=1");
  await page.waitForSelector(".ticker__canvas");
  await page.waitForTimeout(500);
  const r = await page.evaluate(async () => {
    const t = window.__jkTicker;
    const before = { frames: t.stats.frames, draw: t.stats.totalDrawMs };
    t.stats.maxDrawMs = 0;
    // Frame intervals as the page sees them.
    const intervals = [];
    let last = performance.now();
    let running = true;
    const tick = (now) => {
      intervals.push(now - last);
      last = now;
      if (running) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    await new Promise((res) => setTimeout(res, 6000));
    running = false;
    const frames = t.stats.frames - before.frames;
    intervals.sort((a, b) => a - b);
    return {
      seconds: 6,
      tickerFrames: frames,
      tickerFps: frames / 6,
      avgDrawMs: (t.stats.totalDrawMs - before.draw) / Math.max(1, frames),
      maxDrawMs: t.stats.maxDrawMs,
      tilesRendered: t.stats.tilesRendered,
      rafIntervalP50: intervals[Math.floor(intervals.length * 0.5)],
      rafIntervalP99: intervals[Math.floor(intervals.length * 0.99)],
      fontPx: t.fontPx,
      fontScale: t.fontScale,
    };
  });
  // Paused: the loop must stop (no frames while nothing moves).
  await page.keyboard.press("Space");
  await page.waitForTimeout(600);
  const idle = await page.evaluate(async () => {
    const f0 = window.__jkTicker.stats.frames;
    await new Promise((res) => setTimeout(res, 2000));
    return window.__jkTicker.stats.frames - f0;
  });
  r.framesWhilePaused2s = idle;
  report.ticker = r;
  console.log("ticker:", JSON.stringify(r));
  if (r.tickerFps > 61) report.problems.push(`ticker ran at ${r.tickerFps.toFixed(1)} fps (cap 60)`);
  if (r.avgDrawMs > 4) report.problems.push(`ticker average draw ${r.avgDrawMs.toFixed(2)} ms`);
  if (idle > 0) report.problems.push(`ticker drew ${idle} frames while paused`);
  await ctx.close();
}

await browser.close();
server.close();
writeFileSync(join(outDir, "report.json"), JSON.stringify(report, null, 2));
console.log(`\n${report.shots.length} screenshots in ${outDir}`);
if (report.problems.length) {
  console.error(`PROBLEMS:\n${report.problems.join("\n")}`);
  process.exit(1);
}
