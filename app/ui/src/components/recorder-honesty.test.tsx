// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// WORK_ORDER 4.6 "be honest in the UI ... about the ceiling": on Windows the panel says
// the capture comes after Windows' own processing ("Audio enhancements",
// docs/benchmarks/recorder.md); on the Mac that the format shown is ScreenCaptureKit's.
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi } from "../api/mock";
import { store } from "../state/app";

const root = document.createElement("div");
document.body.appendChild(root);
afterAll(() => render(null, root));

async function until(cond: () => boolean, ms = 3000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await new Promise((r) => setTimeout(r, 10));
  }
}

describe("recorder panel: honest about what is captured", () => {
  for (const [platform, text] of [
    ["windows", /after Windows' own processing.*Audio enhancements.*turn them off for the output device/],
    ["macos", /format shown is the one .*ScreenCaptureKit.* delivers/],
  ] as const) {
    it(platform, async () => {
      render(null, root);
      store.set({ ready: false, info: null, screen: "recorder", song: null, songSettings: null });
      render(<App api={new MockApi({ latencyMs: 0, platform, skin: false })} scriptStartMs={0} />, root);
      await until(() => !!root.querySelector(".recorder__honesty"));
      expect(root.querySelector(".recorder__honesty")!.textContent).toMatch(text);
    });
  }
});
