// SPDX-License-Identifier: GPL-3.0-or-later
// Small pure helpers for the placeholder page. Kept free of DOM and Tauri imports
// so they can be unit-tested in Node (src/format.test.ts).
import type { DataMode } from "./api";

/** Plain-language label for where the app keeps its data. */
export function modeLabel(mode: DataMode): string {
  switch (mode) {
    case "portable":
      return "Portable (everything stays in this folder)";
    case "dev":
      return "Developer build (data in the dev folder)";
    case "user":
      return "Installed (data in your user profile)";
  }
}

/** Milliseconds with one decimal, e.g. "12.3 ms"; "n/a" for missing values. */
export function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "n/a";
  return `${ms.toFixed(1)} ms`;
}
