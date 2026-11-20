// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// The owner's request of 18 Nov 2026: "add the ability delete songs from the library"
// (DECISIONS D-230). On the mock, in the jukebox look and the plain look: every card's
// delete control, the Delete key on the focused or current card, Select mode, the
// confirmation (Cancel and Escape delete nothing), deleting the playing song, and the
// Deleted songs view (restore, delete for good, empty, each "for good" asking again).
import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi, type MockOptions } from "../api/mock";
import { loadSong, play, store } from "../state/app";

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
const text = () => root.textContent ?? "";
const strip = (title: RegExp) => qa(".library__strip").find((e) => title.test(e.textContent ?? ""));
const row = (title: RegExp) => qa(".title-strip__row").find((e) => title.test(e.textContent ?? ""));
const deletedRow = (title: RegExp) => qa(".deleted-song").find((e) => title.test(e.textContent ?? ""));
const press = (k: string, target: EventTarget = window, extra: KeyboardEventInit = {}) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...extra }));
const dialog = () => q<HTMLDialogElement>("dialog.confirm");
const toasts = () => store.get().toasts.map((t) => t.text).join("\n");
const libraryIds = async (api: MockApi) => (await api.libraryList()).songs.map((s) => s.song_id);

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

async function open(options: Partial<MockOptions> = {}): Promise<MockApi> {
  render(null, root);
  store.set({
    ready: false,
    info: null,
    settings: null,
    library: null,
    deleted: null,
    screen: "library",
    song: null,
    songSettings: null,
    player: null,
    modal: false,
    toasts: [],
    fullscreen: false,
    overlay: null,
  });
  current?.dispose();
  const api = new MockApi({ latencyMs: 0, ...options });
  current = api;
  render(<App api={api} scriptStartMs={0} />, root);
  await until(() => store.get().ready && (qa(".library__strip").length > 0 || qa(".title-strip").length > 0));
  await wait();
  return api;
}

