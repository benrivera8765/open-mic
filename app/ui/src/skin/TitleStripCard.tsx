// SPDX-License-Identifier: GPL-3.0-or-later
// A song in the library as a jukebox title strip (docs/DESIGN.md "Library"): a cream
// card with a thin red rule top and bottom, the title in bold condensed capitals,
// the artist below, a small cover at the left, key and length at the right, and
// the selection code ("A7") on a chrome tab. TitleStripRack is the chrome-framed
// rack that holds them: two columns on wide screens, one on narrow ones.

import type { ComponentChildren } from "preact";

export interface TitleStripCardProps {
  title: string;
  artist: string;
  /** Short key name, e.g. "A maj"; null/empty when unknown. */
  keyName?: string | null;
  /** Seconds (formatted m:ss) or an already formatted string. */
  duration: number | string;
  /** Selection code, e.g. "A7". */
  code: string;
  /** Cover image URL (data: URL from song_cover). A drawn record when absent. */
  cover?: string | null;
  selected?: boolean;
  onSelect(): void;
  /** Optional small note under the artist (e.g. "lyrics unchecked"). */
  note?: string;
  disabled?: boolean;
  /**
   * Select mode (the Library's "Select", to delete several songs): when set, the strip
   * is a checkbox, ticked or not, with a selector lamp on its chrome tab that lights
   * when ticked; `onSelect` then toggles it.
   */
  checked?: boolean;
}

export function formatDuration(d: number | string): string {
  if (typeof d === "string") return d;
  if (!Number.isFinite(d) || d < 0) return "--:--";
  const s = Math.round(d);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** The record drawn when a song has no cover art. */
function RecordArt() {
  return (
    <svg class="jk-strip-record" viewBox="0 0 40 40" aria-hidden="true">
      <circle cx="20" cy="20" r="19" fill="#141016" />
      <circle cx="20" cy="20" r="15.5" fill="none" stroke="#2f2a33" stroke-width="0.6" />
      <circle cx="20" cy="20" r="12.5" fill="none" stroke="#2f2a33" stroke-width="0.6" />
      <circle cx="20" cy="20" r="9.5" fill="none" stroke="#2f2a33" stroke-width="0.6" />
      <circle cx="20" cy="20" r="6.5" fill="#c8102e" />
      <circle cx="20" cy="20" r="1.3" fill="#f6efd9" />
      <path d="M8 11 A15 15 0 0 1 20 5" stroke="#fff" stroke-opacity="0.35" stroke-width="1.2" fill="none" stroke-linecap="round" />
    </svg>
  );
}

export function TitleStripCard({
  title,
  artist,
  keyName,
  duration,
  code,
  cover,
  selected = false,
  onSelect,
  note,
  disabled = false,
  checked,
}: TitleStripCardProps) {
  const selecting = checked !== undefined;
  return (
    <button
      type="button"
      class={`jk-strip${selected ? " jk-strip--selected" : ""}${selecting ? " jk-strip--selecting" : ""}${checked ? " jk-strip--checked" : ""}`}
      onClick={() => onSelect()}
      disabled={disabled}
      role={selecting ? "checkbox" : undefined}
      aria-checked={selecting ? (checked ? "true" : "false") : undefined}
      aria-pressed={selecting ? undefined : selected ? "true" : "false"}
      title={`${title} - ${artist}`}
    >
      <span class="jk-strip-tab" aria-label={`Selection ${code}`}>
        {code}
        {selecting && <span class="jk-strip-lamp" aria-hidden="true" />}
      </span>
      <span class="jk-strip-cover">{cover ? <img src={cover} alt="" draggable={false} /> : <RecordArt />}</span>
      <span class="jk-strip-text">
        <span class="jk-strip-title">{title}</span>
        <span class="jk-strip-artist">{artist}</span>
        {note && <span class="jk-strip-note">{note}</span>}
      </span>
      <span class="jk-strip-meta">
        <span class="jk-strip-key">{keyName || "—"}</span>
        <span class="jk-strip-duration">{formatDuration(duration)}</span>
      </span>
    </button>
  );
}

export interface TitleStripRackProps {
  children?: ComponentChildren;
  class?: string;
  /** Accessible name of the list. */
  label?: string;
}

/** Chrome-framed rack of title strips; columns adapt to the width (no media queries needed). */
export function TitleStripRack({ children, class: className, label = "Songs" }: TitleStripRackProps) {
  return (
    <div class={`jk-rack${className ? ` ${className}` : ""}`} role="group" aria-label={label}>
      <div class="jk-rack-inner">{children}</div>
    </div>
  );
}
