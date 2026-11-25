// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// The owner's request of 18 Nov 2026: record at the same time the music player starts
// ("Record & play"), and record several songs back to back, kept apart for the Studio
// ("Record songs separately"). The Recorder screen's states on the mock: the player
// choice, no player, the Mac, both recording modes with Stop (and "pause the player"),
// the session list with complete / partial songs, the excluded parts, preview, and the
// hand-off to the Studio with the title, artist and cover from each song's sidecar.
import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi, type MockOptions } from "../api/mock";
import type { StudioSubmitRequest } from "../api/types";
import { store } from "../state/app";

const root = document.createElement("div");
document.body.appendChild(root);
let current: MockApi | null = null;
afterEach(() => {
  render(null, root);
  current?.dispose();
  current = null;
});

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 4000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}
const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));
const button = (text: string | RegExp) =>
  qa("button").find((e) => (typeof text === "string" ? e.textContent?.trim() === text : text.test(e.textContent ?? "")));
const text = () => root.textContent ?? "";
/** The big button is busy until the start command has fully answered. */
const stopEnabled = () => until(() => q<HTMLButtonElement>(".recorder__button")?.disabled === false);

async function open(options: Partial<MockOptions> = {}): Promise<MockApi> {
  render(null, root);
  store.set({ ready: false, info: null, screen: "recorder", song: null, songSettings: null, recordingSessions: [], players: null, previewPath: null, recorder: null });
  current?.dispose();
  const api = new MockApi({ latencyMs: 0, skin: false, ...options });
  current = api;
  render(<App api={api} scriptStartMs={0} />, root);
  await until(() => store.get().ready && !!q(".recorder"));
  return api;
}

