// SPDX-License-Identifier: GPL-3.0-or-later
//! The echo canceller allocates nothing per block after construction (WORK_ORDER-2
//! section 5, "Real-time safety"; jk-pitch rules): a counting global allocator watches this
//! thread while 20 s of generated music, returned as a cable would return it (delay, colour,
//! clock drift), with a singer on top, go through the canceller in device-sized blocks,
//! together with every query the app makes per block (report, trust, measured delay) and a
//! reset. Covers the detector, the delay line, both adaptive filters, the delay tracking and
//! the restarts.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use jk_pitch::echo::testpath::{self, ReturnPath};
use jk_pitch::echo::{EchoCanceller, Presence};
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

fn run(rate: u32, block: usize) {
    let secs = 20.0;
    let music = testpath::synth_music(rate, secs, -16.0, Some((12.0, 13.0)), 41);
    let (echo, _) = testpath::apply(&ReturnPath::cable(-3.0, 0.009, 100.0), &music, 2, rate);
    let voice = tv::sing(rate, secs, Voice::default(), |t| {
        ((t % 1.0) < 0.7).then_some(220.0 + 20.0 * (t / 2.0).floor())
    });
    let input: Vec<f32> = echo.iter().zip(&voice).map(|(a, b)| a + b).collect();
    let mut ec = EchoCanceller::new(rate);
    let mut out = vec![0.0f32; block];
    let mut presence_seen = false;
    let n = count(|| {
        let mut i = 0;
        while i + block <= input.len() {
            ec.process(
                &music[2 * i..2 * (i + block)],
                &input[i..i + block],
                &mut out,
            );
            let r = ec.report();
            presence_seen |= r.presence == Presence::Present;
            let t = i as i64;
            let _ = ec.trust_window(t - 2000, t - 1000);
            let _ = ec.measured_delay_s();
            let _ = ec.measured_suppression_db();
            i += block;
        }
        ec.reset();
        ec.process(&music[..2 * block], &input[..block], &mut out);
    });
    assert!(presence_seen, "the return was found ({rate} Hz)");
    assert_eq!(n, 0, "{n} allocations at {rate} Hz, blocks of {block}");
}

#[test]
fn no_allocation_per_block_48k() {
    run(48_000, 480);
}

#[test]
fn no_allocation_per_block_44k1_odd_blocks() {
    run(44_100, 441);
}

#[test]
fn no_allocation_per_block_16k_and_96k() {
    run(16_000, 160);
    run(96_000, 1024);
}
