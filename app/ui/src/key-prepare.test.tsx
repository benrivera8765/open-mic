// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// Render ahead and cache (WORK_ORDER 4.5): while a computer renders a new key ahead,
// the previous key keeps playing and the key control says "Preparing key +2… 40%".
// The text comes from `PlayerState.key_preparing` (APP_API section 3 additions).
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import type { PlayerState } from "./api/types";
import { KeyPrepareNote, keyPrepareText } from "./components/KeyControl";
import { store } from "./state/app";

describe("keyPrepareText", () => {
  it("names the key and the progress in whole percent", () => {
    expect(keyPrepareText({ semitones: 2, reason: "key", progress: 0.4 })).toBe("Preparing key +2… 40%");
    expect(keyPrepareText({ semitones: -5, reason: "key", progress: 0.999 })).toBe("Preparing key -5… 99%");
    expect(keyPrepareText({ semitones: 3, reason: "mix", progress: 0.1 })).toBe("Preparing the new mix (key +3)… 10%");
  });
  it("says nothing when nothing is prepared, and never shows nonsense numbers", () => {
    expect(keyPrepareText(null)).toBeNull();
    expect(keyPrepareText(undefined)).toBeNull();
    expect(keyPrepareText({ semitones: 1, reason: "key", progress: Number.NaN })).toBe("Preparing key +1… 0%");
    expect(keyPrepareText({ semitones: 1, reason: "key", progress: 7 })).toBe("Preparing key +1… 100%");
  });
});

describe("KeyPrepareNote", () => {
  const root = document.createElement("div");
  document.body.appendChild(root);
  afterAll(() => render(null, root));
  const player = (extra: Partial<PlayerState>): PlayerState => ({
    state: "playing",
    song_id: "a.jksong",
    position_s: 1,
    duration_s: 100,
    device: null,
    underruns: 0,
    error: null,
    key_label: "C major → D major (+2)",
    ...extra,
  });
  const tick = () => new Promise((r) => setTimeout(r, 0));

  it("follows playback://state: shown while preparing, gone once the key is heard", async () => {
    store.set({ player: player({ key_mode: "prerender", key_preparing: { semitones: 2, reason: "key", progress: 0.4 } }) });
    render(<KeyPrepareNote />, root);
    await tick();
    const note = root.querySelector(".key-prepare");
    expect(note?.textContent).toBe("Preparing key +2… 40%");
    expect(note?.getAttribute("role")).toBe("status");
    store.set({ player: player({ key_mode: "prerender", key_preparing: null }) });
    await tick();
    expect(root.querySelector(".key-prepare")).toBeNull();
    // An older core without the field: nothing shown.
    store.set({ player: player({}) });
    await tick();
    expect(root.querySelector(".key-prepare")).toBeNull();
  });
});
