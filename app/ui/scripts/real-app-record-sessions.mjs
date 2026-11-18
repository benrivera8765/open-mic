// SPDX-License-Identifier: GPL-3.0-or-later
// The owner's request of 18 Nov 2026 in the REAL release app (DECISIONS D-221..):
// "Record & play" and "Record songs separately", driven over WebView2's DevTools, with
// a TEST media player this script makes itself: a page in its own Microsoft Edge
// instance (a throwaway profile under <out dir>) that plays generated tones (-60.2 dBFS
// peak, digital-silence gaps, one gapless join, one track skipped half-way, an
// "Advertisement") and announces them through navigator.mediaSession, so Windows lists
// it as a media session like any music app.
//
// SAFETY: the app is only ever pointed at that test session (found by its artist
// "Jukeoke Test Player"; the run stops if it is not exactly one). Other players that
// are open are listed (read only) and never played or paused. The app plays nothing
// itself (null:48000 device, master 0). Nothing above -60 dBFS is played.
//
//   node scripts/real-app-record-sessions.mjs <Jukeoke.exe> <root> <out dir> <player dir> [--no-studio]
//
// <player dir> holds album.wav, cover*.png, index.html and truth.json
// ($JK_BUILD/recsessions-live/gen-player.mjs makes them). Report: <out dir>/record-sessions.json.
import { spawn, spawnSync } from "node:child_process";
import { createReadStream, existsSync, linkSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import http from "node:http";
import { extname, join } from "node:path";
import { chromium } from "playwright-core";

const args = process.argv.slice(2);
const noStudio = args.includes("--no-studio");
const [exe, root, outDir, playerDir] = args.filter((a) => !a.startsWith("--"));
if (!exe || !root || !outDir || !playerDir) {
  console.error("usage: node scripts/real-app-record-sessions.mjs <Jukeoke.exe> <root> <out dir> <player dir> [--no-studio]");
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
const truth = JSON.parse(readFileSync(join(playerDir, "truth.json"), "utf8"));
const ARTIST = "Jukeoke Test Player";
const t0 = Date.now();
const report = { exe, root, started: new Date(t0).toISOString(), checks: [], errors: [], measurements: {} };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${typeof detail === "string" ? detail : JSON.stringify(detail)}`);
  return pass;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const waitFor = async (what, fn, limitMs, everyMs = 250) => {
  const end = Date.now() + limitMs;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out after ${limitMs / 1000} s waiting for ${what}`);
    await sleep(everyMs);
  }
};

// ---------------------------------------------------------------- WAV + analysis
function readWav(file) {
  const b = readFileSync(file);
  let pos = 12;
  let ch = 0;
  let rate = 0;
  let tag = 0;
  while (pos + 8 <= b.length) {
    const id = b.toString("ascii", pos, pos + 4);
    const size = b.readUInt32LE(pos + 4);
    if (id === "fmt ") {
      tag = b.readUInt16LE(pos + 8);
      ch = b.readUInt16LE(pos + 10);
      rate = b.readUInt32LE(pos + 12);
    }
    if (id === "data") {
      const bytes = Math.min(size, b.length - pos - 8);
      const raw = b.subarray(pos + 8, pos + 8 + bytes - (bytes % (4 * ch)));
      return { rate, ch, tag, raw, data: new Float32Array(raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.length)) };
    }
    pos += 8 + size + (size & 1);
  }
  throw new Error(`${file}: no data chunk`);
}
const frames = (w) => w.data.length / w.ch;
/** Runs where every channel stays below `limit` (linear) for at least minLen frames. On
 * this PC the loopback is not exact digital silence between sounds (the driver's
 * effects leave about -84 dBFS), so the truth uses -76 dBFS: far below the -61 dBFS
 * tones, above that floor. */
