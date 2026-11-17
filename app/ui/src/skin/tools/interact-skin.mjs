// SPDX-License-Identifier: GPL-3.0-or-later
// Drives the skin demo in a real browser and checks the controls behave:
// key dial (keyboard, wheel, drag, end stops, commit), fader, plate toggle, title
// strip selection, bubbles/motion switches. Exit code 1 on the first failure.
//
// Usage (from app/ui, with the dev server running: npx vite --port 5191):
//   node src/skin/tools/interact-skin.mjs <playwright-core dir> http://127.0.0.1:5191/skin-demo.html [chromium|webkit]

import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const [pwDir, base, engine = "chromium"] = process.argv.slice(2);
if (!pwDir || !base) {
  console.error("usage: node interact-skin.mjs <playwright-core dir> <demo url> [chromium|webkit]");
  process.exit(2);
}
const require = createRequire(join(resolve(pwDir), "package.json"));
const pw = require("playwright-core");
const browser = await pw[engine].launch({ headless: true });
let failures = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};

/** Polls `read` until `ok` holds (2 s at most); returns the last value read. */
async function poll(read, ok, ms = 2000) {
  const end = Date.now() + ms;
  let v = await read();
  while (!ok(v) && Date.now() < end) {
    await new Promise((r) => setTimeout(r, 25));
    v = await read();
  }
  return v;
}