describe("deleting songs, jukebox look (skin on)", () => {
  it("every card has a trash plate; it asks first, and Cancel or Escape deletes nothing", async () => {
    const api = await open();
    expect(qa(".library__strip")).toHaveLength(6);
    // One trash plate per card, named after its song (damaged files can be deleted too).
    const plates = qa(".library__strip .library__trash");
    expect(plates).toHaveLength(6);
    expect(strip(/Frère Jacques/)!.querySelector(".library__trash")!.getAttribute("aria-label")).toBe("Delete Traditional - Frère Jacques");
    expect(strip(/interrupted copy/)!.querySelector<HTMLButtonElement>(".library__trash")!.disabled).toBe(false);
    expect(plates.every((p) => p.classList.contains("jk-plate") && !!p.querySelector("svg.jk-icon"))).toBe(true);

    // Cancel.
    strip(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__trash")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete “Traditional - Frère Jacques”?");
    expect(q(".confirm__body")!.textContent).toMatch(/^It moves to Deleted songs, where you can restore it or delete it for good\./);
    // Focus moves to Cancel in an effect after the dialog renders (later on a busy PC).
    await until(() => !!document.activeElement?.classList.contains("confirm__cancel"));
    expect(document.activeElement?.classList.contains("confirm__cancel")).toBe(true);
    expect(q(".confirm__ok")!.classList.contains("jk-plate--danger")).toBe(true);
    expect(store.get().modal).toBe(true);
    expect(accessibilityProblems()).toEqual([]);
    q<HTMLButtonElement>(".confirm__cancel")!.click();
    await until(() => !dialog());
    expect(store.get().modal).toBe(false);
    expect(await libraryIds(api)).toContain("Traditional - Frère Jacques.jksong");

    // Escape.
    strip(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__trash")!.click();
    await until(() => !!dialog());
    press("Escape", document.activeElement!);
    await until(() => !dialog());
    await wait();
    expect(await libraryIds(api)).toContain("Traditional - Frère Jacques.jksong");
    expect(qa(".library__strip")).toHaveLength(6);
    expect(toasts()).not.toMatch(/Deleted songs/);

    // Delete.
    strip(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__trash")!.click();
    await until(() => !!dialog());
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".library__strip").length === 5);
    expect(strip(/Frère Jacques/)).toBeUndefined();
    expect(await libraryIds(api)).not.toContain("Traditional - Frère Jacques.jksong");
    expect(toasts()).toMatch(/“Traditional - Frère Jacques” moved to Deleted songs\./);
    expect(q(".library__show-deleted")!.textContent).toBe("Deleted (1)");
    // The free-space line counts what Deleted songs still takes on the drive.
    expect(q(".library__space")!.textContent).toMatch(/18\.7 GB free of 30\.9 GB · 1 deleted \(61\.2 MB\)/);
  });

  it("deleting the song that is playing stops it first and leaves nothing loaded", async () => {
    const api = await open();
    const twinkle = "Jane Taylor - Twinkle, Twinkle, Little Star.jksong";
    await loadSong(twinkle);
    await play();
    await until(() => store.get().player?.state === "playing");
    store.set({ screen: "library" });
    await until(() => !!strip(/Twinkle, Twinkle/)?.classList.contains("library__strip--current"));
    // The current card shows its plate; the Delete key asks about it.
    expect(strip(/Twinkle, Twinkle/)!.querySelector(".library__trash")).not.toBeNull();
    (document.activeElement as HTMLElement | null)?.blur();
    press("Delete");
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete “Jane Taylor (1806) - Twinkle, Twinkle, Little Star”?");
    expect(q(".confirm__note")!.textContent).toBe("It is playing now; it will stop.");
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && !strip(/Twinkle, Twinkle, Little Star/));
    expect(store.get().song).toBeNull();
    expect(store.get().songSettings).toBeNull();
    expect(store.get().screen).toBe("library");
    await until(() => store.get().player?.state === "empty");
    expect((await api.playerState()).song_id).toBeNull();
    expect(await libraryIds(api)).not.toContain(twinkle);
  });

  it("the Delete key asks about the focused card; never while typing in the search", async () => {
    const api = await open();
    const search = q<HTMLInputElement>("#library-search")!;
    search.focus();
    press("Delete", search);
    await wait();
    expect(dialog()).toBeNull();
    search.blur();
    // Nothing focused and nothing loaded: nothing to ask about.
    press("Delete");
    await wait();
    expect(dialog()).toBeNull();
    const card = strip(/Row, Row, Row/)!.querySelector<HTMLButtonElement>(".jk-strip")!;
    card.focus();
    press("Delete", card);
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete “Traditional (1852) - Row, Row, Row Your Boat”?");
    press("Escape", document.activeElement!);
    await until(() => !dialog());
    expect(await libraryIds(api)).toContain("Traditional - Row, Row, Row Your Boat.jksong");
    // The app's own shortcuts stay quiet while the dialog is open.
    card.focus();
    press("Delete", card);
    await until(() => !!dialog());
    press("]");
    press(" ");
    await wait();
    expect(store.get().player?.state ?? "empty").not.toBe("playing");
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && !strip(/Row, Row, Row/));
  });

  it("Select: tick several cards and delete them together; Escape leaves Select", async () => {
    const api = await open();
    q<HTMLButtonElement>(".library__select")!.click();
    await until(() => !!q(".library__select-bar"));
    // The strips are checkboxes now, with a selector lamp; no trash plates, no loading.
    const boxes = qa(".jk-strip");
    expect(boxes.every((b) => b.getAttribute("role") === "checkbox" && b.getAttribute("aria-checked") === "false")).toBe(true);
    expect(qa(".jk-strip-lamp")).toHaveLength(6);
    expect(qa(".library__trash")).toHaveLength(0);
    expect(q(".library__select-count")!.textContent).toBe("Tick the songs to delete");
    expect(q<HTMLButtonElement>(".library__delete-ticked")!.disabled).toBe(true);
    // Damaged files can be ticked too.
    expect(strip(/interrupted copy/)!.querySelector<HTMLButtonElement>(".jk-strip")!.disabled).toBe(false);
    strip(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".jk-strip")!.click();
    strip(/Mary Had/)!.querySelector<HTMLButtonElement>(".jk-strip")!.click();
    await wait();
    expect(store.get().song).toBeNull();
    expect(strip(/Frère Jacques/)!.querySelector(".jk-strip")!.getAttribute("aria-checked")).toBe("true");
    expect(strip(/Frère Jacques/)!.classList.contains("library__strip--ticked")).toBe(true);
    expect(qa(".jk-strip--checked")).toHaveLength(2);
    expect(q(".library__select-count")!.textContent).toBe("2 selected");
    expect(accessibilityProblems()).toEqual([]);
    // Untick one, tick it again.
    strip(/Mary Had/)!.querySelector<HTMLButtonElement>(".jk-strip")!.click();
    await wait();
    expect(q(".library__select-count")!.textContent).toBe("1 selected");
    strip(/Mary Had/)!.querySelector<HTMLButtonElement>(".jk-strip")!.click();
    await wait();
    q<HTMLButtonElement>(".library__delete-ticked")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete 2 songs?");
    expect(q(".confirm__body")!.textContent).toMatch(/They move to Deleted songs, where you can restore them or delete them for good\./);
    expect(qa(".confirm__list li").map((li) => li.textContent)).toEqual(["Traditional - Frère Jacques", "Sarah Josepha Hale (1830) - Mary Had a Little Lamb"]);
    expect(q(".confirm__ok")!.textContent).toBe("Delete 2 songs");
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".library__strip").length === 4);
    expect(q(".library__select-bar")).toBeNull();
    expect(toasts()).toMatch(/2 songs moved to Deleted songs\./);
    const ids = await libraryIds(api);
    expect(ids).not.toContain("Traditional - Frère Jacques.jksong");
    expect(ids).not.toContain("Sarah Josepha Hale - Mary Had a Little Lamb.jksong");

    // All / None, the Delete key on the ticked ones, then Escape: Cancel and leave Select.
    q<HTMLButtonElement>(".library__select")!.click();
    await until(() => !!q(".library__select-bar"));
    q<HTMLButtonElement>(".library__select-all")!.click();
    await wait();
    expect(q(".library__select-count")!.textContent).toBe("4 selected");
    press("Delete");
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete 4 songs?");
    press("Escape", document.activeElement!);
    await until(() => !dialog());
    expect(q(".library__select-count")!.textContent).toBe("4 selected");
    q<HTMLButtonElement>(".library__select-all")!.click();
    await wait();
    expect(q(".library__select-count")!.textContent).toBe("Tick the songs to delete");
    press("Escape");
    await until(() => !q(".library__select-bar"));
    expect(qa(".jk-strip").every((b) => b.getAttribute("role") !== "checkbox")).toBe(true);
    expect(qa(".library__strip")).toHaveLength(4);
  });

  it("Deleted songs: restore, delete for good and empty, each for good asking again", async () => {
    const api = await open();
    await api.libraryDelete([
      "Traditional - Frère Jacques.jksong",
      "Sarah Josepha Hale - Mary Had a Little Lamb.jksong",
      "Traditional - Row, Row, Row Your Boat.jksong",
    ]);
    await until(() => q(".library__show-deleted")?.textContent === "Deleted (3)");
    q<HTMLButtonElement>(".library__show-deleted")!.click();
    await until(() => qa(".deleted-song").length === 3);
    expect(q(".deleted__title")!.textContent).toBe("Deleted songs");
    expect(q(".deleted__summary")!.textContent).toBe("3 songs · 188 MB still on the drive");
    const frere = deletedRow(/Frère Jacques/)!;
    expect(frere.querySelector(".deleted-song__artist")!.textContent).toBe("Traditional");
    expect(frere.querySelector(".deleted-song__size")!.textContent).toBe("61.2 MB");
    expect(frere.querySelector(".deleted-song__date")!.textContent).toMatch(/^Deleted \S/);
    expect(frere.querySelector(".deleted-song__from")!.textContent).toBe("From Traditional - Frère Jacques.jksong");
    expect(accessibilityProblems()).toEqual([]);
    // The search filters this list too.
    const search = q<HTMLInputElement>("#library-search")!;
    search.value = "lamb";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => qa(".deleted-song").length === 1);
    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => qa(".deleted-song").length === 3);

    // Restore: straight back (restoring cannot lose anything, so it does not ask).
    frere.querySelector<HTMLButtonElement>(".deleted-song__restore")!.click();
    await until(() => qa(".deleted-song").length === 2);
    expect(dialog()).toBeNull();
    expect(toasts()).toMatch(/“Traditional - Frère Jacques” is back in the library\./);
    expect(await libraryIds(api)).toContain("Traditional - Frère Jacques.jksong");

    // Delete for good: asks, saying it cannot be undone; Cancel keeps it.
    const freeBefore = (await api.libraryList()).free_bytes;
    deletedRow(/Mary Had/)!.querySelector<HTMLButtonElement>(".deleted-song__purge")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete “Sarah Josepha Hale (1830) - Mary Had a Little Lamb” for good?");
    expect(q(".confirm__body")!.textContent).toBe("This cannot be undone. The song file is removed from the drive, freeing 52.0 MB.");
    expect(q(".confirm__ok")!.textContent).toBe("Delete for good");
    press("Escape", document.activeElement!);
    await until(() => !dialog());
    expect(qa(".deleted-song")).toHaveLength(2);
    deletedRow(/Mary Had/)!.querySelector<HTMLButtonElement>(".deleted-song__purge")!.click();
    await until(() => !!dialog());
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".deleted-song").length === 1);
    expect(toasts()).toMatch(/“Sarah Josepha Hale \(1830\) - Mary Had a Little Lamb” deleted for good \(52\.0 MB freed\)\./);
    // The free space counts it.
    expect((await api.libraryList()).free_bytes).toBe(freeBefore + 52_000_000);
    await until(() => /18\.8 GB free/.test(q(".library__space")?.textContent ?? ""));

    // Empty Deleted songs: asks again; Cancel keeps everything.
    q<HTMLButtonElement>(".deleted__empty-all")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Empty Deleted songs?");
    expect(q(".confirm__body")!.textContent).toBe("This cannot be undone. The song in Deleted songs (74.9 MB) is deleted for good.");
    q<HTMLButtonElement>(".confirm__cancel")!.click();
    await until(() => !dialog());
    expect(qa(".deleted-song")).toHaveLength(1);
    q<HTMLButtonElement>(".deleted__empty-all")!.click();
    await until(() => !!dialog());
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".deleted-song").length === 0);
    expect(text()).toMatch(/No deleted songs\./);
    expect(q(".deleted__summary")!.textContent).toBe("Empty");
    expect(q<HTMLButtonElement>(".deleted__empty-all")!.disabled).toBe(true);
    expect(q(".library__show-deleted")!.textContent).toBe("Deleted");
    // Back to the songs: the restored one is there, the others are gone.
    q<HTMLButtonElement>(".library__views .jk-plate")!.click();
    await until(() => qa(".library__strip").length === 4);
    expect(strip(/Frère Jacques/)).toBeDefined();
  });
});

