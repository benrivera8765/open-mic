// SPDX-License-Identifier: GPL-3.0-or-later
// The lyrics ticker's motion measured in the REAL app (D-216, the owner's request of
// 18 Nov 2026: "word by word instead of jumping"): the release exe from a portable copy,
// driven over WebView2's DevTools port the way real-app-e2e.mjs does, Stage at 1440x900,
// skin on, the scrolling ticker (settings.ui.lyrics_display "scroll"; the page display has
// its own script, real-app-page-lyrics.mjs). For every song in <root>/Library and both paces (settings.ui.lyrics_motion
// "word" and "letter"):
//   - runs the live layout's own check (window.__jkTicker.verify(): letters for the
//     letter pace, words for the word pace) with the real lyrics font;
//   - plays <seconds> s from a start point (default: 1.5 s before the first line) and
//     records every drawn frame through window.__jkTicker.traceStart/traceTake: the
//     frame time, the lyric time and the strip position under the marker (the text's
//     on-screen x-offset is marker x minus that position);
//   - reports the on-screen speed of each frame (distance moved / frame interval) and
//     how much it changes from one frame to the next while a line is sung: the largest
//     change, the 99th percentile, the share of frames changing by more than 25 %, and
//     frames that stood still while a line was sung.
// Then, in the word pace, three Stage screenshots with a held word half way under the
// marker (seek, play 1.3 s, capture).
//
// SAFETY: nothing is played aloud: Settings/settings.json selects the engine's silent
// "null:48000" output device and master volume 0 before the app starts.
//
//   node scripts/real-app-ticker-motion.mjs <Open Mic.exe> <root> <out dir> [seconds=25]
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [exe, root, outDir, secondsArg] = process.argv.slice(2);
if (!exe || !root || !outDir) {
  console.error("usage: node scripts/real-app-ticker-motion.mjs <Open Mic.exe> <root> <out dir> [seconds=25]");
  process.exit(2);
}
const seconds = Number(secondsArg ?? 25);
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify(
    {
      ui: { skin: true, bubbles: true, motion: "full", lyrics_text_size: 56, marker_position: 0.33, lyrics_motion: "word", lyrics_display: "scroll", colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" }, library_sort: "title", fullscreen_stage: false },
      audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} },
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

const report = { exe, root, started: new Date(t0).toISOString(), seconds, checks: [], errors: [], runs: [], screenshots: [] };
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
  await sleep(500);
};
/** Seeks (paused) to `at` seconds through the transport's seek control. */
const seek = async (at) =>
  page.evaluate(async (s) => {
    const input = document.querySelector("#seek");
    input.value = String(Math.max(0, s));
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await new Promise((r) => setTimeout(r, 400));
  }, at);
/** Chooses the pace on the Settings screen ("Lyrics motion"), as the owner would; waits for the core to have it. */
const setPace = async (pace) => {
  await nav("Settings");
  await page.getByLabel("Lyrics motion").selectOption(pace);
  await page.waitForFunction((p) => window.__jkDebug.store.get().settings?.ui.lyrics_motion === p, pace, { timeout: 10000 });
  for (let i = 0; i < 40; i++) {
    if ((await invoke("settings_get")).ui.lyrics_motion === pace) return;
    await sleep(100);
  }
  throw new Error(`the core did not take lyrics_motion = ${pace}`);
};
const playing = () => page.evaluate(() => window.__jkDebug.store.get().player?.state === "playing");
const togglePlay = () => page.locator(".skin-transport__play").click();

