// SPDX-License-Identifier: GPL-3.0-or-later
// Word-paced motion of the lyrics ticker (settings.ui.lyrics_motion = "word", the
// default since the owner's request of 18 Nov 2026; D-216).
//
// The letter-exact motion (motion.ts, D-95) pins every letter's cell to the letter's
// own timing. Letters are drawn at fixed widths but sung at very different speeds
// (60 ms consonants next to held vowels), so the text surges and brakes several times
// a word. The owner found that choppy and asked for the text to move word by word,
// never stopping, with its speed set by how long each word is sung.
//
// Here the knots (layout.ts `wordKnots`) are word-level: a word's first letter reaches
// the marker when the word starts; the word carries the space after it at its own pace
// (no dash across the space); in a short pause the word just sung stays at the marker
// up to LINGER_S longer (the text keeps gliding), only a longer pause slows the text
// down. Knot times may move by up to WORD_TOLERANCE_S, and only where the speeds on the
// two sides of a knot would differ by more than STRAIGHTEN_RATIO (straightenKnots).
//
// RampMotion turns the knots into a speed profile that is easy to read and to check:
//   - a plateau per knot interval (a word is crossed at its own, steady pace);
//   - an S-shaped ramp (smoothstep) around every knot that blends one plateau into the
//     next, reaching up to RAMP_S into each neighbour (at most half of either interval);
//   - the plateau speeds come from one tridiagonal solve so that every interval covers
//     exactly its distance: every knot is passed exactly at its time.
// The speed is continuous and so is its derivative (smoothstep has zero slope at both
// ends), it is never negative (it only ever blends between non-negative plateaus), so
// the text never runs backwards, and it is positive wherever the text has to move.
// Intervals without movement (the rest of a long break) keep speed 0; ramps never reach
// into them. A plateau may sag below its interval's average to feed a ramp into a fast
// neighbour, but never below MIN_PLATEAU of it (the ramps are shortened instead).
//
// Pure maths, no DOM; O(log n) evaluation with a cached interval index.
import type { Knot } from "./motion";

/** What the renderer, the lookahead rule and the checks need from a motion. */
export interface Motion {
  readonly start: number;
  readonly end: number;
  /** Strip position under the marker at `time` (held constant outside the knot range). */
  at(time: number): number;
  /** Speed (px/s) at `time`. */
  speed(time: number): number;
  /** Average speed of the knot interval containing `time`. */
  secantAt(time: number): number;
}

/** A word's first letter reaches the marker within this many seconds of the word's start. */
export const WORD_TOLERANCE_S = 0.04;
/**
 * In a pause, the word just sung may stay at the marker this long after its end before
 * the text slows down to cross the space (nothing else is sung meanwhile).
 */
export const LINGER_S = 0.25;
/** A knot moves (within WORD_TOLERANCE_S) only where its two sides' speeds differ by more than this factor. */
export const STRAIGHTEN_RATIO = 2;
/** Longest reach (s) of a speed ramp into each neighbouring interval. */
export const RAMP_S = 0.35;
/** A plateau never sags below this share of its interval's average speed. */
export const MIN_PLATEAU = 0.5;

/** Smoothstep and its integral from 0. */
const S = (u: number) => u * u * (3 - 2 * u);
const SI = (u: number) => u * u * u * (1 - u / 2);

export class RampMotion implements Motion {
  readonly t: Float64Array;
  readonly x: Float64Array;
  /** Plateau speed (px/s) of interval i = [t[i], t[i+1]]. */
  readonly c: Float64Array;
  /** The ramp around knot j spans [t[j] - ra[j], t[j] + rb[j]]. */
  readonly ra: Float64Array;
  readonly rb: Float64Array;
  private last = 0;

