// SPDX-License-Identifier: GPL-3.0-or-later
// Drives, in the REAL release app from a portable copy, the features no earlier test had
// clicked (docs/TEST_RESULTS.md section 8): the Studio form's per-preset model download
// (Clean and Permissive, real network, pinned URLs), the Recorder and its "Make a karaoke
// song from this" button, a real OS drag-and-drop onto the window (ole-drop.ps1), the
// form's cover replacement, and the Sync editor's "Re-align with Studio" + save. Same
// DevTools method as real-app-e2e.mjs.
//
// SAFETY: nothing is played aloud. Settings/settings.json selects the engine's silent
// "null:48000" output device and master volume 0 before the app starts; the recorder
// captures the real default device's system mix, which this run leaves silent.
//
//   node scripts/real-app-features.mjs <Open Mic.exe> <root> <out dir> <input audio> <cover.jpg> [--no-ole]
//
// Every step is a PASS / FAIL line; the report is <out dir>/real-app-features.json.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const args = process.argv.slice(2);
const noOle = args.includes("--no-ole");
// --drop-only: launch, wait for the Studio, make one real OLE drop of <input audio>, stop.
const dropOnly = args.includes("--drop-only");
const [exe, root, outDir, inputAudio, coverJpg] = args.filter((a) => !a.startsWith("--"));
if (!exe || !root || !outDir || !inputAudio || !coverJpg) {
  console.error("usage: node scripts/real-app-features.mjs <Open Mic.exe> <root> <out dir> <input audio> <cover.jpg> [--no-ole]");
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify(
    {
      ui: { skin: true, bubbles: false, motion: "full", lyrics_text_size: 56, marker_position: 0.33, colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" }, library_sort: "title", fullscreen_stage: false },
      audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} },
    },
    null,
    2,
  ),
);

const t0 = Date.now();
const port = 9300 + Math.floor(Math.random() * 500);
const child = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: ["ignore", "pipe", "pipe"] });
let exited = null;
child.on("exit", (code) => (exited = { code, at_s: (Date.now() - t0) / 1000 }));
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

const report = { exe, root, input: inputAudio, cover: coverJpg, started: new Date(t0).toISOString(), checks: [], errors: [], steps: {} };
const check = (name, pass, detail) => {
  report.checks.push({ name, pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}: ${typeof detail === "string" ? detail : JSON.stringify(detail)}`);
  return pass;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const since = () => ((Date.now() - t0) / 1000).toFixed(1);

let browser;
let page;
const invoke = async (cmd, a = {}) => {
  const r = await page.evaluate(([c, x]) => window.__TAURI_INTERNALS__.invoke(c, x).then((ok) => ({ ok }), (err) => ({ err })), [cmd, a]);
  if ("err" in r) throw new Error(typeof r.err === "string" ? r.err : JSON.stringify(r.err));
  return r.ok;
};
const nav = async (label) => {
  await page.locator(".skin-nav .jk-plate", { hasText: label }).click();
  await sleep(600);
};
const shot = async (name) => {
  const file = join(outDir, `${name}.png`);
  await page.screenshot({ path: file });
  return file;
};
const store = () => page.evaluate(() => {
  const s = window.__jkDebug.store.get();
  return { screen: s.screen, inbox: s.studioInbox, queue: s.studio?.queue ?? [], models_present: s.studio?.models_present, toasts: (s.toasts ?? []).map((t) => t.text) };
});
const waitFor = async (what, fn, limitMs, everyMs = 500) => {
  const end = Date.now() + limitMs;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out after ${limitMs / 1000} s waiting for ${what}`);
    await sleep(everyMs);
  }
};
/** The form of one inbox file (li.studio__item whose name is that file's name). */
const formOf = (path) => page.locator("li.studio__item", { has: page.locator("strong", { hasText: basename(path) }) });
const jobEnd = async (jobId, limitMs) =>
  waitFor(`job ${jobId}`, async () => {
    const st = await invoke("studio_status");
    const j = st.queue.find((q) => q.job_id === jobId);
    return j && ["done", "failed", "cancelled"].includes(j.state) ? j : null;
  }, limitMs, 1000);
