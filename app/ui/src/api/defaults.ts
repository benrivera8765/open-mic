// SPDX-License-Identifier: GPL-3.0-or-later
// Default app settings as the UI understands them (the core owns the saved copy;
// app/src-tauri/src/settings.rs has the same values). Used by "reset" buttons and
// by the browser mock. Master volume 1.0: unity, no hidden attenuation (WORK_ORDER 5).
import type { AppSettings, LookSettings, PracticeAppSettings, ThemeId } from "./types";

/** Practice mode's per-computer settings (docs/APP_API.md 12.7; owner decisions 1, 2, 4, 5). */
export const DEFAULT_PRACTICE_SETTINGS: PracticeAppSettings = {
  piano_side: "left",
  simple_lane: false,
  words_under_notes: true,
  lyrics_below: true,
  strictness: "standard",
  octave_mode: "auto",
  auto_latency: true,
  inputs: {},
  latency_offsets_ms: {},
  measured_latency_ms: {},
};

/** Jukebox customisation defaults: the owner's choices of 2026-11-22 (silver accents, lapis badge with gold lettering, silver lattice and inner trim, pearlescent cabinet). */
export const DEFAULT_LOOK: LookSettings = {
  cabinet: "pearl",
  frame: "neon",
  accents: "silver",
  badge_field: "lapis",
  badge_text: "gold",
  badge_rim: "lapis",
  lattice: "silver",
  inner_trim: true,
  watch_follow: true,
  gpu_fx: true,
  fx_fps: "standard",
  celebrate: true,
  celebration: "mix",
  celebrate_freq: "every",
  pitch_fx: "sparks",
  metal: "gold",
  glow: "pink",
  lane_bars: "pink",
  theme: "deluxe",
};

export const LOOK_CHOICES: { [K in keyof LookSettings]: readonly LookSettings[K][] } = {
  cabinet: ["pearl", "walnut", "maple", "aquarium", "forge", "ocean", "brass", "cyber"],
  frame: ["neon", "pilaster", "lava", "light", "ocean", "fire", "water", "steam", "data"],
  accents: ["silver", "red", "iron", "copper"],
  badge_field: ["lapis", "red", "silver", "magma", "copper", "carbon"],
  badge_text: ["gold", "cream", "ember", "neon", "white"],
  badge_rim: ["lapis", "gold"],
  lattice: ["silver", "gold", "iron"],
  inner_trim: [true, false],
  watch_follow: [true, false],
  gpu_fx: [true, false],
  fx_fps: ["standard", "60", "120", "240"],
  celebrate: [true, false],
  celebration: ["mix", "starburst", "fireworks", "rings", "bloom", "inferno"],
  celebrate_freq: ["often", "every", "second", "third", "rare"],
  pitch_fx: ["off", "sparks", "rings", "stars", "flames", "notes"],
  metal: ["gold", "bronze", "chrome", "brass"],
  glow: ["pink", "ember", "aqua", "violet", "amber"],
  lane_bars: ["pink", "ember", "aqua", "violet", "amber"],
  theme: ["deluxe", "classic", "ocean", "electric", "molten", "water", "steampunk", "cyberpunk", "custom"],
};

/** Note bar colours of the practice lane (ui.look.lane_bars). */
export const LANE_BAR_COLORS: Record<LookSettings["lane_bars"], string> = {
  pink: "#ff3d8b",
  ember: "#ff6a1a",
  aqua: "#29c8ff",
  violet: "#b066ff",
  amber: "#ffb020",
};

/** The look options a theme sets (everything visual; not watch-only follow or celebrate on/off). */
export type ThemeLook = Omit<LookSettings, "watch_follow" | "celebrate" | "celebrate_freq" | "theme" | "gpu_fx" | "fx_fps">;
export interface Theme {
  id: Exclude<ThemeId, "custom">;
  name: string;
  blurb: string;
  /** Swatch colours for the theme card. */
  swatch: [string, string, string];
  look: ThemeLook;
  /** Lyrics: sung, upcoming, marker. */
  colors: { sung: string; upcoming: string; marker: string };
}

/**
 * Themes (owner, 2026-11-23): a preset of every look option and the lyric colours. Each
 * option stays independent: apply a theme, then change anything; applying the same theme
 * again resets it to the preset.
 */
