// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// Regression test for the jukebox console (components/Jukebox.tsx): the key dial
// commits a turn 160 ms after the hand stops (skin/KeyDial.tsx). If another song is
// opened inside that window, the late commit must not change the NEW song's key
// (and so must not be saved into the new song's file).
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import { App } from "./App";
import { MockApi } from "./api/mock";
import { loadSong, store } from "./state/app";

const root = document.createElement("div");
document.body.appendChild(root);
const api = new MockApi({ latencyMs: 0 });
render(<App api={api} scriptStartMs={0} />, root);
afterAll(() => render(null, root));

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 2000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}

describe("jukebox console: key dial and song changes", () => {
  it("a dial turn still waiting to commit does not land on the next song", async () => {
    await until(() => root.querySelectorAll(".jk-strip").length > 0);
    const songs = store.get().library!.songs.filter((s) => !s.error);
    const first = songs.find((s) => s.semitones === 2)!; // the mock's Twinkle, saved at +2
    const second = songs.find((s) => s.song_id !== first.song_id && s.semitones === 0)!;
    expect(first && second).toBeTruthy();

    await loadSong(first.song_id);
    await until(() => store.get().songSettings?.semitones === 2);
    await wait();
    const dial = root.querySelector<HTMLElement>(".jk-dial")!;
    // One step up: the pointer moves at once, the commit waits 160 ms.
    dial.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
    await wait(10);
    expect(dial.getAttribute("aria-valuenow")).toBe("3");
    // Another song is opened before the dial commits.
    await loadSong(second.song_id);
    expect(store.get().song?.summary.song_id).toBe(second.song_id);
    await wait(400); // well past the dial's commit delay
    expect(store.get().songSettings?.semitones).toBe(0);
    const lib = await api.libraryList();
    expect(lib.songs.find((s) => s.song_id === second.song_id)?.semitones).toBe(0);
    expect(root.querySelector(".jk-dial")?.getAttribute("aria-valuenow")).toBe("0");
  });
});