/** Frame statistics of a trace (see the header). */
function stats(frames, lines, fontPx) {
  const sung = (t) => lines.some((l) => t >= l.start && t <= l.end);
  const v = [];
  let gaps = 0;
  for (let i = 0; i + 1 < frames.length; i++) {
    const a = frames[i];
    const b = frames[i + 1];
    const dt = (b.now - a.now) / 1000;
    if (!(dt > 0) || dt > 0.05) {
      gaps++;
      v.push(null);
      continue;
    }
    v.push({ v: (b.x - a.x) / dt, vLyric: b.t > a.t ? (b.x - a.x) / (b.t - a.t) : 0, sung: sung(a.t) && sung(b.t), dt });
  }
  const dvs = [];
  const dvsLyric = [];
  let big = 0;
  let bigLyric = 0;
  let still = 0;
  let maxSpeed = 0;
  for (let i = 0; i + 1 < v.length; i++) {
    const a = v[i];
    const b = v[i + 1];
    if (!a || !b || !a.sung || !b.sung) continue;
    const d = Math.abs(b.v - a.v);
    dvs.push(d);
    if (d > 0.25 * Math.max(a.v, b.v, 1e-9)) big++;
    const dl = Math.abs(b.vLyric - a.vLyric);
    dvsLyric.push(dl);
    if (dl > 0.25 * Math.max(a.vLyric, b.vLyric, 1e-9)) bigLyric++;
    if (b.v <= 0) still++;
    maxSpeed = Math.max(maxSpeed, b.v);
  }
  const pct = (arr, p) => {
    const s = [...arr].sort((x, y) => x - y);
    return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : 0;
  };
  const intervals = frames.slice(1).map((f, i) => f.now - frames[i].now);
  return {
    frames: frames.length,
    pairsWhileSung: dvs.length,
    frameGapsOver50ms: gaps,
    frameIntervalMs: { median: pct(intervals, 0.5), p99: pct(intervals, 0.99), max: Math.max(...intervals) },
    // On-screen: distance per frame over the frame's real interval.
    maxDvEmPerFrame: Math.max(0, ...dvs) / fontPx,
    p99DvEmPerFrame: pct(dvs, 0.99) / fontPx,
    medianDvEmPerFrame: pct(dvs, 0.5) / fontPx,
    over25pct: big,
    over25pctShare: dvs.length ? big / dvs.length : 0,
    // The same with the lyric time of each frame (the motion alone, without frame-timing jitter).
    lyricTime: { maxDvEmPerFrame: Math.max(0, ...dvsLyric) / fontPx, p99DvEmPerFrame: pct(dvsLyric, 0.99) / fontPx, over25pct: bigLyric, over25pctShare: dvsLyric.length ? bigLyric / dvsLyric.length : 0 },
    stillWhileSung: still,
    maxSpeedEm: maxSpeed / fontPx,
  };
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
  report.settings = { lyrics_motion: settings.ui.lyrics_motion, device_id: settings.audio.device_id, master_volume: settings.audio.master_volume };
  check("silent output (null device, master 0)", settings.audio.device_id === "null:48000" && settings.audio.master_volume === 0, JSON.stringify(report.settings));
  check("lyrics motion defaults to word", settings.ui.lyrics_motion === "word", String(settings.ui.lyrics_motion));
  if (!(settings.audio.device_id === "null:48000" && settings.audio.master_volume === 0)) throw new Error("not silent: stopping");

  const lib = await invoke("library_list");
  const songs = lib.songs.filter((s) => s.has_lyrics && !s.error);
  for (const pace of ["word", "letter"]) {
    await setPace(pace);
    for (const s of songs) {
      await nav("Library");
      await page.waitForSelector(".jk-strip");
      await page.locator(".jk-strip", { hasText: s.title }).first().click();
      await page.waitForFunction((p) => window.__jkTicker && window.__jkTicker.glyphs > 0 && window.__jkTicker.pace === p, pace, { timeout: 30000 });
      await sleep(600);
      const info = await page.evaluate(() => {
        const song = window.__jkDebug.store.get().song;
        const lines = (song.lyrics?.lines ?? []).map((l) => ({ start: l.start, end: l.end }));
        const tk = window.__jkTicker;
        return { lines, fontPx: tk.fontPx, fontScale: tk.fontScale, minLineScale: tk.minLineScale, worstLookahead: tk.worstLookahead, pace: tk.pace, duration: song.summary.duration_s };
      });
      const v = await page.evaluate(() => window.__jkTicker.verify());
      const ok = v.wrongLetter === 0 && v.backwards === 0 && v.tooFast === 0 && v.lookaheadMisses === 0 && (pace === "letter" || (v.onsetMisses === 0 && v.stopped === 0));
      check(
        `${s.title} (${pace}): live layout check, real font`,
        ok,
        `${v.sungSamples} sung samples, wrong ${pace === "word" ? "words" : "letters"} ${v.wrongLetter}, backwards ${v.backwards}, lookahead misses ${v.lookaheadMisses} (min ${v.minLookaheadS.toFixed(2)} s)${pace === "word" ? `, onset <= ${v.onsetMaxMs.toFixed(1)} ms, stopped ${v.stopped}` : `, too fast ${v.tooFast}`}, font ${v.fontPx.toFixed(1)} px ${v.firstProblems.slice(0, 2).join("; ")}`,
      );
      const from = Math.max(0, (info.lines[0]?.start ?? 0) - 1.5);
      await seek(from);
      await page.evaluate(() => window.__jkTicker.traceStart());
      await togglePlay();
      await sleep(seconds * 1000);
      const frames = await page.evaluate(() => window.__jkTicker.traceTake());
      const state = await page.evaluate(() => ({ state: window.__jkDebug.store.get().player?.state, underruns: window.__jkDebug.store.get().player?.underruns }));
      if (await playing()) await togglePlay();
      await sleep(300);
      const st = stats(frames, info.lines, info.fontPx);
      report.runs.push({ song: s.title, pace, from, layout: info, verify: v, played: state, stats: st, frames: frames.map((f) => [Math.round(f.now * 100) / 100, Math.round(f.t * 10000) / 10000, Math.round(f.x * 100) / 100, Math.round(f.offset * 100) / 100]) });
      check(
        `${s.title} (${pace}): ${seconds} s traced`,
        st.frames > seconds * 50 && state.state === "playing" && state.underruns === 0,
        `${st.frames} frames (median interval ${st.frameIntervalMs.median.toFixed(2)} ms, ${st.frameGapsOver50ms} gaps > 50 ms), ${st.pairsWhileSung} frame pairs while sung: speed change max ${st.maxDvEmPerFrame.toFixed(2)} em/s, p99 ${st.p99DvEmPerFrame.toFixed(2)} em/s, > 25 % in ${st.over25pct} (${(st.over25pctShare * 100).toFixed(2)} %), still ${st.stillWhileSung}; state ${state.state}, underruns ${state.underruns}`,
      );
    }
  }

  // Three word-pace screenshots with a held word half way under the marker.
  await setPace("word");
  const shots = [];
  for (const s of songs) {
    const targets = await (async () => {
      await nav("Library");
      await page.waitForSelector(".jk-strip");
      await page.locator(".jk-strip", { hasText: s.title }).first().click();
      await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0 && window.__jkTicker.pace === "word", null, { timeout: 30000 });
      await sleep(600);
      return page.evaluate(() => {
        const song = window.__jkDebug.store.get().song;
        const words = (song.lyrics?.lines ?? []).flatMap((l) => l.words.map((w) => ({ start: w.start, end: w.end, text: w.chars.map((c) => c[0]).join("") })));
        const held = words.filter((w) => w.end - w.start >= 0.5 && w.text.length >= 3);
        return held.length ? [held[Math.floor(held.length / 3)], held[Math.floor((2 * held.length) / 3)]] : [];
      });
    })();
    for (const w of targets) shots.push({ song: s.title, word: w });
  }
  for (const [i, sh] of shots.slice(0, 3).entries()) {
    await nav("Library");
    await page.locator(".jk-strip", { hasText: sh.song }).first().click();
    await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0 && window.__jkTicker.pace === "word", null, { timeout: 30000 });
    await sleep(600);
    const mid = (sh.word.start + sh.word.end) / 2;
    await seek(mid - 1.3);
    await togglePlay();
    await sleep(1300);
    const file = join(outDir, `ticker-word-${i + 1}-1440x900.png`);
    await page.screenshot({ path: file });
    const at = await page.evaluate(() => {
      const t = window.__jkDebug.clock.position(performance.now());
      const lines = window.__jkDebug.store.get().song.lyrics?.lines ?? [];
      for (const l of lines) for (const w of l.words) if (w.start <= t && t < w.end) return { t, word: w.chars.map((c) => c[0]).join(""), start: w.start, end: w.end };
      return { t, word: null };
    });
    if (await playing()) await togglePlay();
    await sleep(300);
    report.screenshots.push({ file, song: sh.song, target: sh.word, at });
    console.log(`  saved ${file}: "${at.word}" sung at t=${at.t.toFixed(2)} s`);
  }
  check("three word-pace screenshots", report.screenshots.length === 3, report.screenshots.map((s) => `${s.song}: "${s.at.word}"`).join(", "));
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
writeFileSync(join(outDir, "real-app-ticker-motion.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 1));
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
