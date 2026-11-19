// SPDX-License-Identifier: GPL-3.0-or-later
//! The live pitch tracker allocates nothing per block after construction (WORK_ORDER-2
//! section 5, "Real-time safety", and the tracker brief): a counting global allocator
//! watches this thread while 20 s of generated singing goes through the tracker in
//! device-sized blocks, at every supported rate and through the interleaved entry point.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use jk_pitch::tracker::Tracker;
use jk_pitch::tracker::testvoice::{self as tv, Voice};

struct Counting;

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note_alloc() {
    // try_with: the thread-locals may already be gone while a thread shuts down.
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

/// Allocations (and frees) on this thread while `f` runs.
fn count(f: impl FnOnce()) -> u64 {
    COUNT.with(|c| c.set(0));
    WATCH.with(|w| w.set(true));
    f();
    WATCH.with(|w| w.set(false));
    COUNT.with(|c| c.get())
}

fn singing(rate: u32, seconds: f64) -> Vec<f32> {
    // Notes across the range with gaps, so every path (voiced, unvoiced, onsets) runs.
    tv::sing(rate, seconds, Voice::default(), |t| {
        let k = (t / 0.5) as i32;
        (t - f64::from(k) * 0.5 < 0.4).then(|| tv::midi_hz(f64::from(36 + (k * 5) % 49)))
    })
}

#[test]
fn processing_allocates_nothing() {
    for rate in [16_000u32, 44_100, 48_000, 96_000] {
        let x = singing(rate, 20.0);
        let mut tracker = Tracker::new(rate);
        let mut voiced = 0usize;
        let mut events = 0usize;
        let n = count(|| {
            for block in x.chunks(rate as usize / 100) {
                tracker.process(block, |e| {
                    events += 1;
                    voiced += usize::from(e.hz.is_some());
                });
            }
            tracker.reset();
            for block in x.chunks(333) {
                tracker.process(block, |_| {});
            }
        });
        assert_eq!(n, 0, "{rate} Hz: {n} allocations while processing");
        assert_eq!(events, x.len() / tracker.hop_len());
        assert!(
            voiced > events / 2,
            "{rate} Hz: the test signal was not tracked"
        );
    }
}

#[test]
fn interleaved_processing_allocates_nothing() {
    let rate = 48_000;
    let voice = singing(rate, 10.0);
    let noise = tv::room_noise(rate, 10.0, -40.0, None, 3);
    let stereo = tv::interleave(&[&noise, &voice]);
    let mut tracker = Tracker::new(rate);
    let mut voiced = 0usize;
    let n = count(|| {
        for block in stereo.chunks(2 * 480) {
            tracker.process_interleaved(block, 2, 1, |e| voiced += usize::from(e.hz.is_some()));
        }
    });
    assert_eq!(n, 0, "{n} allocations while processing");
    assert!(voiced > 0);
}

/// The counter works (otherwise the tests above prove nothing).
#[test]
fn the_counter_sees_allocations() {
    let n = count(|| {
        let v: Vec<u8> = std::hint::black_box(Vec::with_capacity(64));
        drop(v);
    });
    assert!(n >= 1);
}
