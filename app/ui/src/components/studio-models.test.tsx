// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// The Studio form's models follow the quality preset chosen in it (STUDIO_PROTOCOL 4,
// APP_API 9 `studio_models_status(preset)` / `studio_models_download(preset)`): a user who
// picks "Clean" or "Permissive" sees which of that preset's models are missing, their size
// and licence, and downloads exactly those, only when pressing the button. The preset
// hints say what docs/benchmarks/separation.md measured.
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi } from "../api/mock";
import type { StudioPreset, StudioSubmitRequest } from "../api/types";
import { refreshStudio, store } from "../state/app";
import { licenceWords, PRESETS } from "./Studio";

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
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));
const button = (text: string | RegExp) => qa("button").find((e) => (typeof text === "string" ? e.textContent?.trim() === text : text.test(e.textContent ?? "")));
const notice = () => root.querySelector<HTMLElement>(".studio__models-notice");

const statusAsked: StudioPreset[] = [];
const downloadAsked: StudioPreset[] = [];
const origStatus = api.studioModelsStatus.bind(api);
api.studioModelsStatus = (p) => {
  statusAsked.push(p);
  return origStatus(p);
};
const origDownload = api.studioModelsDownload.bind(api);
api.studioModelsDownload = (p) => {
  downloadAsked.push(p);
  return origDownload(p);
};
const sent: StudioSubmitRequest[] = [];
const origSubmit = api.studioSubmit.bind(api);
api.studioSubmit = (r) => {
  sent.push(JSON.parse(JSON.stringify(r)));
  return origSubmit(r);
};

async function openForm(): Promise<void> {
  store.set({ studioInbox: [] });
  await until(() => !!button("Choose files…"));
  button("Choose files…")!.click();
  await until(() => !!button("Add to queue"));
}
function choose(preset: StudioPreset): void {
  const radio = qa("input[type=radio]").find((e) => (e as HTMLInputElement).value === preset) as HTMLInputElement;
  radio.click();
}

describe("licenceWords", () => {
  it("says plainly what the licence allows", () => {
    expect(licenceWords({ licence: "none published", commercial: "unclear" })).toBe("licence: none published; commercial use unclear");
    expect(licenceWords({ licence: "GPL-3.0", commercial: "yes" })).toBe("licence: GPL-3.0; commercial use allowed");
    expect(licenceWords({ licence: "CC BY-NC 4.0", commercial: "no" })).toBe("licence: CC BY-NC 4.0; non-commercial use only");
  });
});

describe("Studio form: models follow the chosen preset", () => {
  it("Full on a drive that has its models: no notice", async () => {
    await until(() => store.get().ready && !!store.get().info);
    store.set({ screen: "studio" });
    await openForm();
    await until(() => statusAsked.includes("full"));
    await wait(50);
    expect(notice()).toBeNull();
  });

  it("Clean: names the missing model, its size and 'licence: none published; commercial use unclear', downloads only on request", async () => {
    choose("clean");
    await until(() => !!notice());
    expect(statusAsked.at(-1)).toBe("clean");
    expect(notice()!.dataset.preset).toBe("clean");
    const text = notice()!.textContent ?? "";
    expect(text).toMatch(/One model for the Clean preset is not on this drive yet: 204 MB to download, once\./);
    expect(text).toMatch(/BS-RoFormer Resurrection Inst \(unwa\), 204 MB \(needed for every song\): licence: none published; commercial use unclear/);
    expect(text).toMatch(/Nothing is downloaded until you press the button/);
    expect(downloadAsked).toEqual([]); // nothing downloads by itself
    button("Download 204 MB")!.click();
    await until(() => downloadAsked.length === 1);
    expect(downloadAsked).toEqual(["clean"]);
    await until(() => !notice());
    // The song is made with the preset chosen in the form.
    button("Add to queue")!.click();
    await until(() => sent.length === 1);
    expect(sent[0].preset).toBe("clean");
    await until(() => (store.get().studio?.queue.length ?? 0) > 0);
    expect(store.get().studio!.queue.at(-1)!.state).not.toBe("failed");
  });

  it("Permissive: its own models (MIT), downloaded with its own preset", async () => {
    await openForm();
    choose("permissive");
    await until(() => !!notice());
    const text = notice()!.textContent ?? "";
    expect(text).toMatch(/One model for the Permissive licences only preset/);
    expect(text).toMatch(/Mel-Band RoFormer vocals \(Kimberley Jensen\), 913 MB \(needed for every song\): licence: MIT; commercial use allowed/);
    button("Download 913 MB")!.click();
    await until(() => !notice());
    expect(downloadAsked).toEqual(["clean", "permissive"]);
    choose("full");
    await wait(50);
    expect(notice()).toBeNull();
  });

  it("a drive without any model: the Studio screen offers the default set before a file is added", async () => {
    (api as unknown as { modelsOnDisk: Set<string> }).modelsOnDisk.clear();
    store.set({ studioInbox: [] });
    await refreshStudio();
    await until(() => !!notice());
    expect(notice()!.dataset.preset).toBe("full");
    const text = notice()!.textContent ?? "";
    expect(text).toMatch(/5 models for the Full preset are not on this drive yet/);
    expect(text).toMatch(/needed for songs without lyrics \(transcription\)/);
    // With a file in the form, its own notice replaces the screen-wide one (no duplicate).
    button("Choose files…")!.click();
    await until(() => !!button("Add to queue"));
    await until(() => qa(".studio__models-notice").length === 1);
    expect(qa(".studio__models-notice")[0].closest(".studio__item")).not.toBeNull();
  });
});

describe("preset hints match the measurements (docs/benchmarks/separation.md)", () => {
  const hint = (p: StudioPreset) => PRESETS.find((x) => x.value === p)!.hint;
  it("Full leaves less of the singer than Clean; Clean slightly more, brighter cymbals, wider; Permissive drops backing vocals", () => {
    expect(hint("full")).toMatch(/less of the singer than Clean/);
    expect(hint("full")).toMatch(/cymbal brightness can go with the voice/);
    // Not measured: Full against Permissive; and Full does lose a little in the highs.
    expect(hint("full")).not.toMatch(/least of the singer|exactly as recorded|nothing else is lost/);
    expect(hint("clean")).toMatch(/slightly more of the singer than Full/);
    expect(hint("clean")).toMatch(/cymbals a little brighter/);
    expect(hint("clean")).toMatch(/wider than the recording/);
    expect(hint("clean")).not.toMatch(/less voice/);
    expect(hint("permissive")).toMatch(/backing vocals and harmonies are missing/);
  });
  it("each names its licence in a few words", () => {
    expect(hint("full")).toMatch(/GPL-3\.0 and Apache-2\.0: commercial use allowed/);
    expect(hint("clean")).toMatch(/no published licence: commercial use unclear/);
    expect(hint("permissive")).toMatch(/MIT and Apache-2\.0.*Commercial use allowed/);
  });
});
