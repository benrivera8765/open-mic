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
    lyrics_display: "page",
    show_next_line: true,
    // Gold sung, white upcoming since D-218 (before: #ff3d8b / #f6efd9, which the core
    // migrates to these when a settings file from before D-218 still holds them).
    colors: { sung: "#ffc21a", upcoming: "#f4f4f4", marker: "#7dffd8" },
    library_sort: "title",
    fullscreen_stage: false,
  },
  audio: { device_id: null, exclusive: false, master_volume: 1.0, device_offsets_ms: {}, key_mode: "auto", key_cache_mb: 1024, key_benchmarks: {} },
};