function zeroRuns(w, minLen, limit = Math.pow(10, -76 / 20)) {
  const out = [];
  let s = -1;
  const n = frames(w);
  for (let i = 0; i <= n; i++) {
    let z = i < n;
    for (let c = 0; z && c < w.ch; c++) z = Math.abs(w.data[i * w.ch + c]) < limit;
    if (z && s < 0) s = i;
    if (!z && s >= 0) {
      if (i - s >= minLen) out.push([s, i]);
      s = -1;
    }
  }
  return out;
}
/** Goertzel power of channel 0 at f over [a, a+len). */
function power(w, a, len, f) {
  const k = (2 * Math.PI * f) / w.rate;
  const c = 2 * Math.cos(k);
  let s1 = 0;
  let s2 = 0;
  for (let i = 0; i < len; i++) {
    const s = w.data[(a + i) * w.ch] + c * s1 - s2;
    s2 = s1;
    s1 = s;
  }
  return s1 * s1 + s2 * s2 - c * s1 * s2;
}
/** Where the tone changes from fa to fb near `around` (±3 s): 5 ms windows, 0.5 ms hop. */
function toneChange(w, around, fa, fb) {
  const win = Math.round(w.rate * 0.005);
  const hop = Math.round(w.rate * 0.0005);
  const lo = Math.max(0, around - 3 * w.rate);
  const hi = Math.min(frames(w) - win, around + 3 * w.rate);
  let best = null;
  let prev = null;
  for (let a = lo; a < hi; a += hop) {
    const b = power(w, a, win, fb) > power(w, a, win, fa);
    if (prev === false && b) {
      const at = a + win / 2;
      if (best === null || Math.abs(at - around) < Math.abs(best - around)) best = at;
    }
    prev = b;
  }
  return best;
}
const ms = (f, rate) => Math.round((f / rate) * 1000 * 10) / 10;

// ---------------------------------------------------------------- the test player
const types = { ".html": "text/html", ".wav": "audio/wav", ".png": "image/png", ".json": "application/json" };
const server = http.createServer((req, res) => {
  const name = decodeURIComponent((req.url ?? "/").split("?")[0]).replace(/^\/+/, "") || "index.html";
  const file = join(playerDir, name);
  if (name.includes("..") || !existsSync(file)) {
    res.writeHead(404).end();
    return;
  }
  const size = statSync(file).size;
  const range = /bytes=(\d+)-(\d*)/.exec(req.headers.range ?? "");
  if (range) {
    const a = Number(range[1]);
    const b = range[2] ? Number(range[2]) : size - 1;
    res.writeHead(206, { "Content-Type": types[extname(file)] ?? "application/octet-stream", "Content-Range": `bytes ${a}-${b}/${size}`, "Accept-Ranges": "bytes", "Content-Length": b - a + 1 });
    createReadStream(file, { start: a, end: b }).pipe(res);
  } else {
    res.writeHead(200, { "Content-Type": types[extname(file)] ?? "application/octet-stream", "Content-Length": size, "Accept-Ranges": "bytes" });
    createReadStream(file).pipe(res);
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const httpPort = server.address().port;
const edgeExe = ["C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", "C:/Program Files/Microsoft/Edge/Application/msedge.exe"].find((p) => existsSync(p));
if (!edgeExe) throw new Error("Microsoft Edge not found");
const edgePort = 9800 + Math.floor(Math.random() * 150);
const edgeProfile = join(outDir, "edge-profile");
rmSync(edgeProfile, { recursive: true, force: true });
const edge = spawn(
  edgeExe,
  [
    `--user-data-dir=${edgeProfile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-sync",
    "--autoplay-policy=no-user-gesture-required",
    `--remote-debugging-port=${edgePort}`,
    "--window-size=560,420",
    "--window-position=60,60",
    `--app=http://127.0.0.1:${httpPort}/index.html`,
  ],
  { stdio: "ignore" },
);
let edgeBrowser;
let player;
for (let i = 0; ; i++) {
  try {
    edgeBrowser = await chromium.connectOverCDP(`http://127.0.0.1:${edgePort}`);
    break;
  } catch (e) {
    if (i > 60) throw new Error(`no DevTools connection to the test player: ${e}`);
    await sleep(500);
  }
}
player = await waitFor(
  "the test player page",
  async () => edgeBrowser.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(`127.0.0.1:${httpPort}`)),
  30000,
);
await waitFor("the test player to be ready", () => player.evaluate(() => (window.events ?? []).some((e) => e.m === "ready")), 30000);

// ---------------------------------------------------------------- the app
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify({ ui: { skin: false, bubbles: false, motion: "full" }, audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} } }, null, 2),
);
const appPort = 9300 + Math.floor(Math.random() * 400);
const app = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${appPort}` }, stdio: "ignore" });
let browser;
for (let i = 0; ; i++) {
  try {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${appPort}`);
    break;
  } catch (e) {
    if (i > 120) throw new Error(`no DevTools connection to the app: ${e}`);
    await sleep(500);
  }
}
const ctx = browser.contexts()[0];
const page = ctx.pages()[0] ?? (await ctx.waitForEvent("page"));
page.on("pageerror", (e) => report.errors.push(String(e)));
page.on("console", (m) => m.type() === "error" && report.errors.push(m.text()));
await page.setViewportSize({ width: 1440, height: 900 });
await waitFor("the app", () => page.evaluate(() => !!window.__jkDebug?.store.get().ready).catch(() => false), 60000);
const invoke = async (cmd, a = {}) => {
  const r = await page.evaluate(([c, x]) => window.__TAURI_INTERNALS__.invoke(c, x).then((ok) => ({ ok }), (err) => ({ err })), [cmd, a]);
  if ("err" in r) throw new Error(typeof r.err === "string" ? r.err : JSON.stringify(r.err));
  return r.ok;
};
await page.evaluate(() => window.__jkDebug.store.set({ screen: "recorder" }));
await page.waitForSelector(".recorder", { timeout: 20000 });

