// SPDX-License-Identifier: GPL-3.0-or-later
// End to end through the REAL app (WORKING ORDER section 9 "End to end", "Weak
// computers"): the release exe on a portable stick, its Rust core, the real Studio
// supervisor and the real Studio runtime, driven over WebView2's DevTools port the way
// real-app-screens.mjs does. Results: docs/benchmarks/end-to-end.md.
//
// SAFETY: nothing is played aloud. Settings/settings.json selects the engine's silent
// "null:48000" output device and master volume 0 before the app starts.
//
//   node scripts/real-app-e2e.mjs queue       <Jukeoke.exe> <root> <out dir> <jobs.json>
//   node scripts/real-app-e2e.mjs stage       <Jukeoke.exe> <root> <out dir>
//   node scripts/real-app-e2e.mjs unavailable <Jukeoke.exe> <root> <out dir> <label>
//
// queue:       waits for the Studio probe, then for every job in jobs.json
//              ([{ input, title, artist, lyrics_text_file | null }]) calls studio_inspect
//              and studio_submit (lyrics as text when a file is given, else "auto"), all
//              back to back, and waits for every studio://job to end (done / failed).
//              Records each job's events with wall-clock times, screenshots the Studio
//              screen while the first job runs and when the queue is finished.
// stage:       for every song in the Library: opens it from its title strip, runs the
//              ticker's letter-under-the-marker check on the live layout, seeks so that
//              the middle of a long word in the middle of the song is under the marker
//              after 1.3 s of playback, plays, and takes a 1440x900 Stage screenshot.
// unavailable: start-up with a Studio that cannot run (GPU hidden, Studio folder
//              missing, broken python.exe): time until the probe has answered, the
//              capability and status the UI gets, a screenshot of the Studio screen, and
//              proof that a song still plays (the bundled demo song is not in the
//              Library, so the first Library song is used).
//
// The child process inherits this process's environment (e.g. CUDA_VISIBLE_DEVICES=-1,
// which then reaches the Studio through the supervisor).
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [mode, exe, root, outDir, extra] = process.argv.slice(2);
if (!["queue", "stage", "unavailable"].includes(mode) || !exe || !root || !outDir) {
  console.error("usage: node scripts/real-app-e2e.mjs queue|stage|unavailable <Jukeoke.exe> <root> <out dir> [jobs.json|label]");
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
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

const t0 = Date.now();
const port = 9300 + Math.floor(Math.random() * 500);
const child = spawn(exe, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  stdio: ["ignore", "pipe", "pipe"],
});
let exited = null;
child.on("exit", (code) => (exited = { code, at_s: (Date.now() - t0) / 1000 }));
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

const report = { mode, exe, root, started: new Date(t0).toISOString(), env: { CUDA_VISIBLE_DEVICES: process.env.CUDA_VISIBLE_DEVICES ?? null }, checks: [], errors: [] };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${detail}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const since = () => (Date.now() - t0) / 1000;

