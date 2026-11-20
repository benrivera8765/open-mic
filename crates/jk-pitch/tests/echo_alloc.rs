// SPDX-License-Identifier: GPL-3.0-or-later
//! The echo canceller allocates nothing per block after construction (WORK_ORDER-2
//! section 5, "Real-time safety"; jk-pitch rules): a counting global allocator watches this
//! thread while 20 s of generated music, returned as a cable would return it (delay, colour,
//! clock drift), with a singer on top, go through the canceller in device-sized blocks,
//! together with every query the app makes per block (report, trust, measured delay) and a
//! reset. Covers the detector, the delay line, both adaptive filters, the delay tracking and
//! the restarts; and (Stage 3) the spectral look, the echo-path gain tracker, a return
//! switched off mid-song and a wandering Bluetooth delay.

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
    run_path(rate, block, &ReturnPath::cable(-3.0, 0.009, 100.0), None);
}

/// `off_at`: the return switched off (the mixer's loopback) at that time, s.
fn run_path(rate: u32, block: usize, path: &ReturnPath, off_at: Option<f64>) -> jk_pitch::echo::EchoReport {
    let secs = 20.0;
    let music = testpath::synth_music(rate, secs, -16.0, Some((12.0, 13.0)), 41);
    let (mut echo, _) = testpath::apply(path, &music, 2, rate);
    if let Some(t) = off_at {
        echo[(t * rate as f64) as usize..].fill(0.0);
    }
    // The Stage 2 voice holds exact steady tones; the Stage 3 runs (`off_at` given) sing
    // with vibrato, like a person: a generated voice holding exactly a tone of the generated
    // music is a filtered copy of it, which the detector rightly finds as a "return".
    let vibrato = off_at.is_some() || path.wander.is_some();
    let voice = tv::sing(rate, secs, Voice::default(), |t| {
        let f = 220.0 + 20.0 * (t / 2.0).floor();
        ((t % 1.0) < 0.7).then_some(if vibrato { tv::vibrato_hz(f * 1.03, 30.0, 5.5, t, 0.0) } else { f })
    });
    let input: Vec<f32> = echo.iter().zip(&voice).map(|(a, b)| a + b).collect();
    let mut ec = EchoCanceller::new(rate);
    let mut out = vec![0.0f32; block];
    let mut presence_seen = false;
    let mut last = None;
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
        last = Some(ec.report());
        ec.reset();
        ec.process(&music[..2 * block], &input[..block], &mut out);
    });
    assert!(presence_seen, "the return was found ({rate} Hz)");
    assert_eq!(n, 0, "{n} allocations at {rate} Hz, blocks of {block}");
    last.expect("report")
}

/// Stage 3 paths: the return switched off (path-gain test, suspension, the detector
/// forgetting the echo, "lost"), and a wandering Bluetooth delay (restarts, the gate's
/// wider echo bound).
#[test]
fn no_allocation_when_the_loopback_goes_off_or_the_delay_wanders() {
    // Switched off at 15 s (after the music's break at 12-13 s), a singer over it: the
    // gain test, the suspension and the confirmation by a fresh detector.
    let r = run_path(48_000, 480, &ReturnPath::usb(0.0, 0.010), Some(15.0));
    assert!(r.lost >= 1 && r.presence != Presence::Present, "the switched-off return was not dropped: {r:?}");
    let r = run_path(48_000, 480, &ReturnPath::bluetooth(-6.0, 0.2), None);
    assert_eq!(r.stability, jk_pitch::echo::DelayStability::Wandering, "{r:?}");
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
