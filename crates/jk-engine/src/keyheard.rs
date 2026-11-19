// SPDX-License-Identifier: GPL-3.0-or-later
//! The key that is heard (Work Order 2: section 4.6 "Changing key during a song", trap 1).
//!
//! The key asked for is not the key that is heard: a real-time key change is heard about
//! 0.28 s after the request (one graph latency plus the render-ahead and the device
//! buffer), and in render-ahead mode only once the new key has been rendered, which can
//! take seconds. Practice mode must show and grade the key that is sounding, so the
//! engine publishes it alongside the playback clock:
//!
//! * The render worker knows, for every frame it produces, which key it is in: the graph
//!   reports each key change it schedules (`jk_dsp::PitchGraph::scheduled_switch`: the
//!   midpoint of the 40 ms equal-power crossfade, one graph latency before it is output)
//!   and the key each playback segment starts in. The worker sends these as [`KeyMark`]s
//!   through a lock-free ring (no allocation, no locks on the worker).
//! * Control (the house thread every 5 ms, and every `snapshot`) drains the ring into a
//!   [`KeyHistory`]: per playback segment (a load, seek, device switch or paused re-render
//!   starts one), the song positions where the key heard changes. With the song time at
//!   the DAC from the playback clock it gives the key sounding now, the next change and
//!   where it becomes audible, and the key that was sounding at a recent song position.
//!
//! Because the worker runs ahead of the DAC (about 150 ms plus the device buffer) and a
//! switch is scheduled one graph latency (about 0.1 s) before the worker outputs it, every
//! key change is known about a quarter of a second before it reaches the DAC. In
//! render-ahead mode a switch is scheduled only when the engine really crosses over to the
//! cached key; until then the change is pending without a position.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// A change of the key heard, as the render worker scheduled it in its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyMark {
    /// Playback segment (the engine's generation).
    pub segment: u64,
    /// Song frame from which `semitones` is heard.
    pub frame: u64,
    pub semitones: i32,
    /// The segment's first mark: the key it starts in, at its first frame.
    pub start: bool,
}

/// A key change still to be heard (`KeyStatus::pending`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyPending {
    /// The key that will be heard next.
    pub semitones: i32,
    /// Song time (s) from which it is heard: the midpoint of the 40 ms crossfade, already
    /// scheduled in the audio the engine produced. `None` while the change has no
    /// position yet: render-ahead mode is still rendering the new key (the old key plays
    /// on meanwhile), or the real-time path is about to start the crossfade (a few ms).
    pub at_s: Option<f64>,
}

/// One playback segment of the recent past, for [`crate::Engine::key_timeline`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySegment {
    /// The segment id (`KeyStatus::segment` while it plays).
    pub segment: u64,
    /// Song time (s) where the segment started playing.
    pub start_s: f64,
    /// The furthest song time seen at the DAC in this segment (where it ended, once a
    /// newer segment plays).
    pub heard_until_s: f64,
    /// `(song time s, semitones)`: the key heard from each time on, in order. The first
    /// entry is the key the segment started in. Entries past `heard_until_s` are
    /// scheduled but not heard yet.
    pub keys: Vec<(f64, i32)>,
}

/// Segments kept (each load, seek, device switch or paused re-render starts one).
const MAX_SEGMENTS: usize = 64;
/// Key changes kept per segment.
const MAX_MARKS: usize = 1024;
/// Rounding slack when comparing song times (s).
const EPS: f64 = 1e-9;
/// How far past what the DAC has played a segment still answers [`KeyHistory::key_at`]
/// without a segment id (the house thread updates `heard_until` every 5 ms).
const COVER_SLACK_S: f64 = 0.05;

struct Seg {
    segment: u64,
    start: u64,
    /// `(frame, semitones)`, frames non-decreasing, no two neighbours in the same key.
    marks: Vec<(u64, i32)>,
    heard_until: f64,
}

