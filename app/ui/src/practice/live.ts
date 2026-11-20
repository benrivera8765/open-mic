// SPDX-License-Identifier: GPL-3.0-or-later
// What practice mode has heard so far, as the lane draws it: the tracker values of the
// last few seconds (`practice://live` `pitch`, one per 10 ms) in a fixed ring of typed
// arrays (no allocation per batch or per frame), and the stretches of graded notes the
// scorer has decided (`hits`: lit while on pitch; WO2 4.5 "the lit part stays lit as it
// moves left"). Kept outside the preact store: batches arrive about 60 times a second and
// only the lane's frame loop reads them.
import type { PracticeLive } from "../api/types";

/** 8 s of values at 10 ms. */
const RING = 800;

export class LiveStore {
  private t = new Float64Array(RING);
  private m = new Float64Array(RING);
  private lvl = new Float32Array(RING);
  private head = 0;
  private count = 0;
  /** Lit / off stretches per note: flat [from, to, where, ...]. */
  private hits = new Map<number, number[]>();
  /** Notes finished: credit (NaN: not graded). */
  readonly done = new Map<number, number>();
  segment = -1;
  /** Bumped on every change (the frame loop compares it). */
  version = 0;

  clear(): void {
    this.head = this.count = 0;
    this.hits.clear();
    this.done.clear();
    this.version++;
  }

  /** Forget the pitch values (a seek or a new segment) but keep what was lit. */
  clearPitch(): void {
    this.head = this.count = 0;
    this.version++;
  }

  /** One `practice://live` batch. */
  apply(b: PracticeLive): void {
    if (b.segment !== this.segment) {
      this.segment = b.segment;
      this.clearPitch();
    }
    for (const [ts, midi, level] of b.pitch) this.push(ts, midi, level);
    for (const [note, from, to, where] of b.hits) this.addHit(note, from, to, where);
    for (const [note, credit] of b.done) this.done.set(note, credit === null ? Number.NaN : credit);
    this.version++;
  }

  /** Adds one tracker value (song time, MIDI or null, level dB). Values arrive in time order. */
  push(ts: number, midi: number | null, level: number): void {
    // A value earlier than the newest one means the song jumped back: start the tail afresh.
    if (this.count > 0 && ts < this.t[(this.head + RING - 1) % RING] - 0.5) this.head = this.count = 0;
    this.t[this.head] = ts;
    this.m[this.head] = midi === null ? Number.NaN : midi;
    this.lvl[this.head] = level;
    this.head = (this.head + 1) % RING;
    if (this.count < RING) this.count++;
  }

  /**
   * A decided stretch of a note. A stretch that continues the note's last one (same `where`,
   * starting where it ended) extends it; a stretch that starts more than 50 ms before the
   * note's last one ended is a new attempt (the singer went back): the old stretches of that
   * note are dropped, so a note sung again keeps its latest attempt, as the grade does.
   */
  addHit(note: number, from: number, to: number, where: number): void {
    let list = this.hits.get(note);
    // Going back in time within the note: a new attempt.
    if (list && list.length && from < list[list.length - 2] - 0.05) list = undefined;
    if (!list) {
      list = [];
      this.hits.set(note, list);
    }
    const n = list.length;
    if (n >= 3 && list[n - 1] === where && from <= list[n - 2] + 0.025) {
      list[n - 2] = Math.max(list[n - 2], to);
      return;
    }
    list.push(from, to, where);
  }

  /** The note's decided stretches (flat triples), or undefined. */
  hitsOf(note: number): readonly number[] | undefined {
    return this.hits.get(note);
  }

  /** Number of values kept. */
  get size(): number {
    return this.count;
  }

  /** The newest value: index 0 = newest. Returns false when there is none. */
  at(k: number, out: { t: number; midi: number; level: number }): boolean {
    if (k >= this.count) return false;
    const i = (this.head + RING - 1 - k) % RING;
    out.t = this.t[i];
    out.midi = this.m[i];
    out.level = this.lvl[i];
    return true;
  }
}
