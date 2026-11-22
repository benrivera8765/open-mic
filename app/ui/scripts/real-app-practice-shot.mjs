// SPDX-License-Identifier: GPL-3.0-or-later
// One look at the practice lane in the REAL app: opens a song from a portable folder,
// turns practice on (this PC has no microphone, so it is "watch only"), plays silently
// from a given time and saves a screenshot of the whole window. Nothing is played aloud:
// Settings/settings.json selects the engine's null output device and master volume 0.
// Never prints lyrics.
//
//   node scripts/real-app-practice-shot.mjs <root> <out dir> <song title> <start s> [stop s]
//
// <root> is a portable folder: Windows/<name>.exe (branding.json) and Library/ with the song.
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const [root, outDir, title, startArg, stopArg] = process.argv.slice(2);
if (!root || !outDir || !title || !startArg) {
  console.error("usage: node scripts/real-app-practice-shot.mjs <root> <out dir> <song title> <start s> [stop s]");
  process.exit(2);
}
const name = JSON.parse(readFileSync(new URL("../../../branding.json", import.meta.url), "utf8")).name;
const exe = join(root, "Windows", `${name}.exe`);
mkdirSync(outDir, { recursive: true });
mkdirSync(join(root, "Settings"), { recursive: true });
writeFileSync(
  join(root, "Settings", "settings.json"),
  JSON.stringify({ ui: { skin: true, bubbles: true, motion: "full", library_sort: "title", fullscreen_stage: false }, audio: { device_id: "null:48000", exclusive: false, master_volume: 0, device_offsets_ms: {} } }, null, 2),
);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const port = 9300 + (process.pid % 500);
const child = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: "ignore" });
let browser;
try {
  for (let i = 0; ; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
      break;
    } catch (e) {
      if (i > 60) throw e;
      await sleep(500);
    }
  }
  const ctx = browser.contexts()[0];
  const page = ctx.pages()[0] ?? (await ctx.waitForEvent("page"));
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForSelector(".jk-strip, .title-strip", { timeout: 30000 });
  const invoke = (c, a = {}) => page.evaluate(([c, x]) => window.__TAURI_INTERNALS__.invoke(c, x), [c, a]);
  const badge = await page.evaluate(() => document.querySelector(".jk-frame-crest .jk-badge")?.getAttribute("aria-label") ?? null);
  console.log(`badge: ${badge}; window title: ${await page.title()}`);
  await page.locator(".jk-strip, button.title-strip", { hasText: title }).first().click();
  await page.waitForSelector(".stage__practice", { timeout: 30000 });
  await sleep(800);
  await page.locator(".stage__practice").first().click();
  for (let i = 0; i < 200; i++) {
    const s = await invoke("practice_state");
    if (s.phase === "watch_only" || s.phase === "listening") break;
    await sleep(100);
  }
  const start = Number(startArg);
  const stop = stopArg ? Number(stopArg) : start + 1.5;
  await invoke("player_seek", { position_s: start });
  await invoke("player_play");
  for (let i = 0; i < 400; i++) {
    const p = await invoke("player_state");
    if ((p.position_s ?? 0) >= stop) break;
    await sleep(25);
  }
  await invoke("player_pause");
  await sleep(600);
  const file = join(outDir, `practice-${start}.png`);
  await page.screenshot({ path: file });
  console.log(`saved ${file}; page errors: ${errors.length}`);
  await invoke("player_seek", { position_s: 0 });
} finally {
  await browser?.close().catch(() => {});
  child.kill();
}
