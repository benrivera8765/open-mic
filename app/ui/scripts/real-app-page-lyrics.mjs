// SPDX-License-Identifier: GPL-3.0-or-later
// The page display of the lyrics (static lines with a colour wipe, D-218; the owner's
// request of 18 Nov 2026) in the REAL app: the release exe from a portable copy, driven
// over WebView2's DevTools port the way real-app-ticker-motion.mjs does, Stage at
// 1440x900, skin on.
//   - The settings file is written the way every earlier version wrote it (hot pink and
//     cream, no lyrics_display, no lyrics_motion) apart from the silent output: the core
//     must open it with the page display and the new gold/white colours (D-218 migration).
//   - For every song in <root>/Library: the live layout's own check with the real font
//     (window.__jkTicker.verify(): pageverify.ts (a)-(f)); then <seconds> s played from
//     1.5 s before the first line with every drawn frame traced (__jkTicker.traceStart /
//     traceTake: frame time, lyric time, wipe position, the wipe edge's x, the line), and
//     the word under the wipe probed live 20 times a second (__jkTicker.probe()).
//     Reported: the wipe's on-screen speed per frame and how much it changes from one
//     frame to the next within a line while it is sung (largest change, share of frames
//     changing by more than 25 %, frames standing still), and the probes whose wipe edge
//     was not on the word being sung (outside the word's first and last 40 ms).
//   - The letter pace on the first song, for comparison; switching to the scrolling
//     ticker and back in Settings.
//   - Stage screenshots with the skin on: a line before it starts with its lead-in bar,
//     mid-line with the wipe through a held word (the next line previewed under it), a
//     long line that had to shrink or wrap, a rest with the countdown, and mid-line at
//     1280x800 and 3840x2160.
//
// LYRICS: never printed or saved. The console and the JSON report carry line and word
// numbers, lengths and times only (the Library may hold the owner's songs); the verify
// report's problem strings have their quoted text replaced by [text].
//
// SAFETY: nothing is played aloud: Settings/settings.json selects the engine's silent
// "null:48000" output device and master volume 0 before the app starts.
//
//   node scripts/real-app-page-lyrics.mjs <Open Mic.exe> <root> <out dir> [seconds=25]
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [exe, root, outDir, secondsArg] = process.argv.slice(2);
if (!exe || !root || !outDir) {
  console.error("usage: node scripts/real-app-page-lyrics.mjs <Open Mic.exe> <root> <out dir> [seconds=25]");
  process.exit(2);
}
const seconds = Number(secondsArg ?? 25);
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
// As every version before D-218 wrote it (the owner's own file has these colours), but silent.
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify(
    {
      ui: { skin: true, bubbles: true, motion: "full", lyrics_text_size: 56, marker_position: 0.33, colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" }, library_sort: "title", fullscreen_stage: false },
      audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} },
    },
    null,
    2,
  ),
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** Quoted text (line and word text in the verify report's messages) replaced by [text]. */
const redact = (str) => String(str).replace(/"[^"]*"/g, "[text]");
/** A probe without its text: line, word numbers and times only. */
const probeNumbers = (p) => ({ t: p.t, line: p.line, wipe: p.wipe, edgeX: p.edgeX, edgeWord: p.edgeWord, sungWord: p.sungWord, sungStart: p.sungStart, sungEnd: p.sungEnd });
/** A verify report with its messages redacted. */
const verifyNumbers = (v) => ({ ...v, firstProblems: v.firstProblems.map(redact), leadExceptions: (v.leadExceptions ?? []).map((e) => ({ ...e, reason: redact(e.reason) })) });
/** Page lines without their text (lengths instead). */
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
const seek = async (at) =>
  page.evaluate(async (s) => {
    const input = document.querySelector("#seek");
    input.value = String(Math.max(0, s));
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await new Promise((r) => setTimeout(r, 400));
  }, at);
const playing = () => page.evaluate(() => window.__jkDebug.store.get().player?.state === "playing");
const togglePlay = () => page.locator(".skin-transport__play").click();
const pause = async () => {
  if (await playing()) await togglePlay();
  await sleep(300);
};
/** Chooses an option of a Settings select by its label, as the owner would; waits for the core to have it. */
const setSelect = async (label, value, key) => {
  await nav("Settings");
  await page.getByLabel(label).selectOption(value);
  await page.waitForFunction(([k, v]) => window.__jkDebug.store.get().settings?.ui[k] === v, [key, value], { timeout: 10000 });
  for (let i = 0; i < 40; i++) {
    if ((await invoke("settings_get")).ui[key] === value) return;
    await sleep(100);
  }
  throw new Error(`the core did not take ${key} = ${value}`);
};
const openSong = async (title, display = "page", pace = null) => {
  await nav("Library");
  await page.waitForSelector(".jk-strip");
  await page.locator(".jk-strip", { hasText: title }).first().click();
  await page.waitForFunction(([d, p]) => window.__jkTicker && window.__jkTicker.glyphs > 0 && window.__jkTicker.display === d && (!p || window.__jkTicker.pace === p), [display, pace], { timeout: 30000 });
  await sleep(600);
};

/** Wipe statistics of a trace: speed per frame within a line while it is sung, and its changes. */
function stats(frames, lines) {
  const sung = (f) => f.line >= 0 && lines[f.line] && f.t >= lines[f.line].start && f.t <= lines[f.line].end;
  const pairs = [];
  let gaps = 0;
  for (let i = 0; i + 1 < frames.length; i++) {
    const a = frames[i];
    const b = frames[i + 1];
    const dt = (b.now - a.now) / 1000;
    if (!(dt > 0) || dt > 0.05) {
      gaps++;
      pairs.push(null);
      continue;
    }
    if (a.line !== b.line || !sung(a) || !sung(b) || !Number.isFinite(a.x) || !Number.isFinite(b.x)) {
      pairs.push(null);
      continue;
    }
    const em = lines[a.line].px;
    pairs.push({ v: (b.x - a.x) / dt / em, vLyric: b.t > a.t ? (b.x - a.x) / (b.t - a.t) / em : 0, back: b.x < a.x - 1e-6 });
  }
  const dvs = [];
  const dvsLyric = [];
  let big = 0;
  let bigLyric = 0;
  let still = 0;
  let back = 0;
  let maxSpeed = 0;
  for (let i = 0; i + 1 < pairs.length; i++) {
    const a = pairs[i];
    const b = pairs[i + 1];
    if (!a || !b) continue;
    const d = Math.abs(b.v - a.v);
    dvs.push(d);
    if (d > 0.25 * Math.max(a.v, b.v, 1e-9)) big++;
    const dl = Math.abs(b.vLyric - a.vLyric);
    dvsLyric.push(dl);
    if (dl > 0.25 * Math.max(a.vLyric, b.vLyric, 1e-9)) bigLyric++;
    if (b.v <= 0) still++;
    if (b.back) back++;
    maxSpeed = Math.max(maxSpeed, b.v);
  }
  const pct = (arr, p) => {
    const s = [...arr].sort((x, y) => x - y);
    return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : 0;
  };
  const intervals = frames.slice(1).map((f, i) => f.now - frames[i].now);
  const switches = frames.filter((f, i) => i > 0 && f.line !== frames[i - 1].line).length;
  return {
    frames: frames.length,
    pairsWhileSung: dvs.length,
    frameGapsOver50ms: gaps,
    frameIntervalMs: { median: pct(intervals, 0.5), p99: pct(intervals, 0.99), max: intervals.length ? Math.max(...intervals) : 0 },
    maxDvEmPerFrame: Math.max(0, ...dvs),
    p99DvEmPerFrame: pct(dvs, 0.99),
    over25pct: big,
    over25pctShare: dvs.length ? big / dvs.length : 0,
    lyricTime: { maxDvEmPerFrame: Math.max(0, ...dvsLyric), over25pct: bigLyric, over25pctShare: dvsLyric.length ? bigLyric / dvsLyric.length : 0 },
    stillWhileSung: still,
    backwards: back,
    maxSpeedEm: maxSpeed,
    lineChanges: switches,
  };
}

/** Plays from `at - lead`, captures at about `at`, pauses; returns what the wipe showed. */
async function capture(name, at, lead = 1.0) {
  await seek(at - lead);
  await togglePlay();
  await sleep(lead * 1000);
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  const probe = await page.evaluate(() => window.__jkTicker.probe());
  await pause();
  report.screenshots.push({ file, name, target: at, probe: probeNumbers(probe) });
  console.log(`  saved ${file}: t=${probe.t.toFixed(2)} s, line ${probe.line + 1}, wipe on word ${probe.edgeWord + 1}, sung word ${probe.sungWord + 1}`);
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
  check("silent output (null device, master 0)", settings.audio.device_id === "null:48000" && settings.audio.master_volume === 0, `${settings.audio.device_id}, master ${settings.audio.master_volume}`);
  if (!(settings.audio.device_id === "null:48000" && settings.audio.master_volume === 0)) throw new Error("not silent: stopping");
  check(
    "a settings file from before D-218 opens with the page display and the new colours",
    settings.ui.lyrics_display === "page" && settings.ui.show_next_line === true && settings.ui.lyrics_motion === "word" && settings.ui.colors.sung === "#ffc21a" && settings.ui.colors.upcoming === "#f4f4f4" && settings.ui.colors.marker === "#7dffd8",
    JSON.stringify({ display: settings.ui.lyrics_display, next: settings.ui.show_next_line, motion: settings.ui.lyrics_motion, colors: settings.ui.colors }),
  );

  const lib = await invoke("library_list");
  const songs = lib.songs.filter((s) => s.has_lyrics && !s.error);
  const runs = [...songs.map((s) => ({ s, pace: "word" })), ...(songs.length ? [{ s: songs.find((x) => /twinkle/i.test(x.title)) ?? songs[0], pace: "letter" }] : [])];
  let pace = "word";
  for (const { s, pace: want } of runs) {
    if (want !== pace) {
      await setSelect("Lyrics motion", want, "lyrics_motion");
      pace = want;
    }
    await openSong(s.title, "page", pace);
    const lines = await page.evaluate(() => window.__jkTicker.pageLines());
    const info = await page.evaluate(() => ({ fontPx: window.__jkTicker.fontPx, fontScale: window.__jkTicker.fontScale, minLineScale: window.__jkTicker.minLineScale, font: getComputedStyle(document.querySelector(".page-lyrics")).getPropertyValue("--ticker-font").trim(), weight: getComputedStyle(document.querySelector(".page-lyrics")).getPropertyValue("--page-weight").trim(), area: (() => { const r = document.querySelector(".page-lyrics").getBoundingClientRect(); return { w: r.width, h: r.height }; })() }));
    const v = verifyNumbers(await page.evaluate(() => window.__jkTicker.verify()));
    const ok = v.wrongLetter === 0 && v.backwards === 0 && v.lookaheadMisses === 0 && v.hiddenFrames === 0 && v.overflowFrames === 0 && v.leadInProblems === 0 && v.switchProblems === 0 && (pace === "letter" || (v.onsetMisses === 0 && v.stopped === 0));
    check(
      `${s.title} (${pace}): live layout check, real font`,
      ok,
      `${v.lines} display lines (${lines.reduce((a, l) => a + l.sourceLines.length, 0)} song lines), font ${v.fontPx.toFixed(1)} px, ${v.shrunkLines} smaller, ${v.wrappedLines} wrapped, smallest x${v.minLineScale.toFixed(2)}; wrong ${pace === "word" ? "words" : "letters"} ${v.wrongLetter}, backwards ${v.backwards}${pace === "word" ? `, onset <= ${v.onsetMaxMs.toFixed(1)} ms, stopped ${v.stopped}` : ""}; shown >= ${v.minLeadS === null ? "inf" : Number(v.minLeadS).toFixed(2)} s ahead (${v.leadExceptions.length} exceptions, ${v.lookaheadMisses} misses), overflow frames ${v.overflowFrames}, lead-ins ${v.leadIns} (problems ${v.leadInProblems}), line changes <= ${v.maxSwitchMs.toFixed(0)} ms ${v.firstProblems.slice(0, 2).join("; ")}`,
    );
    const from = Math.max(0, (lines[0]?.start ?? 0) - 1.5);
    await seek(from);
    await page.evaluate(() => {
      window.__jkTicker.traceStart();
      window.__pageProbes = [];
      window.__pageProbeTimer = setInterval(() => window.__pageProbes.push(window.__jkTicker.probe()), 50);
    });
    await togglePlay();
    await sleep(seconds * 1000);
    const frames = await page.evaluate(() => {
      clearInterval(window.__pageProbeTimer);
      return window.__jkTicker.traceTake();
    });
    const probes = await page.evaluate(() => window.__pageProbes);
    const st = await page.evaluate(() => ({ state: window.__jkDebug.store.get().player?.state, underruns: window.__jkDebug.store.get().player?.underruns, stats: { ...window.__jkTicker.stats } }));
    await pause();
    const tr = stats(frames, lines);
    // Live: the word under the wipe edge is the word being sung (outside each word's first and last 40 ms).
    let probed = 0;
    let wrong = 0;
    const wrongs = [];
    for (const p of probes) {
      if (p.sungWord < 0 || p.sungStart === null) continue;
      if (p.t < p.sungStart + 0.04 || p.t >= p.sungEnd - 0.04) continue;
      probed++;
      if (p.edgeWord !== p.sungWord) {
        wrong++;
        if (wrongs.length < 5) wrongs.push(`t=${p.t.toFixed(2)} line ${p.line + 1}: sung word ${p.sungWord + 1}, wipe on word ${p.edgeWord + 1}`);
      }
    }
    report.runs.push({ song: s.title, pace, from, info, verify: v, lines: linesNumbers(lines), played: st, trace: tr, probes: { probed, wrong, wrongs }, frames: frames.map((f) => [Math.round(f.now * 100) / 100, Math.round(f.t * 10000) / 10000, Math.round(f.x * 100) / 100, Math.round(f.offset * 100) / 100, f.line]) });
    check(
      `${s.title} (${pace}): ${seconds} s traced`,
      tr.frames > seconds * 50 && st.state === "playing" && st.underruns === 0 && tr.backwards === 0 && (pace === "letter" || tr.stillWhileSung === 0),
      `${tr.frames} frames (median interval ${tr.frameIntervalMs.median.toFixed(2)} ms, ${tr.frameGapsOver50ms} gaps > 50 ms), ${tr.lineChanges} line changes, ${tr.pairsWhileSung} frame pairs while a line is sung: wipe speed change max ${tr.maxDvEmPerFrame.toFixed(2)} em/s, p99 ${tr.p99DvEmPerFrame.toFixed(2)} em/s, > 25 % in ${tr.over25pct} (${(tr.over25pctShare * 100).toFixed(2)} %), still ${tr.stillWhileSung}, backwards ${tr.backwards}; draw avg ${(st.stats.totalDrawMs / Math.max(1, st.stats.frames)).toFixed(2)} ms, max ${st.stats.maxDrawMs.toFixed(2)} ms; state ${st.state}, underruns ${st.underruns}`,
    );
    check(`${s.title} (${pace}): word under the wipe, live`, probed > 50 && (pace === "letter" || wrong === 0), `${probed} probes while a word was sung, wipe on another word in ${wrong} ${wrongs.join("; ")}`);
  }
  if (pace !== "word") await setSelect("Lyrics motion", "word", "lyrics_motion");

  // The scrolling ticker is still there and comes back.
  if (songs.length) {
    await setSelect("Lyrics display", "scroll", "lyrics_display");
    await openSong(songs[0].title, "scroll");
    const tv = await page.evaluate(() => window.__jkTicker.verify());
    check("Settings > Lyrics display > Scrolling ticker", tv.wrongLetter === 0 && tv.backwards === 0 && tv.lookaheadMisses === 0, `ticker drawn (display "scroll"), wrong words ${tv.wrongLetter}, lookahead >= ${tv.minLookaheadS.toFixed(2)} s`);
    await setSelect("Lyrics display", "page", "lyrics_display");
  }

  // Screenshots (1440x900, skin on).
  const demo = songs.find((x) => /twinkle/i.test(x.title)) ?? songs[0];
  const other = songs.find((x) => x !== demo);
  if (demo) {
    await openSong(demo.title);
    const lines = await page.evaluate(() => window.__jkTicker.pageLines());
    // A lead-in after a pause in the song (else the intro's).
    const led = lines.find((l) => l.index > 0 && l.leadIn) ?? lines.find((l) => l.leadIn);
    if (led) await capture("page-1-lead-in-1440x900", led.leadIn.from + 0.25 * (led.leadIn.to - led.leadIn.from));
    const held = lines.flatMap((l) => l.words.filter((w) => w.end - w.start >= 0.5 && w.text.length >= 4).map((w) => ({ ...w, line: l.index })));
    const w = held[Math.floor(held.length / 3)] ?? lines[1].words[1];
    const p = await capture("page-2-wipe-1440x900", (w.start + w.end) / 2);
    check("mid-word screenshot: the wipe is on the word being sung", p.wordSung !== null && p.edgeWord === p.sungWord, `line ${p.line + 1}: sung word ${p.sungWord + 1}, wipe on word ${p.edgeWord + 1}`);
    const brk = lines.find((l, i) => i > 0 && l.breakBefore);
    if (brk) await capture("page-4-countdown-1440x900", brk.current - 2.5);
    for (const vp of [
      { width: 1280, height: 800 },
      { width: 3840, height: 2160 },
    ]) {
      await page.setViewportSize(vp);
      await sleep(800);
      await capture(`page-2-wipe-${vp.width}x${vp.height}`, (w.start + w.end) / 2);
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    await sleep(800);
  }
  if (other) {
    await openSong(other.title);
    const lines = await page.evaluate(() => window.__jkTicker.pageLines());
    const P = await page.evaluate(() => window.__jkTicker.fontPx);
    const wrapped = lines.filter((l) => l.rows > 1);
    const shrunk = lines.filter((l) => l.rows === 1 && l.px < P - 0.5);
    const pickMid = (l) => {
      const w = l.words[Math.floor(l.words.length / 2)];
      return (w.start + w.end) / 2;
    };
    if (wrapped.length) await capture("page-3-wrapped-1440x900", pickMid(wrapped[0]));
    if (shrunk.length) await capture("page-3-shrunk-1440x900", pickMid(shrunk.sort((a, b) => a.px - b.px)[0]));
    const mid = lines[Math.floor(lines.length / 3)];
    await capture("page-2-wipe-other-1440x900", pickMid(mid));
  }
  check("screenshots", report.screenshots.length >= 5, report.screenshots.map((s) => `${s.name}: sung word ${s.probe.sungWord + 1} / wipe on word ${s.probe.edgeWord + 1}`).join(", "));
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
writeFileSync(join(outDir, "real-app-page-lyrics.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 1));
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