/** Job ids before/after a click on "Add to queue". */
const submitForm = async (form) => {
  const before = new Set((await invoke("studio_status")).queue.map((q) => q.job_id));
  await form.getByRole("button", { name: "Add to queue" }).click();
  const st = await waitFor("the new job in the queue", async () => {
    const s = await invoke("studio_status");
    const n = s.queue.filter((q) => !before.has(q.job_id));
    return n.length ? n : null;
  }, 30000);
  return st[0];
};
/** WAV header + peak of a recording (format tag, channels, rate, bits, frames). */
function wavInfo(file) {
  const b = readFileSync(file);
  const out = { bytes: b.length, riff: b.toString("ascii", 0, 4), wave: b.toString("ascii", 8, 12) };
  let pos = 12;
  let fmt = null;
  while (pos + 8 <= b.length) {
    const id = b.toString("ascii", pos, pos + 4);
    const size = b.readUInt32LE(pos + 4);
    if (id === "fmt ") {
      fmt = { tag: b.readUInt16LE(pos + 8), channels: b.readUInt16LE(pos + 10), rate: b.readUInt32LE(pos + 12), bits: b.readUInt16LE(pos + 22) };
      if (fmt.tag === 0xfffe && size >= 40) fmt.subformat_tag = b.readUInt16LE(pos + 8 + 24);
    }
    if (id === "data" && fmt) {
      const frames = Math.floor(size / (fmt.channels * (fmt.bits / 8)));
      let peak = 0;
      if (fmt.bits === 32 && (fmt.tag === 3 || fmt.subformat_tag === 3)) {
        for (let o = pos + 8; o + 4 <= pos + 8 + size; o += 4) peak = Math.max(peak, Math.abs(b.readFloatLE(o)));
      }
      Object.assign(out, fmt, { frames, seconds: frames / fmt.rate, peak, peak_dbfs: peak > 0 ? 20 * Math.log10(peak) : -Infinity });
      break;
    }
    pos += 8 + size + (size % 2);
  }
  return out;
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
  await page.evaluate(() => {
    window.__feat = { events: [] };
    const I = window.__TAURI_INTERNALS__;
    for (const event of ["studio://dropped", "studio://models", "studio://job", "tauri://drag-drop", "tauri://drag-enter"]) {
      const handler = I.transformCallback((e) => window.__feat.events.push({ at: Date.now(), event, payload: e.payload }));
      void I.invoke("plugin:event|listen", { event, target: { kind: "Any" }, handler });
    }
  });
  const probe = await waitFor("the Studio probe", async () => {
    const st = await invoke("studio_status");
    return st.checking ? null : st;
  }, 120000);
  check("Studio available", probe.available, `at ${since()} s; models_present (default preset) ${probe.models_present}`);
  const info = await invoke("app_info");
  report.app_info = { root: info.root, paths: info.paths, version: info.version };
  for (const p of ["full", "clean", "permissive"]) {
    const s = await invoke("studio_models_status", { preset: p });
    report.steps[`models_before_${p}`] = { all_present: s.all_present, missing: s.models.filter((m) => !m.present).map((m) => `${m.name} ${m.size_bytes} ${m.licence}`) };
  }
  check("before: Full present, Clean and Permissive missing their own model",
    report.steps.models_before_full.all_present && !report.steps.models_before_clean.all_present && !report.steps.models_before_permissive.all_present,
    { clean: report.steps.models_before_clean.missing, permissive: report.steps.models_before_permissive.missing });

  if (dropOnly) {
    await nav("Studio");
    const r = spawnSync("powershell", ["-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File", join(here, "ole-drop.ps1"), "-TargetPid", String(child.pid), "-Files", inputAudio], { encoding: "utf8", timeout: 60000 });
    const line = (r.stdout || "").trim().split(/\r?\n/).pop();
    let helper;
    try {
      helper = JSON.parse(line);
    } catch {
      helper = { ok: false, error: `${r.stdout} ${r.stderr}`.trim().slice(0, 400) };
    }
    await sleep(1500);
    const ev = await page.evaluate(() => window.__feat.events.filter((e) => e.event === "studio://dropped" || e.event.startsWith("tauri://drag")));
    report.steps.ole_drop = helper;
    report.steps.drop_events = ev;
    check("real OS drag-and-drop (OLE, CF_HDROP) reaches the core's DragDrop handler -> studio://dropped", ev.some((e) => e.event === "studio://dropped" && e.payload.paths.includes(inputAudio)), { helper, events: ev.map((e) => `${e.event} ${JSON.stringify(e.payload)}`) });
    const st = await store();
    check("dropped file is in the Studio form", st.inbox.includes(inputAudio) && st.screen === "studio", `inbox ${JSON.stringify(st.inbox)}`);
    await shot("studio-after-ole-drop");
  }
  // ------------------------------------------------------------ A. Recorder (4a)
  if (!dropOnly) {
    const lib = await invoke("library_list");
    const demo = (lib.songs ?? lib).find((s) => /demo|twinkle/i.test(`${s.title} ${s.file_name ?? ""} ${s.song_id}`)) ?? (lib.songs ?? lib)[0];
    await nav("Library");
    await page.locator(".jk-strip", { hasText: demo.title }).first().click();
    await waitFor("the demo song loaded", async () => (await page.evaluate(() => window.__jkDebug.store.get().song?.summary.song_id)) === demo.song_id, 20000);
    await invoke("player_play");
    await sleep(1000);
    const ps = await invoke("player_state");
    check("demo song playing on the null device at master 0", ps.state === "playing" && /null/i.test(JSON.stringify(ps.device ?? "")), `${ps.state}, device ${JSON.stringify(ps.device)}`);
    await nav("Recorder");
    const before = (await invoke("recordings_list")).recordings.map((r) => r.path);
    await page.getByRole("button", { name: "Start recording" }).click();
    const recT0 = Date.now();
    await sleep(10500);
    await page.getByRole("button", { name: "Stop recording" }).click();
    const rec = await waitFor("the new recording", async () => {
      const l = (await invoke("recordings_list")).recordings;
      return l.find((r) => !before.includes(r.path)) ?? null;
    }, 20000);
    const wallS = (Date.now() - recT0) / 1000;
    await invoke("player_pause").catch(() => undefined);
    const full = rec.path.match(/^[A-Za-z]:|^\\\\/) ? rec.path : join(root, rec.path);
    const w = wavInfo(full);
    report.steps.recorder = { listed: rec, wav: w, wall_s: wallS };
    check("recording created: 32-bit float WAV at the device rate, about 10 s", w.riff === "RIFF" && w.wave === "WAVE" && w.bits === 32 && (w.tag === 3 || w.subformat_tag === 3) && w.channels >= 1 && w.rate >= 44100 && w.seconds > 9 && w.seconds < wallS + 0.5,
      `${basename(full)}: tag ${w.tag}${w.subformat_tag ? `/${w.subformat_tag}` : ""}, ${w.channels} ch, ${w.rate} Hz, ${w.bits}-bit, ${w.frames} frames = ${w.seconds.toFixed(3)} s (button to button ${wallS.toFixed(2)} s), peak ${w.peak_dbfs} dBFS; list says ${rec.duration_s} s, ${rec.sample_rate} Hz, ${rec.channels} ch`);
    await shot("recorder-after-recording");
    const item = page.locator(".recordings__item", { hasText: rec.name });
    await item.getByRole("button", { name: "Make a karaoke song from this" }).click();
    const st = await waitFor("the recording in the Studio inbox", async () => {
      const s = await store();
      return s.inbox.includes(rec.path) && s.screen === "studio" ? s : null;
    }, 10000);
    check("'Make a karaoke song from this' opens the Studio form with the recording", true, `screen ${st.screen}, inbox ${JSON.stringify(st.inbox)}`);
    const form = formOf(rec.path);
    await form.getByRole("button", { name: "Add to queue" }).waitFor({ timeout: 60000 });
    await form.locator("input[type=radio][value=none]").check();
    await shot("studio-form-from-recording");
    const job = await submitForm(form);
    check("the recording's job is in the Studio queue", !!job, `${job.job_id} ${job.input_name} ${job.state}`);
    const end = await jobEnd(job.job_id, 10 * 60000);
    report.steps.recorder_job = end;
    check("the recording's job finished", end.state === "done", `${end.state} ${end.error ?? ""} -> ${end.output_song_id ?? "-"}`);
  }

  // ------------------------------------------------------------ B. Clean preset (task 1), real OLE drop (4d), cover (4c)
  let cleanSong = null;
  if (!dropOnly) {
    await nav("Studio");
    let dropped = null;
    if (!noOle) {
      const r = spawnSync("powershell", ["-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File", join(here, "ole-drop.ps1"), "-TargetPid", String(child.pid), "-Files", inputAudio], { encoding: "utf8", timeout: 60000 });
      const line = (r.stdout || "").trim().split(/\r?\n/).pop();
      try {
        dropped = JSON.parse(line);
      } catch {
        dropped = { ok: false, error: `${r.stdout} ${r.stderr}`.trim().slice(0, 400) };
      }
      report.steps.ole_drop = dropped;
      await sleep(1500);
    }
    const ev = await page.evaluate(() => window.__feat.events.filter((e) => e.event === "studio://dropped" || e.event.startsWith("tauri://drag")));
    report.steps.drop_events = ev;
    const viaOle = !noOle && ev.some((e) => e.event === "studio://dropped" && e.payload.paths.includes(inputAudio));
    if (!noOle) check("real OS drag-and-drop (OLE, CF_HDROP) reaches the core's DragDrop handler -> studio://dropped", viaOle, { helper: dropped, events: ev.map((e) => `${e.event} ${JSON.stringify(e.payload)}`) });
    if (!viaOle) {
      // Fallback: the event the core emits for a drop, sent from the page (covers the UI half only).
      await page.evaluate((p) => window.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event: "studio://dropped", payload: { paths: [p] } }), inputAudio);
      await sleep(800);
    }
    const st = await waitFor("the dropped file in the inbox", async () => {
      const s = await store();
      return s.inbox.includes(inputAudio) ? s : null;
    }, 10000);
    check("dropped file is in the Studio form", true, `via ${viaOle ? "a real OLE drop" : "an emitted studio://dropped event"}; inbox ${JSON.stringify(st.inbox)}`);
    const form = formOf(inputAudio);
    await form.getByRole("button", { name: "Add to queue" }).waitFor({ timeout: 60000 });
    await form.getByLabel("Title").fill("Feel (Stripped) excerpt, Clean");
    await form.getByLabel("Artist").fill("Cortez");
    await form.locator("input[type=radio][value=clean]").check();
    const notice = form.locator(".studio__models-notice");
    await notice.waitFor({ timeout: 20000 });
    const text = (await notice.innerText()).replace(/\s+/g, " ");
    report.steps.clean_notice = text;
    await shot("studio-clean-notice");
    check("Clean: the notice names the missing model, its size and 'licence: none published; commercial use unclear'",
      /for the Clean preset/.test(text) && /204 MB/.test(text) && /licence: none published; commercial use unclear/.test(text), text);
    const t1 = Date.now();
    await notice.getByRole("button", { name: /^Download/ }).click();
    await page.waitForTimeout(1500);
    await shot("studio-clean-downloading");
    await notice.waitFor({ state: "detached", timeout: 30 * 60000 });
    const prog = await page.evaluate(() => window.__feat.events.filter((e) => e.event === "studio://models").map((e) => e.payload));
    report.steps.clean_download = { seconds: (Date.now() - t1) / 1000, progress_events: prog.length, last: prog.at(-1) };
    const after = await invoke("studio_models_status", { preset: "clean" });
    check("Clean: confirmed download finished and the preset is complete", after.all_present, `${report.steps.clean_download.seconds.toFixed(1)} s, ${prog.length} progress events, last ${JSON.stringify(prog.at(-1))}; toasts ${JSON.stringify((await store()).toasts)}`);
    // Cover through the form: the native dialog cannot be automated, so its answer is
    // supplied at the IPC boundary. `__TAURI_INTERNALS__.invoke` cannot be replaced (not
    // writable, not configurable), but Tauri's invoke sends each command as a fetch to
    // http://ipc.localhost/<command>: only the `pick_cover_image` request is answered here,
    // exactly as the core answers it (a JSON string, header Tauri-Response: ok). Everything
    // else, including the click, the UI's pick(), studio_cover_preview, the form state and
    // studio_submit, is the real path.
    const patch = await page.evaluate((cover) => {
      const I = window.__TAURI_INTERNALS__;
      const d = Object.getOwnPropertyDescriptor(I, "invoke");
      const origFetch = window.fetch;
      window.__feat.ipcFetches = [];
      window.fetch = function (input, init) {
        const url = typeof input === "string" ? input : (input && input.url) || String(input);
        if (/ipc\.localhost/.test(url)) window.__feat.ipcFetches.push(url);
        if (/\/pick_cover_image(\?|$)/.test(url)) {
          return Promise.resolve(new Response(JSON.stringify(cover), { status: 200, headers: { "Content-Type": "application/json", "Tauri-Response": "ok" } }));
        }
        return origFetch.call(this, input, init);
      };
      return { invoke_writable: d ? !!d.writable : null, invoke_configurable: d ? !!d.configurable : null };
    }, coverJpg);
    await invoke("app_info"); // does invoke go through fetch here? (else the real dialog would open)
    const viaFetch = await page.evaluate(() => window.__feat.ipcFetches.some((u) => /app_info/.test(u)));
    check("IPC goes through fetch, so the dialog's answer can be supplied without opening it", viaFetch, { ...patch, ipc_fetches: await page.evaluate(() => window.__feat.ipcFetches) });
    if (!viaFetch) throw new Error("cannot supply the cover dialog's answer; not clicking (the native dialog would open)");
    await form.getByRole("button", { name: /Replace cover…|Choose a cover picture…/ }).click();
    await waitFor("the picked cover in the form", async () => /Your picture: /.test((await form.locator(".studio__cover").innerText()) ?? ""), 15000);
    const coverImg = await form.locator(".studio__cover-img").getAttribute("src").catch(() => null);
    report.steps.cover_ipc = await page.evaluate(() => window.__feat.ipcFetches.filter((u) => /cover/.test(u)));
    check("cover row shows the picked picture (thumbnail from studio_cover_preview)", !!coverImg && coverImg.startsWith("data:image/"), `${(await form.locator(".studio__cover").innerText()).replace(/\s+/g, " ")}; thumbnail ${coverImg ? coverImg.slice(0, 30) : null}`);
    await shot("studio-clean-form-ready");
    const job = await submitForm(form);
    const end = await jobEnd(job.job_id, 15 * 60000);
    report.steps.clean_job = end;
    cleanSong = end.output_song_id;
    check("Clean song made", end.state === "done", `${end.state} ${end.error ?? ""} -> ${cleanSong}`);
  }

  // ------------------------------------------------------------ C. Permissive preset
  let permSong = null;
  if (!dropOnly) {
    await page.evaluate((p) => window.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event: "studio://dropped", payload: { paths: [p] } }), inputAudio);
    await waitFor("the file in the inbox again", async () => (await store()).inbox.includes(inputAudio), 10000);
    const form = formOf(inputAudio);
    await form.getByRole("button", { name: "Add to queue" }).waitFor({ timeout: 60000 });
    await form.getByLabel("Title").fill("Feel (Stripped) excerpt, Permissive");
    await form.getByLabel("Artist").fill("Cortez");
    await form.locator("input[type=radio][value=permissive]").check();
    const notice = form.locator(".studio__models-notice");
    await notice.waitFor({ timeout: 20000 });
    const text = (await notice.innerText()).replace(/\s+/g, " ");
    report.steps.permissive_notice = text;
    await shot("studio-permissive-notice");
    check("Permissive: the notice names its model, size and licence", /Permissive licences only preset/.test(text) && /913 MB/.test(text) && /licence: MIT; commercial use allowed/.test(text), text);
    const t1 = Date.now();
    const n0 = await page.evaluate(() => window.__feat.events.filter((e) => e.event === "studio://models").length);
    await notice.getByRole("button", { name: /^Download/ }).click();
    await notice.waitFor({ state: "detached", timeout: 40 * 60000 });
    const prog = await page.evaluate((n) => window.__feat.events.filter((e) => e.event === "studio://models").slice(n).map((e) => e.payload), n0);
    report.steps.permissive_download = { seconds: (Date.now() - t1) / 1000, progress_events: prog.length, last: prog.at(-1) };
    const after = await invoke("studio_models_status", { preset: "permissive" });
    check("Permissive: confirmed download finished and the preset is complete", after.all_present, `${report.steps.permissive_download.seconds.toFixed(1)} s, ${prog.length} progress events, last ${JSON.stringify(prog.at(-1))}`);
    const job = await submitForm(form);
    const end = await jobEnd(job.job_id, 15 * 60000);
    report.steps.permissive_job = end;
    permSong = end.output_song_id;
    check("Permissive song made", end.state === "done", `${end.state} ${end.error ?? ""} -> ${permSong}`);
    const st = await invoke("studio_status");
    check("default preset still marked present", st.models_present === true, `models_present ${st.models_present}`);
    await shot("studio-queue-after");
  }

  // ------------------------------------------------------------ D. Sync editor re-align (4b)
  if (cleanSong) {
    await page.locator("li.studio__job", { hasText: basename(inputAudio) }).filter({ hasText: "Done" }).first().getByRole("button", { name: "Sing it now" }).click();
    await waitFor("the Clean song loaded", async () => (await page.evaluate(() => window.__jkDebug.store.get().song?.summary.song_id)) === cleanSong, 20000);
    await nav("Fix timing");
    const input = page.locator("#line-0");
    await input.waitFor({ timeout: 10000 });
    const old = await input.inputValue();
    const words = old.split(/\s+/);
    const k = words.length > 1 ? 1 : 0;
    const replacement = words[k].toLowerCase() === "jukebox" ? "karaoke" : "jukebox";
    words[k] = replacement;
    const edited = words.join(" ");
    await input.fill(edited);
    await input.press("Enter");
    await sleep(300);
    report.steps.realign = { line0_before: old, line0_edited: edited, word: replacement };
    const t1 = Date.now();
    await page.getByRole("button", { name: "Re-align with Studio" }).click();
    await waitFor("the re-align to finish", async () => (await store()).toasts.some((t) => /^Re-align/.test(t)), 15 * 60000, 500);
    const toasts = (await store()).toasts.filter((t) => /Re-align/.test(t));
    report.steps.realign.seconds = (Date.now() - t1) / 1000;
    report.steps.realign.toasts = toasts;
    check("Re-align with Studio", toasts.some((t) => t.startsWith("Re-aligned")), `${report.steps.realign.seconds.toFixed(1)} s; ${JSON.stringify(toasts)}`);
    await shot("sync-after-realign");
    const saveBtn = page.getByRole("button", { name: "Save into the song" });
    if (await saveBtn.isEnabled()) {
      await saveBtn.click();
      await sleep(1500);
    }
    report.steps.realign.save_enabled_after = await saveBtn.isEnabled();
    report.steps.realign.line0_after = await page.locator("#line-0").inputValue();
    check("Save into the song", !report.steps.realign.save_enabled_after, `line 0 now "${report.steps.realign.line0_after}"`);
  }
  report.songs = { clean: cleanSong, permissive: permSong };
  check("no page errors", report.errors.length === 0, report.errors.slice(0, 5).join(" | ") || "none");
} catch (err) {
  check("run", false, String(err && err.stack ? err.stack : err));
  await shot("failure").catch(() => undefined);
} finally {
  await browser?.close().catch(() => undefined);
  child.kill();
}
await sleep(800);
report.exited = exited;
report.finished_s = since();
writeFileSync(join(outDir, dropOnly ? "real-app-features-drop.json" : "real-app-features.json"), JSON.stringify({ ...report, log: log.slice(-4000) }, null, 2));
process.exit(report.checks.every((c) => c.pass) ? 0 : 1);
