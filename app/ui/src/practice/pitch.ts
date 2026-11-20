// SPDX-License-Identifier: GPL-3.0-or-later
// Practice mode's one rule for pitch, key and octave (docs/APP_API.md 12.1), as the UI
// uses it to draw. The core grades with the same rule (jk-pitch::score), so the lane, the
// piano keys, the note names and the grade always agree (WO2 4.6 "everything moves
// together"):
//
//   target(note, t) = note.midi + key_at(t) + 12 * octave_shift   // MIDI with fractions, never rounded
//   lane(m)         = m - tuning_cents / 100                        // vertical position in the lane
//   note name       = name(round(lane(target)), spelling)           // "A3"; spelling: sharps | flats
//   singer(t)       = 69 + 12 * log2(hz / 440)                      // drawn at lane(singer(t))
//
// `key_at(t)` is the key HEARD (PlayerState.key_heard / key_pending, D-239), never the key
// asked for: the KeyTimeline below keeps what the clock events said, so the bars move when
// the key that is heard changes, at the right song time (WO2 trap 1).
//
// Pure TypeScript, no DOM.
import type { KeyPending, NoteSpelling, SongKey } from "../api/types";

export const SHARP_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"] as const;
export const FLAT_NAMES = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"] as const;
/** Pitch classes of the black keys. */
const BLACK = [false, true, false, true, false, false, true, false, true, false, true, false];

/** Pitch class 0..11 of a MIDI note (any integer). */
export const pitchClassOf = (midi: number): number => ((Math.round(midi) % 12) + 12) % 12;

/** True for C#, D#, F#, G#, A#. */
export const isBlackKey = (midi: number): boolean => BLACK[pitchClassOf(midi)];

/**
 * The name of a piano key, ASCII ("C4", "Db4", "A#3"), MIDI 60 = C4. The jk-pitch namer
 * gives the same strings (tested for every key -12..12 in pitch.test.ts).
 */
export function noteName(midi: number, spelling: NoteSpelling): string {
  const m = Math.round(midi);
  const octave = Math.floor(m / 12) - 1;
  return `${(spelling === "flats" ? FLAT_NAMES : SHARP_NAMES)[pitchClassOf(m)]}${octave}`;
}