export const THEMES: readonly Theme[] = [
  {
    id: "deluxe",
    name: "Deluxe",
    blurb: "Pearlescent cabinet, neon bubble tubes, lapis badge, silver trim",
    swatch: ["#ffb3cf", "#ffc21a", "#2148b4"],
    look: { cabinet: "pearl", frame: "neon", accents: "silver", badge_field: "lapis", badge_text: "gold", badge_rim: "lapis", lattice: "silver", inner_trim: true, celebration: "mix", pitch_fx: "sparks", metal: "gold", glow: "pink", lane_bars: "pink" },
    colors: { sung: "#ffc21a", upcoming: "#f4f4f4", marker: "#7dffd8" },
  },
  {
    id: "classic",
    name: "Classic",
    blurb: "Walnut, neon bubble tubes, red enamel and gold, as the jukebox first looked",
    swatch: ["#5a3a22", "#c8102e", "#ecd38e"],
    look: { cabinet: "walnut", frame: "neon", accents: "red", badge_field: "red", badge_text: "cream", badge_rim: "gold", lattice: "gold", inner_trim: false, celebration: "mix", pitch_fx: "sparks", metal: "gold", glow: "pink", lane_bars: "pink" },
    colors: { sung: "#ffc21a", upcoming: "#f4f4f4", marker: "#7dffd8" },
  },
  {
    id: "ocean",
    name: "Ocean",
    blurb: "Aquarium cabinet, ocean tube with fish, aqua glow",
    swatch: ["#0a7ab0", "#7dffd8", "#ff7a1a"],
    look: { cabinet: "aquarium", frame: "ocean", accents: "silver", badge_field: "lapis", badge_text: "gold", badge_rim: "lapis", lattice: "silver", inner_trim: true, celebration: "bloom", pitch_fx: "notes", metal: "gold", glow: "aqua", lane_bars: "aqua" },
    colors: { sung: "#5fe6ff", upcoming: "#f4f4f4", marker: "#ffd23f" },
  },
  {
    id: "electric",
    name: "Electric",
    blurb: "Lightning tube, dark walnut, violet glow",
    swatch: ["#1858b8", "#a866ff", "#f4f4f4"],
    look: { cabinet: "walnut", frame: "light", accents: "silver", badge_field: "lapis", badge_text: "gold", badge_rim: "lapis", lattice: "silver", inner_trim: true, celebration: "rings", pitch_fx: "rings", metal: "gold", glow: "violet", lane_bars: "violet" },
    colors: { sung: "#c08cff", upcoming: "#f4f4f4", marker: "#7dffd8" },
  },
  {
    id: "molten",
    name: "Molten",
    blurb: "Fire in the pipes, a forge cabinet, bronze and iron, ember glow",
    swatch: ["#ff4a0a", "#ffb02a", "#3a2c24"],
    look: { cabinet: "forge", frame: "fire", accents: "iron", badge_field: "magma", badge_text: "ember", badge_rim: "gold", lattice: "iron", inner_trim: true, celebration: "inferno", pitch_fx: "flames", metal: "bronze", glow: "ember", lane_bars: "ember" },
    colors: { sung: "#ff8a1f", upcoming: "#fff1e0", marker: "#ffd23f" },
  },
  {
    id: "water",
    name: "Water",
    blurb: "Ocean waves rolling over the cabinet, blue water coursing through the pipes, chrome",
    swatch: ["#0a5ad8", "#36c8ff", "#e8f8ff"],
    look: { cabinet: "ocean", frame: "water", accents: "silver", badge_field: "lapis", badge_text: "white", badge_rim: "gold", lattice: "silver", inner_trim: true, celebration: "bloom", pitch_fx: "rings", metal: "chrome", glow: "aqua", lane_bars: "aqua" },
    colors: { sung: "#4fd8ff", upcoming: "#f0fbff", marker: "#ffffff" },
  },
  {
    id: "steampunk",
    name: "Steampunk",
    blurb: "Riveted brass and copper, turning gears, steam in the pipes, gaslight amber",
    swatch: ["#c8a044", "#8a4018", "#3a8a70"],
    look: { cabinet: "brass", frame: "steam", accents: "copper", badge_field: "copper", badge_text: "gold", badge_rim: "gold", lattice: "gold", inner_trim: true, celebration: "fireworks", pitch_fx: "sparks", metal: "brass", glow: "amber", lane_bars: "amber" },
    colors: { sung: "#ffc04a", upcoming: "#f6ead0", marker: "#7dffd8" },
  },
  {
    id: "cyberpunk",
    name: "Cyberpunk",
    blurb: "A neon city at night in the rain, data racing through the pipes, carbon and chrome",
    swatch: ["#2ae0ff", "#ffd23a", "#05080c"],
    look: { cabinet: "cyber", frame: "data", accents: "iron", badge_field: "carbon", badge_text: "neon", badge_rim: "gold", lattice: "iron", inner_trim: true, celebration: "rings", pitch_fx: "sparks", metal: "chrome", glow: "aqua", lane_bars: "amber" },
    colors: { sung: "#2af0ff", upcoming: "#eef6ff", marker: "#ffd23a" },
  },
];

/** The theme the look came from, and whether anything was changed after it. */
export function themeState(ui: { look?: Partial<LookSettings>; colors?: { sung: string; upcoming: string; marker: string } } | null | undefined): { theme: Theme | null; modified: boolean } {
  const look = lookOf(ui);
  const theme = THEMES.find((t) => t.id === look.theme) ?? null;
  if (!theme) return { theme: null, modified: false };
  const differs = (Object.keys(theme.look) as (keyof ThemeLook)[]).some((k) => look[k] !== theme.look[k]);
  const c = ui?.colors;
  const colorsDiffer = !!c && (c.sung.toLowerCase() !== theme.colors.sung || c.upcoming.toLowerCase() !== theme.colors.upcoming || c.marker.toLowerCase() !== theme.colors.marker);
  return { theme, modified: differs || colorsDiffer };
}

/** The look in effect: saved values this version knows, defaults for the rest. */
export function lookOf(ui: { look?: Partial<LookSettings> } | null | undefined): LookSettings {
  const saved = (ui?.look ?? {}) as Record<string, unknown>;
  const out = { ...DEFAULT_LOOK } as Record<string, unknown>;
  for (const k of Object.keys(LOOK_CHOICES) as (keyof LookSettings)[]) {
    const v = saved[k];
    if ((LOOK_CHOICES[k] as readonly unknown[]).includes(v)) out[k] = v;
  }
  return out as unknown as LookSettings;
}

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
    show_marker: true,
    // Gold sung, white upcoming since D-218 (before: #ff3d8b / #f6efd9, which the core
    // migrates to these when a settings file from before D-218 still holds them).
    colors: { sung: "#ffc21a", upcoming: "#f4f4f4", marker: "#7dffd8" },
    library_sort: "title",
    fullscreen_stage: false,
  },
  audio: { device_id: null, exclusive: false, master_volume: 1.0, device_offsets_ms: {}, key_mode: "auto", key_cache_mb: 1024, key_benchmarks: {} },
  practice: DEFAULT_PRACTICE_SETTINGS,
};
