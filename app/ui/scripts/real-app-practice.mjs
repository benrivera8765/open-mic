// SPDX-License-Identifier: GPL-3.0-or-later
// Practice mode in the REAL app (Work Order 2, Stage 3b / 5): the release exe from its own
// portable folder, driven over WebView2's DevTools, with generated singers fed through
// the real capture-side path by `practice_test_input` (the app runs with the internal
// `--practice-test` flag). This PC has no microphone; no microphone is ever opened and no
// sound is made: Settings/settings.json selects the engine's "null" output device (no audio
// hardware, paced in real time) and master volume 0 before the app starts.
//
//   node scripts/real-app-practice.mjs <root> <out dir> <singers dir> [--only a,b,...] [--soak MINUTES]
//
// <root> is the portable folder (Windows/Jukeoke.exe, Library/ with the songs below).
// <singers dir> holds generated singer WAVs (48 kHz float, sample k = song time k/48000):
// demo_perfect.wav, demo_silent.wav, demo_cents_70.wav, demo_octave_1.wav, key_<k>.wav
// (made by `jukeoke-cli practice run <demo> --singer ... --write-input ...`).
// Library songs used: "Twinkle, Twinkle, Little Star" (the demo song with its guide) and
// one song without a guide (title given by NOGUIDE_TITLE, default "Feel").
//
// Writes <out dir>/practice-real-app.json and screenshots. Never prints lyrics.
import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, existsSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const args = process.argv.slice(2);
const [rootArg, outArg, singersArg] = args;
if (!rootArg || !outArg || !singersArg) {
  console.error("usage: node scripts/real-app-practice.mjs <root> <out dir> <singers dir> [--only a,b] [--soak MINUTES]");
  process.exit(2);
}
const root = resolve(rootArg);
const outDir = resolve(outArg);
const singers = resolve(singersArg);
const onlyIdx = args.indexOf("--only");
const only = onlyIdx > 0 ? new Set(args[onlyIdx + 1].split(",")) : null;
const soakIdx = args.indexOf("--soak");
const soakMinutes = soakIdx > 0 ? Number(args[soakIdx + 1]) : 0;
const exe = join(root, "Windows", "Jukeoke.exe");
const DEMO = "Twinkle, Twinkle, Little Star";
const NOGUIDE = process.env.NOGUIDE_TITLE ?? "Feel";
mkdirSync(outDir, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const report = { exe, root, started: new Date().toISOString(), checks: [], scenarios: {}, errors: [] };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${typeof detail === "string" ? detail : JSON.stringify(detail)}`);
};
const save = () => writeFileSync(join(outDir, "practice-real-app.json"), JSON.stringify(report, null, 2));

function writeSettings(extra = {}) {
  mkdirSync(join(root, "Settings"), { recursive: true });
  const base = {
    ui: { skin: true, bubbles: true, motion: "full", lyrics_text_size: 56, marker_position: 0.33, library_sort: "title", fullscreen_stage: false },
    // The null device: no audio hardware at all; master 0 on top (WO2 test rule).
    audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {}, key_mode: "realtime" },
  };
  const p = join(root, "Settings", "settings.json");
  let cur = {};
  try {
    cur = JSON.parse(readFileSync(p, "utf8"));
  } catch {
    /* fresh */
  }
  const merged = { ...cur, ...base, ...extra, ui: { ...(cur.ui ?? {}), ...base.ui, ...(extra.ui ?? {}) }, audio: { ...(cur.audio ?? {}), ...base.audio, ...(extra.audio ?? {}) } };
  writeFileSync(p, JSON.stringify(merged, null, 2));
}

let child = null;
let browser = null;
let page = null;
let port = 0;
async function launch() {
  port = 9300 + Math.floor(Math.random() * 500);
  // The DevTools port is added to the web view's arguments. The variable takes the place
  // of the app's own arguments, so the app's defaults (wry: no "mini menu", no PDF UI, no
  // SmartScreen lookups) are passed again: the network audit then sees the web view as the
  // real app runs it, not a web view with SmartScreen switched back on by the test.
  child = spawn(exe, ["--practice-test"], {
    env: {
      ...process.env,
      JK_BUILD: "",
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", () => {});
  child.stderr.on("data", () => {});
  for (let i = 0; ; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
      break;
    } catch (e) {
      if (i > 80) throw e;
      await sleep(500);
    }
  }
  const ctx = browser.contexts()[0];
  page = ctx.pages()[0] ?? (await ctx.waitForEvent("page"));
  page.on("pageerror", (e) => report.errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && report.errors.push(m.text()));
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForSelector(".jk-strip, .title-strip, .library__empty", { timeout: 60000 });
  await sleep(800);
}
async function quit() {
  try {
    await browser?.close();
  } catch {
    /* gone */
  }
  if (child && child.exitCode === null) {
    child.kill();
    await new Promise((r) => child.once("exit", r));
  }
  child = null;
}

const invoke = async (cmd, a = {}) => {
  const r = await page.evaluate(([c, x]) => window.__TAURI_INTERNALS__.invoke(c, x).then((ok) => ({ ok }), (err) => ({ err })), [cmd, a]);
  if ("err" in r) throw new Error(typeof r.err === "string" ? r.err : JSON.stringify(r.err));
  return r.ok;
};
const state = () => invoke("practice_state");
const player = () => invoke("player_state");
const shot = async (name) => {
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  console.log(`  screenshot ${file}${overflow > 0 ? ` (overflow ${overflow}px)` : ""}`);
  return file;
};
const nav = async (label) => {
  const plate = page.locator(".skin-nav .jk-plate", { hasText: label });
  if (await plate.count()) await plate.first().click();
  await sleep(500);
};
async function openSong(title) {
  await nav("Library");
  await page.waitForSelector(".jk-strip, .title-strip");
  await page.locator(".jk-strip, button.title-strip", { hasText: title }).first().click();
  await page.waitForSelector(".stage__practice", { timeout: 30000 });
  await sleep(700);
  // The generated singers sing in the written key. A song opens in the key saved with it
  // (WO2 4.6), which an earlier key-change run may have left elsewhere (round 3's first
  // attempt opened the demo song at -3 and its perfect singer was graded 3 semitones off),
  // so every scenario starts from key 0; the key scenarios set their own keys after this.
  const ps = await player();
  (report.opened_keys ??= []).push({ title, key_requested: ps.key_requested ?? null, key_heard: ps.key_heard ?? null });
  // Through the UI's own key shortcut, so the page's song settings follow (a player_set
  // behind the UI's back left its key at -3, and the next three "[" presses asked for -6).
  const n = ps.key_requested ?? 0;
  for (let i = 0; i < Math.abs(n); i++) await page.keyboard.press(n < 0 ? "]" : "[");
  if (n !== 0) {
    await waitFor("key 0 heard", 60, async () => {
      const p = await player();
      return (p.key_requested ?? 0) === 0 && (p.key_heard ?? 0) === 0;
    });
  }
}
const waitFor = async (what, secs, fn) => {
  const t0 = Date.now();
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() - t0 > secs * 1000) throw new Error(`timed out (${secs} s) waiting for ${what}`);
    await sleep(100);
  }
};
const practiceOn = () => page.evaluate(() => window.__jkPractice?.store.get().on ?? false);
async function pressPractice() {
  await page.locator(".stage__practice").first().click();
  await sleep(300);
}
async function stopPractice() {
  if (await practiceOn()) await pressPractice();
  await waitFor("practice off", 10, async () => !(await practiceOn()));
  await sleep(300);
}
async function pauseAt0() {
  const p = await player();
  if (p.state === "playing") await invoke("player_pause");
  await invoke("player_seek", { position_s: 0 });
  await sleep(300);
}
async function play() {
  const p = await player();
  if (p.state !== "playing") await page.locator(".skin-transport__play, .transport__play").first().click();
  await waitFor("playing", 10, async () => (await player()).state === "playing");
}

/**
 * Samples the lane's own frame (window.__jkLane.last(): target note, singer marker, lit
 * parts, key at the now line) every ~33 ms while `until` is false.
 */
async function sampleLane(untilFn, maxS) {
  const t0 = Date.now();
  const frames = [];
  while (Date.now() - t0 < maxS * 1000) {
    const f = await page.evaluate(() => {
      const L = window.__jkLane?.last?.();
      if (!L) return null;
      return {
        t: L.t,
        key: L.key,
        octave: L.octave,
        target: L.target ? { note: L.target.note, lane: L.target.lane, name: L.target.name, graded: L.target.graded } : null,
        singer: L.singer ? { lane: L.singer.lane, name: L.singer.name, edge: L.singer.edge, cue: L.singer.cue } : null,
        lit: L.lit.length / 4,
      };
    });
    if (f) frames.push(f);
    if (await untilFn(f)) break;
    await sleep(33);
  }
  return frames;
}

/**
 * Marker statistics over lane frames: on the target (within half a semitone, octave folded
 * or not). `settled*` counts only frames at least 120 ms after the target note began: the
 * marker shows the newest tracker value, which is 36-62 ms old (APP_API 12.5), so for a
 * moment after each note change it still shows the previous note, and on a song of short
 * notes (rap) those moments are a large share of all frames.
 */
function markerStats(frames, fold) {
  let inNote = 0;
  let shown = 0;
  let onTarget = 0;
  let litFrames = 0;
  let settledShown = 0;
  let settledOn = 0;
  const firstSeen = new Map();
  for (const f of frames) {
    if (f.lit > 0) litFrames++;
    if (!f.target || !f.target.graded) continue;
    if (!firstSeen.has(f.target.note)) firstSeen.set(f.target.note, f.t);
    inNote++;
    if (!f.singer) continue;
    shown++;
    let d = f.singer.lane - f.target.lane;
    if (fold) d -= 12 * Math.round(d / 12);
    const on = Math.abs(d) < 0.5;
    if (on) onTarget++;
    if (f.t - firstSeen.get(f.target.note) >= 0.12) {
      settledShown++;
      if (on) settledOn++;
    }
  }
  return { frames: frames.length, inNote, shown, onTarget, settledShown, settledOn, litFrames };
}

async function runSinger(name, wav, { untilResult = true, octave = null, shots = null } = {}) {
  await stopPractice();
  await pauseAt0();
  await invoke("practice_test_input", { source: { kind: "wav", path: wav } });
  if (octave) await invoke("practice_song_set", { song_id: (await player()).song_id, changes: { octave } });
  await pressPractice();
  const st = await waitFor("listening", 20, async () => {
    const s = await state();
    return s.phase === "listening" ? s : null;
  });
  await play();
  let shotTaken = false;
  const frames = await sampleLane(async (f) => {
    if (shots && !shotTaken && f && f.t >= shots.at) {
      shotTaken = true;
      await shot(shots.name);
    }
    return untilResult && (await page.locator(".practice-result").count()) > 0;
  }, 60);
  let result = null;
  if (untilResult) {
    await waitFor("the result screen", 20, async () => (await page.locator(".practice-result").count()) > 0);
    await sleep(600);
    const s = await state();
    result = s.last_result;
  }
  return { st, frames, result };
}

async function scenarioSingers() {
  const out = {};
  await openSong(DEMO);
  // (a) perfect singer, with the practice and result screenshots at 1440x900.
  const p = await runSinger("perfect", join(singers, "demo_perfect.wav"), { shots: { at: 11.0, name: "practice-screen-1440x900" } });
  await shot("result-screen-1440x900");
  out.perfect = { result: p.result, marker: markerStats(p.frames, true) };
  check("perfect singer: marker on the bars, bars light, high grade", p.result?.overall >= 85 && out.perfect.marker.onTarget > 0.8 * out.perfect.marker.shown && out.perfect.marker.litFrames > 50, { overall: p.result?.overall, grade: p.result?.grade, graded: `${p.result?.graded} of ${p.result?.total}`, ...out.perfect.marker });
  // silent
  const s = await runSinger("silent", join(singers, "demo_silent.wav"));
  out.silent = { result: s.result, marker: markerStats(s.frames, true) };
  check("silent singer: no marker, lowest grade", (s.result?.overall ?? 0) <= 1 && out.silent.marker.shown === 0, { overall: s.result?.overall, grade: s.result?.grade, ...out.silent.marker });
  // +70 cents
  const c = await runSinger("cents70", join(singers, "demo_cents_70.wav"));
  out.cents70 = { result: c.result, marker: markerStats(c.frames, true) };
  check("+70 cents: a lower band", c.result?.overall < 80 && c.result?.overall > 20, { overall: c.result?.overall, grade: c.result?.grade, tendency_cents: c.result?.tendency_cents });
  // An octave low: right by default (auto), wrong in strict.
  const o = await runSinger("octave_auto", join(singers, "demo_octave_1.wav"), { octave: { mode: "auto", shift: 0 } });
  out.octave_auto = { result: o.result, marker: markerStats(o.frames, true) };
  const lastOct = o.frames.at(-1)?.octave;
  // The lane moves to the singer's octave once auto has decided (WO2 4.6): the share of the
  // lane's frames after the first phrases (from 10 s) drawn an octave down.
  const late = o.frames.filter((f) => f.t >= 10);
  const laneDown = late.filter((f) => f.octave === -1).length;
  const firstDown = o.frames.find((f) => f.octave === -1)?.t ?? null;
  check("an octave low, default (auto): correct, lane moved to the singer's octave", o.result?.overall >= 85 && late.length > 0 && laneDown >= 0.9 * late.length, { overall: o.result?.overall, grade: o.result?.grade, result_octave: o.result?.octave, lane_octave_last: lastOct, lane_down_frames_after_10s: `${laneDown} of ${late.length}`, lane_moved_at_s: firstDown });
  const os = await runSinger("octave_strict", join(singers, "demo_octave_1.wav"), { octave: { mode: "strict" } });
  out.octave_strict = { result: os.result };
  check("an octave low, strict: fails", (os.result?.overall ?? 0) < 60, { overall: os.result?.overall, grade: os.result?.grade });
  await stopPractice();
  await invoke("practice_song_set", { song_id: (await player()).song_id, changes: { octave: { mode: "auto", shift: 0 } } });
  report.scenarios.singers = out;
  save();
}

/**
 * Practice on real songs: copies of the owner's song and of a Creative Commons song, with
 * guides added by `jukeoke-cli guide add`. `<singers>/real-songs.json` lists them:
 * [{ title, perfect, start_s, seconds }] (`perfect`: a generated singer from that song's
 * guide, sample k = song time k / 48000). For each: the perfect singer from `start_s` for
 * `seconds`, then "Finish" (a take that short is graded and shown, not saved), then the same
 * stretch with digital silence. No screenshots (song lyrics are never copied anywhere).
 */
async function realSongs() {
  const list = join(singers, "real-songs.json");
  if (!existsSync(list)) {
    console.log("  (no real-songs.json: real songs skipped)");
    return;
  }
  const songs = JSON.parse(readFileSync(list, "utf8"));
  const out = {};
  for (const song of songs) {
    await openSong(song.title);
    const run = async (source) => {
      await stopPractice();
      const p = await player();
      if (p.state === "playing") await invoke("player_pause");
      await invoke("player_seek", { position_s: song.start_s });
      await invoke("practice_test_input", { source });
      await pressPractice();
      await waitFor("listening", 30, async () => (await state()).phase === "listening");
      await play();
      const t0 = Date.now();
      const frames = await sampleLane(async () => Date.now() - t0 > song.seconds * 1000, song.seconds + 10);
      const fin = await invoke("practice_finish");
      await sleep(400);
      return { result: fin.result, marker: markerStats(frames, true) };
    };
    const perfect = await run({ kind: "wav", path: song.perfect });
    const silent = await run({ kind: "silence" });
    await stopPractice();
    // Titles are kept out of the report (the owner's song); the index says which.
    const key = `song_${Object.keys(out).length + 1}`;
    out[key] = {
      start_s: song.start_s,
      seconds: song.seconds,
      perfect: { overall: perfect.result?.overall, grade: perfect.result?.grade, graded: perfect.result?.graded, total: perfect.result?.total, saved: perfect.result?.saved, trust: perfect.result?.trust, ...perfect.marker },
      silent: { overall: silent.result?.overall, grade: silent.result?.grade, graded: silent.result?.graded, ...silent.marker },
    };
    const o = out[key];
    check(`real song ${Object.keys(out).length} (${song.label ?? "song"}): perfect generated singer high, silence lowest, marker on the bars`, o.perfect.overall >= 85 && o.perfect.graded > 0 && o.perfect.settledShown > 0 && o.perfect.settledOn >= 0.8 * o.perfect.settledShown && (o.silent.overall ?? 0) <= 1 && o.silent.shown === 0, o);
  }
  await invoke("practice_test_input", { source: null });
  report.scenarios.real_songs = out;
  save();
}

/** (b) A key change during the song: the bars, the lit key, the names and the target move when it is HEARD. */
async function keyChange(mode) {
  await stopPractice();
  await invoke("settings_set", { changes: { audio: { key_mode: mode } } });
  await nav("Library");
  await openSong(DEMO);
  await pauseAt0();
  await invoke("practice_test_input", { source: { kind: "wav", path: join(singers, "demo_perfect.wav") } });
  await pressPractice();
  await waitFor("listening", 20, async () => (await state()).phase === "listening");
  await play();
  // Render ahead makes the change heard only once the rest of the song is rendered in the
  // new key on a low-priority thread (about 1 s on an idle PC, half a minute when other
  // work fills every core), so there it is asked early enough for notes to follow.
  const askAt = mode === "prerender" ? 3.0 : 9.0;
  await waitFor(`song at ${askAt} s`, 20, async () => (await player()).position_s >= askAt);
  const before = await page.evaluate(() => window.__jkLane.last());
  // Three presses of "[" (the key control's shortcut), as the singer would.
  const clickPos = (await player()).position_s;
  for (let i = 0; i < 3; i++) await page.keyboard.press("[");
  // Frame by frame: when does the lane's key at the now line change? Three quick presses
  // are usually one change (0 -> -3); on a busy computer they can arrive far enough apart
  // to be heard as steps (0 -> -1 -> -2 -> -3). Every step the lane takes is checked
  // against the moment the engine says that key became HEARD (its key history, read
  // through practice_target_at, APP_API 12.1), never against the press.
  const t0 = Date.now();
  const laneSteps = [];
  let laneKey = before?.key ?? 0;
  const pendings = [];
  while (Date.now() - t0 < 36000) {
    const [f, ps] = await Promise.all([page.evaluate(() => window.__jkLane.last()), player()]);
    const p = ps.key_pending;
    if (p?.at_s != null && !pendings.some((x) => x.semitones === p.semitones && Math.abs(x.at_s - p.at_s) < 1e-6)) pendings.push({ semitones: p.semitones, at_s: p.at_s, seen_at_pos: ps.position_s });
    if (f && f.key !== laneKey) {
      laneSteps.push({ key: f.key, t: f.t, target: f.target ? { name: f.target.name, lane: f.target.lane } : null });
      laneKey = f.key;
    }
    if (laneKey === -3 && ps.key_heard === -3) break;
    await sleep(15);
  }
  // The engine's history is asked within THIS playback segment (player_state.key_segment):
  // without a segment id, key_at answers from the newest segment that has already played
  // the position, so a probe a little ahead of the playback (the search starts 80 ms past
  // the lane's step) fell through to an older take of the same song in key 0 (round 3's
  // first full run: "heard from null" in render-ahead mode while the lane, the target and
  // the grading had all switched at the heard moment). Also wait until the probes are
  // behind the playback.
  const segNow = (await player()).key_segment;
  await sleep(300);
  const keyAt = async (t) => (await invoke("practice_target_at", segNow == null ? { position_s: t } : { position_s: t, segment: segNow })).key;
  // The first song time from which the engine's history says key k is heard (1 ms steps).
  const heardFrom = async (k, hi) => {
    let lo = clickPos;
    if ((await keyAt(hi)) !== k) return null;
    while (hi - lo > 0.001) {
      const mid = (lo + hi) / 2;
      if ((await keyAt(mid)) === k) hi = mid;
      else lo = mid;
    }
    return hi;
  };
  for (const st of laneSteps) {
    // Just after the lane step (before any next step): the key there is the one searched for.
    st.heard_from_s = await heardFrom(st.key, st.t + 0.08);
    st.lane_minus_heard_ms = st.heard_from_s != null ? Math.round((st.t - st.heard_from_s) * 1000) : null;
  }
  // The lane draws each frame with the UI's key timeline (key_at(t) of APP_API 12.1, built
  // from playback://clock's key_pending): the song time at which IT switches is the moment
  // the bars, the lit key and the names move, exact to the frame whatever the frame rate
  // or how often this script looks. Each switch against the engine's heard moment:
  const uiChanges = await page.evaluate(() => window.__jkPractice.keys.view().changes.map((c) => ({ at: c.at, semitones: c.semitones, from: c.from })));
  const uiSwitches = [];
  for (const c of uiChanges) {
    if (c.at < clickPos - 0.5) continue;
    const h = await heardFrom(c.semitones, c.at + 0.08);
    uiSwitches.push({ key: c.semitones, from: c.from, ui_at_s: c.at, heard_from_s: h, ui_minus_heard_ms: h != null ? Math.round((c.at - h) * 1000) : null });
  }
  const last = laneSteps.at(-1) ?? null;
  const heardAt = last?.key === -3 ? last.heard_from_s : null;
  // When the engine's history and the UI disagree, what the history says around the change:
  // every 10 ms from the press to 1 s past the lane's last step, the key by
  // practice_target_at (no segment), player_key_at without and with the current segment.
  let historyGrid = null;
  if (laneSteps.some((st) => st.heard_from_s == null) || uiSwitches.some((u) => u.heard_from_s == null)) {
    const psNow = await player();
    historyGrid = { segment_now: psNow.key_segment, position_now: psNow.position_s, key_heard_now: psNow.key_heard, rows: [] };
    const end = (last?.t ?? clickPos) + 1.0;
    for (let t = clickPos; t <= end; t += 0.01) {
      const [a, b, c] = await Promise.all([
        keyAt(t),
        invoke("player_key_at", { position_s: t }).then((r) => r.semitones),
        invoke("player_key_at", { position_s: t, segment: psNow.key_segment }).then((r) => r.semitones),
      ]);
      const prev = historyGrid.rows.at(-1);
      if (!prev || prev.target_key !== a || prev.key_at !== b || prev.key_at_segment !== c) historyGrid.rows.push({ t: Math.round(t * 1000) / 1000, target_key: a, key_at: b, key_at_segment: c });
    }
  }
  // The grading target around the switch to -3 (the scorer's rule, APP_API 12.1).
  const tBefore = heardAt != null ? await invoke("practice_target_at", { position_s: heardAt - 0.05 }) : null;
  const tAfter = heardAt != null ? await invoke("practice_target_at", { position_s: heardAt + 0.05 }) : null;
  await sleep(2500);
  await shot(`practice-after-key-change-${mode}`);
  const fin = await invoke("practice_finish");
  // Notes already sung keep their key; notes after the switch are graded in the new key.
  const done = await page.evaluate(() => window.__jkPractice.batches.flatMap((b) => b.done));
  const keyOf = {};
  for (const d of done) keyOf[d[0]] = { credit: d[1], key: d[2] };
  const out = {
    mode,
    click_song_pos: clickPos,
    lane_key_before: before?.key,
    lane_steps: laneSteps,
    ui_switches: uiSwitches,
    pending_seen: pendings,
    history_grid: historyGrid,
    heard_from_s: heardAt,
    delay_click_to_heard_ms: heardAt != null ? Math.round((heardAt - clickPos) * 1000) : null,
    lane_switch_t: last?.t ?? null,
    lane_switch_minus_heard_ms: last?.lane_minus_heard_ms ?? null,
    target_before: tBefore,
    target_after: tAfter,
    notes: Object.entries(keyOf).map(([n, v]) => [Number(n), v.key, v.credit]),
    result: fin.result ? { overall: fin.result.overall, key_changes: fin.result.key_changes, key: fin.result.key } : null,
  };
  report.scenarios[`key_change_${mode}`] = out;
  const sungBefore = out.notes.filter(([, k, c]) => k === 0 && c != null && c > 0.8).length;
  const after = out.notes.filter(([, k]) => k === -3).length;
  // WO2 section 9: the target follows the audio produced to within 50 ms. The UI's switch
  // times decide; the frames this script sampled (every 15 ms plus the time a page call
  // takes, far more when other work fills the processor) only have to agree within 150 ms.
  const uiOk = uiSwitches.length > 0 && uiSwitches.some((u) => u.key === -3) && uiSwitches.every((u) => u.heard_from_s != null && u.heard_from_s > clickPos && Math.abs(u.ui_minus_heard_ms) <= 50);
  const stepsOk = uiOk && laneSteps.length > 0 && laneSteps.every((st) => st.heard_from_s != null && st.heard_from_s > clickPos && st.lane_minus_heard_ms >= -5 && st.lane_minus_heard_ms < 150);
  check(
    `key change during the song (${mode}): everything follows the key HEARD`,
    heardAt != null && stepsOk && tBefore?.key !== -3 && tAfter?.key === -3 && sungBefore > 0 && after > 0,
    {
      click_at_s: clickPos,
      heard_from_s: heardAt,
      delay_ms: out.delay_click_to_heard_ms,
      ui_switches: uiSwitches.map((u) => `${u.from} -> ${u.key} at ${u.ui_at_s.toFixed(3)} s, heard from ${u.heard_from_s?.toFixed(3)} (${u.ui_minus_heard_ms} ms)`),
      lane_frames: laneSteps.map((st) => `${st.key} first drawn in a frame sampled at ${st.t.toFixed(3)} s (${st.lane_minus_heard_ms} ms after heard)`),
      target_key_before: tBefore?.key,
      target_key_after: tAfter?.key,
      target_before: tBefore?.name,
      target_after: tAfter?.name,
      notes_kept_key0: sungBefore,
      notes_in_new_key: after,
    },
  );
  await stopPractice();
  await invoke("player_set", { changes: { semitones: 0 } });
  save();
}

/** (c) Every key -12..+12 through the real app: player key k, the original-singer stand-in at k. */
async function everyKey() {
  await stopPractice();
  await invoke("settings_set", { changes: { audio: { key_mode: "realtime" } } });
  await nav("Library");
  await openSong(DEMO);
  const rows = [];
  for (let k = -12; k <= 12; k++) {
    await stopPractice();
    await pauseAt0();
    await invoke("player_set", { changes: { semitones: k } });
    await invoke("practice_test_input", { source: { kind: "wav", path: join(singers, `key_${k}.wav`) } });
    await pressPractice();
    await waitFor("listening", 20, async () => (await state()).phase === "listening");
    await waitFor("key heard", 10, async () => (await player()).key_heard === k);
    await play();
    await waitFor("song at 18.5 s", 40, async () => (await player()).position_s >= 18.5);
    const t = await invoke("practice_target_at", { position_s: 3.1 });
    const fin = await invoke("practice_finish");
    rows.push({ key: k, overall: fin.result?.overall ?? null, graded: fin.result?.graded, total: fin.result?.total, result_key: fin.result?.key, target_3s: t?.name, target_key: t?.key });
    console.log(`  key ${k}: ${fin.result?.overall} (key in result ${fin.result?.key}, target at 3.1 s ${t?.name})`);
  }
  await stopPractice();
  await invoke("player_set", { changes: { semitones: 0 } });
  report.scenarios.every_key = rows;
  const base = rows.find((r) => r.key === 0)?.overall ?? 0;
  check("every key -12..+12: the grade stays within a few points of key 0", rows.every((r) => r.overall != null && Math.abs(r.overall - base) <= 5 && r.result_key === r.key), rows.map((r) => `${r.key}:${r.overall}`).join(" "));
  save();
}

/** (d) Missing things: no input device (this PC), permission refused, input unplugged, no guide. */
async function missing() {
  const out = {};
  await openSong(DEMO);
  await stopPractice();
  await invoke("practice_test_input", { source: null });
  await pressPractice();
  const st = await waitFor("watch only", 20, async () => {
    const s = await state();
    return s.phase === "watch_only" ? s : null;
  });
  await sleep(800);
  const chip = await page.evaluate(() => document.querySelector(".practice-chip--watch")?.getAttribute("title") ?? document.querySelector(".practice-note")?.textContent ?? null);
  await shot("watch-only-no-input-device");
  out.no_device = { code: st.watch_only?.code, message: st.watch_only?.message, on_screen: chip };
  check("no input device (this PC): watch only, explained", st.watch_only?.code === "no_device" && !!chip, out.no_device);
  await stopPractice();

  await invoke("practice_test_input", { source: { kind: "permission_denied" } });
  // The keyboard shortcut this time (P), as the owner would.
  await page.keyboard.press("p");
  await waitFor("practice on (P)", 10, practiceOn);
  out.shortcut_p = true;
  const sp = await waitFor("watch only", 20, async () => {
    const s = await state();
    return s.phase === "watch_only" ? s : null;
  });
  await sleep(500);
  out.permission = { code: sp.watch_only?.code, message: sp.watch_only?.message };
  await shot("watch-only-permission-refused");
  check("permission refused (simulated): watch only with the exact steps", sp.watch_only?.code === "permission_denied" && /Privacy/.test(sp.watch_only?.message ?? ""), out.permission);
  await stopPractice();
  await invoke("practice_test_input", { source: null });

  // The input unplugged mid-song (a generated singer that stops after 8 s).
  await pauseAt0();
  await invoke("practice_test_input", { source: { kind: "wav", path: join(singers, "demo_perfect.wav"), unplug_after_s: 8 } });
  await pressPractice();
  await waitFor("listening", 20, async () => (await state()).phase === "listening");
  await play();
  const lost = await waitFor("input lost", 30, async () => {
    const s = await state();
    return s.watch_only?.code === "input_lost" ? s : null;
  });
  await sleep(700);
  await shot("input-unplugged-mid-song");
  const fin = await waitFor("the result", 60, async () => {
    const s = await state();
    return s.last_result ? s.last_result : null;
  });
  const phaseAfter = (await state()).phase;
  await sleep(800);
  await shot("result-after-input-lost");
  out.unplugged = { message: lost.watch_only?.message, input_state: lost.input?.state, result_overall: fin.overall, trust: fin.trust, phase_after: phaseAfter };
  check("input unplugged mid-song: plain message, no crash, the take flagged", lost.input?.state === "unplugged" && fin.trust.includes("input_lost"), out.unplugged);
  await stopPractice();
  await invoke("practice_test_input", { source: null });

  // A song without a guide and no Studio in this folder: made on the spot.
  await nav("Library");
  await openSong(NOGUIDE);
  await invoke("practice_test_input", { source: { kind: "silence" } });
  const phases = [];
  await pressPractice();
  let prepShot = false;
  const t0 = Date.now();
  for (;;) {
    const s = await state();
    if (!phases.length || phases.at(-1).phase !== s.phase) phases.push({ phase: s.phase, at_ms: Date.now() - t0, guide: s.guide.state, progress: s.guide.progress });
    if (s.phase === "preparing" && !prepShot) {
      prepShot = true;
      await shot("preparing-guide-on-the-spot");
    }
    if (s.phase !== "preparing") break;
    if (Date.now() - t0 > 60000) break;
    await sleep(50);
  }
  const studio = await invoke("studio_status");
  out.no_guide = { phases, studio_available: studio.available, studio_reason: studio.reason ?? null };
  check("song without a guide, no Studio: built on the spot, then practice", phases.some((p) => p.phase === "preparing") && phases.at(-1).phase === "listening", out.no_guide);
  await stopPractice();
  await invoke("practice_test_input", { source: null });
  report.scenarios.missing = out;
  save();
}

/** The input is open only while practice is on screen: leaving the singing screen closes it; a hidden window suspends it. */
async function leaving() {
  await openSong(DEMO);
  await stopPractice();
  await invoke("practice_test_input", { source: { kind: "silence" } });
  await pressPractice();
  await waitFor("listening", 20, async () => (await state()).phase === "listening");
  const before = await state();
  // A hidden window suspends the input (the UI calls practice_suspend_input).
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  const hidden = await waitFor("suspended", 5, async () => {
    const s = await state();
    return s.input?.state === "suspended" ? s : null;
  });
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  const back = await waitFor("running again", 5, async () => {
    const s = await state();
    return s.input?.state === "running" ? s : null;
  });
  await nav("Library");
  const after = await waitFor("practice off", 10, async () => {
    const s = await state();
    return !s.active ? s : null;
  });
  report.scenarios.leaving = { before: before.phase, hidden: hidden.input?.state, back: back.input?.state, after_leaving: { active: after.active, phase: after.phase } };
  check("the input is open only while practice is on screen (hidden: suspended; leaving the singing screen: closed)", hidden.input?.state === "suspended" && back.input?.state === "running" && !after.active, report.scenarios.leaving);
  await invoke("practice_test_input", { source: null });
  save();
}

/** (g, part) Results in the song's slot survive a restart. */
async function persistence() {
  const lib = await invoke("library_list");
  const demo = lib.songs.find((s) => s.title === DEMO);
  const h1 = await invoke("practice_history", { song_id: demo.song_id });
  await quit();
  await launch();
  const h2 = await invoke("practice_history", { song_id: demo.song_id });
  const lib2 = await invoke("library_list");
  const best = lib2.songs.find((s) => s.title === DEMO)?.practice_best;
  report.scenarios.persistence = { before: { recent: h1.recent.length, best: h1.best?.overall }, after: { recent: h2.recent.length, best: h2.best?.overall }, library_best: best };
  check("results kept in the song's slot survive a restart", h1.recent.length > 0 && h2.recent.length === h1.recent.length && h2.best?.overall === h1.best?.overall, report.scenarios.persistence);
  save();
}

/** Network connections of the app and its web view processes. */
function connections(pid) {
  try {
    const ps = `$ids = @(${pid}) + @(Get-CimInstance Win32_Process | Where-Object { $_.ParentProcessId -eq ${pid} } | ForEach-Object { $_.ProcessId }); $ids += @(Get-CimInstance Win32_Process | Where-Object { $ids -contains $_.ParentProcessId } | ForEach-Object { $_.ProcessId }); Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess } | ForEach-Object { "$($_.OwningProcess) $((Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue).ProcessName) $($_.LocalAddress):$($_.LocalPort) $($_.RemoteAddress):$($_.RemotePort) $($_.State)" }`;
    return execFileSync("powershell", ["-NoProfile", "-Command", ps], { encoding: "utf8" }).split(/\r?\n/).filter(Boolean);
  } catch (e) {
    return [`error: ${e}`];
  }
}

/** (e) A long real-app run with practice on, fed by the generated singer: bubbles on, busy threads. */
// `--soak-control`: the same run with practice OFF (the ordinary singing screen, the song
// looping), to tell what practice adds from what the computer's load does on its own.
const soakControl = args.includes("--soak-control");
async function soak(minutes, threads = 14) {
  await openSong(DEMO);
  await stopPractice();
  await invoke("settings_set", { changes: { ui: { skin: true, bubbles: true, motion: "full" } } });
  // The generated singer sings in the written key: the song opens in the key saved with it
  // (WO2 4.6), which an earlier key-change run may have left elsewhere (the round-2 soak ran
  // at -3, so its perfect singer was graded 3 semitones off).
  await invoke("player_set", { changes: { semitones: 0 } });
  await pauseAt0();
  if (!soakControl) {
    await invoke("practice_test_input", { source: { kind: "wav", path: join(singers, "demo_perfect.wav") } });
    await pressPractice();
    await waitFor("listening", 20, async () => (await state()).phase === "listening");
  }
  const BURNER = `const { Worker } = require("node:worker_threads"); const n = Number(process.argv[1]); for (let i = 0; i < n; i++) new Worker("let x=0; for(;;){ for(let j=0;j<1e6;j++) x=(x*1103515245+12345)|0; }", { eval: true }); setTimeout(() => process.exit(0), ${Math.round(minutes * 60 + 120)} * 1000);`;
  const burner = spawn(process.execPath, ["-e", BURNER, String(threads)], { stdio: "ignore" });
  const pid = child.pid;
  // Whole-app memory: the app and the web view processes it started (and their children).
  // The process tree is looked up with CIM, which can fail or time out under the 14 busy
  // threads (the first 20-minute run lost every reading after 13 minutes that way), so the
  // last tree found is kept and the working sets are read with Get-Process, with a retry.
  let tree = [pid];
  const findTree = () => {
    try {
      const ps = `$ids = @(${pid}) + @(Get-CimInstance Win32_Process | Where-Object { $_.ParentProcessId -eq ${pid} } | ForEach-Object { $_.ProcessId }); $ids += @(Get-CimInstance Win32_Process | Where-Object { $ids -contains $_.ParentProcessId } | ForEach-Object { $_.ProcessId }); ($ids | Select-Object -Unique) -join ','`;
      const ids = execFileSync("powershell", ["-NoProfile", "-Command", ps], { encoding: "utf8", timeout: 60000 })
        .trim()
        .split(",")
        .map(Number)
        .filter((x) => x > 0);
      if (ids.length > 1) tree = ids;
    } catch {
      /* keep the last tree */
    }
  };
  const memErrors = [];
  const mem = () => {
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        const ps = `(Get-Process -Id ${tree.join(",")} -ErrorAction SilentlyContinue | Measure-Object -Property WorkingSet64 -Sum).Sum`;
        const v = Number(execFileSync("powershell", ["-NoProfile", "-Command", ps], { encoding: "utf8", timeout: 60000 }).trim());
        if (v > 0) return v;
        findTree();
      } catch (e) {
        memErrors.push(String(e).slice(0, 200));
      }
    }
    return null;
  };
  findTree();
  const samples = [];
  const t0 = Date.now();
  await play();
  let loops = 0;
  let results = 0;
  let lastRes = null;
  const takeGrades = []; // each graded take's overall (the perfect singer: near 100)
  let frameStats = [];
  while (Date.now() - t0 < minutes * 60000) {
    const [ps, s, diag] = await Promise.all([player(), state(), invoke("player_diagnostics")]);
    if (s.last_result && s.last_result.date !== lastRes) {
      lastRes = s.last_result.date;
      results++;
      takeGrades.push(s.last_result.overall ?? null);
    }
    // The song (40 s) ended: "Sing again" on the result screen (back to 0, a new take, playing).
    if (soakControl && ps.state === "ended") {
      loops++;
      await invoke("player_seek", { position_s: 0 });
      await invoke("player_play");
      await sleep(500);
    } else if (!soakControl && (ps.state === "ended" || (s.phase === "result" && ps.state !== "playing"))) {
      loops++;
      const again = page.locator(".practice-result__again");
      if (await again.count()) await again.first().click();
      else await invoke("practice_again");
      await sleep(500);
    }
    const lane = await page.evaluate(() => ({ frames: window.__jkLane?.stats.frames ?? 0, draw: window.__jkLane?.stats }));
    frameStats.push({ at: Date.now() - t0, frames: lane.frames });
    if (samples.length % 10 === 0) {
      if (samples.length % 100 === 0) findTree();
      samples.push({ at_s: Math.round((Date.now() - t0) / 1000), underruns: ps.underruns, starved: ps.device_starved, peak_db: ps.output_peak_db, mem_bytes: mem(), processes: tree.length, phase: s.phase, take: s.take, dropped_blocks: s.input?.dropped_blocks, diag: { min_lead_ms: diag?.min_lead_ms, max_callback_us: diag?.max_callback_us, worker_max_block_ms: diag?.worker_max_block_ms } });
      const mb = samples.at(-1).mem_bytes;
      console.log(`  soak ${samples.at(-1).at_s} s: underruns ${ps.underruns}, mem ${mb ? (mb / 1e6).toFixed(0) + " MB" : "n/a"} (${tree.length} processes), takes ${results}, phase ${s.phase}`);
    } else {
      samples.push({ at_s: Math.round((Date.now() - t0) / 1000), underruns: ps.underruns, phase: s.phase });
    }
    await sleep(1000);
  }
  burner.kill();
  // Frame rate of the lane over the run.
  const fr = frameStats.length > 1 ? (frameStats.at(-1).frames - frameStats[0].frames) / ((frameStats.at(-1).at - frameStats[0].at) / 1000) : null;
  const rafRate = await page.evaluate(async () => {
    let n = 0;
    const t = performance.now();
    await new Promise((r) => {
      const f = () => {
        n++;
        if (performance.now() - t < 3000) requestAnimationFrame(f);
        else r();
      };
      requestAnimationFrame(f);
    });
    return n / ((performance.now() - t) / 1000);
  });
  const final = await player();
  const st = await state();
  const lib = await invoke("library_list");
  const demo = lib.songs.find((s) => s.title === DEMO);
  const hist = await invoke("practice_history", { song_id: demo.song_id });
  const mems = samples.map((s) => s.mem_bytes).filter((x) => x);
  report.scenarios.soak = {
    minutes,
    threads,
    underruns: final.underruns,
    device_starved: final.device_starved,
    lane_frames_per_s: fr,
    raf_per_s_end: rafRate,
    takes_graded: results,
    take_grades: takeGrades,
    loops,
    history_recent: hist.recent.length,
    best: hist.best?.overall,
    mem_mb_first: mems[0] / 1e6,
    mem_mb_max: Math.max(...mems) / 1e6,
    mem_mb_last: mems.at(-1) / 1e6,
    dropped_blocks: st.input?.dropped_blocks,
    mem_readings: mems.length,
    mem_errors: memErrors.slice(0, 10),
    connections: connections(child.pid),
    samples,
  };
  report.scenarios.soak.control = soakControl;
  report.scenarios.soak.diag_end = await invoke("player_diagnostics").catch(() => null);
  const memOk = mems.length >= 0.9 * samples.filter((x) => "mem_bytes" in x).length && Math.max(...mems) < 1e9;
  check(`${minutes}-minute real-app run with practice ${soakControl ? "OFF (control)" : "on"}, bubbles on, ${threads} busy threads`, final.underruns === 0 && (soakControl || fr > 50) && memOk, {
    underruns: final.underruns,
    lane_fps: fr,
    raf_fps_end: rafRate,
    mem_mb: [mems[0] / 1e6, Math.max(...mems) / 1e6, mems.at(-1) / 1e6].map((x) => Math.round(x)),
    takes: results,
    history_recent: hist.recent.length,
  });
  await stopPractice();
  save();
}

try {
  writeSettings();
  await launch();
  const pid = child.pid;
  report.pid = pid;
  const want = (k) => !only || only.has(k);
  if (soakMinutes > 0) {
    await soak(soakMinutes);
  } else {
    if (want("missing")) await missing();
    if (want("singers")) await scenarioSingers();
    if (want("songs")) await realSongs();
    report.connections_mid = connections(child.pid);
    if (want("key")) {
      await keyChange("realtime");
      await keyChange("prerender");
    }
    if (want("keys")) await everyKey();
    if (want("leave")) await leaving();
    if (want("persist")) await persistence();
    report.connections_end = connections(child.pid);
  }
} catch (e) {
  report.errors.push(`script: ${e?.stack ?? e}`);
  console.error(e);
  try {
    await shot("error-state");
  } catch {
    /* ignore */
  }
} finally {
  report.finished = new Date().toISOString();
  save();
  await quit();
  const failed = report.checks.filter((c) => !c.pass).length;
  console.log(`${report.checks.length - failed} passed, ${failed} failed; errors ${report.errors.length}`);
  process.exit(failed || report.errors.length ? 1 : 0);
}
