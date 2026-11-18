// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// The whole UI against the browser mock, in a simulated DOM: the library, loading a
// song, the key control (buttons, keyboard, wheel), play/pause, shortcuts, the help
// overlay, the sync editor saving into the song, Studio/recorder states on a Mac,
// and basic accessibility (every button and form field has a name). First in the
// plain look (settings.ui.skin off), then the same core paths in the jukebox look
// (title strips, key dial, plates, faders, and the settings that drive the skin).
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import { App } from "./App";
import { MockApi } from "./api/mock";
import { changeSettings, changeSongSettings, loadSong, store } from "./state/app";
import { reasonsForPanel } from "./components/Studio";

const root = document.createElement("div");
document.body.appendChild(root);
const api = new MockApi({ latencyMs: 0, skin: false });
render(<App api={api} scriptStartMs={0} />, root);

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 2000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}
const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));
const byText = (sel: string, text: string | RegExp) =>
  qa(sel).find((e) => (typeof text === "string" ? e.textContent?.trim() === text : text.test(e.textContent ?? "")));
const key = (k: string, extra: KeyboardEventInit = {}) => window.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...extra }));

/** Every button has an accessible name; every form control has a label. */
function accessibilityProblems(): string[] {
  const out: string[] = [];
  for (const b of qa("button")) {
    if (!(b.getAttribute("aria-label") || b.textContent?.trim())) out.push(`button without a name: ${b.outerHTML.slice(0, 80)}`);
  }
  for (const el of qa("input, select, textarea")) {
    const id = el.id;
    const labelled = el.getAttribute("aria-label") || el.getAttribute("aria-labelledby") || el.closest("label") || (id && root.querySelector(`label[for="${id}"]`));
    if (!labelled) out.push(`form control without a label: ${el.outerHTML.slice(0, 80)}`);
  }
  return out;
}

afterAll(() => render(null, root));