try {
  // ---- the test player as Windows lists it; everything else is only listed.
  const players = await waitFor(
    "the test player in the app's player list",
    async () => {
      const r = await invoke("recorder_media_sessions");
      return r.players.some((p) => p.artist === ARTIST) ? r : null;
    },
    30000,
  );
  const mine = players.players.filter((p) => p.artist === ARTIST);
  report.measurements.players = players.players.map((p) => ({ id: p.id, app: p.app_name, status: p.status, test_player: p.artist === ARTIST }));
  if (!check("exactly one test player session", mine.length === 1, report.measurements.players)) throw new Error("stopping: the test player is not unique");
  const testId = mine[0].id;
  check("the test player is paused on Test Track 1", mine[0].status === "paused" && mine[0].title === "Test Track 1", mine[0]);
  await page.waitForSelector(".recorder__player select", { timeout: 20000 });
  await page.selectOption(".recorder__player select", testId);
  await page.screenshot({ path: join(outDir, "1-recorder-idle.png") });

  // ---- (1) Record & play
  const before = new Set((await invoke("recordings_list")).recordings.map((r) => r.path));
  const tClick = Date.now();
  await page.getByRole("button", { name: "Record & play" }).click();
  await waitFor("recording", () => page.evaluate(() => window.__jkDebug.store.get().recorder?.mode === "record_and_play"), 20000, 20);
  const tRec = Date.now();
  await waitFor("the player playing", () => player.evaluate(() => window.events.some((e) => e.m === "playing" && e.ct < 1 && e.t > performance.now() - 15000)), 15000, 20);
  const st = await invoke("recorder_status");
  check("Record & play: recording, mode and player", st.state === "recording" && st.mode === "record_and_play" && st.player?.id === testId, { mode: st.mode, player: st.player, note: st.note });
  await sleep(4000);
  await page.screenshot({ path: join(outDir, "2-record-and-play.png") });
  check("'Pause the player' is on", await page.isChecked(".recorder__modes input[type=checkbox]"), "");
  await waitFor("Stop enabled", () => page.isEnabled(".recorder__button"), 10000, 50);
  await page.click(".recorder__button");
  await waitFor("idle", () => page.evaluate(() => window.__jkDebug.store.get().recorder?.state === "idle"), 20000, 50);
  const recs = (await invoke("recordings_list")).recordings.filter((r) => !before.has(r.path));
  check("Record & play made one recording", recs.length === 1, recs.map((r) => r.name));
  const ev = await player.evaluate(() => window.events.map((e) => ({ ct: e.ct, m: e.m })));
  const pausedByApp = ev.some((e) => e.m === "action pause");
  check("Stop & pause paused the test player", pausedByApp, ev.slice(-4));
  const w1 = readWav(recs[0].path);
  const thr = Math.pow(10, -70 / 20);
  let onset = -1;
  for (let i = 0; i < frames(w1); i++) {
    if (Math.abs(w1.data[i * w1.ch]) > thr) {
      onset = i;
      break;
    }
  }
  const leadMs = ms(onset, w1.rate);
  report.measurements.record_and_play = {
    file: recs[0].name,
    length_s: frames(w1) / w1.rate,
    first_sound_ms_after_start: leadMs,
    click_to_status_ms: tRec - tClick,
    silent_before_onset: onset > 0,
    peak_dbfs: Math.round(20 * Math.log10(w1.data.reduce((m, x) => Math.max(m, Math.abs(x)), 0)) * 10) / 10,
  };
  check("Record & play: the first test track's onset is inside the recording", onset > 0, `first sound ${leadMs} ms after the recording started (digital silence before it)`);

  // ---- (2) Record songs separately over the generated album
  await player.evaluate(() => window.reset());
  await sleep(1500);
  await page.fill(".recorder__options input >> nth=0", "6");
  await page.fill(".recorder__options input >> nth=1", "");
  await waitFor("the session button", () => page.isEnabled("button:has-text('Record songs separately')"), 10000, 100);
  await page.click("button:has-text('Record songs separately')");
  const live = await waitFor("the session", async () => (await invoke("recorder_status")).session, 20000, 50);
  const sessionDir = live.path;
  const link = join(outDir, "master-link.wav");
  rmSync(link, { force: true });
  linkSync(join(sessionDir, "master.wav"), link); // keeps the master's bytes after the app deletes it
  const liveLog = [];
  await waitFor(
    "the session to stop by itself",
    async () => {
      const s = await invoke("recorder_status");
      if (s.session) liveLog.push({ at_s: (Date.now() - t0) / 1000, songs: s.session.songs, current: s.session.current?.title, playback: s.session.playback, auto_stop_in_s: s.session.auto_stop_in_s });
      if (liveLog.length === 40) await page.screenshot({ path: join(outDir, "3-session-recording.png") });
      return s.state === "idle";
    },
    180000,
    500,
  );
  report.measurements.session_live = liveLog.filter((x, i, a) => i === 0 || x.current !== a[i - 1].current || x.playback !== a[i - 1].playback);
  const folder = sessionDir.split(/[\\/]/).pop();
  const sess = await waitFor(
    "the split",
    async () => (await invoke("recorder_sessions_list")).sessions.find((s) => s.folder === folder && ["split", "kept_master", "failed"].includes(s.state)),
    120000,
    500,
  );
  await page.waitForTimeout(500);
  await page.locator(`li.rec-session[aria-label="Session ${sess.created}"]`).scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(outDir, "4-session-split.png") });
  check("session split", sess.state === "split", { state: sess.state, message: sess.message, stop_reason: sess.stop_reason });
  check("stopped by itself after 6 s paused", sess.stop_reason === "idle", sess.stop_reason);
  check("master deleted after the verified split", !sess.master_present && !existsSync(join(sessionDir, "master.wav")), "");
  const sf = JSON.parse(readFileSync(join(sessionDir, "session.json"), "utf8"));
  const master = readWav(link);
  // Bit for bit: the parts joined in order are the master.
  const parts = sf.parts.map((p) => ({ ...p, w: readWav(join(sessionDir, p.file)) }));
  const joined = Buffer.concat(parts.map((p) => p.w.raw));
  check("the song files and excluded parts, joined, are the master bit for bit", joined.equals(master.raw), { parts: parts.length, master_frames: frames(master), joined_frames: joined.length / 4 / master.ch });
  check("every part verified by the app", sf.parts.every((p) => p.verified), "");
  // Truth from the master: digital-silence gaps, and tone changes for the joins.
  const gaps = zeroRuns(master, Math.round(master.rate * 0.1)).filter((g) => g[0] > 0);
  const peakDb = (a, b) => {
    let p = 0;
    for (let i = a * master.ch; i < b * master.ch; i++) p = Math.max(p, Math.abs(master.data[i]));
    return p === 0 ? "digital silence" : Math.round(20 * Math.log10(p) * 10) / 10;
  };
  report.measurements.gaps = gaps.map((g) => ({ from_ms: ms(g[0], master.rate), to_ms: ms(g[1], master.rate), peak_dbfs: peakDb(g[0], g[1]) }));
  const freqOf = (title) => truth.tracks.find((t) => t.title === title)?.f;
  const markers = sf.markers.filter((m) => m.kind === "track");
  const cuts = [];
  for (let k = 1; k < parts.length; k++) {
    const cut = parts[k].start_frame;
    const gap = gaps.find((g) => cut >= g[0] && cut <= g[1]);
    const entry = { between: `${parts[k - 1].file} | ${parts[k].file}`, cut_s: Math.round((cut / master.rate) * 1000) / 1000 };
    if (gap) {
      const mid = (gap[0] + gap[1]) / 2;
      if (gap[1] >= frames(master)) {
        // Playback ended: the silence runs to the end; the song keeps at most 1 s of it.
        Object.assign(entry, { kind: "playback ended", silence_kept_ms: ms(cut - gap[0], master.rate) });
      } else {
        Object.assign(entry, { kind: "gap", gap_ms: ms(gap[1] - gap[0], master.rate), error_vs_gap_middle_ms: ms(cut - mid, master.rate), next_onset_after_cut_ms: ms(gap[1] - cut, master.rate) });
      }
    } else {
      const fa = freqOf(markers.slice().reverse().find((m) => m.frame <= cut + master.rate * 3 && (m.estimate_frame ?? m.frame) < cut - 1)?.title);
      const fb = freqOf(markers.find((m) => (m.estimate_frame ?? m.frame) >= cut - master.rate * 3 && (m.estimate_frame ?? m.frame) >= cut - 1)?.title);
      const change = fa && fb ? toneChange(master, cut, fa, fb) : null;
      Object.assign(entry, { kind: "no gap", tones: [fa, fb], true_change_s: change === null ? null : Math.round((change / master.rate) * 1000) / 1000, error_ms: change === null ? null : ms(cut - change, master.rate) });
    }
    cuts.push(entry);
  }
  report.measurements.cuts = cuts;
  report.measurements.markers = sf.markers.map((m) => ({ kind: m.kind, at_s: Math.round((m.frame / master.rate) * 1000) / 1000, estimate_s: m.estimate_frame ? Math.round((m.estimate_frame / master.rate) * 1000) / 1000 : null, title: m.title, duration_s: m.duration_s, cover: m.cover ?? null }));
  report.measurements.songs = sess.songs.map((s) => ({ name: s.name, status: s.status, partial_reason: s.partial_reason, start: s.start_confidence, end: s.end_confidence, duration_s: s.duration_s, track_duration_s: s.track_duration_s, cover: !!s.cover_path }));
  report.measurements.excluded = sess.excluded.map((p) => ({ name: p.name, reason: p.reason, duration_s: p.duration_s }));
  for (const c of cuts) console.log("   cut", JSON.stringify(c));
  const songTitles = sess.songs.map((s) => s.title);
  check("five songs, named after the player's titles", JSON.stringify(songTitles) === JSON.stringify(["Test Track 1", "Test Track 2", "Test Track 3", "Test Track 4", "Test Track 5"]), songTitles);
  check("the advertisement is excluded", sess.excluded.some((p) => p.reason === "advertisement"), report.measurements.excluded);
  const t4 = sess.songs.find((s) => s.title === "Test Track 4");
  check("the skipped track is partial", t4?.status === "partial", t4);
  check("the others are complete", sess.songs.filter((s) => s.title !== "Test Track 4").every((s) => s.status === "complete"), report.measurements.songs);
  check("covers saved for every song", sess.songs.every((s) => s.cover_path && existsSync(s.cover_path)), sess.songs.map((s) => s.cover_path));
  const gapCuts = cuts.filter((c) => c.kind === "gap");
  check("cuts at gaps are in the gap's middle (within 5 ms)", gapCuts.length >= 3 && gapCuts.every((c) => Math.abs(c.error_vs_gap_middle_ms) <= 5), gapCuts);

  // The same screen in the jukebox look (the owner's default).
  await invoke("settings_set", { changes: { ui: { skin: true } } });
  await page.evaluate(() => window.__jkDebug.store.set({ settings: { ...window.__jkDebug.store.get().settings, ui: { ...window.__jkDebug.store.get().settings.ui, skin: true } } }));
  await page.waitForTimeout(1500);
  await page.locator(`li.rec-session[aria-label="Session ${sess.created}"]`).scrollIntoViewIfNeeded().catch(() => undefined);
  await page.screenshot({ path: join(outDir, "5-session-split-skin.png") });
  await invoke("settings_set", { changes: { ui: { skin: false } } });
  await page.evaluate(() => window.__jkDebug.store.set({ settings: { ...window.__jkDebug.store.get().settings, ui: { ...window.__jkDebug.store.get().settings.ui, skin: false } } }));
  await page.waitForTimeout(800);

  // ---- (3) Studio hand-off
  if (!noStudio) {
    const queuedBefore = new Set((await invoke("studio_status")).queue.map((j) => j.job_id));
    const makeAll = page.locator(`li.rec-session[aria-label="Session ${sess.created}"] .rec-session__make-all`);
    await waitFor("the Studio button", () => makeAll.isVisible(), 120000, 1000);
    const label = await makeAll.textContent();
    await makeAll.click();
    const jobs = await waitFor(
      "the Studio jobs",
      async () => {
        const q = (await invoke("studio_status")).queue.filter((j) => !queuedBefore.has(j.job_id));
        return q.length >= 4 ? q : null;
      },
      60000,
      500,
    );
    report.measurements.studio_jobs = jobs.map((j) => ({ id: j.job_id, input: j.input_name, state: j.state }));
    check("'Make karaoke songs from all complete songs' queued one job per complete song", jobs.length === 4 && /\(4\)/.test(label ?? ""), report.measurements.studio_jobs);
    // Let the first one finish (12 s of tone), cancel the others once it has.
    const first = jobs[0];
    const end = await waitFor(
      "the first job",
      async () => {
        const j = (await invoke("studio_status")).queue.find((q) => q.job_id === first.job_id);
        return j && ["done", "failed", "cancelled"].includes(j.state) ? j : null;
      },
      900000,
      2000,
    );
    report.measurements.studio_first_job = end;
    for (const j of jobs.slice(1)) await invoke("studio_cancel", { job_id: j.job_id }).catch((e) => report.errors.push(String(e)));
    if (end.state === "done" && end.output_song_id) {
      const lib = await invoke("library_list");
      const song = (lib.songs ?? []).find((s) => s.song_id === end.output_song_id);
      const cover = song ? await invoke("song_cover", { song_id: song.song_id, max_px: 64 }).catch(() => null) : null;
      report.measurements.studio_song = song ? { song_id: song.song_id, title: song.title, artist: song.artist, cover: !!cover?.data_url } : null;
      check("the finished song has the player's title, artist and cover", song?.title === "Test Track 1" && song?.artist === ARTIST && !!cover?.data_url, report.measurements.studio_song);
    } else {
      check("the first Studio job finished", false, end);
    }
  }
} catch (e) {
  report.errors.push(String(e?.stack ?? e));
  console.log("ERROR", e);
} finally {
  writeFileSync(join(outDir, "record-sessions.json"), JSON.stringify(report, null, 2));
  try {
    const s = await edgeBrowser.newBrowserCDPSession();
    await s.send("Browser.close");
  } catch {
    // already gone
  }
  await browser?.close().catch(() => undefined);
  // Close the app's window like the X button does (the app shuts down cleanly).
  spawnSync("powershell", ["-NoProfile", "-Command", `(Get-Process -Id ${app.pid} -ErrorAction SilentlyContinue).CloseMainWindow() | Out-Null`]);
  await new Promise((r) => {
    const t = setTimeout(r, 20000);
    app.on("exit", () => {
      clearTimeout(t);
      r();
    });
  });
  server.close();
  void edge;
  const failed = report.checks.filter((c) => !c.pass).length;
  console.log(`${report.checks.length - failed} passed, ${failed} failed, ${report.errors.length} errors; ${((Date.now() - t0) / 1000).toFixed(0)} s`);
  process.exit(failed || report.errors.length ? 1 : 0);
}