/// Control-side history of the key heard (allocates; never touched by audio threads).
#[derive(Default)]
pub(crate) struct KeyHistory {
    segs: VecDeque<Seg>,
    rate: f64,
    /// Marks of older segments belong to a previous song and are ignored.
    first_segment: u64,
}

impl KeyHistory {
    /// Forget everything: a new song at `rate` whose segments are `first_segment` and later.
    pub fn reset(&mut self, rate: u32, first_segment: u64) {
        self.segs.clear();
        self.rate = rate.max(1) as f64;
        self.first_segment = first_segment;
    }

    fn time(&self, frame: u64) -> f64 {
        frame as f64 / self.rate
    }

    fn find(&self, segment: u64) -> Option<&Seg> {
        self.segs.iter().rev().find(|s| s.segment == segment)
    }

    /// Add a mark from the worker.
    pub fn add(&mut self, m: KeyMark) {
        if m.segment < self.first_segment || self.rate == 0.0 {
            return;
        }
        let pos = match self.segs.iter().rposition(|s| s.segment == m.segment) {
            Some(i) => i,
            None => {
                if self.segs.back().is_some_and(|s| s.segment > m.segment) {
                    return; // a late mark of a segment already forgotten
                }
                if self.segs.len() == MAX_SEGMENTS {
                    self.segs.pop_front();
                }
                let t = self.time(m.frame);
                self.segs.push_back(Seg { segment: m.segment, start: m.frame, marks: Vec::new(), heard_until: t });
                self.segs.len() - 1
            }
        };
        let seg = &mut self.segs[pos];
        if m.start {
            // The key the segment starts in (always its first mark).
            seg.start = m.frame;
            seg.marks.clear();
        }
        let frame = m.frame.max(seg.start);
        // A mark at or before the last one replaces what it overlaps: a crossfade whose
        // target changed (a cached key that went missing turns into the bypass), or a
        // switch scheduled before the segment's first frame (it starts in the new key).
        while seg.marks.last().is_some_and(|&(f, _)| f >= frame) {
            seg.marks.pop();
        }
        if seg.marks.last().is_some_and(|&(_, k)| k == m.semitones) {
            return; // no change of key (e.g. a cached mix change at the same key)
        }
        if seg.marks.len() == MAX_MARKS {
            seg.marks.remove(1);
        }
        seg.marks.push((frame, m.semitones));
    }

    /// The DAC is at song time `pos_s` of `segment`.
    pub fn note_dac(&mut self, segment: u64, pos_s: f64) {
        if let Some(s) = self.segs.iter_mut().rev().find(|s| s.segment == segment)
            && pos_s > s.heard_until
        {
            s.heard_until = pos_s;
        }
    }

    /// Key heard at `pos_s` in `seg` (the start key before the start).
    fn key_in(&self, seg: &Seg, pos_s: f64) -> Option<i32> {
        let mut key = seg.marks.first()?.1;
        for &(f, k) in &seg.marks {
            if self.time(f) <= pos_s + EPS {
                key = k;
            } else {
                break;
            }
        }
        Some(key)
    }

    /// The key heard at `pos_s` of `segment`, if that segment has started (its first mark
    /// arrived).
    pub fn heard(&self, segment: u64, pos_s: f64) -> Option<i32> {
        self.key_in(self.find(segment)?, pos_s)
    }

    /// The first key change in `segment` after `pos_s`: (song time, key).
    pub fn next_after(&self, segment: u64, pos_s: f64) -> Option<(f64, i32)> {
        let seg = self.find(segment)?;
        seg.marks.iter().skip(1).map(|&(f, k)| (self.time(f), k)).find(|&(t, _)| t > pos_s + EPS)
    }

    /// The key last heard at the DAC: in the newest segment that has started, at the
    /// furthest position the DAC reached in it (a change scheduled there but cut off by a
    /// seek was never heard and does not count).
    pub fn last_heard(&self) -> Option<i32> {
        self.segs.iter().rev().find_map(|s| self.key_in(s, s.heard_until))
    }