describe("functional UI on the mock", () => {
  it("shows the library with cover art, free space and damaged songs with their error", async () => {
    await until(() => qa(".title-strip").length > 0);
    expect(qa(".title-strip")).toHaveLength(6);
    expect(qa(".title-strip--damaged")).toHaveLength(2);
    expect(q(".title-strip--damaged .title-strip__error")?.textContent).toMatch(/newer version|incomplete/);
    expect(q(".library__space")?.textContent).toMatch(/18\.7 GB/);
    // Covers load lazily when visible (IntersectionObserver); the mock serves them as generated SVG.
    expect((await api.songCover(store.get().library!.songs[0].song_id, 72)).data_url).toMatch(/^data:image\/svg\+xml/);
    expect(accessibilityProblems()).toEqual([]);
  });

  it("searches without caring about accents, and Ctrl+F focuses the search", async () => {
    key("f", { ctrlKey: true });
    await wait();
    const search = q<HTMLInputElement>("#library-search")!;
    expect(document.activeElement).toBe(search);
    search.value = "frere";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await wait();
    expect(qa(".title-strip__title").map((e) => e.textContent)).toEqual(["Frère Jacques"]);
    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    search.blur();
    await wait();
  });

  it("opens a song on the stage with its saved key", async () => {
    byText(".title-strip", /Twinkle, Twinkle, Little Star/)!.click();
    await until(() => store.get().screen === "stage" && !!store.get().song);
    await wait();
    expect(q(".stage__title")?.textContent).toBe("Twinkle, Twinkle, Little Star");
    expect(q(".key-control__label")?.textContent).toBe("A major → B major (+2)");
    expect(accessibilityProblems()).toEqual([]);
  });

  it("changes the key with the buttons, [ ] + - keys and the wheel, clamped to ±12, and saves it", async () => {
    q<HTMLButtonElement>(".key-control__up")!.click();
    await wait();
    expect(q(".key-control__label")?.textContent).toBe("A major → C major (+3)");
    key("[");
    key("-");
    await wait();
    expect(store.get().songSettings?.semitones).toBe(1);
    key("]");
    key("+");
    await wait();
    expect(store.get().songSettings?.semitones).toBe(3);
    const face = q(".key-control")!;
    face.dispatchEvent(new WheelEvent("wheel", { deltaY: -120, bubbles: true, cancelable: true }));
    await wait();
    expect(store.get().songSettings?.semitones).toBe(5);
    for (let i = 0; i < 20; i++) key("]");
    await wait(50);
    expect(store.get().songSettings?.semitones).toBe(12);
    expect(q<HTMLButtonElement>(".key-control__up")!.disabled).toBe(true);
    byText("button", "Original key")!.click();
    await wait(50);
    expect(store.get().songSettings?.semitones).toBe(0);
    // The mock saved it into the "song file": reloading the library shows it.
    const lib = await api.libraryList();
    expect(lib.songs.find((s) => s.title.startsWith("Twinkle, Twinkle"))?.semitones).toBe(0);
  });

  it("plays and pauses with Space and seeks with the arrows", async () => {
    key(" ");
    await until(() => store.get().player?.state === "playing");
    key("ArrowRight");
    await wait(30);
    expect(api.debugPosition()).toBeGreaterThan(4.9);
    key(" ");
    await until(() => store.get().player?.state === "paused");
  });

  it("toggles the full-screen stage with F and leaves it with Esc", async () => {
    key("f");
    await wait();
    expect(store.get().fullscreen).toBe(true);
    expect(q(".app-header")).toBeNull();
    key("Escape");
    await wait();
    expect(store.get().fullscreen).toBe(false);
  });

  it("documents the shortcuts in a ? overlay", async () => {
    key("?");
    await wait();
    const dialog = q(".help");
    expect(dialog).not.toBeNull();
    expect(dialog!.textContent).toMatch(/Play \/ pause/);
    expect(dialog!.textContent).toMatch(/Key down \/ up one semitone/);
    q<HTMLButtonElement>(".dialog__close")!.click();
    await wait();
    store.set({ overlay: null });
  });

  it("sync editor: nudges a line, saves it into the song, and resets it", async () => {
    store.set({ screen: "sync" });
    await until(() => qa(".sync__line").length > 0);
    const before = store.get().song!.lyrics!.lines[1].start;
    (qa(".sync__line")[1].querySelectorAll("button")[1] as HTMLButtonElement).click();
    await wait();
    expect(Number(qa(".sync__line-time")[1].textContent)).toBeCloseTo(before + 0.05, 2);
    expect(accessibilityProblems()).toEqual([]);
    const saveBtn = byText("button", "Save into the song") as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(false);
    saveBtn.click();
    await until(() => store.get().song?.lyrics_is_override === true);
    expect(store.get().song!.lyrics!.lines[1].start).toBeCloseTo(before + 0.05, 3);
    byText("button", "Reset to original")!.click();
    await wait();
    byText("button", "Yes, reset")!.click();
    await until(() => store.get().song?.lyrics_is_override === false);
    expect(store.get().song!.lyrics!.lines[1].start).toBeCloseTo(before, 6);
    // The editor's own working copy must follow the reset too (it showed the old
    // corrected times until the verifier's fix).
    await wait();
    expect(Number(qa(".sync__line-time")[1].textContent)).toBeCloseTo(before, 2);
    expect(byText("button", "Save into the song")?.hasAttribute("disabled")).toBe(true);
  });

  it("mixer: guide vocal and levels go to the engine", async () => {
    store.set({ screen: "settings" });
    await until(() => !!q(".mixer"));
    const guide = qa(".mixer input[type=range]").find((i) => i.id && root.querySelector(`label[for="${i.id}"]`)?.textContent?.startsWith("Guide vocal")) as HTMLInputElement;
    guide.value = "0.35";
    guide.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(120);
    expect(store.get().songSettings?.guide_vocal).toBeCloseTo(0.35, 6);
    expect((await api.songLoad(store.get().song!.summary.song_id)).settings.guide_vocal).toBeCloseTo(0.35, 6);
    expect(accessibilityProblems()).toEqual([]);
  });

  it("settings: lyrics motion is smooth by word by default and can be set to exact letters (D-216)", async () => {
    store.set({ screen: "settings" });
    await until(() => !!q(".settings"));
    const select = qa(".settings select").find((s) => root.querySelector(`label[for="${s.id}"]`)?.textContent === "Lyrics motion") as HTMLSelectElement;
    expect(select).toBeTruthy();
    expect(select.value).toBe("word");
    expect(Array.from(select.options).map((o) => o.textContent)).toEqual(["Smooth, paced by word", "Exact, letter by letter"]);
    select.value = "letter";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await until(() => store.get().settings?.ui.lyrics_motion === "letter");
    await wait(40); // the store changes at once, the core a timer tick later
    expect((await api.settingsGet()).ui.lyrics_motion).toBe("letter");
    changeSettings({ ui: { lyrics_motion: "word" } });
    await until(() => store.get().settings?.ui.lyrics_motion === "word");
    expect(accessibilityProblems()).toEqual([]);
  });

  it("settings: static lines with a colour wipe by default, the scrolling ticker on request (D-218)", async () => {
    store.set({ screen: "settings" });
    await until(() => !!q(".settings"));
    const labelled = (text: string) => qa(".settings select, .settings input").find((s) => root.querySelector(`label[for="${s.id}"]`)?.textContent === text);
    const display = labelled("Lyrics display") as HTMLSelectElement;
    expect(display.value).toBe("page");
    expect(Array.from(display.options).map((o) => o.textContent)).toEqual(["Static lines with a colour wipe", "Scrolling ticker"]);
    const next = labelled("Show the next line") as HTMLInputElement;
    expect(next.checked).toBe(true);
    expect(next.disabled).toBe(false);
    const marker = labelled("Marker position (from the left)") as HTMLInputElement;
    expect(marker.disabled).toBe(true); // the page display's needle rides the wipe; no fixed position
    expect(store.get().settings?.ui.colors.sung).toBe("#ffc21a");
    // The Stage draws the page display.
    store.set({ screen: "stage" });
    await until(() => !!q(".stage__lane .page-lyrics"));
    // Scrolling ticker: chosen in Settings, saved by the core, drawn on the Stage.
    store.set({ screen: "settings" });
    await until(() => !!q(".settings"));
    const display2 = labelled("Lyrics display") as HTMLSelectElement;
    display2.value = "scroll";
    display2.dispatchEvent(new Event("change", { bubbles: true }));
    await until(() => store.get().settings?.ui.lyrics_display === "scroll");
    await wait(40);
    expect((await api.settingsGet()).ui.lyrics_display).toBe("scroll");
    await until(() => (labelled("Show the next line") as HTMLInputElement).disabled);
    expect((labelled("Marker position (from the left)") as HTMLInputElement).disabled).toBe(false);
    store.set({ screen: "stage" });
    await until(() => !!q(".stage__lane .ticker") && !q(".stage__lane .page-lyrics"));
    changeSettings({ ui: { lyrics_display: "page" } });
    await until(() => !!q(".stage__lane .page-lyrics"));
    expect(accessibilityProblems()).toEqual([]);
  });

  it("settings: the static lines' progress bar is on by default, can be turned off, and uses the marker colour", async () => {
    store.set({ screen: "settings" });
    await until(() => !!q(".settings"));
    const labelled = (text: string) => qa(".settings select, .settings input").find((s) => root.querySelector(`label[for="${s.id}"]`)?.textContent === text);
    const bar = labelled("Show the progress bar") as HTMLInputElement;
    expect(bar).toBeTruthy();
    expect(bar.checked).toBe(true);
    expect(bar.disabled).toBe(false);
    // The marker colour applies to the page display's bar now (it was disabled there before).
    const colour = labelled("Marker") as HTMLInputElement;
    expect(colour.disabled).toBe(false);
    expect(colour.value).toBe("#7dffd8");
    // Off: saved by the core, and the colour field is disabled (nothing uses it in this display).
    bar.click();
    await until(() => store.get().settings?.ui.show_marker === false);
    await wait(40);
    expect((await api.settingsGet()).ui.show_marker).toBe(false);
    await until(() => (labelled("Marker") as HTMLInputElement).disabled);
    // The Stage still draws the page display.
    store.set({ screen: "stage" });
    await until(() => !!q(".stage__lane .page-lyrics"));
    // The scrolling ticker always has its marker: the switch is disabled there, the colour enabled.
    changeSettings({ ui: { lyrics_display: "scroll" } });
    store.set({ screen: "settings" });
    await until(() => !!q(".settings") && (labelled("Show the progress bar") as HTMLInputElement).disabled);
    expect((labelled("Marker") as HTMLInputElement).disabled).toBe(false);
    changeSettings({ ui: { lyrics_display: "page", show_marker: true } });
    await until(() => store.get().settings?.ui.show_marker === true && (labelled("Show the progress bar") as HTMLInputElement).checked);
    expect(accessibilityProblems()).toEqual([]);
  });

  it("a fader moved just before opening another song is saved in the song it was moved on", async () => {
    // Found by the independent verifier: the throttled player_set reached the core
    // after song_load and was applied to the next song.
    const songs = store.get().library!.songs.filter((s) => !s.error);
    const first = store.get().song!.summary.song_id;
    const other = songs.find((s) => s.song_id !== first)!.song_id;
    const otherBefore = (await api.songLoad(other)).settings.backing_level;
    await loadSong(first);
    changeSongSettings({ backing_level: 0.42 }, 40);
    await loadSong(other);
    await wait(80);
    expect(store.get().songSettings?.backing_level).toBeCloseTo(otherBefore, 6);
    expect((await api.songLoad(other)).settings.backing_level).toBeCloseTo(otherBefore, 6);
    expect((await api.songLoad(first)).settings.backing_level).toBeCloseTo(0.42, 6);
    // Back to the song the next tests expect.
    await loadSong(first);
  });

  it("studio: opens once the background probe answers, although app_info said 'checking'", async () => {
    // The real core answers app_info() before its Studio probe has finished, then sends
    // studio://status (end-to-end run: the screen stayed on "Still checking..." for good).
    const info = store.get().info!;
    const checking = { available: false, reasons: ["Still checking whether this computer can run the Studio..."], gpu: null, vram_mb: null, checking: true };
    store.set({ screen: "studio", info: { ...info, capabilities: { ...info.capabilities, studio: checking } } });
    await until(() => !!q(".studio--unavailable"));
    expect(q(".studio--unavailable")!.textContent).toMatch(/Still checking/);
    const st = await api.studioStatus();
    (api as unknown as { emit: (event: string, payload: unknown) => void }).emit("studio://status", { ...st, checking: false });
    await until(() => !q(".studio--unavailable") && !!byText("button", "Choose files…"));
    expect(store.get().info!.capabilities.studio.available).toBe(true);
    expect(store.get().info!.capabilities.studio.checking).toBeFalsy();
    await until(() => store.get().info!.capabilities.studio.gpu !== null); // app_info() fetched again
  });

  it("studio: queues a picked file and shows every stage", async () => {
    store.set({ screen: "studio" });
    await until(() => !!byText("button", "Choose files…"));
    byText("button", "Choose files…")!.click();
    await until(() => !!byText("button", "Add to queue"));
    expect(accessibilityProblems()).toEqual([]);
    byText("button", "Add to queue")!.click();
    await until(() => qa(".studio__job").length === 1);
    expect(qa(".studio__stage").map((e) => e.textContent)).toEqual(["Read", "Separate", "Lyrics", "Align", "Key & tempo", "Save"]);
  });

  it("studio: lyrics from the online lookup are labelled online:<service>, pasted ones are not", async () => {
    // Verifier finding: the UI never sent lyrics.source, so a lookup result was saved as
    // "lrc" / "user_text" (STUDIO_PROTOCOL 7 asks for "online:lrclib").
    const sent: { lyrics: { mode: string; source?: string | null } }[] = [];
    const orig = api.studioSubmit.bind(api);
    api.studioSubmit = (r) => {
      sent.push(JSON.parse(JSON.stringify(r)));
      return orig(r);
    };
    try {
      store.set({ screen: "studio", studioInbox: [] });
      await until(() => !!byText("button", "Choose files…"));
      byText("button", "Choose files…")!.click();
      await until(() => !!byText("button", "Look up lyrics online"));
      byText("button", "Look up lyrics online")!.click();
      await until(() => !!byText("button", "Use timed lyrics"));
      byText("button", "Use timed lyrics")!.click();
      await wait();
      byText("button", "Add to queue")!.click();
      await until(() => sent.length === 1);
      expect(sent[0].lyrics.mode).toBe("lrc");
      expect(sent[0].lyrics.source).toBe("online:lrclib");

      // Same file again; a lookup result first, then the user switches to pasting text:
      // the online label must not stick.
      byText("button", "Choose files…")!.click();
      await until(() => !!byText("button", "Look up lyrics online"));
      byText("button", "Look up lyrics online")!.click();
      await until(() => !!byText("button", "Use text"));
      byText("button", "Use text")!.click();
      await wait();
      const radio = qa("input[type=radio]").find((e) => (e as HTMLInputElement).value === "text" && e.closest("label")?.textContent?.includes("Paste")) as HTMLInputElement;
      const auto = qa("input[type=radio]").find((e) => (e as HTMLInputElement).value === "auto") as HTMLInputElement;
      auto.click();
      await wait();
      radio.click();
      await wait();
      const ta = q<HTMLTextAreaElement>("textarea")!;
      ta.value = "My own words\nTyped by hand";
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      await wait();
      byText("button", "Add to queue")!.click();
      await until(() => sent.length === 2);
      expect(sent[1].lyrics.mode).toBe("text");
      expect(sent[1].lyrics.source ?? null).toBeNull();
    } finally {
      api.studioSubmit = orig;
    }
  });
});