describe("deleting songs, plain look (skin off)", () => {
  it("delete control, Select with checkboxes, and the Deleted songs view", async () => {
    const api = await open({ skin: false });
    await until(() => qa(".title-strip").length === 6);
    expect(qa(".title-strip__row .library__trash")).toHaveLength(6);
    expect(row(/Frère Jacques/)!.querySelector(".library__trash")!.getAttribute("aria-label")).toBe("Delete Traditional - Frère Jacques");
    row(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__trash")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__ok")!.classList.contains("button--danger")).toBe(true);
    expect(document.activeElement?.classList.contains("confirm__cancel")).toBe(true);
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".title-strip").length === 5);
    expect(await libraryIds(api)).not.toContain("Traditional - Frère Jacques.jksong");

    // Select: checkboxes.
    q<HTMLButtonElement>(".library__select")!.click();
    await until(() => qa(".title-strip--picking").length === 5);
    expect(qa(".title-strip--picking").every((b) => b.getAttribute("role") === "checkbox")).toBe(true);
    row(/Mary Had/)!.querySelector<HTMLButtonElement>(".title-strip")!.click();
    row(/interrupted copy/)!.querySelector<HTMLButtonElement>(".title-strip")!.click();
    await wait();
    expect(qa(".title-strip--ticked")).toHaveLength(2);
    expect(qa(".title-strip__check--on")).toHaveLength(2);
    expect(accessibilityProblems()).toEqual([]);
    q<HTMLButtonElement>(".library__delete-ticked")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__title")!.textContent).toBe("Delete 2 songs?");
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".title-strip").length === 3);

    // Deleted songs view.
    q<HTMLButtonElement>(".library__show-deleted")!.click();
    await until(() => qa(".deleted-song").length === 3);
    expect(q(".library__show-deleted")!.getAttribute("aria-pressed")).toBe("true");
    deletedRow(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".deleted-song__restore")!.click();
    await until(() => qa(".deleted-song").length === 2);
    deletedRow(/Mary Had/)!.querySelector<HTMLButtonElement>(".deleted-song__purge")!.click();
    await until(() => !!dialog());
    expect(q(".confirm__body")!.textContent).toMatch(/^This cannot be undone\./);
    q<HTMLButtonElement>(".confirm__ok")!.click();
    await until(() => !dialog() && qa(".deleted-song").length === 1);
    expect(accessibilityProblems()).toEqual([]);
    q<HTMLButtonElement>(".library__views .button")!.click();
    await until(() => qa(".title-strip").length === 4);
  });
});
