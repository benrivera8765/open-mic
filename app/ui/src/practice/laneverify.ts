// SPDX-License-Identifier: GPL-3.0-or-later
// Whole-song check of the practice lane (WO2 section 9, "The screen"): at every sampled
// moment of a song, each bar in view is at the height of its note on the piano keys, each
// word is under its note, the marker is at the height of the sung pitch, and the lit piano
// key is the right one, at the key and octave given.
//
// The expectations are computed here from the guide and docs/APP_API.md 12.1 directly
// (target = midi + key + 12 x octave, lane = target - tuning / 100, name of the rounded lane
// pitch with its own name table), not from lane.ts's own helpers, so a wrong formula in the
// lane cannot agree with itself. Used by lane.test.ts and, through `window.__jkLane.verify`,
// by the screenshot script in a real browser.
import type { Lyrics, NoteSpelling, OctaveMode, PracticeGuide } from "../api/types";
import { barOf, FUTURE_S, type LaneLayout, laneFrame, pianoRows, xOf } from "./lane";
import { LiveStore } from "./live";
import type { KeyView } from "./pitch";

/** Name table of its own (not pitch.ts's), so the check is independent. */
const NAMES: Record<NoteSpelling, string[]> = {
  sharps: ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"],
  flats: ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"],
};
const nameOf = (p: number, sp: NoteSpelling) => `${NAMES[sp][((p % 12) + 12) % 12]}${Math.floor(p / 12) - 1}`;

export interface LaneScenario {
  /** Key heard throughout (steady). */
  key: number;
  /** Octave shift of the lane. */
  octave: number;
  spelling: NoteSpelling;
  octaveMode?: OctaveMode;
  /** The simulated singer's offset from the target, semitones (0 = on pitch). */
  singerOffset?: number;
  /** The simulated singer's own octave (default: the lane's). */
  singerOctave?: number;
  reduceMotion?: boolean;
  simple?: boolean;
  /** Sampling step, s (default 0.1). */
  step?: number;
}

export interface LaneVerifyReport {
  frames: number;
  barsChecked: number;
  barErrors: number;
  /** Largest distance between a bar's centre and its note's height on the keys, px. */
  worstBarPx: number;
  nameErrors: number;
  wordsChecked: number;
  wordErrors: number;
  wordOverlaps: number;
  targetChecked: number;
  targetErrors: number;
  markerChecked: number;
  markerErrors: number;
  worstMarkerPx: number;
  cueErrors: number;
  /** Frames where the note being sung was outside the bar area. */
  outOfView: number;
  /** Seconds of upcoming notes visible right of the now line. */
  lookaheadS: number;
  /** The now line's position in the bar area (0..1 from the left). */
  nowFraction: number;
  rows: number;
  tailFrames: number;
  countdownFrames: number;
  firstErrors: string[];
}

function steadyKeys(key: number, octave: number): KeyView {
  return { segment: 0, base: key, changes: [], octave, octaveFrom: octave, octaveSeenMs: -1e12 };
}