    /// Which key was heard at song position `pos_s`: in `segment` if given (any position
    /// from its start on, scheduled changes included), otherwise in the newest segment
    /// that played that position.
    pub fn key_at(&self, pos_s: f64, segment: Option<u64>) -> Option<i32> {
        if !pos_s.is_finite() {
            return None;
        }
        match segment {
            Some(g) => {
                let seg = self.find(g)?;
                (pos_s + EPS >= self.time(seg.start)).then(|| self.key_in(seg, pos_s)).flatten()
            }
            None => self.segs.iter().rev().find_map(|seg| {
                let last = seg.marks.last().map_or(0.0, |m| self.time(m.0));
                let end = seg.heard_until.max(last) + COVER_SLACK_S;
                (pos_s + EPS >= self.time(seg.start) && pos_s <= end).then(|| self.key_in(seg, pos_s)).flatten()
            }),
        }
    }

    pub fn timeline(&self) -> Vec<KeySegment> {
        self.segs
            .iter()
            .map(|s| KeySegment {
                segment: s.segment,
                start_s: self.time(s.start),
                heard_until_s: s.heard_until,
                keys: s.marks.iter().map(|&(f, k)| (self.time(f), k)).collect(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(segment: u64, frame: u64, semitones: i32, start: bool) -> KeyMark {
        KeyMark { segment, frame, semitones, start }
    }

    #[test]
    fn history_answers_by_position_and_segment() {
        let mut h = KeyHistory::default();
        h.reset(1000, 5);
        h.add(mark(4, 0, 7, true)); // previous song: ignored
        h.add(mark(5, 1000, 0, true)); // starts at 1.0 s in key 0
        h.add(mark(5, 3000, -3, false)); // -3 from 3.0 s
        h.add(mark(5, 6000, 2, false)); // +2 from 6.0 s (scheduled)
        h.note_dac(5, 4.0);
        assert_eq!(h.heard(5, 0.5), Some(0)); // before the start: the start key
        assert_eq!(h.heard(5, 2.999), Some(0));
        assert_eq!(h.heard(5, 3.0), Some(-3));
        assert_eq!(h.next_after(5, 4.0), Some((6.0, 2)));
        assert_eq!(h.next_after(5, 6.0), None);
        assert_eq!(h.last_heard(), Some(-3), "the +2 scheduled at 6.0 s was not heard yet");
        // A seek back to 2.0 s starts segment 6 in key +2.
        h.add(mark(6, 2000, 2, true));
        h.note_dac(6, 2.5);
        assert_eq!(h.key_at(3.5, None), Some(-3), "3.5 s was last heard in segment 5");
        assert_eq!(h.key_at(2.4, None), Some(2), "2.4 s was heard again in segment 6");
        assert_eq!(h.key_at(3.5, Some(6)), Some(2));
        assert_eq!(h.key_at(1.5, Some(6)), None, "before segment 6 started");
        assert_eq!(h.key_at(0.5, None), None, "never played");
        assert_eq!(h.key_at(9.0, None), None, "not played yet");
        assert_eq!(h.last_heard(), Some(2));
    }

    #[test]
    fn a_retargeted_switch_replaces_the_scheduled_one() {
        let mut h = KeyHistory::default();
        h.reset(1000, 1);
        h.add(mark(1, 0, 0, true));
        h.add(mark(1, 2000, 4, false));
        h.add(mark(1, 2000, 0, false)); // the cached key went missing: bypass instead
        assert_eq!(h.next_after(1, 1.0), None);
        assert_eq!(h.heard(1, 2.5), Some(0));
        // A switch scheduled before the segment's first frame: it starts in that key.
        h.add(mark(2, 5000, 0, true));
        h.add(mark(2, 4900, -5, false));
        assert_eq!(h.heard(2, 5.0), Some(-5));
        assert_eq!(h.timeline().last().unwrap().keys, vec![(5.0, -5)]);
        // A switch to the key already heard (a mix change in render-ahead mode) is no change.
        h.add(mark(2, 7000, -5, false));
        assert_eq!(h.next_after(2, 5.0), None);
    }
}