  /** Knots must have strictly increasing t and non-decreasing x. */
  constructor(knots: Knot[], maxRamp = RAMP_S, minPlateau = MIN_PLATEAU) {
    const n = knots.length;
    if (n === 0) throw new Error("RampMotion needs at least one knot");
    this.t = new Float64Array(n);
    this.x = new Float64Array(n);
    for (let k = 0; k < n; k++) {
      this.t[k] = knots[k].t;
      this.x[k] = knots[k].x;
      if (k > 0) {
        if (!(this.t[k] > this.t[k - 1])) throw new Error(`knot times must increase (k=${k}: ${this.t[k - 1]} -> ${this.t[k]})`);
        if (this.x[k] < this.x[k - 1]) throw new Error(`knot positions must not decrease (k=${k})`);
      }
    }
    const m = Math.max(0, n - 1);
    this.c = new Float64Array(m);
    this.ra = new Float64Array(n);
    this.rb = new Float64Array(n);
    if (m === 0) return;
    const h = new Float64Array(m);
    const dx = new Float64Array(m);
    for (let i = 0; i < m; i++) {
      h[i] = this.t[i + 1] - this.t[i];
      dx[i] = this.x[i + 1] - this.x[i];
    }
    // Ramps reach into moving intervals only, at most half of each.
    for (let j = 0; j < n; j++) {
      this.ra[j] = j > 0 && dx[j - 1] > 0 ? Math.min(maxRamp, h[j - 1] / 2) : 0;
      this.rb[j] = j < m && dx[j] > 0 ? Math.min(maxRamp, h[j] / 2) : 0;
    }
    const alpha = new Float64Array(n);
    const beta = new Float64Array(n);
    const diag = new Float64Array(m);
    const rhs = new Float64Array(m);
    const solve = () => {
      for (let j = 0; j < n; j++) {
        const L = this.ra[j] + this.rb[j];
        if (L <= 0) {
          alpha[j] = beta[j] = 0;
          continue;
        }
        const u0 = this.ra[j] / L;
        // Interval j-1 holds the ramp's part [0, u0], interval j the part [u0, 1]. Their
        // integrals differ from a plateau by alpha * (c_next - c_prev) and
        // beta * (c_prev - c_next) respectively.
        alpha[j] = L * SI(u0);
        beta[j] = L * (0.5 - u0 + SI(u0));
      }
      // Interval i: beta_i c_{i-1} + (h_i - beta_i - alpha_{i+1}) c_i + alpha_{i+1} c_{i+1} = dx_i,
      // with the speed 0 before the first and after the last knot. Diagonally dominant
      // (beta_i + alpha_{i+1} <= h_i / 2), so the Thomas algorithm is stable.
      for (let i = 0; i < m; i++) {
        diag[i] = h[i] - beta[i] - alpha[i + 1];
        rhs[i] = dx[i];
      }
      for (let i = 1; i < m; i++) {
        const w = beta[i] / diag[i - 1];
        diag[i] -= w * alpha[i];
        rhs[i] -= w * rhs[i - 1];
      }
      this.c[m - 1] = rhs[m - 1] / diag[m - 1];
      for (let i = m - 2; i >= 0; i--) this.c[i] = (rhs[i] - alpha[i + 1] * this.c[i + 1]) / diag[i];
    };
    // The ramps into a sagging plateau are halved 12 times, then removed. A plateau with
    // no ramp reaching into it equals its interval's average exactly, so from then on
    // every pass removes at least one ramp and the loop ends. The plateaus are always
    // solved for the ramps finally used, so every knot is passed exactly (a pass that
    // changed ramps without solving again made the text jump at a knot).
    for (let iter = 0; ; iter++) {
      solve();
      let bad = false;
      for (let i = 0; i < m; i++) {
        if (dx[i] <= 0) {
          this.c[i] = 0;
          continue;
        }
        if (this.c[i] < minPlateau * (dx[i] / h[i])) {
          bad = true;
          // Shorter ramps into this interval: less of it goes to its neighbours' pace.
          const f = iter < 12 ? 0.5 : 0;
          this.rb[i] *= f;
          this.ra[i + 1] *= f;
        }
      }
      if (!bad) break;
      if (iter > 13 + 2 * n) {
        // Not reachable (see above); the safe fallback is no ramps at all.
        this.ra.fill(0);
        this.rb.fill(0);
        solve();
        for (let i = 0; i < m; i++) if (dx[i] <= 0) this.c[i] = 0;
        break;
      }
    }
  }

  get start(): number {
    return this.t[0];
  }
  get end(): number {
    return this.t[this.t.length - 1];
  }

