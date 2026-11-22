// SPDX-License-Identifier: GPL-3.0-or-later
// Lyrics for a song about to be made (owner, 2026-11-22): find them automatically, or paste
// or type them, and the Studio lines the given words up with the singing (STUDIO_PROTOCOL 7,
// lyrics mode "text"). Used by the Recorder's "Make a karaoke song" dialog and by songs
// waiting in the Studio queue. Class hooks: .lyrics-choice, .lyrics-choice__text.
import { useId } from "./common";

export interface LyricsPick {
  mode: "auto" | "text";
  text: string;
}

export const AUTO_LYRICS: LyricsPick = { mode: "auto", text: "" };

/** The request's `lyrics` for a pick (pasted text that is blank counts as automatic). */
export function lyricsRequest(p: LyricsPick): { mode: "auto" | "text"; text: string | null; lrc: null } {
  return p.mode === "text" && p.text.trim() ? { mode: "text", text: p.text, lrc: null } : { mode: "auto", text: null, lrc: null };
}

export function LyricsChoice({ value, onChange, label }: { value: LyricsPick; onChange: (v: LyricsPick) => void; label: string }) {
  const id = useId("lyrics");
  return (
    <fieldset class="lyrics-choice">
      <legend>{label}</legend>
      <label class="radio">
        <input type="radio" name={id} checked={value.mode === "auto"} onChange={() => onChange({ ...value, mode: "auto" })} /> Find the lyrics automatically
      </label>
      <label class="radio">
        <input type="radio" name={id} checked={value.mode === "text"} onChange={() => onChange({ ...value, mode: "text" })} /> I'll paste or type the lyrics
      </label>
      {value.mode === "text" && (
        <textarea
          class="lyrics-choice__text"
          aria-label={`Lyrics for ${label}, one sung line per line`}
          rows={7}
          value={value.text}
          placeholder="Paste or type the lyrics, one sung line per line. They are lined up with the singing."
          onInput={(e) => onChange({ mode: "text", text: (e.currentTarget as HTMLTextAreaElement).value })}
        />
      )}
    </fieldset>
  );
}