describe("Recorder: the music player link", () => {
  it("offers the open player, both new buttons, and the honest notes", async () => {
    await open();
    await until(() => !!q<HTMLSelectElement>(".recorder__player select"));
    const sel = q<HTMLSelectElement>(".recorder__player select")!;
    expect(sel.options[0].textContent).toBe("MusicPlayer: The Demo Band – Morning Light (paused)");
    expect(button("Record & play")!.hasAttribute("disabled")).toBe(false);
    expect(button("Record songs separately")!.hasAttribute("disabled")).toBe(false);
    expect(button("Start recording") ?? qa(".recorder__button")[0]).toBeTruthy();
    const notes = q(".recorder__player-notes")!.textContent!;
    expect(notes).toMatch(/player's volume and the Windows volume are in it/);
    expect(notes).toMatch(/switch off the player's crossfade and automix/);
    expect(notes).toMatch(/Windows only/);
    expect(notes).toMatch(/terms do not allow recording/);
    // The idle time and the song limit are offered before starting.
    expect(text()).toMatch(/stop by itself when the player is paused for/);
    expect(text()).toMatch(/Stop after this many songs/);
  });

  it("several players: the one playing is suggested; none open: explained, buttons off", async () => {
    const api = await open({ players: 3 });
    await until(() => (q<HTMLSelectElement>(".recorder__player select")?.options.length ?? 0) === 3);
    expect(q<HTMLSelectElement>(".recorder__player select")!.value).toBe("MusicPlayer.exe");
    expect(q<HTMLSelectElement>(".recorder__player select")!.options[2].textContent).toBe("Radio (stopped, nothing loaded)");
    void api;
    await open({ players: 0 });
    await until(() => /No music player is open/.test(text()));
    expect(button("Record & play")!.hasAttribute("disabled")).toBe(true);
    expect(button("Record songs separately")!.hasAttribute("disabled")).toBe(true);
    expect(button("Record")).toBeTruthy(); // plain recording still works
  });

  it("on the Mac only the plain recorder is offered, with the reason", async () => {
    await open({ platform: "macos", recorder: "granted" });
    await until(() => /work on Windows only for now/.test(text()));
    expect(button("Record & play")).toBeUndefined();
    expect(button("Record songs separately")).toBeUndefined();
    expect(q(".rec-sessions")).toBeNull();
  });
});

describe("Recorder: Record & play", () => {
  it("starts the recording and the player; Stop also pauses the player", async () => {
    const api = await open();
    await until(() => !!button("Record & play") && !button("Record & play")!.hasAttribute("disabled"));
    button("Record & play")!.click();
    await until(() => store.get().recorder?.mode === "record_and_play");
    await until(() => /MusicPlayer<\/strong> was told to play|MusicPlayer was told to play/.test(q(".recorder__live")?.innerHTML ?? "") || /MusicPlayer was told to play/.test(text()));
    expect((await api.recorderMediaSessions()).players[0].status).toBe("playing");
    // While recording, the mode buttons are gone; the big button stops.
    expect(button("Record & play")).toBeUndefined();
    expect(q<HTMLInputElement>(".recorder__modes input[type=checkbox]")!.checked).toBe(true);
    await stopEnabled();
    q<HTMLButtonElement>(".recorder__button")!.click();
    await until(() => store.get().recorder?.state === "idle");
    await until(() => store.get().toasts.some((t) => /^Saved /.test(t.text)));
    expect((await api.recorderMediaSessions()).players[0].status).toBe("paused");
  });
});

describe("Recorder: songs recorded separately", () => {
  it("shows the live song, stops, splits, and hands complete songs to the Studio", async () => {
    const api = await open({ empty: true });
    const sent: StudioSubmitRequest[] = [];
    const orig = api.studioSubmit.bind(api);
    api.studioSubmit = (r) => {
      sent.push(JSON.parse(JSON.stringify(r)));
      return orig(r);
    };
    await until(() => /None yet/.test(text()));
    // Song limit 2 and 20 s idle, remembered for next time.
    const [idle, max] = qa(".recorder__options input") as HTMLInputElement[];
    idle.value = "20";
    idle.dispatchEvent(new Event("input", { bubbles: true }));
    max.value = "2";
    max.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => !!button("Record songs separately") && !button("Record songs separately")!.hasAttribute("disabled"));
    button("Record songs separately")!.click();
    await until(() => store.get().recorder?.mode === "session");
    await until(() => /Recording songs separately from MusicPlayer: song 1 of 2 — The Demo Band – Morning Light/.test(q(".recorder__live")?.textContent ?? ""));
    await until(() => qa(".rec-session").length === 1 && /Recording…/.test(q(".rec-session")!.textContent!));
    // The plain stop is refused by the core while a session runs; the screen's Stop
    // uses the session's stop.
    await stopEnabled();
    q<HTMLButtonElement>(".recorder__button")!.click();
    await until(() => store.get().recorder?.state === "idle");
    await until(() => /Split into songs/.test(q(".rec-session")?.textContent ?? ""));
    const songs = qa(".rec-song");
    expect(songs.length).toBe(1);
    expect(songs[0].textContent).toMatch(/01 Morning Light/);
    expect(songs[0].textContent).toMatch(/Partial/);
    expect(songs[0].textContent).toMatch(/The recording stopped during the song/);
    // Nothing complete yet, so no "all complete songs" button.
    expect(button(/Make karaoke songs from all complete songs/)).toBeUndefined();
    button("Make a karaoke song")!.click();
    // The lyrics choice first: automatic is the default.
    await until(() => !!button("Add to the Studio queue"));
    button("Add to the Studio queue")!.click();
    await until(() => sent.length === 1);
    expect(sent[0].metadata).toEqual({
      title: "Morning Light",
      artist: "The Demo Band",
      album: "Demo Album",
      cover_path: expect.stringMatching(/01 The Demo Band - Morning Light\.jpg$/),
    });
    expect(sent[0].input).toMatch(/01 The Demo Band - Morning Light\.wav$/);
    expect(sent[0].lyrics.mode).toBe("auto");
    expect(localStorage.getItem("jk.recorder.maxSongs")).toBe("2");
  });

  it("lists a finished session: complete and partial songs, unsure cuts, excluded parts, preview", async () => {
    const api = await open();
    const sent: StudioSubmitRequest[] = [];
    const orig = api.studioSubmit.bind(api);
    api.studioSubmit = (r) => {
      sent.push(JSON.parse(JSON.stringify(r)));
      return orig(r);
    };
    await until(() => qa(".rec-song").length === 3);
    const s = q(".rec-session")!;
    expect(s.textContent).toMatch(/2026-11-17 20-15-03/);
    expect(s.textContent).toMatch(/from MusicPlayer · Split into songs · 3 songs · stopped by itself after the player was paused/);
    const badges = qa(".rec-song__badge").map((b) => b.textContent);
    expect(badges).toEqual(["Complete", "Complete", "Partial"]);
    expect(qa(".rec-song")[1].textContent).toMatch(/Check the end: there was no silence/);
    expect(qa(".rec-song")[2].textContent).toMatch(/skipped or interrupted/);
    await until(() => qa("img.rec-song__cover").length === 3);
    expect(q(".rec-session__excluded summary")!.textContent).toMatch(/2 parts not used as songs/);
    // Preview: plays natively, the button shows it, and stops again.
    const play = qa(".rec-song__preview")[0];
    play.click();
    await until(() => play.getAttribute("aria-pressed") === "true");
    expect(play.textContent).toMatch(/Stop/);
    play.click();
    await until(() => play.getAttribute("aria-pressed") === "false");
    // All complete songs to the Studio at once: two jobs, with their own metadata.
    button("Make karaoke songs from all complete songs (2)")!.click();
    // Each song gets its own lyrics choice: the second one's are pasted.
    await until(() => root.querySelectorAll(".lyrics-choice").length === 2);
    const second = root.querySelectorAll<HTMLElement>(".lyrics-choice")[1];
    second.querySelectorAll<HTMLInputElement>("input[type=radio]")[1].click();
    await until(() => !!second.querySelector("textarea"));
    const ta = second.querySelector<HTMLTextAreaElement>("textarea")!;
    ta.value = "first line" + String.fromCharCode(10) + "second line";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(); // let the dialog re-render with the text
    button("Add to the Studio queue")!.click();
    await until(() => sent.length === 2);
    expect(sent.map((r) => r.metadata.title)).toEqual(["Morning Light", "Paper Boats"]);
    expect(sent.map((r) => r.lyrics.mode)).toEqual(["auto", "text"]);
    expect(sent[1].lyrics.text).toBe("first line" + String.fromCharCode(10) + "second line");
    expect(sent.every((r) => typeof r.metadata.cover_path === "string")).toBe(true);
    await until(() => store.get().toasts.some((t) => t.text === "Added 2 songs to the Studio queue."));
    await until(() => (store.get().studio?.queue.length ?? 0) >= 2);
    // In the queue, the waiting song shows its lyrics and can switch back to automatic.
    store.set({ screen: "studio" });
    await until(() => /Lyrics: your own/.test(text()));
    expect(qa(".studio__job-lyrics-edit").map((b) => b.textContent)).toContain("Change lyrics");
    const edit = qa(".studio__job-lyrics-edit").find((b) => b.textContent === "Change lyrics")!;
    edit.click();
    await until(() => !!button("Find them automatically instead"));
    button("Find them automatically instead")!.click();
    await until(() => !/Lyrics: your own/.test(text()));
  });

  it("a song already made into a karaoke song folds away, says Made and offers Sing (D-281)", async () => {
    localStorage.removeItem("jk.recorder.hidden");
    localStorage.removeItem("jk.recorder.showFinished");
    await open({ madeFromRecording: true });
    // "Morning Light" was made into a karaoke song: only the other two are listed.
    await until(() => /1 song made into karaoke songs or hidden/.test(text()));
    expect(qa(".rec-song").map((r) => r.querySelector(".rec-song__title")!.textContent)).toEqual(["02 Paper Boats", "03 Slow River"]);
    expect(q(".rec-session__done")!.textContent).toBe("1 made into karaoke song");
    // "All complete songs" means the ones still to make.
    expect(button("Make karaoke songs from all complete songs (1)")).toBeTruthy();
    button("Show finished songs (1)")!.click();
    await until(() => qa(".rec-song").length === 3);
    expect(qa(".rec-song__badge").map((b) => b.textContent)).toEqual(["Made", "Complete", "Partial"]);
    const made = qa(".rec-song")[0];
    expect(made.querySelector(".rec-song__hide")).toBeNull();
    expect(Array.from(made.querySelectorAll("button")).map((b) => b.textContent)).toEqual(["▶ Play", "Sing"]);
    expect(localStorage.getItem("jk.recorder.showFinished")).toBe("1");
    made.querySelector<HTMLButtonElement>(".rec-song__sing")!.click();
    await until(() => store.get().screen === "stage" && store.get().song?.summary.song_id === "The Demo Band - Morning Light.jksong");
    localStorage.removeItem("jk.recorder.showFinished");
  });

  it("Hide folds a song away (remembered); a session with nothing left moves to Finished sessions (D-281)", async () => {
    localStorage.removeItem("jk.recorder.hidden");
    localStorage.removeItem("jk.recorder.showFinished");
    try {
      await open();
      await until(() => qa(".rec-song").length === 3);
      const hideRow = (title: RegExp) => qa(".rec-song").find((r) => title.test(r.textContent ?? ""))!.querySelector<HTMLButtonElement>(".rec-song__hide")!.click();
      hideRow(/Paper Boats/);
      await until(() => qa(".rec-song").length === 2);
      expect(JSON.parse(localStorage.getItem("jk.recorder.hidden")!)).toEqual(["2026-11-17 20-15-03/02 The Demo Band - Paper Boats.wav"]);
      expect(q(".rec-session__done")!.textContent).toBe("1 hidden");
      expect(button("Make karaoke songs from all complete songs (1)")).toBeTruthy();
      hideRow(/Morning Light/);
      hideRow(/Slow River/);
      // Nothing left to do in the session: it waits, folded, under Finished sessions.
      await until(() => !!q(".rec-sessions__finished"));
      expect(q(".rec-sessions__finished summary")!.textContent).toBe("Finished sessions (1): every song made into a karaoke song or hidden");
      expect(q(".rec-sessions__finished")!.hasAttribute("open")).toBe(false);
      expect(q(".rec-sessions > ul.rec-sessions__list")!.children.length).toBe(0);
      // Remembered: the screen opened again still has them hidden.
      await open();
      await until(() => !!q(".rec-sessions__finished") && /3 songs made into karaoke songs or hidden/.test(text()));
      button("Show finished songs (3)")!.click();
      await until(() => qa(".rec-song__hide").length === 3);
      expect(qa(".rec-song__hide").map((b) => b.textContent)).toEqual(["Unhide", "Unhide", "Unhide"]);
      qa(".rec-song__hide")[1].click();
      await until(() => JSON.parse(localStorage.getItem("jk.recorder.hidden")!).length === 2);
      button("Hide finished songs")!.click();
      await until(() => qa(".rec-song").length === 1 && /Paper Boats/.test(q(".rec-song")!.textContent!));
      expect(q(".rec-sessions__finished")).toBeNull();
    } finally {
      localStorage.removeItem("jk.recorder.hidden");
      localStorage.removeItem("jk.recorder.showFinished");
    }
  });

  it("a song waiting in the Studio says so and cannot be sent twice (D-281)", async () => {
    localStorage.removeItem("jk.recorder.hidden");
    await open();
    await until(() => qa(".rec-song").length === 3);
    const row = () => qa(".rec-song").find((r) => /Paper Boats/.test(r.textContent ?? ""))!;
    Array.from(row().querySelectorAll<HTMLButtonElement>("button")).find((b) => b.textContent === "Make a karaoke song")!.click();
    await until(() => !!button("Add to the Studio queue"));
    button("Add to the Studio queue")!.click();
    await until(() => row().querySelector(".rec-song__badge")?.textContent === "In the Studio");
    expect(Array.from(row().querySelectorAll("button")).map((b) => b.textContent)).toEqual(["▶ Play", "Hide"]);
    expect(button("Make karaoke songs from all complete songs (1)")).toBeTruthy();
  });

  it("without the Studio there is nothing to hand off, but the songs are listed", async () => {
    await open({ studio: "off" });
    await until(() => qa(".rec-song").length === 3);
    expect(button(/Make karaoke songs from all complete songs/)).toBeUndefined();
    expect(button("Make a karaoke song")).toBeUndefined();
    expect(qa(".rec-song .rec-song__preview").length).toBe(3);
    // The excluded parts can be listened to as well.
    expect(qa(".rec-session__part .rec-song__preview").length).toBe(2);
  });

  it("Delete asks first (Cancel keeps it), then removes the session from the list", async () => {
    await open();
    await until(() => qa(".rec-session").length === 1);
    button("Delete")!.click();
    await until(() => !!q(".confirm__cancel"));
    expect(text()).toMatch(/cannot be undone/);
    expect(text()).toMatch(/stay in your Library/);
    q<HTMLButtonElement>(".confirm__cancel")!.click();
    await until(() => !q(".confirm__cancel"));
    expect(qa(".rec-session").length).toBe(1);
    button("Delete")!.click();
    await until(() => !!button("Delete for good"));
    button("Delete for good")!.click();
    await until(() => qa(".rec-session").length === 0);
  });
});