/** The same with the music signs the app's key names use ("D♭4", "C♯4"). */
export const displayName = (name: string): string => name.replace(/^([A-G])#/, "$1♯").replace(/^([A-G])b/, "$1♭");

const TONICS = SHARP_NAMES as readonly string[];
/** Major keys written with flats (signature has flats): F, Bb, Eb, Ab, Db. */
const FLAT_MAJOR = new Set([5, 10, 3, 8, 1]);
/** Minor keys written with flats: D, G, C, F, Bb, Eb. */
const FLAT_MINOR = new Set([2, 7, 0, 5, 10, 3]);

/**
 * How notes are spelled in the key heard (APP_API section 3, `key_spelling`): flats in keys
 * whose signature has flats (F, Bb, Eb, Ab, Db major; D, G, C, F, Bb, Eb minor), sharps in
 * every other key and for a song with no detected key. Agrees with the key name the app
 * shows (lib/keys.ts: Db, Eb, Ab, Bb major and Eb, Bb minor with flats; F# major and C#, F#,
 * G# minor with sharps). The core sends it; this is the fallback and the mock's source.
 */
export function spellingFor(key: SongKey | null, semitones: number): NoteSpelling {
  if (!key) return "sharps";
  const base = TONICS.indexOf(key.tonic.replace("♯", "#"));
  if (base < 0) return "sharps";
  const pc = (((base + semitones) % 12) + 12) % 12;
  return (key.mode === "minor" ? FLAT_MINOR : FLAT_MAJOR).has(pc) ? "flats" : "sharps";
}

/** lane(m) of the rule: the guide's pitch moved onto the piano rows by the recording's tuning. */
export const laneOf = (midi: number, tuningCents: number): number => midi - tuningCents / 100;

/** target(note, t) of the rule. */
export const targetMidi = (noteMidi: number, key: number, octave: number): number => noteMidi + key + 12 * octave;

/** The name of the piano key a guide note is sung on at a key and octave ("a C4 at -3 is an A3"). */
export function targetName(noteMidi: number, key: number, octave: number, tuningCents: number, spelling: NoteSpelling): string {
  return noteName(Math.round(laneOf(targetMidi(noteMidi, key, octave), tuningCents)), spelling);
}

/** singer(t) of the rule. */
export const midiOfHz = (hz: number): number => 69 + 12 * Math.log2(hz / 440);

/** Cents folded into -600..600 (a whole number of octaves away counts as on the note, octave modes auto / fixed). */
export function foldCents(cents: number): number {
  const c = ((((cents + 600) % 1200) + 1200) % 1200) - 600;
  return c === -600 && cents > 0 ? 600 : c;
}

// ---------------------------------------------------------------------------
// The key heard over song time (APP_API 12.1 "key_at(t) for the lane")
// ---------------------------------------------------------------------------

/** A change of the key heard: from song time `at` on, `semitones` sound. */
export interface KeyChange {
  at: number;
  semitones: number;
  /** The key before it (what the bars slide from). */
  from: number;
  /** performance.now() when the UI first learned of it (the bars' slide starts then). */
  seenMs: number;
  /** performance.now() when the playback clock passed `at` (the view re-fits from then); NaN until then. */
  effectiveMs: number;
  /** Learned ahead from the engine's pending change (`at` is exact), not inferred from a late event. */
  fromPending?: boolean;
}

/**
 * How long after a change learned from its pending an event may still carry the key from
 * before it (the core's position runs at most 0.1 s past its snapshot), seconds.
 */
const STALE_S = 0.15;

/** What the lane needs of the key heard: an immutable snapshot of a KeyTimeline. */
export interface KeyView {
  segment: number;
  /** The key before the first change. */
  base: number;
  /** In song-time order; the last may be in the future (a pending change). */
  changes: readonly KeyChange[];
  /** The singer's octave shift (practice state `octave.shift`) and its own slide. */
  octave: number;
  octaveFrom: number;
  octaveSeenMs: number;
}

/** key_at(t) of a snapshot: the key heard at song time t. */
export function keyAt(v: KeyView, t: number): number {
  let k = v.base;
  for (const c of v.changes) {
    if (c.at <= t) k = c.semitones;
    else break;
  }
  return k;
}

/**
 * Follows `playback://clock` / `playback://state`: the key heard now, the pending change
 * (from `at_s` on; with `at_s` null the old key stays) and the segment. A new segment
 * (load, seek, device switch, paused re-render) starts afresh from the key heard. Changes
 * learned within one segment are kept (at most 32) so that bars sung before a change keep
 * the key they were sung in while they scroll away.
 */
export class KeyTimeline {
  private segment = -1;
  private base = 0;
  private changes: KeyChange[] = [];
  private octave = 0;
  private octaveFrom = 0;
  private octaveSeenMs = -Infinity;
  private snap: KeyView | null = null;

  /** Resets everything (a new song or practice session). */
  reset(keyHeard: number, segment: number, octave = this.octave): void {
    this.segment = segment;
    this.base = keyHeard;
    this.changes = [];
    this.octave = this.octaveFrom = octave;
    this.octaveSeenMs = -Infinity;
    this.snap = null;
  }

  /**
   * One clock or state event: `position` is the song time `keyHeard` belongs to, `nowMs`
   * the moment it arrived (performance.now()). Returns true when the picture changed.
   */
  update(position: number, keyHeard: number, pending: KeyPending | null | undefined, segment: number | undefined, nowMs: number): boolean {
    const seg = segment ?? this.segment;
    if (seg !== this.segment) {
      // A new segment: nothing before it carries over (APP_API: values are never mapped across it).
      this.segment = seg;
      this.base = keyHeard;
      this.changes = [];
      this.addPending(pending, nowMs);
      this.snap = null;
      return true;
    }
    let changed = false;
    // The clock passed a change: note when (the view's re-fit starts from that moment).
    for (const c of this.changes) {
      if (Number.isNaN(c.effectiveMs) && c.at <= position) {
        c.effectiveMs = nowMs - (position - c.at) * 1000;
        changed = true;
      }
    }
    // The key heard now disagrees with what the timeline says for `position`: a change
    // the UI did not see coming (a missed pending): it happened at `position` at the latest.
    // Except a stale pair: an event whose position ran on just past a change the UI knows
    // from its pending (at most STALE_S ago) while still carrying the key from before it.
    // Taking that as a change back would move the bars to the new key, back, and to the
    // new key again on the next event (an older core sent such pairs; the core now reports
    // the key at the position it sends, src-tauri/src/playback.rs `heard_at`).
    if (keyAt(this.view(), position) !== keyHeard && !this.staleAfterKnownChange(position, keyHeard)) {
      this.cutFrom(position);
      this.push({ at: position, semitones: keyHeard, from: keyAt(this.view(), position), seenMs: nowMs, effectiveMs: nowMs });
      changed = true;
    }
    if (this.addPending(pending, nowMs)) changed = true;
    if (changed) this.snap = null;
    return changed;
  }

  /** The singer's octave shift changed (auto decided, or the singer chose one). */
  setOctave(shift: number, nowMs: number): boolean {
    if (shift === this.octave) return false;
    this.octaveFrom = this.octave;
    this.octave = shift;
    this.octaveSeenMs = nowMs;
    this.snap = null;
    return true;
  }

  view(): KeyView {
    if (!this.snap) {
      this.snap = {
        segment: this.segment,
        base: this.base,
        changes: this.changes.map((c) => ({ ...c })),
        octave: this.octave,
        octaveFrom: this.octaveFrom,
        octaveSeenMs: this.octaveSeenMs,
      };
    }
    return this.snap;
  }

  /**
   * True when `keyHeard` at `position` is only the key from before the latest change the
   * timeline learned from a pending (`effectiveMs` was NaN when it was added) and that
   * change is at most STALE_S before `position`: the event's key is older than its position.
   */
  private staleAfterKnownChange(position: number, keyHeard: number): boolean {
    let last: KeyChange | null = null;
    for (const c of this.changes) {
      if (c.at <= position) last = c;
      else break;
    }
    return !!last && last.fromPending === true && position - last.at <= STALE_S && last.from === keyHeard;
  }

  private addPending(pending: KeyPending | null | undefined, nowMs: number): boolean {
    if (!pending || pending.at_s === null || !Number.isFinite(pending.at_s)) return false;
    const at = pending.at_s;
    const known = this.changes.find((c) => Math.abs(c.at - at) < 1e-6);
    if (known && known.semitones === pending.semitones) return false;
    // A pending change replaces any later one the UI knew (the newest request wins).
    this.cutFrom(at);
    this.push({ at, semitones: pending.semitones, from: keyAt(this.view(), at), seenMs: nowMs, effectiveMs: Number.NaN, fromPending: true });
    return true;
  }

  private cutFrom(at: number): void {
    const before = this.changes.length;
    this.changes = this.changes.filter((c) => c.at < at);
    if (this.changes.length !== before) this.snap = null;
  }

  private push(c: KeyChange): void {
    this.snap = null;
    if (c.semitones === c.from) return;
    this.changes.push(c);
    if (this.changes.length > 32) {
      // Forget the oldest: its key becomes the base.
      const old = this.changes.shift()!;
      this.base = old.semitones;
    }
  }
}