  /** Index i with t[i] <= time < t[i+1] (clamped to the knot range). */
  private segment(time: number): number {
    const t = this.t;
    const n = t.length;
    const k = this.last;
    if (k < n - 1 && t[k] <= time && time < t[k + 1]) return k;
    if (k + 1 < n - 1 && t[k + 1] <= time && time < t[k + 2]) return (this.last = k + 1);
    let lo = 0;
    let hi = n - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (t[mid] <= time) lo = mid;
      else hi = mid;
    }
    this.last = lo;
    return lo;
  }

  at(time: number): number {
    const n = this.t.length;
    if (n === 1 || time <= this.t[0]) return this.x[0];
    if (time >= this.t[n - 1]) return this.x[n - 1];
    const i = this.segment(time);
    const m = n - 1;
    const tau = time - this.t[i];
    const h = this.t[i + 1] - this.t[i];
    const ci = this.c[i];
    const cPrev = i > 0 ? this.c[i - 1] : 0;
    const cNext = i + 1 < m ? this.c[i + 1] : 0;
    let acc = 0;
    // Rising (or falling) out of the ramp around knot i.
    const rb = this.rb[i];
    if (rb > 0) {
      const L = this.ra[i] + rb;
      const u0 = this.ra[i] / L;
      const ta = Math.min(tau, rb);
      acc += cPrev * ta + (ci - cPrev) * L * (SI(u0 + ta / L) - SI(u0));
    }
    // Plateau.
    const pEnd = h - this.ra[i + 1];
    if (tau > rb) acc += ci * (Math.min(tau, pEnd) - rb);
    // Into the ramp around knot i+1.
    if (tau > pEnd) {
      const L = this.ra[i + 1] + this.rb[i + 1];
      const tc = tau - pEnd;
      acc += ci * tc + (cNext - ci) * L * SI(tc / L);
    }
    const x0 = this.x[i];
    const x1 = this.x[i + 1];
    const v = x0 + acc;
    return v < x0 ? x0 : v > x1 ? x1 : v;
  }

  speed(time: number): number {
    const n = this.t.length;
    if (n === 1 || time <= this.t[0] || time >= this.t[n - 1]) return 0;
    const i = this.segment(time);
    const m = n - 1;
    const tau = time - this.t[i];
    const h = this.t[i + 1] - this.t[i];
    const ci = this.c[i];
    const rb = this.rb[i];
    if (tau < rb) {
      const cPrev = i > 0 ? this.c[i - 1] : 0;
      const L = this.ra[i] + rb;
      return cPrev + (ci - cPrev) * S((this.ra[i] + tau) / L);
    }
    const pEnd = h - this.ra[i + 1];
    if (tau <= pEnd) return ci;
    const cNext = i + 1 < m ? this.c[i + 1] : 0;
    const L = this.ra[i + 1] + this.rb[i + 1];
    return ci + (cNext - ci) * S((tau - pEnd) / L);
  }

  secantAt(time: number): number {
    const n = this.t.length;
    if (n === 1 || time < this.t[0] || time >= this.t[n - 1]) return 0;
    const i = this.segment(time);
    return (this.x[i + 1] - this.x[i]) / (this.t[i + 1] - this.t[i]);
  }
}

/** A knot whose time may move within [lo, hi] (lo = hi = t for a fixed knot). */
export interface FloatingKnot extends Knot {
  lo: number;
  hi: number;
}

/**
 * Moves floating knots within their windows where the average speeds of the two
 * intervals a knot joins differ by more than `maxRatio`, each only as far from its
 * original time as needed to bring that ratio down to `maxRatio` (or to the edge of its
 * window). Gauss-Seidel sweeps, so neighbours settle together. A word sung in 20 ms
 * between two ordinary words then takes up to 100 ms on screen instead of a 20 ms
 * dash, while most words keep their exact start. Order is kept with a minimum spacing
 * of `minDt` seconds. Returns plain knots.
 */
export function straightenKnots(knots: FloatingKnot[], maxRatio = STRAIGHTEN_RATIO, sweeps = 40, minDt = 0.002): Knot[] {
  const n = knots.length;
  const t = knots.map((k) => k.t);
  const x = knots.map((k) => k.x);
  for (let s = 0; s < sweeps; s++) {
    let moved = 0;
    for (let j = 1; j + 1 < n; j++) {
      const k = knots[j];
      if (!(k.hi > k.lo)) continue;
      const d0 = x[j] - x[j - 1];
      const d1 = x[j + 1] - x[j];
      if (d0 <= 0 || d1 <= 0) continue;
      // The time at which (right speed) / (left speed) equals q.
      const a = t[j - 1];
      const b = t[j + 1];
      const at = (q: number) => (d1 * a + q * d0 * b) / (d1 + q * d0);
      const lo = Math.max(k.lo, a + minDt, at(1 / maxRatio));
      const hi = Math.min(k.hi, b - minDt, at(maxRatio));
      const lo2 = Math.max(k.lo, a + minDt);
      const hi2 = Math.min(k.hi, b - minDt);
      if (!(hi2 >= lo2)) continue;
      // Closest to the original time inside the band; the window edge if the band lies outside it.
      let target = k.t < lo ? lo : k.t > hi ? hi : k.t;
      target = target < lo2 ? lo2 : target > hi2 ? hi2 : target;
      moved = Math.max(moved, Math.abs(target - t[j]));
      t[j] = target;
    }
    if (moved < 1e-6) break;
  }
  return t.map((tt, j) => ({ t: tt, x: x[j] }));
}
