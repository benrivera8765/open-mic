// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// WORK_ORDER 4.1 step 1, "let the user correct ... cover art": the Studio form shows the
// cover read from the file and lets the user replace it with a JPEG / PNG picture or
// remove it; the choice reaches the Studio as `metadata.cover_path` (STUDIO_PROTOCOL 7):
// a path, null for "no cover", absent for "the file's own cover".
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi } from "../api/mock";
import type { StudioSubmitRequest } from "../api/types";
import { store } from "../state/app";

const root = document.createElement("div");
document.body.appendChild(root);
const api = new MockApi({ latencyMs: 0, skin: false });
render(<App api={api} scriptStartMs={0} />, root);
afterAll(() => render(null, root));

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 3000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}
const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));
const button = (text: string) => qa("button").find((e) => e.textContent?.trim() === text);

const sent: StudioSubmitRequest[] = [];
const origSubmit = api.studioSubmit.bind(api);
api.studioSubmit = (r) => {
  sent.push(JSON.parse(JSON.stringify(r)));
  return origSubmit(r);
};

async function openForm(): Promise<void> {
  store.set({ studioInbox: [] });
  await until(() => !!button("Choose files…"));
  button("Choose files…")!.click(); // the mock picks a FLAC that carries a cover
  await until(() => !!button("Add to queue"));
}

describe("Studio form: cover picture", () => {
  it("shows the file's cover and sends nothing when it is left alone", async () => {
    await until(() => store.get().ready && !!store.get().info);
    store.set({ screen: "studio" });
    await openForm();
    await until(() => !!q<HTMLImageElement>(".studio__cover-img"));
    expect(q<HTMLImageElement>(".studio__cover-img")!.src).toMatch(/^data:image\//);
    expect(q(".studio__cover")!.textContent).toMatch(/From the file/);
    button("Add to queue")!.click();
    await until(() => sent.length === 1);
    expect("cover_path" in sent[0].metadata).toBe(false);
  });

  it("replaces the cover with a picked JPEG / PNG picture", async () => {
    await openForm();
    await until(() => !!button("Replace cover…"));
    button("Replace cover…")!.click();
    await until(() => /Your picture: My cover\.jpg/.test(q(".studio__cover")?.textContent ?? ""));
    await until(() => !!q<HTMLImageElement>(".studio__cover-img"));
    expect(button("Use the file's cover")).toBeTruthy();
    button("Add to queue")!.click();
    await until(() => sent.length === 2);
    expect(sent[1].metadata.cover_path).toBe("C:\\Users\\you\\Pictures\\My cover.jpg");
  });

  it("removes the cover (null), and a picture that is not JPEG / PNG is refused", async () => {
    await openForm();
    await until(() => !!button("Remove cover"));
    button("Remove cover")!.click();
    await until(() => /No cover/.test(q(".studio__cover")?.textContent ?? ""));
    expect(q(".studio__cover-img")).toBeNull();
    expect(button("Remove cover")).toBeUndefined();
    // A GIF from the dialog: refused with a message, the choice stays "no cover".
    const origPick = api.pickCoverImage.bind(api);
    api.pickCoverImage = () => Promise.resolve("C:\\Users\\you\\Pictures\\anim.gif");
    try {
      button("Choose a cover picture…")!.click();
      await until(() => (store.get().toasts ?? []).some((t) => /JPEG or PNG/.test(t.text)));
      expect(q(".studio__cover")!.textContent).toMatch(/No cover/);
    } finally {
      api.pickCoverImage = origPick;
    }
    button("Add to queue")!.click();
    await until(() => sent.length === 3);
    expect(sent[2].metadata.cover_path).toBeNull();
  });
});