describe("on a Mac (mock platform=macos)", () => {
  it("explains Screen Recording before asking, and the Studio says why it is unavailable", async () => {
    render(null, root);
    const mac = new MockApi({ latencyMs: 0, platform: "macos", skin: false });
    store.set({ ready: false, info: null, screen: "recorder", song: null, songSettings: null, studioInbox: [] });
    render(<App api={mac} scriptStartMs={0} />, root);
    await until(() => !!q(".recorder__permission"));
    expect(q(".recorder__permission")!.textContent).toMatch(/Screen Recording/);
    expect(q(".recorder__button")).toBeNull(); // no record button before the permission
    expect((await mac.appInfo()).capabilities.recorder.permission).toBe("not_determined"); // not asked yet
    byText("button", "Continue to the macOS permission prompt")!.click();
    await until(() => !!q(".recorder__button"));
    store.set({ screen: "studio" });
    await until(() => !!q(".studio--unavailable"));
    expect(q(".studio--unavailable")!.textContent).toMatch(/created on a stronger computer/);
  });

  it("the unavailable Studio says 'created on a stronger computer' once, however many reasons repeat it", async () => {
    render(null, root);
    // Windows without a usable GPU: app_info and studio_status each give reasons, most
    // ending with the same sentence (the core's and the Studio's wording differ slightly).
    const weak = new MockApi({ latencyMs: 0, studio: "off", skin: false });
    store.set({ ready: false, info: null, screen: "studio", song: null, songSettings: null, studioInbox: [], studio: null });
    render(<App api={weak} scriptStartMs={0} />, root);
    await until(() => !!q(".studio--unavailable") && qa(".studio__reasons li").length > 0);
    await wait(50);
    const text = q(".studio--unavailable")!.textContent ?? "";
    expect(text.match(/stronger computer/g)).toHaveLength(1);
    const items = qa(".studio__reasons li").map((li) => li.textContent);
    // The mock's studio_status reason (the panel follows studio://status, D-159), without the sentence.
    expect(items).toContain("The Studio is not available on this computer.");
    expect(items.every((t) => t && !/stronger computer|played anywhere/.test(t))).toBe(true);
    expect(new Set(items).size).toBe(items.length);
  });

  it("reasonsForPanel drops the repeated sentence and duplicates, and keeps everything else", () => {
    expect(
      reasonsForPanel([
        "No NVIDIA graphics card with CUDA was found. Songs are created on a stronger computer and can be played anywhere.",
        "The graphics card has 4.0 GB of memory; the Studio needs at least 6 GB. Songs are created on a stronger computer and can be played anywhere.",
        "The Studio is not available on this computer. Songs are created on a stronger computer and played anywhere.",
        "This computer cannot run the Studio. Songs are created on a stronger computer and can be played anywhere.",
        "No NVIDIA graphics card with CUDA was found.",
        "Songs are created on a stronger computer and can be played anywhere.",
        "The Studio engine did not answer within 60 seconds after starting. Details are in Settings/logs/studio.log.",
      ]),
    ).toEqual([
      "No NVIDIA graphics card with CUDA was found.",
      "The graphics card has 4.0 GB of memory; the Studio needs at least 6 GB.",
      "The Studio is not available on this computer.",
      "This computer cannot run the Studio.",
      "The Studio engine did not answer within 60 seconds after starting. Details are in Settings/logs/studio.log.",
    ]);
  });
});