let browser;
let page;
// Tauri rejects with the core's { code, message } object: hand it back as data, then throw
// an Error whose message is that JSON (Playwright would flatten a non-Error rejection).
const invoke = async (cmd, args = {}) => {
  const r = await page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a).then((ok) => ({ ok }), (err) => ({ err })), [cmd, args]);
  if ("err" in r) throw new Error(typeof r.err === "string" ? r.err : JSON.stringify(r.err));
  return r.ok;
};
const nav = async (label) => {
  await page.locator(".skin-nav .jk-plate", { hasText: label }).click();
  await sleep(500);
};
const shot = async (name) => {
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  console.log(`  saved ${file}`);
  return file;
};
/** Waits until the background probe has answered (checking false); returns the status. */
const waitProbe = async (limitS) => {
  for (;;) {
    const st = await invoke("studio_status");
    if (!st.checking) return { status: st, at_s: since() };
    if (since() > limitS) return { status: st, at_s: since(), timedOut: true };
    await sleep(250);
  }
};

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
  report.window_ready_s = since();
  // Every studio://job and studio://status event, with the wall-clock time it arrived.
  await page.evaluate(() => {
    window.__e2e = { events: [] };
    const I = window.__TAURI_INTERNALS__;
    for (const event of ["studio://job", "studio://status"]) {
      const handler = I.transformCallback((e) => window.__e2e.events.push({ at: Date.now(), event, payload: e.payload }));
      void I.invoke("plugin:event|listen", { event, target: { kind: "Any" }, handler });
    }
  });

  if (mode === "queue") {
    const jobs = JSON.parse(readFileSync(extra, "utf8"));
    const probe = await waitProbe(120);
    report.probe = { at_s: probe.at_s, available: probe.status.available, reasons: probe.status.reasons, models_present: probe.status.models_present };
    check("Studio available after the probe", probe.status.available && !probe.timedOut, `${probe.at_s.toFixed(1)} s after launch; ${JSON.stringify(probe.status.reasons)}; models_present ${probe.status.models_present}`);
    const info = await invoke("app_info");
    report.app_info = { mode: info.mode, root: info.root, paths: info.paths, studio: info.capabilities.studio, version: info.version };
    report.jobs = [];
    for (const j of jobs) {
      const entry = { input: j.input, title: j.title, artist: j.artist };
      const ti = Date.now();
      try {
        const r = await invoke("studio_inspect", { path: j.input });
        entry.inspect = { ok: true, seconds: (Date.now() - ti) / 1000, title: r.title, artist: r.artist, duration_seconds: r.duration_seconds, container: r.container, codec: r.codec, sample_rate: r.sample_rate, channels: r.channels, bits_per_sample: r.bits_per_sample, lossy: r.lossy, has_video: r.has_video, title_from_tags: r.title_from_tags };
      } catch (e) {
        entry.inspect = { ok: false, seconds: (Date.now() - ti) / 1000, error: String(e?.message ?? JSON.stringify(e)) };
      }
      const text = j.lyrics_text_file ? readFileSync(j.lyrics_text_file, "utf8") : null;
      const lyrics = text ? { mode: "text", text, lrc: null, source: "user_text" } : { mode: "auto", text: null, lrc: null, source: null };
      const job = { input: j.input, preset: "full", language: "en", metadata: { title: j.title, artist: j.artist, album: null }, lyrics };
      try {
        const r = await invoke("studio_submit", { job });
        entry.job_id = r.job_id;
        entry.submitted_at = Date.now();
      } catch (e) {
        entry.submit_error = String(e?.message ?? JSON.stringify(e));
      }
      report.jobs.push(entry);
      console.log(`submitted ${j.input}: inspect ${JSON.stringify(entry.inspect)} job ${entry.job_id ?? entry.submit_error}`);
    }
    await nav("Studio");
    let midShot = false;
    const deadline = Date.now() + 60 * 60 * 1000;
    let st;
    for (;;) {
      st = await invoke("studio_status");
      const open = st.queue.filter((q) => q.state === "queued" || q.state === "running");
      const running = st.queue.find((q) => q.state === "running");
      if (!midShot && running && running.overall > 0.3) {
        await shot("studio-queue-running-1440x900");
        midShot = true;
      }
      if (open.length === 0 || Date.now() > deadline) break;
      await sleep(2000);
    }
    report.queue_done_s = since();
    report.final_queue = st.queue;
    await sleep(800);
    report.studio_screen = await page.evaluate(() => document.querySelector(".studio")?.innerText ?? null);
    await shot("studio-queue-finished-1440x900");
    const events = await page.evaluate(() => window.__e2e.events);
    for (const e of report.jobs) {
      const mine = events.filter((x) => x.event === "studio://job" && x.payload.job_id === e.job_id);
      const stages = [];
      for (const x of mine) {
        const s = x.payload.stage;
        if (s && (stages.length === 0 || stages[stages.length - 1].stage !== s)) stages.push({ stage: s, from: x.at });
      }
      const started = mine.find((x) => x.payload.state === "running");
      const ended = mine.find((x) => ["done", "failed", "cancelled"].includes(x.payload.state));
      e.events = mine.length;
      e.state = ended?.payload.state ?? "unfinished";
      e.error = ended?.payload.error ?? null;
      e.error_code = ended?.payload.error_code ?? null;
      e.message = ended?.payload.message ?? null;
      e.output_song_id = ended?.payload.output_song_id ?? null;
      e.wall_running_s = started && ended ? (ended.at - started.at) / 1000 : null;
      e.stages_wall_s = stages.map((s, i) => ({ stage: s.stage, seconds: ((stages[i + 1]?.from ?? ended?.at ?? s.from) - s.from) / 1000 }));
      e.started_at = started ? new Date(started.at).toISOString() : null;
      e.ended_at = ended ? new Date(ended.at).toISOString() : null;
    }
    const want = jobs.map((j) => (j.expect ?? "done"));
    for (const [i, e] of report.jobs.entries()) {
      check(`job ${i + 1} ${e.title ?? e.input}: ${want[i]}`, e.state === want[i], `${e.state}${e.error ? ` - "${e.error}" (${e.error_code})` : ""}${e.wall_running_s ? `, ${e.wall_running_s.toFixed(1)} s` : ""}`);
    }
  }

  if (mode === "stage") {
    const lib = await invoke("library_list");
    report.library = lib.songs.map((s) => ({ song_id: s.song_id, title: s.title, artist: s.artist, duration_s: s.duration_s, key: s.key?.name ?? null, tempo_bpm: s.tempo_bpm, has_lyrics: s.has_lyrics, lyrics_checked: s.lyrics_checked, file_bytes: s.file_bytes, error: s.error }));
    report.songs = [];
    for (const [i, s] of lib.songs.entries()) {
      await nav("Library");
      await page.waitForSelector(".jk-strip");
      await page.locator(".jk-strip", { hasText: s.title }).first().click();
      await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 30000 });
      await sleep(600);
      // The middle of a word at least 0.5 s long, in the middle third of the song.
      const target = await page.evaluate(() => {
        const song = window.__jkDebug.store.get().song;
        const lines = song.lyrics?.lines ?? [];
        const d = song.summary.duration_s;
        const words = lines.flatMap((l, li) => l.words.map((w) => ({ ...w, li, text: w.chars.map((c) => c[0]).join(""), line: l.words.map((x) => x.chars.map((c) => c[0]).join("")).join(" ") })));
        const mid = words.filter((w) => w.start > d / 3 && w.start < (2 * d) / 3);
        const pick = mid.find((w) => w.end - w.start >= 0.5) ?? mid[0] ?? words[Math.floor(words.length / 2)];
        return pick ? { t: (pick.start + pick.end) / 2, word: pick.text, line: pick.line, start: pick.start, end: pick.end, lines: lines.length, words: words.length, source: song.lyrics?.source, checked: song.lyrics?.checked } : null;
      });
      const v = await page.evaluate(() => window.__jkTicker.verify());
      const entry = { title: s.title, target, ticker: { wrongLetter: v.wrongLetter, backwards: v.backwards, tooFast: v.tooFast, lookaheadMisses: v.lookaheadMisses, sungSamples: v.sungSamples, fontPx: v.fontPx, fontScale: v.fontScale, minLookaheadS: v.minLookaheadS, firstProblems: v.firstProblems } };
      check(`${s.title}: letter under the marker across the whole song (1440x900, WebView2)`, v.wrongLetter === 0 && v.backwards === 0 && v.tooFast === 0 && v.lookaheadMisses === 0, `${v.sungSamples} sung samples, font ${v.fontPx.toFixed(1)} px (x${v.fontScale.toFixed(2)}), min lookahead ${v.minLookaheadS.toFixed(2)} s ${v.firstProblems.join("; ")}`);
      if (target) {
        const played = await page.evaluate(async (t) => {
          const dbg = window.__jkDebug;
          const input = document.querySelector("#seek");
          input.value = String(Math.max(0, t - 1.3));
          input.dispatchEvent(new Event("change", { bubbles: true }));
          await new Promise((r) => setTimeout(r, 400));
          dbg.clockEvents.length = 0;
          document.querySelector(".skin-transport__play").click();
          await new Promise((r) => setTimeout(r, 1300));
          const ev = dbg.clockEvents.filter((e) => e.playing);
          return { state: dbg.store.get().player?.state, events: ev.length, from: ev[0]?.position_s, to: ev[ev.length - 1]?.position_s, underruns: dbg.store.get().player?.underruns };
        }, target.t);
        const file = await shot(`stage-${i + 1}-1440x900`);
        // What the lyrics say is sung at the moment of the screenshot (UI clock, offsets 0).
        entry.at_shot = await page.evaluate(() => {
          const dbg = window.__jkDebug;
          const t = dbg.clock.position(performance.now());
          const lines = dbg.store.get().song.lyrics?.lines ?? [];
          for (const l of lines) for (const w of l.words) for (const c of w.chars) if (c[1] <= t && t < c[2]) return { t, grapheme: c[0], word: w.chars.map((x) => x[0]).join("") };
          return { t, grapheme: null };
        });
        entry.played = played;
        entry.screenshot = file;
        check(`${s.title}: plays with the ticker`, played.state === "playing" && played.events > 20, `state ${played.state}, ${played.events} clock events, ${played.from?.toFixed(2)} -> ${played.to?.toFixed(2)} s; at the screenshot t=${entry.at_shot.t.toFixed(2)} s, "${entry.at_shot.grapheme}" of "${entry.at_shot.word ?? ""}" is sung; underruns ${played.underruns}`);
        await page.locator(".skin-transport__play").click();
        await sleep(300);
      }
      report.songs.push(entry);
    }
  }

  if (mode === "unavailable") {
    report.label = extra ?? null;
    const first = await invoke("studio_status");
    report.first_status = { at_s: since(), checking: first.checking, available: first.available, reasons: first.reasons };
    const probe = await waitProbe(180);
    report.probe = { at_s: probe.at_s, timedOut: !!probe.timedOut, available: probe.status.available, reasons: probe.status.reasons, running: probe.status.running };
    const info = await invoke("app_info");
    report.capability = info.capabilities.studio;
    check("probe answered without a hang", !probe.timedOut, `checking became false ${probe.at_s.toFixed(1)} s after launch (window ready ${report.window_ready_s.toFixed(1)} s)`);
    check("Studio shown as unavailable with a reason", !probe.status.available && probe.status.reasons.length > 0, JSON.stringify(probe.status.reasons));
    await nav("Studio");
    await sleep(500);
    report.studio_screen = await page.evaluate(() => document.querySelector(".studio")?.innerText ?? null);
    report.screenshot = await shot(`studio-unavailable-${extra ?? "x"}-1440x900`);
    // A submit must be refused at once with the same reason, not queued forever.
    const tsub = Date.now();
    try {
      await invoke("studio_submit", { job: { input: "C:\\nonexistent.mp3", preset: "full", language: "en", metadata: { title: "x", artist: null, album: null }, lyrics: { mode: "auto" } } });
      report.submit = { refused: false, seconds: (Date.now() - tsub) / 1000 };
    } catch (e) {
      report.submit = { refused: true, seconds: (Date.now() - tsub) / 1000, error: String(e?.message ?? JSON.stringify(e)) };
    }
    check("studio_submit refused at once", report.submit.refused && report.submit.seconds < 5, JSON.stringify(report.submit));
    // Playback still works: the first Library song, 2 s from 30 s.
    const lib = await invoke("library_list");
    if (lib.songs.length > 0) {
      await nav("Library");
      await page.locator(".jk-strip").first().click();
      await page.waitForFunction(() => window.__jkTicker && window.__jkTicker.glyphs > 0, null, { timeout: 30000 }).catch(() => undefined);
      const played = await page.evaluate(async () => {
        const dbg = window.__jkDebug;
        const input = document.querySelector("#seek");
        input.value = "30";
        input.dispatchEvent(new Event("change", { bubbles: true }));
        await new Promise((r) => setTimeout(r, 400));
        dbg.clockEvents.length = 0;
        document.querySelector(".skin-transport__play").click();
        await new Promise((r) => setTimeout(r, 2000));
        const ev = dbg.clockEvents.filter((e) => e.playing);
        return { song: dbg.store.get().song?.summary.title, state: dbg.store.get().player?.state, events: ev.length, from: ev[0]?.position_s, to: ev[ev.length - 1]?.position_s };
      });
      report.played = played;
      check("a song still plays", played.state === "playing" && played.events > 30, JSON.stringify(played));
      await page.locator(".skin-transport__play").click();
    } else {
      check("a song still plays", false, "no song in the Library");
    }
  }
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
writeFileSync(join(outDir, `real-app-e2e-${mode}${extra && mode === "unavailable" ? `-${extra}` : ""}.json`), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 2));
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
