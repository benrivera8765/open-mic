// SPDX-License-Identifier: GPL-3.0-or-later
// Monotone cubic interpolation of the ticker's time -> strip-position mapping.
//
// The layout (layout.ts) produces knots (t_k, x_k): the edges of every grapheme's
// cell at its start and end time, plus the points that shape instrumental breaks.
// Between knots the raw mapping would be linear, so the scroll speed would jump at
// every letter boundary (letters have fixed widths but are sung at varying speeds).
//
// We interpolate the knots with Steffen's monotone cubic (M. Steffen, "A simple
// method for monotonic interpolation in one dimension", Astron. Astrophys. 239, 1990):
//   - it passes through every knot, so each grapheme still starts and ends exactly
//     where the timing data says;
//   - it is monotone and never overshoots between two knots, so for t inside a
//     grapheme's [start, end) the position stays inside that grapheme's cell:
//     the smoothing only redistributes motion within each grapheme's own interval
//     (the hard constraint of WORK_ORDER 4.4);
//   - its first derivative is continuous (no speed jumps), and at a knot it is at
//     most twice the smaller neighbouring secant, which bounds the peak speed to a
//     small multiple of the local average speed.
//
// Pure maths, no DOM. Evaluation is O(log n) by binary search, with a cached index
// for the common case of time moving forward a little each frame.

export interface Knot {
  t: number;
  x: number;
}

export class MonotoneSpline {
  readonly t: Float64Array;
  readonly x: Float64Array;
  /** Derivative (px/s) at each knot. */
  readonly d: Float64Array;
  private last = 0;

  /** Knots must have strictly increasing t and non-decreasing x. */
  constructor(knots: Knot[]) {
    const n = knots.length;
    if (n === 0) throw new Error("MonotoneSpline needs at least one knot");
    this.t = new Float64Array(n);
    this.x = new Float64Array(n);
    this.d = new Float64Array(n);
    for (let k = 0; k < n; k++) {
      this.t[k] = knots[k].t;
      this.x[k] = knots[k].x;
      if (k > 0) {
        if (!(this.t[k] > this.t[k - 1])) throw new Error(`knot times must increase (k=${k}: ${this.t[k - 1]} -> ${this.t[k]})`);
        if (this.x[k] < this.x[k - 1]) throw new Error(`knot positions must not decrease (k=${k})`);
      }
    }
    if (n === 1) return;
    const h = (k: number) => this.t[k + 1] - this.t[k];
    const s = (k: number) => (this.x[k + 1] - this.x[k]) / h(k);
    // Ends: one-sided Steffen formula, limited the same way.
    for (let k = 0; k < n; k++) {
      if (k === 0 || k === n - 1) {
        const j = k === 0 ? 0 : n - 2;
        this.d[k] = s(j);
        continue;
      }
      const s0 = s(k - 1);
      const s1 = s(k);
      if (s0 <= 0 || s1 <= 0) {
        this.d[k] = 0;
        continue;
      }
      const p = (s0 * h(k) + s1 * h(k - 1)) / (h(k - 1) + h(k));
      this.d[k] = Math.min(2 * s0, 2 * s1, p);
    }
    // At the ends, use a quadratic-like slope clamped to [0, 2 * secant].
    if (n >= 3) {
      const s0 = s(0);
      this.d[0] = s0 <= 0 ? 0 : Math.min(2 * s0, Math.max(0, 1.5 * s0 - 0.5 * this.d[1]));
      const sl = s(n - 2);
      this.d[n - 1] = sl <= 0 ? 0 : Math.min(2 * sl, Math.max(0, 1.5 * sl - 0.5 * this.d[n - 2]));
    }
  }

  get start(): number {
    return this.t[0];
  }
  get end(): number {
    return this.t[this.t.length - 1];
  }

  /** Index k with t[k] <= time < t[k+1] (clamped to the knot range). */
  private segment(time: number): number {
    const t = this.t;
    const n = t.length;
    let k = this.last;
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

  /** Strip position at `time`; held constant outside the knot range. */
  at(time: number): number {
    const n = this.t.length;
    if (n === 1 || time <= this.t[0]) return this.x[0];
    if (time >= this.t[n - 1]) return this.x[n - 1];
    const k = this.segment(time);
    const h = this.t[k + 1] - this.t[k];
    const u = (time - this.t[k]) / h;
    const u2 = u * u;
    const u3 = u2 * u;
    const x0 = this.x[k];
    const x1 = this.x[k + 1];
    const v = (2 * u3 - 3 * u2 + 1) * x0 + (u3 - 2 * u2 + u) * h * this.d[k] + (-2 * u3 + 3 * u2) * x1 + (u3 - u2) * h * this.d[k + 1];
    // Guard against rounding outside the segment (the maths cannot overshoot).
    return v < x0 ? x0 : v > x1 ? x1 : v;
  }

  /** Speed (px/s) at `time`. */
  speed(time: number): number {
    const n = this.t.length;
    if (n === 1 || time <= this.t[0] || time >= this.t[n - 1]) return 0;
    const k = this.segment(time);
    const h = this.t[k + 1] - this.t[k];
    const u = (time - this.t[k]) / h;
    const u2 = u * u;
    const x0 = this.x[k];
    const x1 = this.x[k + 1];
    return ((6 * u2 - 6 * u) * x0 + (6 * u2 - 6 * u) * -x1) / h + (3 * u2 - 4 * u + 1) * this.d[k] + (3 * u2 - 2 * u) * this.d[k + 1];
  }

  /** Average speed (secant) of the knot interval containing `time`. */
  secantAt(time: number): number {
    const n = this.t.length;
    if (n === 1 || time < this.t[0] || time >= this.t[n - 1]) return 0;
    const k = this.segment(time);
    return (this.x[k + 1] - this.x[k]) / (this.t[k + 1] - this.t[k]);
  }
}
