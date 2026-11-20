// SPDX-License-Identifier: GPL-3.0-or-later
//! The practice analysis chain allocates nothing per block once it runs (docs/ARCHITECTURE.md
//! 12.4: the analysis thread "allocates only at session start and on a device change"): a
//! counting global allocator watches this thread while 20 s of a generated singer plus
//! generated music returned by a USB loopback go through the whole chain (channel pick,
//! echo canceller, tracker, gate, clocks, scorer, live batches drained every block), after a
//! two-second warm-up in which the live batch buffers settle.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use jk_pitch::echo::testpath::{self, ReturnPath};
use jk_pitch::practice::{Chain, ChainConfig, ClockAnchor, PracticeLive};
use jk_pitch::score::{KeyTimeline, LineBasis, ScoreGuide, ScoreNote};
use jk_pitch::tracker::testvoice::{self as tv, Voice};

struct Counting;

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note_alloc() {
    let _ = WATCH.try_with(|w| {
        if w.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_alloc();
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn the_chain_allocates_nothing_per_block() {
    let rate = 48_000u32;
    let secs = 22.0;
    // Notes every 0.5 s (0.4 s long) around C3..C4.
    let notes: Vec<ScoreNote> = (0..40)
        .map(|i| {
            let start = 1.0 + 0.5 * i as f64;
            ScoreNote { start, end: start + 0.4, midi: 48.0 + f64::from((i * 5) % 12), pitched: true, confidence: 0.9, octave_flagged: false, glide_in: 0.0, line: None }
        })
        .collect();
    let lines = ScoreGuide::phrases(&notes);
    let guide = ScoreGuide { notes: notes.clone(), low_confidence: 0.5, lines, basis: LineBasis::Phrases, outdated: false, duration_s: secs, label: "t".into() };
    let voice = tv::sing(rate, secs, Voice::default(), |t| notes.iter().find(|n| t >= n.start && t < n.end).map(|n| tv::midi_hz(n.midi)));
    let music = testpath::synth_music(rate, secs, -24.0, None, 3);
    let (ret, _) = testpath::apply(&ReturnPath::usb(-6.0, 0.010), &music, 2, rate);
    let input: Vec<f32> = voice.iter().zip(&ret).map(|(a, b)| a + b).collect();
    let mut chain = Chain::new(ChainConfig::new(rate, 1, 0), guide, KeyTimeline::constant(0), Vec::new());
    let mut live = PracticeLive::default();
    let block = 480usize;
    let warm = 2 * rate as usize;
    let mut allocs = 0u64;
    let mut shown = 0usize;
    let mut f = 0usize;
    while f + block <= input.len() {
        let host = 1_000_000_000 + (f as i64 * 1_000_000_000) / i64::from(rate);
        let mut step = || {
            chain.playback_anchor(ClockAnchor { host_ns: host, song_s: f as f64 / f64::from(rate), segment: 0, playing: true });
            chain.process(host, &input[f..f + block], &music[2 * f..2 * (f + block)]);
            chain.drain_live(&mut live);
            shown += live.pitch.iter().filter(|p| p.1.is_some()).count();
        };
        if f >= warm {
            COUNT.with(|c| c.set(0));
            WATCH.with(|w| w.set(true));
            step();
            WATCH.with(|w| w.set(false));
            allocs += COUNT.with(|c| c.get());
        } else {
            step();
        }
        f += block;
    }
    assert!(shown > 500, "the singer was tracked ({shown} values shown)");
    assert_eq!(allocs, 0, "allocations per block after the warm-up");
    let r = chain.finish(true, &[]);
    // (How much of it could be graded with the music coming back is measured in
    // docs/benchmarks/practice-score.md; here only that the take ends cleanly.)
    assert_eq!(r.total, 40, "{r:?}");
}
