// SPDX-License-Identifier: GPL-3.0-or-later
// Musical key names for the key control: "A major → B major (+2)".
//
// The song file stores tonics with sharps only (SONG_FORMAT 4.1); players may show
// flats. We spell keys the way sheet music usually does: Bb major rather than
// A# major, F# major, Eb minor, C# minor and so on.
import type { SongKey } from "../api/types";

export const KEY_MIN = -12;
export const KEY_MAX = 12;

const SHARP_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/** Conventional spelling of the 12 major keys (index = pitch class). */
const MAJOR_SPELLING = ["C", "D♭", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
/** Conventional spelling of the 12 minor keys. */
const MINOR_SPELLING = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "G♯", "A", "B♭", "B"];
/** The same in plain ASCII (file names, logs). */
const ascii = (s: string) => s.replace("♭", "b").replace("♯", "#");

/** Pitch class 0..11 of a tonic written with a sharp or flat (ASCII or Unicode), or -1. */
export function pitchClass(tonic: string): number {
  const t = ascii(tonic.trim());
  const base = SHARP_NAMES.indexOf(t[0]?.toUpperCase() ?? "");
  if (base < 0) return -1;
  let pc = base;
  for (const c of t.slice(1)) {
    if (c === "#") pc += 1;
    else if (c === "b") pc -= 1;
    else return -1;
  }
  return ((pc % 12) + 12) % 12;
}

/** Name of the key `semitones` above (or below) the given key, e.g. "B major". */
export function transposedKeyName(key: SongKey, semitones: number, asciiOnly = false): string {
  const pc = pitchClass(key.tonic);
  if (pc < 0) return key.name;
  const to = (((pc + semitones) % 12) + 12) % 12;
  const tonic = (key.mode === "minor" ? MINOR_SPELLING : MAJOR_SPELLING)[to];
  const name = `${tonic} ${key.mode}`;
  return asciiOnly ? ascii(name) : name;
}

/** "+2", "-3", "0" */
export const signed = (n: number): string => (n > 0 ? `+${n}` : `${n}`);

/**
 * The key control's main label: "A major → B major (+2)"; "A major (original key)"
 * at 0; "+2 semitones" when the key is unknown. `short` gives "B major" alone (file
 * names of exports).
 */
export function keyLabel(key: SongKey | null, semitones: number, arrow = "→", short = false): string {
  if (!key) {
    if (short) return semitones === 0 ? "original key" : `${signed(semitones)} semitones`;
    return semitones === 0 ? "Original key" : `${signed(semitones)} semitone${Math.abs(semitones) === 1 ? "" : "s"}`;
  }
  const from = transposedKeyName(key, 0, arrow !== "→");
  if (short) return transposedKeyName(key, semitones, true);
  if (semitones === 0) return `${from} (original key)`;
  return `${from} ${arrow} ${transposedKeyName(key, semitones, arrow !== "→")} (${signed(semitones)})`;
}

/** Short key name for title-strip cards: "A maj", "E♭ min". */
export function keyShort(key: SongKey | null, semitones = 0): string {
  if (!key) return "";
  const name = transposedKeyName(key, semitones);
  return name.replace(" major", " maj").replace(" minor", " min");
}

/** Clamp to the key control's range and round to a whole semitone. */
export const clampKey = (n: number): number => Math.max(KEY_MIN, Math.min(KEY_MAX, Math.round(n)));