describe("jukebox skin on the mock (settings.ui.skin on, the default)", () => {
  const sk = new MockApi({ latencyMs: 0 });
  const dialKey = (k: string) => q(".jk-dial")!.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  const fader = (name: string) => qa(".jk-fader-track").find((e) => e.getAttribute("aria-label") === name)!;

  it("draws the frame and its console, and the library as title strips", async () => {
    render(null, root);
    store.set({ ready: false, info: null, settings: null, screen: "library", song: null, songSettings: null, studioInbox: [], fullscreen: false, overlay: null });
    render(<App api={sk} scriptStartMs={0} />, root);
    await until(() => qa(".jk-strip").length > 0);
    await until(() => document.documentElement.dataset.skin === "on");
    expect(q(".jk-frame")).not.toBeNull();
    expect(q(".app-header")).toBeNull();
    // The console holds the transport plates, the key dial and the faders. (The crest
    // and its badge appear once the frame has measured itself, which needs real
    // layout: checked in the browser screenshots, not in this simulated DOM.)
    expect(q(".jk-console-slot .skin-console .skin-transport__play")).not.toBeNull();
    expect(q(".jk-console-slot .skin-console .jk-dial")).not.toBeNull();
    expect(qa(".jk-console-slot .jk-fader-track").map((e) => e.getAttribute("aria-label"))).toEqual(["Guide", "Backing", "Master"]);
    expect(qa(".jk-strip")).toHaveLength(6);
    // Damaged files are listed with their error and cannot be opened.
    expect(qa(".library__strip--damaged .jk-strip")).toHaveLength(2);
    expect(qa(".library__strip--damaged .jk-strip").every((b) => (b as HTMLButtonElement).disabled)).toBe(true);
    expect(q(".library__strip--damaged .jk-strip-note")?.textContent).toMatch(/newer version|incomplete/);
    expect(q(".library__space")?.textContent).toMatch(/18\.7 GB/);
    // Sorting with the plates.
    byText(".library__sort--plates .jk-plate", "Artist")!.click();
    await wait();
    expect(store.get().settings?.ui.library_sort).toBe("artist");
    byText(".library__sort--plates .jk-plate", "Title")!.click();
    await wait();
    expect(accessibilityProblems()).toEqual([]);
  });

  it("opens a song from its strip; the key dial shows the key and keeps the shortcuts", async () => {
    byText(".jk-strip", /Twinkle, Twinkle, Little Star/)!.click();
    await until(() => store.get().screen === "stage" && !!store.get().song);
    await wait();
    expect(q(".stage__title")?.textContent).toBe("Twinkle, Twinkle, Little Star");
    expect(q(".jk-keydial-label")?.textContent).toBe("A major → B major (+2)");
    expect(q(".jk-dial")?.getAttribute("aria-valuenow")).toBe("2");
    // App-wide shortcuts change the key at once, as in the plain look.
    key("]");
    await wait();
    expect(store.get().songSettings?.semitones).toBe(3);
    expect(q(".jk-keydial-label")?.textContent).toBe("A major → C major (+3)");
    key("[");
    key("-");
    await wait();
    expect(store.get().songSettings?.semitones).toBe(1);
    // The dial's own keys move the pointer at once and commit once the hand stops.
    dialKey("ArrowUp");
    dialKey("ArrowUp");
    await wait(10);
    expect(q(".jk-dial")?.getAttribute("aria-valuenow")).toBe("3");
    expect(store.get().songSettings?.semitones).toBe(1);
    await until(() => store.get().songSettings?.semitones === 3, 1500);
    // The + plate, the wheel, the end stop and "0 = original key".
    q<HTMLButtonElement>(".jk-keydial-up")!.click();
    await until(() => store.get().songSettings?.semitones === 4, 1500);
    q(".jk-dial")!.dispatchEvent(new WheelEvent("wheel", { deltaY: -120, bubbles: true, cancelable: true }));
    await until(() => store.get().songSettings?.semitones === 5, 1500);
    dialKey("End");
    await until(() => store.get().songSettings?.semitones === 12, 1500);
    await wait(20);
    expect(q<HTMLButtonElement>(".jk-keydial-up")!.disabled).toBe(true);
    dialKey("0");
    await until(() => store.get().songSettings?.semitones === 0, 1500);
    await wait(50);
    const lib = await sk.libraryList();
    expect(lib.songs.find((s) => s.title.startsWith("Twinkle, Twinkle"))?.semitones).toBe(0);
    expect(accessibilityProblems()).toEqual([]);
  });

  it("faders set the guide vocal and the master volume; the play plate plays and pauses", async () => {
    fader("Guide").dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp", bubbles: true, cancelable: true }));
    await wait(80);
    expect(store.get().songSettings?.guide_vocal).toBeCloseTo(0.1, 6);
    fader("Master").dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }));
    await wait();
    expect(store.get().settings?.audio.master_volume).toBe(1);
    const playPlate = () => q<HTMLButtonElement>(".skin-transport__play")!;
    playPlate().click();
    await until(() => store.get().player?.state === "playing");
    await wait();
    expect(playPlate().getAttribute("aria-pressed")).toBe("true");
    key(" ");
    await until(() => store.get().player?.state === "paused");
    // The mock saved the guide level into the "song file".
    expect((await sk.songLoad(store.get().song!.summary.song_id)).settings.guide_vocal).toBeCloseTo(0.1, 6);
  });

  it("full screen keeps the frame and shows the stage alone", async () => {
    key("f");
    await wait();
    expect(store.get().fullscreen).toBe(true);
    expect(q(".jk-frame")).not.toBeNull();
    expect(q(".skin-nav")).toBeNull();
    expect(q(".stage--fullscreen")).not.toBeNull();
    key("Escape");
    await wait();
    expect(store.get().fullscreen).toBe(false);
    expect(q(".skin-nav")).not.toBeNull();
  });

  it("settings drive the skin: motion (system follows the OS), bubbles, and the plain look", async () => {
    changeSettings({ ui: { motion: "reduce" } });
    await wait();
    expect(q(".jk-frame")?.getAttribute("data-jk-motion")).toBe("reduce");
    expect(document.documentElement.dataset.bubbles).toBe("off");
    changeSettings({ ui: { motion: "system" } });
    await wait();
    const osReduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    expect(q(".jk-frame")?.getAttribute("data-jk-motion")).toBe(osReduce ? "reduce" : "full");
    expect(document.documentElement.dataset.bubbles).toBe(osReduce ? "off" : "on");
    changeSettings({ ui: { bubbles: false } });
    await wait();
    expect(document.documentElement.dataset.bubbles).toBe("off");
    changeSettings({ ui: { skin: false, bubbles: true } });
    await wait();
    expect(document.documentElement.dataset.skin).toBe("off");
    expect(q(".jk-frame")).toBeNull();
    expect(q(".app-header")).not.toBeNull();
    expect(q(".key-control__label")?.textContent).toMatch(/^A major/);
    changeSettings({ ui: { skin: true } });
    await wait();
    expect(q(".jk-frame")).not.toBeNull();
  });
});
