// SPDX-License-Identifier: GPL-3.0-or-later
//! The practice-mode capture side never allocates (WO2 section 5: "The
//! microphone callback only copies samples and a time stamp into a lock-free
//! buffer").
//!
//! A counting global allocator attributes every allocation and free to the
//! thread that made it, by name. Counted, from the steady state on:
//!
//! * `jk-input-signal`: the generated source's delivery thread, which plays
//!   the part of a capture callback (`CaptureProducer::push_interleaved`, the
//!   host clock, real-time pacing) through `InputStream`, with the reader
//!   draining the rings and metering on the test thread meanwhile;
//! * `jk-input-convert`: a thread that hands over 16-bit integer blocks with
//!   `CaptureProducer::push_iter`, the path the WASAPI and Core Audio
//!   callbacks use for integer formats;
//! * `control`: a thread that allocates on purpose, proving the counters see
//!   named threads (positive control).
//!
//! The real device callbacks (WASAPI thread, cpal callback) call only these
//! producer methods plus the OS read; they cannot run in CI without a
//! microphone, so they are covered by construction and by this test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use jk_recorder::input::ring::capture_ring;
use jk_recorder::input::signal::{OtherChannel, VoiceTone, place_voice};
use jk_recorder::input::{InputStream, SignalSource, clock};

const UNKNOWN: u8 = 0;
const BUSY: u8 = 1;
const OTHER: u8 = 2;
const WATCHED: u8 = 3;
const CONTROL: u8 = 4;

static ARMED: AtomicBool = AtomicBool::new(false);
static WATCHED_ALLOCS: AtomicU64 = AtomicU64::new(0);
static WATCHED_FREES: AtomicU64 = AtomicU64::new(0);
static CONTROL_ALLOCS: AtomicU64 = AtomicU64::new(0);
static CONVERT_DONE: AtomicBool = AtomicBool::new(false);
static CONVERT_READY: AtomicBool = AtomicBool::new(false);

thread_local! {
    static KIND: Cell<u8> = const { Cell::new(UNKNOWN) };
}

/// The calling thread's kind, classified on first use; allocations made while
/// classifying see BUSY and are not counted.
fn kind() -> u8 {
    KIND.try_with(|k| {
        let v = k.get();
        if v != UNKNOWN {
            return v;
        }
        k.set(BUSY);
        let c = match std::thread::current().name() {
            Some(n) if n.starts_with("jk-input-") => WATCHED,
            Some("control") => CONTROL,
            _ => OTHER,
        };
        k.set(c);
        c
    })
    .unwrap_or(OTHER)
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Relaxed) {
            match kind() {
                WATCHED => {
                    WATCHED_ALLOCS.fetch_add(1, Relaxed);
                }
                CONTROL => {
                    CONTROL_ALLOCS.fetch_add(1, Relaxed);
                }
                _ => {}
            }
        }
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Relaxed) && kind() == WATCHED {
            WATCHED_FREES.fetch_add(1, Relaxed);
        }
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn capture_side_never_allocates() {
    // --- 1. A generated "voice on channel 2 of 2" at 48 kHz, delivered in real
    // time through InputStream while the test thread reads and meters.
    let rate = 48_000;
    let voice = VoiceTone::default_a3().render(rate, 3.0);
    let x = place_voice(&voice, rate, 2, 1, OtherChannel::Noise { rms_dbfs: -70.0 });
    let mut stream = InputStream::open(Box::new(SignalSource::new("rt-alloc", rate, 2, x)))
        .expect("open the generated source");
    let mut buf = Vec::with_capacity(4096);
    let read_for = |stream: &mut InputStream, buf: &mut Vec<f32>, d: Duration| {
        let t = Instant::now();
        let mut blocks = 0u32;
        while t.elapsed() < d {
            while stream.read(buf).is_some() {
                blocks += 1;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        blocks
    };
    // Warm-up: the source thread's first block, its thread start-up.
    let warm = read_for(&mut stream, &mut buf, Duration::from_millis(300));
    ARMED.store(true, Relaxed);
    let counted = read_for(&mut stream, &mut buf, Duration::from_millis(1500));
    ARMED.store(false, Relaxed);
    let delivery_allocs = WATCHED_ALLOCS.load(Relaxed);
    let delivery_frees = WATCHED_FREES.load(Relaxed);
    stream.stop();
    println!(
        "generated source, real time, 48 kHz stereo: {warm} warm-up blocks, {counted} blocks counted; \
         delivery thread: {delivery_allocs} allocations, {delivery_frees} frees"
    );
    assert!(counted >= 120, "too few blocks counted: {counted}");
    assert_eq!((delivery_allocs, delivery_frees), (0, 0));

    // --- 2. Integer conversion straight into the ring (push_iter).
    let (mut producer, mut consumer, _status) = capture_ring(2, 48_000, 512);
    let pcm: Vec<i16> = (0..960)
        .map(|i| ((i * 37) % 65_536 - 32_768) as i16)
        .collect();
    let t = std::thread::Builder::new()
        .name("jk-input-convert".into())
        .spawn(move || {
            // Warm-up block (thread-local set-up), then the counted ones.
            producer.push_iter(480, pcm.iter().map(|&s| f32::from(s) / 32_768.0), 0, 0);
            CONVERT_READY.store(true, Relaxed);
            while !ARMED.load(Relaxed) {
                std::thread::yield_now();
            }
            let mut pushed = 0u32;
            for _ in 0..200 {
                let host = clock::host_time_ns();
                if producer.push_iter(480, pcm.iter().map(|&s| f32::from(s) / 32_768.0), host, 0) {
                    pushed += 1;
                }
            }
            // Stay alive until the counters are read: a thread's exit frees its
            // captures and thread-locals, which is not the callback's work.
            CONVERT_DONE.store(true, Relaxed);
            while ARMED.load(Relaxed) {
                std::thread::yield_now();
            }
            pushed
        })
        .unwrap();
    // Arm only once the thread has started and handed over its warm-up block.
    while !CONVERT_READY.load(Relaxed) {
        std::thread::yield_now();
    }
    let before = (WATCHED_ALLOCS.load(Relaxed), WATCHED_FREES.load(Relaxed));
    ARMED.store(true, Relaxed);
    // Drain while it pushes, so the ring never fills (reader on this thread).
    let mut out = Vec::with_capacity(4096);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !CONVERT_DONE.load(Relaxed) && Instant::now() < deadline {
        while consumer.pop_block(&mut out).is_some() {}
    }
    let after = (WATCHED_ALLOCS.load(Relaxed), WATCHED_FREES.load(Relaxed));
    ARMED.store(false, Relaxed);
    let pushed = t.join().unwrap();
    println!(
        "push_iter (i16 -> f32): {pushed} blocks; {} allocations, {} frees",
        after.0 - before.0,
        after.1 - before.1
    );
    assert_eq!(pushed, 200);
    assert_eq!(after, before);

    // --- 3. Positive control: the counters see a named thread that allocates.
    ARMED.store(true, Relaxed);
    std::thread::Builder::new()
        .name("control".into())
        .spawn(|| {
            let v: Vec<u8> = Vec::with_capacity(1024);
            std::hint::black_box(v);
        })
        .unwrap()
        .join()
        .unwrap();
    ARMED.store(false, Relaxed);
    let control = CONTROL_ALLOCS.load(Relaxed);
    println!("positive control: {control} allocations seen on the control thread");
    assert!(control >= 1);
}