/** Checks the whole song (or [from, to]) at `L`'s size for one scenario. */
export function verifyLane(L: LaneLayout, guide: PracticeGuide, lyrics: Lyrics | null, sc: LaneScenario, from = 0, to = L.duration): LaneVerifyReport {
  const rep: LaneVerifyReport = {
    frames: 0,
    barsChecked: 0,
    barErrors: 0,
    worstBarPx: 0,
    nameErrors: 0,
    wordsChecked: 0,
    wordErrors: 0,
    wordOverlaps: 0,
    targetChecked: 0,
    targetErrors: 0,
    markerChecked: 0,
    markerErrors: 0,
    worstMarkerPx: 0,
    cueErrors: 0,
    outOfView: 0,
    lookaheadS: (L.barX + L.barW - L.nowX) / L.pxPerSec,
    nowFraction: (L.nowX - L.barX) / L.barW,
    rows: L.rows,
    tailFrames: 0,
    countdownFrames: 0,
    firstErrors: [],
  };
  const err = (msg: string) => {
    if (rep.firstErrors.length < 12) rep.firstErrors.push(msg);
  };
  const keys = steadyKeys(sc.key, sc.octave);
  const tuning = guide.tuning_cents;
  const expectedLane = (midi: number) => midi + sc.key + 12 * sc.octave - tuning / 100;
  const singerOct = sc.singerOctave ?? sc.octave;
  const offset = sc.singerOffset ?? 0;
  const step = sc.step ?? 0.1;
  const live = new LiveStore();
  const nowMs = 1e9;

  // Words: overlaps within a row, over the whole song (layout, not time dependent).
  for (let r = 0; r < L.wordRows; r++) {
    let end = -Infinity;
    for (const n of L.notes) {
      if (n.wordRow !== r) continue;
      const x = n.start * L.pxPerSec;
      if (x < end - 0.5) rep.wordOverlaps++;
      end = x + n.wordW;
    }
  }

  for (let t = from; t <= to; t += step) {
    rep.frames++;
    // The singer: the target note's pitch (or nothing between notes), 30 ms ago.
    const cur = guide.notes.findIndex((n) => n.start <= t && t < n.end);
    const due = cur >= 0 && guide.notes[cur].kind === "pitched" ? guide.notes[cur] : null;
    live.clear();
    let sung: number | null = null;
    if (due) {
      sung = due.midi + sc.key + 12 * singerOct + offset;
      live.push(t - 0.06, sung, -20);
      live.push(t - 0.05, sung, -20);
      live.push(t - 0.04, sung, -20);
      live.push(t - 0.03, sung, -20);
    }
    const f = laneFrame(L, {
      t,
      nowMs,
      keys,
      spelling: sc.spelling,
      octaveMode: sc.octaveMode ?? "auto",
      live,
      liveMs: nowMs,
      watchOnly: false,
      reduceMotion: !!sc.reduceMotion,
      simple: !!sc.simple,
    });
    const rows = pianoRows(L, f);
    const rowAt = (y: number) => rows.find((r) => y >= r.y0 - 1e-6 && y <= r.y1 + 1e-6);
    if (f.tail.length) rep.tailFrames++;
    if (f.countdown !== null) rep.countdownFrames++;

    // Bars in view: at their note's height on the keys, named right.
    for (let i = 0; i < guide.notes.length; i++) {
      const g = guide.notes[i];
      if (g.end < t - L.pastS || g.start > t + FUTURE_S) continue;
      if (g.kind !== "pitched") continue;
      const b = barOf(L, f, i);
      const want = expectedLane(g.midi);
      const p = Math.round(want);
      const inView = b.y >= L.barTop - 0.5 && b.y <= L.barBottom + 0.5;
      // Scroll mode keeps the note being sung and the next 1.5 s in view; older notes may scroll out.
      const near = g.end > t - 0.5 && g.start < t + 1.5;
      if (!inView) {
        if (L.mode === "fit" || near) {
          rep.barErrors++;
          err(`t=${t.toFixed(2)} note ${i}: bar outside the lane (y=${b.y.toFixed(1)})`);
        }
        continue;
      }
      rep.barsChecked++;
      const row = rowAt(b.y);
      if (!row) {
        rep.barErrors++;
        err(`t=${t.toFixed(2)} note ${i}: no piano key at y=${b.y.toFixed(1)}`);
        continue;
      }
      // Where the note should be: its key's row centre, moved by the note's fraction of a semitone.
      const wantY = (row.y0 + row.y1) / 2 - (want - row.p) * L.rowH;
      const d = Math.abs(b.y - wantY);
      rep.worstBarPx = Math.max(rep.worstBarPx, d);
      if (row.p !== p || d > 0.5) {
        rep.barErrors++;
        err(`t=${t.toFixed(2)} note ${i}: on key ${row.name} (${row.p}), want ${nameOf(p, sc.spelling)} (${p}), off by ${d.toFixed(2)} px`);
      }
      if (row.name !== nameOf(row.p, sc.spelling)) {
        rep.nameErrors++;
        err(`t=${t.toFixed(2)}: key ${row.p} named ${row.name}, want ${nameOf(row.p, sc.spelling)}`);
      }
      // Its word: under it, starting where it starts, the word of its reference.
      const n = L.notes[i];
      if (n.word !== null) {
        rep.wordsChecked++;
        const x = xOf(L, t, n.start);
        const ref = g.word;
        const text = ref && lyrics ? lyrics.lines[ref[0]].words[ref[1]].chars.map((c) => c[0]).join("").trim() : null;
        if (Math.abs(x - b.x0) > 0.5 || L.wordTop < L.barBottom || text !== n.word) {
          rep.wordErrors++;
          err(`t=${t.toFixed(2)} note ${i}: word "${n.word}" at x=${x.toFixed(1)} (bar ${b.x0.toFixed(1)}), want "${text}"`);
        }
      }
    }

    // The key to sing lights: the rounded lane pitch of the note due now.
    if (due) {
      rep.targetChecked++;
      const p = Math.round(expectedLane(due.midi));
      if (!f.target || f.target.row !== p || f.target.name !== nameOf(p, sc.spelling)) {
        rep.targetErrors++;
        err(`t=${t.toFixed(2)}: lit key ${f.target?.name ?? "none"}, want ${nameOf(p, sc.spelling)}`);
      }
      const b = barOf(L, f, cur);
      if (b.y < L.barTop || b.y > L.barBottom) rep.outOfView++;
    } else if (f.target && guide.notes[f.target.note].kind === "pitched") {
      rep.targetErrors++;
      err(`t=${t.toFixed(2)}: key ${f.target.name} lit between notes`);
    }

    // The marker: at the height of the sung pitch (or pinned at the edge with its arrow).
    if (sung !== null) {
      rep.markerChecked++;
      const lane = sung - tuning / 100;
      if (!f.singer) {
        rep.markerErrors++;
        err(`t=${t.toFixed(2)}: no marker while singing`);
      } else {
        const top = f.viewLow + L.rows - 1;
        const edge = lane > top + 0.5 ? 1 : lane < f.viewLow - 0.5 ? -1 : 0;
        if (edge !== f.singer.edge) {
          rep.markerErrors++;
          err(`t=${t.toFixed(2)}: marker edge ${f.singer.edge}, want ${edge}`);
        } else if (edge === 0) {
          const row = rowAt(f.singer.y);
          const wantY = row ? (row.y0 + row.y1) / 2 - (lane - row.p) * L.rowH : Number.NaN;
          const d = Math.abs(f.singer.y - wantY);
          rep.worstMarkerPx = Math.max(rep.worstMarkerPx, Number.isFinite(d) ? d : 999);
          if (!row || row.p !== Math.round(lane) || d > 0.5 || f.singer.name !== nameOf(Math.round(lane), sc.spelling)) {
            rep.markerErrors++;
            err(`t=${t.toFixed(2)}: marker on ${row?.name ?? "nothing"} (${f.singer.y.toFixed(1)}), want ${nameOf(Math.round(lane), sc.spelling)}`);
          }
        }
        // The cue: "higher" below the note, "lower" above it, nothing within half a semitone (octaves forgiven unless strict).
        let e = (singerOct - sc.octave) * 12 + offset;
        if ((sc.octaveMode ?? "auto") !== "strict") e = ((((e + 6) % 12) + 12) % 12) - 6;
        const wantCue = due && due.graded && !due.dim ? (e > 0.5 ? -1 : e < -0.5 ? 1 : 0) : 0;
        if (f.singer.cue !== wantCue) {
          rep.cueErrors++;
          err(`t=${t.toFixed(2)}: cue ${f.singer.cue}, want ${wantCue}`);
        }
      }
    } else if (f.singer) {
      rep.markerErrors++;
      err(`t=${t.toFixed(2)}: marker shown with nobody singing`);
    }
  }
  return rep;
}
