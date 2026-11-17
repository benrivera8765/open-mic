// SPDX-License-Identifier: GPL-3.0-or-later
// The UI side of the playback clock (docs/ARCHITECTURE.md section 4, WORK_ORDER trap 7).
//
// The engine sends `playback://clock` about 30 times per second with the song time of
// the sample now leaving the DAC. Between events the UI interpolates with
// performance.now(). It never runs a free timer: every event re-anchors the estimate.
//
// Re-anchoring is gentle: small errors (IPC jitter, clock drift) are absorbed by
// running the interpolation up to 5 % faster or slower for a moment, so the lyrics
// never visibly jump; only a real discontinuity (a seek, > 250 ms) snaps.
//
// Pauses: when the UI itself asks for a pause it calls `hold()`, which freezes the
// display at once, so it cannot run ahead of the audio while the request travels;
// the engine's "paused" event then settles the last few ms forward. A pause the UI did
// not ask for (end of song, device lost) can only be seen when its event arrives, so
// at most one event interval (~33 ms) is shown ahead; that is glided back in 120 ms.
// If events stop arriving while playing, extrapolation stops after 250 ms.
// Seeks the UI asks for while playing show the target at once and wait there until
// the engine reports that the new position is actually playing (see `seekHold`).
//
// Pure (no DOM): `now` is always passed in, so the tests drive it with a fake clock.

export interface ClockOptions {
  /** Errors larger than this (seconds) snap instead of slewing. */
  snapThreshold: number;
  /** Errors are absorbed over about this many seconds. */
  horizon: number;
  /** Largest relative speed change while absorbing an error. */
  maxRateDeviation: number;
  /** Stop extrapolating this long (ms) after the last event. */
  staleMs: number;
  /** Duration (ms) of the settle glide after a pause. */
  glideMs: number;
}

const DEFAULTS: ClockOptions = {
  snapThreshold: 0.25,
  horizon: 0.5,
  maxRateDeviation: 0.05,
  staleMs: 250,
  glideMs: 120,
};

export class PlaybackClock {
  private o: ClockOptions;
  /** Linear model while running: position = base + (now - baseT) / 1000 * rate. */
  private base = 0;
  private baseT = 0;
  private rate = 1;
  private running = false;
  private lastEventT = -Infinity;
  /** Largest value shown while running (the display never goes backwards). */
  private shown = 0;
  /** Settle glide while stopped. */
  private glide: { from: number; to: number; t0: number } | null = null;
  private held = false;
  private heldAt = 0;
  /**
   * A seek the UI asked for while playing. The engine reports the target itself until
   * the new audio reaches the device (measured about 30 ms on the null device), then
   * moves on; until it does, the display waits at the target instead of running ahead
   * (without this it ran 25-30 ms ahead for 1-2 s after every seek).
   */
  private seekHold: { target: number; at: number } | null = null;

  constructor(options: Partial<ClockOptions> = {}) {
    this.o = { ...DEFAULTS, ...options };
  }

  /** True while the display may still change without a new event. */
  isAnimating(now: number): boolean {
    if (this.running) return now - this.lastEventT < this.o.staleMs + 50;
    return this.glide !== null && now - this.glide.t0 < this.o.glideMs;
  }

  get playing(): boolean {
    return this.running;
  }

  private model(now: number): number {
    const t = Math.min(now, this.lastEventT + this.o.staleMs);
    return this.base + ((t - this.baseT) / 1000) * this.rate;
  }

  /** Song time to show at `now` (ms, performance.now() timebase). */
  position(now: number): number {
    if (this.running) {
      if (this.seekHold) return this.shown; // waiting at the seek target (see seekHold)
      const v = Math.max(this.shown, this.model(now));
      this.shown = v;
      return v;
    }
    if (this.glide) {
      const u = Math.min(1, (now - this.glide.t0) / this.o.glideMs);
      if (u >= 1) {
        this.base = this.glide.to;
        this.glide = null;
        return this.base;
      }
      const e = u * u * (3 - 2 * u); // smoothstep
      return this.glide.from + (this.glide.to - this.glide.from) * e;
    }
    return this.base;
  }

