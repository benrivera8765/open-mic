// SPDX-License-Identifier: GPL-3.0-or-later
// Default app settings as the UI understands them (the core owns the saved copy;
// app/src-tauri/src/settings.rs has the same values). Used by "reset" buttons and
// by the browser mock. Master volume 1.0: unity, no hidden attenuation (WORK_ORDER 5).
import type { AppSettings } from "./types";

export const DEFAULT_APP_SETTINGS: AppSettings = {
  ui: {
    skin: true,
    bubbles: true,
    motion: "system",
    lyrics_text_size: 56,
    marker_position: 0.33,
    lyrics_motion: "word",
    colors: { sung: "#ff3d8b", upcoming: "#f6efd9", marker: "#7dffd8" },
    library_sort: "title",
    fullscreen_stage: false,
  },
  audio: { device_id: null, exclusive: false, master_volume: 1.0, device_offsets_ms: {}, key_mode: "auto", key_cache_mb: 1024, key_benchmarks: {} },
};