async function open(query) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}?hud=0&${query}`, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
  return { page, errors };
}

// ------------------------------------------------------------- key dial
{
  const { page, errors } = await open("view=stage&motion=full");
  // The console dial is the last slider labelled "Key" on the page.
  const dial = page.locator('.jk-console [role="slider"][aria-label="Key"]');
  const label = page.locator(".jk-console .jk-keydial-label");
  const now = async () => Number(await dial.getAttribute("aria-valuenow"));
  {
    const v = await now();
    check(v === 2, "dial starts at +2", String(v));
  }
  // Two quick steps: the pointer moves at once, the commit waits for the pause.
  // Dispatched inside the page so the check does not depend on round-trip speed.
  const quick = await page.evaluate(async () => {
    const d = document.querySelector('.jk-console [role="slider"][aria-label="Key"]');
    d.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    d.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    await new Promise((r) => setTimeout(r, 0));
    return { now: d.getAttribute("aria-valuenow"), cls: document.querySelector(".jk-console .jk-keydial").className };
  });
  check(quick.now === "4", "two steps move the pointer at once to +4", quick.now);
  check(quick.cls.includes("jk-keydial--pending"), "label shown as pending before the commit", quick.cls);
  {
    const t = await poll(() => label.textContent(), (x) => x.includes("(+4)"));
    check(t.includes("(+4)"), "one commit after the pause updates the label", t);
  }
  await dial.focus();
  await page.keyboard.press("[");
  {
    const v = await poll(now, (x) => x === 3);
    check(v === 3, "[ = down one", String(v));
  }
  await page.keyboard.press("End");
  {
    const v = await poll(now, (x) => x === 12);
    check(v === 12, "End = +12", String(v));
  }
  await page.keyboard.press("ArrowUp");
  {
    const cls = await poll(() => dial.getAttribute("class"), (x) => /bump/.test(x));
    const v = await now();
    check(v === 12 && /bump/.test(cls), "end stop holds at +12 and shakes", `${v} ${cls}`);
  }
  await page.keyboard.press("0");
  {
    const t = await poll(() => label.textContent(), (x) => x.includes("original"));
    const v = await now();
    check(v === 0 && t.includes("original"), "0 = original key", `${v}: ${t}`);
  }
  // Wheel: one notch up = one semitone up.
  const box = await dial.boundingBox();
  const cx = box.x + box.width / 2;
  const cy = box.y + box.height / 2;
  await page.mouse.move(cx, cy);
  await page.mouse.wheel(0, -100);
  {
    const v = await poll(now, (x) => x === 1);
    check(v === 1, "wheel notch up = +1", String(v));
  }
  // Drag clockwise round the ring by 45 degrees = 4 detents.
  const R = box.width * 0.42;
  await page.mouse.move(cx, cy - R);
  await page.mouse.down();
  for (let a = 0; a <= 45; a += 5) {
    const rad = (a * Math.PI) / 180;
    await page.mouse.move(cx + R * Math.sin(rad), cy - R * Math.cos(rad));
  }
  const during = await now();
  await page.mouse.up();
  {
    const v = await now();
    check(during === 5 && v === 5, "drag 45 degrees clockwise = +4 detents", `${during} -> ${v}`);
  }
  {
    const t = await poll(() => label.textContent(), (x) => x.includes("(+5)"));
    check(t.includes("(+5)"), "drag commits on release", t);
  }
  // Plates.
  await page.locator('.jk-console button[aria-label="Key down one semitone"]').click();
  {
    const v = await poll(now, (x) => x === 4);
    check(v === 4, "minus plate = down one", String(v));
  }
  // Fader and play plate.
  const guide = page.locator('.jk-console [role="slider"][aria-label="Guide"]');
  await guide.focus();
  for (let i = 0; i < 5; i++) await page.keyboard.press("ArrowUp");
  check((await guide.getAttribute("aria-valuenow")) === "10", "fader: five ArrowUp = 10 %", await guide.getAttribute("aria-valuenow"));
  const play = page.locator('.jk-console button[aria-label="Play"]');
  await play.click();
  check((await page.locator('.jk-console button[aria-label="Pause"]').getAttribute("aria-pressed")) === "true", "play plate lights up (aria-pressed)");
  check(errors.length === 0, "no page errors", errors.join("; "));
  await page.close();
}

// ------------------------------------------------------------- library
{
  const { page } = await open("view=library");
  const card = page.locator(".jk-strip", { hasText: "Greensleeves" });
  await card.click();
  check((await card.getAttribute("aria-pressed")) === "true", "clicking a title strip selects it");
  await page.close();
}

// ------------------------------------------------------------- switches
{
  const { page } = await open("view=stage&motion=full&bubbles=1");
  const st = await page.evaluate(() => ({
    canvas: document.querySelectorAll("canvas.jk-bubbles").length,
    drift: document.getAnimations().filter((a) => a.effect?.target?.hasAttribute?.("data-jk-drift")).length,
  }));
  check(st.canvas === 1 && st.drift === 4, "full motion: one bubble canvas, four drifting light strips", JSON.stringify(st));
  await page.close();
}
{
  const { page } = await open("view=stage&motion=full&bubbles=0");
  const n = await page.evaluate(() => document.querySelectorAll("canvas").length);
  check(n === 0, "bubbles=false: no canvas at all", String(n));
  await page.close();
}
{
  const { page } = await open("view=stage&motion=reduce&bubbles=1");
  const st = await page.evaluate(() => ({
    canvas: document.querySelectorAll("canvas").length,
    anims: document.getAnimations().length,
  }));
  check(st.canvas === 0 && st.anims === 0, "motion=reduce: no bubbles, no animations", JSON.stringify(st));
  await page.close();
}
{
  // prefers-reduced-motion followed when the setting is "system".
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
  const page = await ctx.newPage();
  await page.goto(`${base}?hud=0&view=stage&motion=system&bubbles=1`, { waitUntil: "load" });
  await page.waitForTimeout(400);
  const st = await page.evaluate(() => ({ canvas: document.querySelectorAll("canvas").length, motion: document.querySelector(".jk-frame")?.getAttribute("data-jk-motion") }));
  check(st.canvas === 0 && st.motion === "reduce", "motion=system follows prefers-reduced-motion: reduce", JSON.stringify(st));
  await ctx.close();
}
{
  const { page } = await open("view=stage&motion=full&bubbles=1&paused=1");
  const before = await page.evaluate(() => window.__jkSkin.bubbleStats.frames);
  await page.waitForTimeout(1000);
  const st = await page.evaluate(() => ({
    frames: window.__jkSkin.bubbleStats.frames,
    paused: document.getAnimations().filter((a) => a.playState === "paused").length,
  }));
  check(st.frames === before && st.paused === 4, "paused prop: bubble loop stopped, drift paused", JSON.stringify({ before, ...st }));
  await page.close();
}

await browser.close();
console.log(failures === 0 ? `OK (${engine})` : `${failures} failure(s) (${engine})`);
process.exit(failures === 0 ? 0 : 1);