  /** Feed a `playback://clock` event received at `now`. */
  onEvent(ev: { position_s: number; playing: boolean }, now: number): void {
    const target = ev.position_s;
    if (ev.playing) {
      // A "playing" event sent before our pause request reached the engine: ignore it
      // (for at most 1 s, in case the request was lost).
      if (this.held && now - this.heldAt < 1000) return;
      if (this.seekHold && this.running) {
        const sh = this.seekHold;
        // Still at the target (audio not moving yet), or an event sent before the seek
        // reached the engine: keep waiting (for at most 1 s, in case the seek was lost).
        const notMoving = target <= sh.target + 0.001;
        const stale = Math.abs(target - sh.target) > this.o.snapThreshold;
        if (now - sh.at < 1000 && (notMoving || stale)) {
          this.lastEventT = now;
          return;
        }
        // The new position is playing: anchor on it (a forward step of a few ms).
        this.seekHold = null;
        this.glide = null;
        this.held = false;
        this.base = target;
        this.baseT = now;
        this.rate = 1;
        this.shown = target;
        this.lastEventT = now;
        return;
      }
      this.glide = null;
      this.held = false;
      if (!this.running) {
        this.running = true;
        this.base = target;
        this.baseT = now;
        this.rate = 1;
        this.shown = target;
      } else {
        const predicted = this.model(now);
        const err = target - predicted;
        if (Math.abs(err) > this.o.snapThreshold) {
          this.base = target;
          this.baseT = now;
          this.rate = 1;
          this.shown = target; // a real jump (seek): allowed to go backwards
        } else {
          const dev = Math.max(-this.o.maxRateDeviation, Math.min(this.o.maxRateDeviation, err / this.o.horizon));
          this.base = predicted;
          this.baseT = now;
          this.rate = 1 + dev;
        }
      }
      this.lastEventT = now;
      return;
    }
    // Stopped (paused, ended, error...): settle on the engine's position.
    const shownNow = this.position(now);
    this.seekHold = null;
    this.running = false;
    this.lastEventT = now;
    if (Math.abs(target - shownNow) > this.o.snapThreshold || this.o.glideMs <= 0) {
      this.base = target;
      this.glide = null;
    } else if (target !== shownNow) {
      this.glide = { from: shownNow, to: target, t0: now };
      this.base = target;
    } else {
      this.base = target;
      this.glide = null;
    }
    this.held = false;
  }

  /** The UI asked for a pause: freeze the display now (see header). */
  hold(now: number): void {
    if (!this.running) return;
    const v = this.position(now);
    this.seekHold = null;
    this.running = false;
    this.base = v;
    this.glide = null;
    this.held = true;
    this.heldAt = now;
  }

  /** The UI asked to play (or its pause request failed): stop ignoring "playing" events. */
  release(): void {
    this.held = false;
  }

  /** True between hold() and the engine's answer. */
  get isHeld(): boolean {
    return this.held;
  }

  /** The UI asked for a seek: show the target at once (the engine's event confirms it). */
  jump(position: number, now: number): void {
    this.glide = null;
    this.base = position;
    this.baseT = now;
    this.rate = 1;
    this.shown = position;
    if (this.running) {
      this.lastEventT = now;
      this.seekHold = { target: position, at: now };
    }
  }
}

/**
 * Lyric time from the playback position (APP_API section 3):
 * lyric_time = position_s - (song.lyrics_offset_ms + device_offset_ms) / 1000.
 * A positive offset shows the lyrics later.
 */
export function lyricTime(positionS: number, songOffsetMs: number, deviceOffsetMs: number): number {
  return positionS - (songOffsetMs + deviceOffsetMs) / 1000;
}
