// SPDX-License-Identifier: GPL-3.0-or-later
// The page display's progress needle (page.ts point 6; the owner's request of 18 Nov 2026
// to bring back "the bar ... that would mark the progress of where the song is") in the
// REAL app: the release exe from a portable test folder, driven over WebView2's DevTools
// port like real-app-page-lyrics.mjs, Stage at 1440x900, skin on.
//   - The settings file is written as the previous version (D-218) wrote it, without
//     `ui.show_marker`, apart from the silent output: the core must open it with the
//     needle on and the marker colour unchanged.
//   - The live layout's own check with the real font (window.__jkTicker.verify(),
//     pageverify.ts (a)-(f) and (n), the needle on every frame of the song).
//   - <seconds> s played from 1.5 s before the first line with every drawn frame traced
//     (__jkTicker.traceStart / traceTake): the needle's x as drawn, the x where its row was
//     split between the sung and the upcoming colour, and the wipe edge from the line's
//     motion (clamped to the text, scaled with a line moving into place). Reported: the
//     largest difference per frame, needle vs split and needle vs edge; frames where the
//     needle moved left on a settled row; frames with a line in the slot but no needle.
//   - Pixels: at paused moments across the song, the canvas is read back: the needle's
//     columns (its colour) with the needle on, the gold/white boundary in the letters of
//     its row with the needle off (a debug toggle of the UI store, nothing saved); the
//     distance between the needle's centre and the boundary, in device pixels.
//   - Screenshots (paused, so each shows exactly the lyric time asked for): a lead-in with
//     the needle waiting at the first letter, mid-line with the needle at the gold/white
//     edge half way through a word, the same moment with Settings > Lyrics > "Show the
//     progress bar" turned off (through the Settings screen, saved by the core), a line
//     change (the needle moving up with the next line) and a rest with the countdown (no
//     needle); then the setting back on.
//
// LYRICS: never printed or saved. The console and the JSON report carry line and word
// numbers, lengths, times and pixel positions only; the verify report's problem strings
// have their quoted text replaced by [text]. Use the public-domain demo song.
//
// SAFETY: nothing is played aloud: Settings/settings.json selects the engine's silent
// "null:48000" output device and master volume 0 before the app starts.
//
//   node scripts/real-app-page-needle.mjs <Jukeoke.exe> <root> <out dir> [seconds=20]
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [exe, root, outDir, secondsArg] = process.argv.slice(2);
if (!exe || !root || !outDir) {
  console.error("usage: node scripts/real-app-page-needle.mjs <Jukeoke.exe> <root> <out dir> [seconds=20]");
  process.exit(2);
}
const seconds = Number(secondsArg ?? 20);
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
// As the D-218 version wrote it (no show_marker), but silent.
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify(
    {
      ui: {
        skin: true,
        bubbles: true,
        motion: "full",
        lyrics_text_size: 56,
        marker_position: 0.33,
        lyrics_motion: "word",
        lyrics_display: "page",
        show_next_line: true,
        colors: { sung: "#ffc21a", upcoming: "#f4f4f4", marker: "#7dffd8" },
        library_sort: "title",
        fullscreen_stage: false,
      },
      audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} },
    },
    null,
    2,
  ),
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const redact = (str) => String(str).replace(/"[^"]*"/g, "[text]");
const verifyNumbers = (v) => ({ ...v, firstProblems: v.firstProblems.map(redact), leadExceptions: (v.leadExceptions ?? []).map((e) => ({ ...e, reason: redact(e.reason) })) });
const probeNumbers = (p) => ({ t: p.t, line: p.line, wipe: p.wipe, edgeX: p.edgeX, edgeWord: p.edgeWord, sungWord: p.sungWord, needleX: p.needleX, needleRow: p.needleRow, needleTop: p.needleTop, needleBottom: p.needleBottom });
const linesNumbers = (lines) => lines.map(({ text, words, ...rest }) => ({ ...rest, chars: text.length, words: words.map(({ text: wt, ...w }) => ({ ...w, chars: wt.length })) }));
const t0 = Date.now();
const since = () => (Date.now() - t0) / 1000;
const port = 9300 + Math.floor(Math.random() * 500);
const child = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: ["ignore", "pipe", "pipe"] });
let exited = null;
child.on("exit", (code) => (exited = { code, at_s: since() }));
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

