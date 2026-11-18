// SPDX-License-Identifier: GPL-3.0-or-later
// The owner's request of 18 Nov 2026, "add the ability delete songs from the library"
// (DECISIONS D-230), in the REAL release app, driven over WebView2's DevTools like the
// other real-app scripts: delete the song that is playing, delete several with Select,
// restore one, delete one for good, empty the rest. After every step the files on disk
// (Library/, Deleted/, Deleted/index.json) and the Library on screen (cards, free space)
// are checked; screenshots at 1440x900 with the jukebox look.
//
// SAFETY: run it only on a portable folder of your own with test songs (never the owner's
// dist/). The app plays nothing audible: the "null:48000" output device and master 0 are
// written into <root>/Settings/settings.json before it starts.
//
//   node scripts/real-app-delete-songs.mjs <Jukeoke.exe> <root> <out dir>
//
// <root>/Library must hold these test songs (any audio):
//   Demo - Twinkle.jksong, Pop/Test Song A.jksong, Pop/Test Song B.jksong,
//   Test Song C.jksong, Test Song D.jksong
// Report: <out dir>/delete-songs.json; every step prints PASS / FAIL.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [exe, root, outDir] = process.argv.slice(2);
if (!exe || !root || !outDir) {
  console.error("usage: node scripts/real-app-delete-songs.mjs <Jukeoke.exe> <root> <out dir>");
  process.exit(2);
}
if (/[\\/]dist[\\/]?$/i.test(root) || /[\\/]dist[\\/]/i.test(join(root, "x"))) {
  console.error("refusing to run on a dist/ folder: use a portable folder of your own");
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
const report = { exe, root, started: new Date().toISOString(), checks: [], errors: [], measurements: {} };
let failures = 0;
const check = (name, pass, detail = "") => {
  report.checks.push({ name, pass, detail });
  if (!pass) failures++;
  console.log(`${pass ? "PASS" : "FAIL"} ${name}${detail === "" ? "" : `: ${typeof detail === "string" ? detail : JSON.stringify(detail)}`}`);
  return pass;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const waitFor = async (what, fn, limitMs = 15000, everyMs = 100) => {
  const end = Date.now() + limitMs;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out after ${limitMs / 1000} s waiting for ${what}`);
    await sleep(everyMs);
  }
};

const lib = (p) => join(root, "Library", ...p.split("/"));
const del = (n) => join(root, "Deleted", n);
const sha = (f) => createHash("sha256").update(readFileSync(f)).digest("hex").slice(0, 16);
const size = (f) => statSync(f).size;
const deletedFiles = () => (existsSync(join(root, "Deleted")) ? readdirSync(join(root, "Deleted")).sort() : []);
const index = () => JSON.parse(readFileSync(join(root, "Deleted", "index.json"), "utf8"));
/** True when no program has the file open: Windows lets us open it with no sharing at all. */
function nobodyHolds(file) {
  const ps = `try { $f = [System.IO.File]::Open('${file.replaceAll("'", "''")}', 'Open', 'Read', 'None'); $f.Close(); 'free' } catch { 'held: ' + $_.Exception.Message }`;
  const r = spawnSync("powershell", ["-NoProfile", "-Command", ps], { encoding: "utf8" });
  return r.stdout.trim();
}

const SONGS = ["Demo - Twinkle.jksong", "Pop/Test Song A.jksong", "Pop/Test Song B.jksong", "Test Song C.jksong", "Test Song D.jksong"];
for (const s of SONGS) if (!existsSync(lib(s))) throw new Error(`missing test song ${lib(s)}`);
if (existsSync(join(root, "Deleted")) && deletedFiles().length > 0) throw new Error(`${join(root, "Deleted")} is not empty: start from a fresh folder`);
const before = Object.fromEntries(SONGS.map((s) => [s, { bytes: size(lib(s)), sha: sha(lib(s)) }]));

// ---------------------------------------------------------------- the app
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify({ ui: { skin: true, bubbles: true, motion: "full" }, audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} } }, null, 2),
);
const port = 9300 + Math.floor(Math.random() * 400);
const app = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: "ignore" });
let browser;
for (let i = 0; ; i++) {
  try {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
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
const shot = async (name) => {
  await sleep(250);
  await page.screenshot({ path: join(outDir, name) });
  console.log(`     screenshot ${name}`);
};
const appState = () => page.evaluate(() => {
  const s = window.__jkDebug.store.get();
  return { screen: s.screen, song: s.song?.summary.song_id ?? null, player: s.player?.state ?? null, modal: s.modal, deleted: s.deleted?.count ?? null };
});
const cards = () => page.$$eval(".library__strip", (els) => els.map((e) => e.getAttribute("data-song-id")));
const spaceText = () => page.$eval(".library__space", (e) => e.textContent.trim());
const strip = (id) => page.locator(`.library__strip[data-song-id="${id}"]`);
const dialogGone = () => waitFor("the dialog to close", async () => (await page.$("dialog.confirm")) === null);

try {
  // ---- set up: names for the generated songs (they all say "Test Song" inside).
  for (const s of SONGS.filter((x) => x.includes("Test Song"))) {
    const t = s.split("/").pop().replace(".jksong", "");
    await invoke("metadata_save", { song_id: s, metadata: { title: t, artist: "Jukeoke Test" } });
  }
  await page.evaluate(() => window.__jkDebug.store.set({ screen: "library" }));
  await waitFor("five cards", async () => (await cards()).length === 5, 20000);
  for (const s of SONGS) before[s] = { bytes: size(lib(s)), sha: sha(lib(s)) };
  report.measurements.before = before;
  const list0 = await invoke("library_list");
  check("Library lists the 5 test songs, nothing deleted", list0.songs.length === 5 && list0.deleted_count === 0, { songs: list0.songs.length, deleted_count: list0.deleted_count });

  // ---- 1. delete the song that is playing.
  const A = "Pop/Test Song A.jksong";
  await strip(A).locator(".jk-strip").click();
  await waitFor("Test Song A on the stage", async () => (await appState()).song === A);
  await invoke("player_play");
  await waitFor("playing", async () => (await appState()).player === "playing");
  await sleep(1500);
  const playing = await invoke("player_state");
  check("Test Song A plays (null device, master 0)", playing.state === "playing" && playing.position_s > 0.8, { state: playing.state, position_s: playing.position_s, device: playing.device?.name });
  await invoke("player_set", { changes: { semitones: 3 } }); // a pending slot save, written within 1 s
  await page.locator(".skin-nav__item", { hasText: "Library" }).click();
  await waitFor("the library screen", async () => (await appState()).screen === "library");
  await strip(A).hover();
  await shot("1-card-delete-plate.png");
  const plateOpacity = await strip(A).locator(".library__trash").evaluate((e) => getComputedStyle(e).opacity);
  check("the playing song's card shows its trash plate", plateOpacity === "1", { opacity: plateOpacity });
  await strip(A).locator(".library__trash").click();
  await page.waitForSelector("dialog.confirm");
  const q1 = await page.$eval("dialog.confirm", (d) => ({ title: d.querySelector(".confirm__title").textContent, body: d.querySelector(".confirm__body").textContent, focused: document.activeElement?.className ?? "" }));
  check("the question names the song and says where it goes", q1.title === "Delete “Jukeoke Test - Test Song A”?" && /moves to Deleted songs, where you can restore it or delete it for good/.test(q1.body) && /playing now; it will stop/.test(q1.body), q1);
  check("Cancel has the focus", /confirm__cancel/.test(q1.focused), q1.focused);
  await shot("2-confirm-playing.png");
  // Escape first: nothing happens.
  await page.keyboard.press("Escape");
  await dialogGone();
  await sleep(300);
  check("Escape deleted nothing; still playing", existsSync(lib(A)) && (await invoke("player_state")).state === "playing", "");
  await strip(A).hover();
  await strip(A).locator(".library__trash").click();
  await page.waitForSelector("dialog.confirm");
  const t0 = Date.now();
  await page.locator(".confirm__ok").click();
  await dialogGone();
  await waitFor("the card to go", async () => !(await cards()).includes(A));
  report.measurements.delete_playing_ms = Date.now() - t0;
  const st1 = await invoke("player_state");
  check("the song was stopped and closed", st1.state === "empty" && st1.song_id === null, { state: st1.state, song_id: st1.song_id });
  check("its file left Library/ and is in Deleted/", !existsSync(lib(A)) && existsSync(del("Test Song A.jksong")), deletedFiles());
  check("same size (moved, not rewritten)", size(del("Test Song A.jksong")) === before[A].bytes, { before: before[A].bytes, after: size(del("Test Song A.jksong")) });
  check("nobody holds the moved file (every handle released)", nobodyHolds(del("Test Song A.jksong")) === "free", nobodyHolds(del("Test Song A.jksong")));
  const ix1 = index();
  check("index.json remembers where it was", ix1.format === "jukeoke-deleted-songs" && ix1.songs.length === 1 && ix1.songs[0].original === A && !("pending" in ix1.songs[0]), ix1.songs);
  const list1 = await invoke("library_list");
  check("library_list: 4 songs, 1 deleted", list1.songs.length === 4 && list1.deleted_count === 1 && list1.deleted_bytes === before[A].bytes, { songs: list1.songs.length, deleted_count: list1.deleted_count, deleted_bytes: list1.deleted_bytes });
  const space1 = await spaceText();
  check("the Library header counts the deleted song", /· 1 deleted \(/.test(space1), space1);
  check("the app left the stage and holds no song", (await appState()).song === null, await appState());

  // ---- 2. delete several with Select.
  const B = "Pop/Test Song B.jksong";
  const C = "Test Song C.jksong";
  await page.locator(".library__select").click();
  await page.waitForSelector(".library__select-bar");
  await strip(B).locator(".jk-strip").click();
  await strip(C).locator(".jk-strip").click();
  const ticked = await page.$$eval(".jk-strip--checked", (els) => els.length);
  const countText = await page.$eval(".library__select-count", (e) => e.textContent);
  check("Select ticks two cards", ticked === 2 && countText === "2 selected", { ticked, countText });
  check("ticking a card does not load it", (await appState()).song === null, "");
  await shot("3-select.png");
  await page.locator(".library__delete-ticked").click();
  await page.waitForSelector("dialog.confirm");
  const q2 = await page.$eval("dialog.confirm", (d) => ({ title: d.querySelector(".confirm__title").textContent, list: [...d.querySelectorAll(".confirm__list li")].map((li) => li.textContent) }));
  check("the question asks about 2 songs and lists them", q2.title === "Delete 2 songs?" && q2.list.length === 2, q2);
  await shot("4-confirm-several.png");
  await page.locator(".confirm__ok").click();
  await dialogGone();
  await waitFor("two cards to go", async () => (await cards()).length === 2);
  check("both files moved into Deleted/", !existsSync(lib(B)) && !existsSync(lib(C)) && existsSync(del("Test Song B.jksong")) && existsSync(del("Test Song C.jksong")), deletedFiles());
  check("byte for byte the same files", sha(del("Test Song B.jksong")) === before[B].sha && sha(del("Test Song C.jksong")) === before[C].sha, "");
  check("Select ended after the delete", (await page.$(".library__select-bar")) === null, "");
  const ix2 = index();
  check("index.json has 3 entries, all settled", ix2.songs.length === 3 && ix2.songs.every((e) => !("pending" in e)), ix2.songs.map((e) => [e.file, e.original]));

  // ---- 3. the Deleted songs view.
  await page.locator(".library__show-deleted").click();
  await waitFor("three deleted songs", async () => (await page.$$(".deleted-song")).length === 3);
  const view = await page.$eval(".deleted", (e) => ({
    summary: e.querySelector(".deleted__summary").textContent,
    rows: [...e.querySelectorAll(".deleted-song")].map((r) => ({ id: r.getAttribute("data-deleted-id"), title: r.querySelector(".deleted-song__title").textContent, size: r.querySelector(".deleted-song__size").textContent, date: r.querySelector(".deleted-song__date").textContent })),
  }));
  check("Deleted songs lists title, size and deletion date", view.rows.length === 3 && view.rows.every((r) => r.size.endsWith("MB") && r.date.startsWith("Deleted ")), view);
  await shot("5-deleted-songs.png");

  // ---- 4. restore Test Song A: back in Pop/, with the key change saved before the delete.
  await page.locator('.deleted-song[data-deleted-id="Test Song A.jksong"] .deleted-song__restore').click();
  await waitFor("two deleted songs", async () => (await page.$$(".deleted-song")).length === 2);
  check("Test Song A is back in Library/Pop/", existsSync(lib(A)) && !existsSync(del("Test Song A.jksong")), deletedFiles());
  const list4 = await invoke("library_list");
  const a4 = list4.songs.find((s) => s.song_id === A);
  check("listed again, with the key change it had when it was deleted (+3)", !!a4 && a4.semitones === 3, a4 && { semitones: a4.semitones, title: a4.title });
  check("index.json forgot it", index().songs.every((e) => e.file !== "Test Song A.jksong"), index().songs.map((e) => e.file));

  // ---- 5. delete Test Song B for good.
  const freeBefore = (await invoke("library_list")).free_bytes;
  await page.locator('.deleted-song[data-deleted-id="Test Song B.jksong"] .deleted-song__purge').click();
  await page.waitForSelector("dialog.confirm");
  const q5 = await page.$eval("dialog.confirm", (d) => ({ title: d.querySelector(".confirm__title").textContent, body: d.querySelector(".confirm__body").textContent }));
  check("for good asks again, saying it cannot be undone", q5.title === "Delete “Jukeoke Test - Test Song B” for good?" && /cannot be undone/.test(q5.body), q5);
  await shot("6-confirm-for-good.png");
  await page.locator(".confirm__ok").click();
  await dialogGone();
  await waitFor("one deleted song", async () => (await page.$$(".deleted-song")).length === 1);
  const freeAfter = (await invoke("library_list")).free_bytes;
  report.measurements.purge_b = { bytes: before[B].bytes, free_before: freeBefore, free_after: freeAfter, gained: freeAfter - freeBefore };
  check("its file is gone for good", !existsSync(lib(B)) && !existsSync(del("Test Song B.jksong")), deletedFiles());
  check("the free space grew by about its size", freeAfter - freeBefore > before[B].bytes * 0.5, report.measurements.purge_b);

  // ---- 6. empty the rest.
  await page.locator(".deleted__empty-all").click();
  await page.waitForSelector("dialog.confirm");
  const q6 = await page.$eval("dialog.confirm", (d) => ({ title: d.querySelector(".confirm__title").textContent, body: d.querySelector(".confirm__body").textContent }));
  check("Empty asks again", q6.title === "Empty Deleted songs?" && /cannot be undone/.test(q6.body), q6);
  await shot("7-confirm-empty.png");
  await page.locator(".confirm__ok").click();
  await dialogGone();
  await waitFor("no deleted songs", async () => /No deleted songs/.test(await page.$eval(".deleted", (e) => e.textContent)));
  check("Deleted/ holds only index.json", JSON.stringify(deletedFiles()) === JSON.stringify(["index.json"]), deletedFiles());
  check("index.json is empty", index().songs.length === 0, index());
  await shot("8-deleted-empty.png");

  // ---- back to the Library.
  await page.locator(".library__views .jk-plate", { hasText: "Songs" }).click();
  await waitFor("three cards", async () => (await cards()).length === 3);
  const finalCards = (await cards()).sort();
  check("the Library shows Demo, Test Song A and Test Song D", JSON.stringify(finalCards) === JSON.stringify(["Demo - Twinkle.jksong", A, "Test Song D.jksong"].sort()), finalCards);
  const finalList = await invoke("library_list");
  check("library_list agrees, nothing left in Deleted songs", finalList.songs.length === 3 && finalList.deleted_count === 0 && finalList.deleted_bytes === 0, { songs: finalList.songs.length, deleted_count: finalList.deleted_count });
  const space6 = await spaceText();
  check("the header no longer mentions deleted songs", !/deleted/.test(space6), space6);
  check("untouched songs are byte for byte the same", ["Demo - Twinkle.jksong", "Test Song D.jksong"].every((s) => sha(lib(s)) === before[s].sha), "");
  await shot("9-library-after.png");
  // The restored song plays again.
  await strip(A).locator(".jk-strip").click();
  await waitFor("Test Song A loaded", async () => (await appState()).song === A);
  await invoke("player_play");
  await waitFor("playing again", async () => (await invoke("player_state")).state === "playing");
  check("the restored song plays", true, "");
  await invoke("player_pause");
  check("no errors in the page", report.errors.length === 0, report.errors);
} catch (e) {
  report.errors.push(String(e?.stack ?? e));
  failures++;
  console.log("ERROR", e);
  await page.screenshot({ path: join(outDir, "error.png") }).catch(() => undefined);
} finally {
  report.failures = failures;
  report.finished = new Date().toISOString();
  writeFileSync(join(outDir, "delete-songs.json"), JSON.stringify(report, null, 2));
  await browser?.close().catch(() => undefined);
  spawnSync("powershell", ["-NoProfile", "-Command", `(Get-Process -Id ${app.pid} -ErrorAction SilentlyContinue).CloseMainWindow() | Out-Null`]);
  await new Promise((r) => {
    const t = setTimeout(r, 20000);
    app.on("exit", () => {
      clearTimeout(t);
      r();
    });
  });
}
console.log(failures === 0 ? "ALL PASSED" : `${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