const report = { exe, root, started: new Date(t0).toISOString(), seconds, checks: [], errors: [], trace: null, pixels: [], screenshots: [] };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${detail}`);
};

let browser;
let page;
const invoke = async (cmd, args = {}) => {
  const r = await page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a).then((ok) => ({ ok }), (err) => ({ err })), [cmd, args]);
  if ("err" in r) throw new Error(typeof r.err === "string" ? r.err : JSON.stringify(r.err));
  return r.ok;
};
const nav = async (label) => {
  await page.locator(".skin-nav .jk-plate", { hasText: label }).click();
  await sleep(600);
};
const seek = async (at) =>
  page.evaluate(async (s) => {
    const input = document.querySelector("#seek");
    input.value = String(Math.max(0, s));
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await new Promise((r) => setTimeout(r, 500));
  }, at);
const playing = () => page.evaluate(() => window.__jkDebug.store.get().player?.state === "playing");
const togglePlay = () => page.locator(".skin-transport__play").click();
const pause = async () => {
  if (await playing()) await togglePlay();
  await sleep(300);
};
const openSong = async (title) => {
  await nav("Library");
  await page.waitForSelector(".jk-strip");
  await page.locator(".jk-strip", { hasText: title }).first().click();
  await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0 && window.__jkTicker.display === "page", null, { timeout: 30000 });
  await sleep(800);
};
/** Debug toggle of the needle in the UI store only (nothing saved): for the pixel check. */
const storeMarker = async (on) => {
  await page.evaluate((v) => {
    const st = window.__jkDebug.store;
    const s = st.get().settings;
    st.set({ settings: { ...s, ui: { ...s.ui, show_marker: v } } });
  }, on);
  await sleep(250);
};

/**
 * Reads the lyrics canvas back around x (CSS px) over the needle's shaft [top, bottom]:
 * the needle's columns (its colour over most of the shaft) and, in the middle half of the
 * row's band, the columns holding gold and white letter pixels. Device pixels.
 */
const readPixels = (x, top, bottom, marker, half = 60) =>
  page.evaluate(
    ([x, top, bottom, marker, half]) => {
      const c = document.querySelector(".page-lyrics canvas");
      const rect = c.getBoundingClientRect();
      const k = c.width / rect.width;
      const ctx = c.getContext("2d");
      const x0 = Math.max(0, Math.floor((x - half) * k));
      const x1 = Math.min(c.width, Math.ceil((x + half) * k));
      const y0 = Math.max(0, Math.floor(top * k));
      const y1 = Math.min(c.height, Math.ceil(bottom * k));
      const img = ctx.getImageData(x0, y0, x1 - x0, y1 - y0);
      const W = x1 - x0;
      const H = y1 - y0;
      const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
      const m = hex(marker);
      const needleCols = [];
      const gold = [];
      const white = [];
      // The letters sit in the middle of the band (the shaft reaches 0.12 em beyond it).
      const ly0 = Math.floor(H * 0.3);
      const ly1 = Math.ceil(H * 0.7);
      for (let cx = 0; cx < W; cx++) {
        let nm = 0;
        let ng = 0;
        let nw = 0;
        for (let cy = 0; cy < H; cy++) {
          const i = (cy * W + cx) * 4;
          const [r, g, b, a] = [img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3]];
          if (a < 200) continue;
          if (Math.abs(r - m[0]) < 50 && Math.abs(g - m[1]) < 50 && Math.abs(b - m[2]) < 50) nm++;
          if (cy >= ly0 && cy < ly1) {
            if (r > 200 && g > 140 && g < 225 && b < 110) ng++;
            else if (r > 215 && g > 215 && b > 215) nw++;
          }
        }
        // Over most of the shaft; in a scan of the whole canvas, any run of 20 such pixels.
        if (nm >= (half > 100 ? 20 : 0.6 * H)) needleCols.push(x0 + cx);
        if (ng >= 2) gold.push(x0 + cx);
        if (nw >= 2) white.push(x0 + cx);
      }
      return { k, needleCols, gold, white };
    },
    [x, top, bottom, marker, half],
  );

/** Pauses, seeks to `at` (the frame drawn is exactly that lyric time), captures; returns the probe. */
async function capture(name, at) {
  await pause();
  await seek(at);
  await sleep(300);
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  const probe = await page.evaluate(() => window.__jkTicker.probe());
  report.screenshots.push({ file, name, target: at, probe: probeNumbers(probe) });
  console.log(`  saved ${file}: t=${probe.t.toFixed(2)} s, line ${probe.line + 1}, needle ${probe.needleX === null ? "none" : `x ${probe.needleX.toFixed(1)} row ${probe.needleRow + 1}`}, edge ${probe.edgeX === null ? "outside the text" : probe.edgeX.toFixed(1)}`);
  return probe;
}

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
  await page.waitForSelector(".jk-strip, .library__empty", { timeout: 60000 });
  const settings = await invoke("settings_get");
  report.settings = { ui: settings.ui, device_id: settings.audio.device_id, master_volume: settings.audio.master_volume };
  const silent = settings.audio.device_id === "null:48000" && settings.audio.master_volume === 0;
  check("silent output (null device, master 0)", silent, `${settings.audio.device_id}, master ${settings.audio.master_volume}`);
  if (!silent) throw new Error("not silent: stopping");
  check("a settings file without show_marker opens with the progress bar on", settings.ui.show_marker === true && settings.ui.colors.marker === "#7dffd8" && settings.ui.lyrics_display === "page", JSON.stringify({ show_marker: settings.ui.show_marker, marker: settings.ui.colors.marker, display: settings.ui.lyrics_display }));
  const marker = settings.ui.colors.marker;

  const lib = await invoke("library_list");
  const demo = lib.songs.find((s) => s.has_lyrics && !s.error && /twinkle/i.test(s.title)) ?? lib.songs.find((s) => s.has_lyrics && !s.error);
  if (!demo) throw new Error("no song with lyrics in the library");
  await openSong(demo.title);
  const lines = await page.evaluate(() => window.__jkTicker.pageLines());
  const info = await page.evaluate(() => {
    const el = document.querySelector(".page-lyrics");
    const r = el.getBoundingClientRect();
    const c = el.querySelector("canvas");
    return { fontPx: window.__jkTicker.fontPx, area: { w: r.width, h: r.height }, dpr: window.devicePixelRatio, canvas: { w: c.width, h: c.height } };
  });
  report.info = info;
  report.lines = linesNumbers(lines);

  // Live layout check, real font: (a)-(f) and the needle on every frame (n).
  const v = verifyNumbers(await page.evaluate(() => window.__jkTicker.verify()));
  report.verify = v;
  check(
    "live layout check with the real font, needle included",
    v.needleProblems === 0 && v.wrongLetter === 0 && v.backwards === 0 && v.overflowFrames === 0 && v.leadInProblems === 0 && v.switchProblems === 0 && v.needleFrames > 0,
    `${v.lines} lines, font ${v.fontPx.toFixed(1)} px; needle on ${v.needleFrames} of ${v.frames} frames, off the boundary by <= ${v.needleMaxDxPx.toFixed(4)} px, parked through ${v.needleParkedFrames} lead-in frames, problems ${v.needleProblems} (wrong line ${v.needleWrongLine}, wrong row ${v.needleWrongRow}, backwards ${v.needleBackwards}, not parked ${v.needleNotParked}, in rests ${v.needleInRest}, outside ${v.needleOutside}); wipe: wrong words ${v.wrongLetter}, backwards ${v.backwards}, overflow ${v.overflowFrames} ${v.firstProblems.slice(0, 2).join("; ")}`,
  );

  // Trace: the needle and the wipe edge, sampled on every drawn frame.
  const from = Math.max(0, (lines[0]?.start ?? 0) - 1.5);
  await seek(from);
  await page.evaluate(() => window.__jkTicker.traceStart());
  await togglePlay();
  await sleep(seconds * 1000);
  const frames = await page.evaluate(() => window.__jkTicker.traceTake());
  const st = await page.evaluate(() => ({ state: window.__jkDebug.store.get().player?.state, underruns: window.__jkDebug.store.get().player?.underruns, stats: { ...window.__jkTicker.stats } }));
  await pause();
  let withNeedle = 0;
  let split = 0;
  let maxSplit = 0;
  let vsEdge = 0;
  let maxEdge = 0;
  let maxEdgeSettled = 0;
  let moving = 0;
  let missing = 0;
  let leaving = 0;
  let back = 0;
  let prev = null;
  const worst = [];
  for (const f of frames) {
    const n = f.needle;
    const has = Number.isFinite(n);
    if (has) withNeedle++;
    if (!has && f.line >= 0) missing++;
    if (has && f.line < 0) leaving++;
    if (has && Number.isFinite(f.split)) {
      split++;
      maxSplit = Math.max(maxSplit, Math.abs(n - f.split));
    }
    if (has && f.line >= 0 && Number.isFinite(f.edge)) {
      vsEdge++;
      const d = Math.abs(n - f.edge);
      maxEdge = Math.max(maxEdge, d);
      if (f.scale === 1) maxEdgeSettled = Math.max(maxEdgeSettled, d);
      else moving++;
      if (d > 0.5 && worst.length < 5) worst.push({ t: f.t, line: f.line, needle: n, edge: f.edge, scale: f.scale });
    }
    if (has && prev && f.scale === 1 && prev.scale === 1 && prev.line === f.line && prev.needleRow === f.needleRow && n < prev.needle - 1e-6) back++;
    prev = has ? f : null;
  }
  const intervals = frames.slice(1).map((f, i) => f.now - frames[i].now).sort((a, b) => a - b);
  report.trace = {
    from,
    frames: frames.length,
    withNeedle,
    splitFrames: split,
    maxNeedleVsSplitPx: maxSplit,
    edgeFrames: vsEdge,
    movingFrames: moving,
    maxNeedleVsEdgePx: maxEdge,
    maxNeedleVsEdgeSettledPx: maxEdgeSettled,
    missing,
    onLeavingLine: leaving,
    backwards: back,
    worst,
    frameIntervalMedianMs: intervals[Math.floor(intervals.length / 2)] ?? 0,
    drawAvgMs: st.stats.totalDrawMs / Math.max(1, st.stats.frames),
    drawMaxMs: st.stats.maxDrawMs,
    state: st.state,
    underruns: st.underruns,
    samples: frames.map((f) => [Math.round(f.now * 100) / 100, Math.round(f.t * 10000) / 10000, f.line, f.needleRow, Math.round(f.needle * 1000) / 1000, Math.round(f.split * 1000) / 1000, Math.round(f.edge * 1000) / 1000, f.scale]),
  };
  check(
    `${seconds} s traced: the needle on the wipe edge on every drawn frame`,
    frames.length > seconds * 50 && withNeedle > seconds * 30 && maxSplit <= 1e-6 && maxEdge <= 0.5 && missing === 0 && back === 0 && st.state === "playing" && st.underruns === 0,
    `${frames.length} frames (median interval ${report.trace.frameIntervalMedianMs.toFixed(2)} ms), needle drawn on ${withNeedle} (${leaving} on a line leaving before a rest or at the end); needle vs row split: max ${maxSplit.toExponential(2)} px over ${split} frames; needle vs wipe edge: max ${maxEdge.toFixed(4)} px over ${vsEdge} frames (${maxEdgeSettled.toFixed(4)} px on settled lines, ${moving} frames while a line moved into place); line in the slot without a needle ${missing}; moved left on a row ${back}; draw avg ${report.trace.drawAvgMs.toFixed(3)} ms, max ${st.stats.maxDrawMs.toFixed(2)} ms; state ${st.state}, underruns ${st.underruns}`,
  );

  // Pixels: the needle's columns vs the gold/white boundary in its row.
  const letters = (w) => [...w.text].length; // used here only, never reported
  const moments = lines.flatMap((l) => l.words.filter((w) => w.end - w.start >= 0.25 && letters(w) > 1).map((w) => ({ line: l.index, t: w.start + 0.6 * (w.end - w.start) })));
  const step = Math.max(1, Math.floor(moments.length / 14));
  const picked = moments.filter((_, i) => i % step === 0).slice(0, 14);
  for (const m of picked) {
    await seek(m.t);
    const p = await page.evaluate(() => window.__jkTicker.probe());
    if (p.needleX === null) continue;
    const on = await readPixels(p.needleX, p.needleTop, p.needleBottom, marker);
    await storeMarker(false);
    const off = await readPixels(p.needleX, p.needleTop, p.needleBottom, marker);
    await storeMarker(true);
    const k = on.k;
    const nc = on.needleCols;
    const centre = nc.length ? (nc[0] + nc[nc.length - 1] + 1) / 2 : NaN; // device px, pixel edges
    const goldMax = off.gold.length ? Math.max(...off.gold.filter((c) => c <= p.needleX * k + 3 * k)) + 1 : NaN; // right edge of the last gold column near the needle
    const whiteMin = off.white.length ? Math.min(...off.white.filter((c) => c >= p.needleX * k - 3 * k)) : NaN; // left edge of the first white column
    const lo = Math.min(goldMax, whiteMin);
    const hi = Math.max(goldMax, whiteMin);
    const dist = centre < lo ? lo - centre : centre > hi ? centre - hi : 0;
    const row = { t: p.t, line: p.line, needleRow: p.needleRow, needleCss: p.needleX, needleCentreDev: centre, needleCols: nc.length, needleOffDrawn: off.needleCols.length, goldEndDev: goldMax, whiteStartDev: whiteMin, distDev: dist, scale: k };
    report.pixels.push(row);
    console.log(`  pixels t=${p.t.toFixed(2)} line ${p.line + 1}: needle centre ${centre.toFixed(1)} dev px (${nc.length} columns), boundary gold..white ${goldMax}..${whiteMin}, off by ${dist.toFixed(2)} dev px; needle off: ${off.needleCols.length} needle columns`);
  }
  const px = report.pixels.filter((r) => Number.isFinite(r.distDev));
  const maxDist = px.length ? Math.max(...px.map((r) => r.distDev)) : Infinity;
  check(
    "pixels: the needle's columns sit on the gold/white boundary of its row",
    px.length >= 8 && maxDist <= 1.0 && report.pixels.every((r) => r.needleOffDrawn === 0 && r.needleCols >= 2),
    `${px.length} paused moments, device pixel ratio ${info.dpr}: needle centre to the boundary <= ${maxDist.toFixed(2)} device px (boundary = last gold column .. first white column in the letters of its row, read with the needle off); needle ${report.pixels.map((r) => r.needleCols).join("/")} columns wide; none drawn when off`,
  );

  // Screenshots (1440x900, skin on).
  const led = lines.find((l) => l.index > 0 && l.leadIn) ?? lines.find((l) => l.leadIn);
  if (led) {
    const p = await capture("needle-1-lead-in-1440x900", led.leadIn.from + 0.4 * (led.leadIn.to - led.leadIn.from));
    const first = led.rowBoxes[0].left;
    check("lead-in: the needle waits at the first letter", p.line === led.index && p.needleX !== null && Math.abs(p.needleX - first) < 1e-6 && p.needleRow === 0, `line ${p.line + 1}, needle x ${p.needleX?.toFixed(2)}, first letter x ${first.toFixed(2)}`);
  }
  // Half way through a word of six letters or more that is not a line's first word: the
  // lyric time when the wipe is there, found with the live layout's own probe.
  const cands = lines.flatMap((l) => l.words.slice(1).filter((w) => letters(w) >= 6).map((w) => ({ ...w, line: l.index })));
  const w = cands[0] ?? lines[0].words[0];
  const mid = await page.evaluate(([a, b, s, e]) => {
    const target = a + 0.45 * (b - a);
    let lo = s;
    let hi = e;
    for (let i = 0; i < 40; i++) {
      const m = (lo + hi) / 2;
      if (window.__jkTicker.probe(m).wipe < target) lo = m;
      else hi = m;
    }
    return (lo + hi) / 2;
  }, [w.a, w.b, w.start, w.end]);
  const p2 = await capture("needle-2-mid-line-1440x900", mid);
  check("mid-line: the needle at the gold/white edge", p2.needleX !== null && p2.edgeX !== null && Math.abs(p2.needleX - p2.edgeX) < 1e-6, `needle x ${p2.needleX?.toFixed(2)}, wipe edge x ${p2.edgeX?.toFixed(2)}`);
  // Off through Settings, as the owner would; saved by the core.
  await nav("Settings");
  const sw = page.getByLabel("Show the progress bar");
  await sw.click();
  await page.waitForFunction(() => window.__jkDebug.store.get().settings?.ui.show_marker === false, null, { timeout: 10000 });
  let saved = null;
  for (let i = 0; i < 40 && saved !== false; i++) {
    saved = (await invoke("settings_get")).ui.show_marker;
    if (saved !== false) await sleep(100);
  }
  await nav("Sing");
  await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.display === "page", null, { timeout: 10000 });
  await sleep(600);
  const p3 = await capture("needle-3-off-1440x900", mid);
  const offPx = await readPixels(p3.needleX ?? 720, p3.needleTop ?? 300, p3.needleBottom ?? 400, marker);
  check("Settings > Show the progress bar off: saved, nothing drawn", saved === false && offPx.needleCols.length === 0, `saved show_marker = ${saved}; needle columns in the capture ${offPx.needleCols.length}`);
  await nav("Settings");
  await page.getByLabel("Show the progress bar").click();
  await page.waitForFunction(() => window.__jkDebug.store.get().settings?.ui.show_marker === true, null, { timeout: 10000 });
  await sleep(500);
  check("progress bar back on", (await invoke("settings_get")).ui.show_marker === true, "saved");
  await nav("Sing");
  await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.display === "page", null, { timeout: 10000 });
  await sleep(600);
  // A line change: the needle moving up with the next line (half way through the change).
  const change = lines.find((l, i) => i > 1 && !l.breakBefore && l.previewFrom !== null);
  if (change) {
    const p4 = await capture("needle-4-line-change-1440x900", change.current + 0.1);
    check("line change: the needle on the next line as it moves up", p4.line === change.index && p4.needleX !== null && p4.needleRow === 0, `line ${p4.line + 1}, needle x ${p4.needleX?.toFixed(2)} (first letter at x ${change.rowBoxes[0].left.toFixed(2)} once in place), shaft y ${p4.needleTop?.toFixed(1)}..${p4.needleBottom?.toFixed(1)}`);
  }
  // A rest with the countdown: no needle.
  const brk = lines.find((l, i) => i > 0 && l.breakBefore);
  if (brk) {
    const p5 = await capture("needle-5-rest-countdown-1440x900", brk.current - 2.5);
    const px5 = await readPixels(info.area.w / 2, 0, info.area.h, marker, info.area.w / 2 + 2);
    check("rest with the countdown: no needle", p5.needleX === null && px5.needleCols.length === 0, `t=${p5.t.toFixed(2)}: needle ${p5.needleX === null ? "none" : p5.needleX.toFixed(1)}, needle columns on the canvas ${px5.needleCols.length}`);
  }
  check("screenshots", report.screenshots.length >= 3, report.screenshots.map((s) => s.name).join(", "));
  check("no page errors", report.errors.length === 0, report.errors.slice(0, 5).join(" | ") || "none");
} catch (err) {
  check("run", false, String(err && err.stack ? err.stack : err));
} finally {
  await browser?.close().catch(() => undefined);
  child.kill();
}
await sleep(500);
report.exited = exited;
report.finished_s = since();
writeFileSync(join(outDir, "real-app-page-needle.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 1));
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
