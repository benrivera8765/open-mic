// SPDX-License-Identifier: GPL-3.0-or-later
//! The real-time path never allocates or frees (WORK_ORDER trap 11: "No allocation or
//! locks in the real-time path").
//!
//! A counting global allocator sees every Rust allocation, and on Windows with the GNU
//! toolchain the import slots of the C runtime's `malloc`, `calloc`, `realloc`, `free`,
//! `_aligned_malloc` and `_aligned_free` are redirected through counters too, so the C++
//! of Rubber Band and libstdc++'s `operator new` / `delete` are counted as well. Each
//! allocation is attributed to the thread that made it, by thread name:
//!
//! - `jk-render`: the render worker (graph, Rubber Band, resampler, cache feeds);
//! - `jk-null-out` / `jk-wasapi-out`: the device thread, whose loop is the device
//!   callback (`Filler::fill`) plus the device API calls;
//! - `jk-engine-house`: the house-keeping thread, which is *expected* to allocate (it
//!   builds Rubber Band instances and frees what the worker hands back). It is the
//!   positive control that the counters see engine threads, Rust and C++ alike.
//!
//! The test plays a generated song on the null device at 48 kHz (so the 44.1 -> 48 kHz
//! output resampler runs too) and counts, per phase, from the steady state on: plain
//! play, key changes, seeks (also at a shifted key), mixer and master changes, pause
//! and resume, a song change while playing; then the prerender key mode (cache feeds).
//! The output tap (practice mode's echo reference, src/tap.rs) is attached and read by an
//! analysis thread for one phase (with a seek and master changes) and then detached.
//! Every phase must show 0 allocations and 0 frees on the worker and on the device
//! thread. The start of each thread (its own buffers, thread priority name) is outside
//! the counted window: that is set-up, not the real-time loop.
//!
//! Lock waits are not measurable this way. The code paths hold no lock by design (the
//! worker and the callback use only atomics and SPSC rings); the test prints the longest
//! device callback and worker block, which a lock wait on the control thread would
//! stretch.
//!
//! With `JK_RT_ALLOC_REAL_DEVICE=1` the same phases also run on the default output
//! device at master 0 (digital silence). With `JK_RT_ALLOC_BACKTRACE=1` the first
//! offending allocation of each thread is printed with a backtrace.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use jk_engine::{Engine, EngineConfig, KeyMode, MixParams, PlaybackState};

// ------------------------------------------------------------------ the counters

const UNKNOWN: u8 = 0;
const BUSY: u8 = 1;
const OTHER: u8 = 2;
const WORKER: u8 = 3;
const DEVICE: u8 = 4;
const HOUSE: u8 = 5;
const KINDS: [(u8, &str); 3] = [(WORKER, "worker"), (DEVICE, "device"), (HOUSE, "house")];

// [kind - WORKER][op]: 0 Rust alloc, 1 Rust free, 2 C alloc, 3 C free
const RUST_ALLOC: usize = 0;
const RUST_FREE: usize = 1;
const C_ALLOC: usize = 2;
const C_FREE: usize = 3;
#[allow(clippy::declare_interior_mutable_const)]
const Z: AtomicU64 = AtomicU64::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ROW: [AtomicU64; 4] = [Z; 4];
static COUNTS: [[AtomicU64; 4]; 3] = [ROW; 3];
static SEEN: [AtomicBool; 3] = [AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false)];
static FIRST_BAD_SIZE: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];
static BACKTRACE_ON: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static BACKTRACES: Mutex<Vec<String>> = Mutex::new(Vec::new());

thread_local! {
    static KIND: Cell<u8> = const { Cell::new(UNKNOWN) };
}

/// The calling thread's kind; classifies it on its first allocation. Nested allocations
/// made while classifying (or while recording a backtrace) see BUSY and are not counted.
fn kind_for_alloc() -> u8 {
    KIND.try_with(|k| {
        let v = k.get();
        if v != UNKNOWN {
            return v;
        }
        k.set(BUSY);
        let kind = match std::thread::current().name() {
            Some("jk-render") => WORKER,
            Some("jk-null-out") | Some("jk-wasapi-out") => DEVICE,
            Some("jk-engine-house") => HOUSE,
            _ => OTHER,
        };
        if kind >= WORKER {
            SEEN[(kind - WORKER) as usize].store(true, Relaxed);
        }
        k.set(kind);
        kind
    })
    .unwrap_or(OTHER)
}

/// Frees never classify (a thread being torn down must not call `thread::current`).
fn kind_for_free() -> u8 {
    KIND.try_with(|k| k.get()).unwrap_or(OTHER)
}

fn note(kind: u8, op: usize, size: usize) {
    if kind < WORKER {
        return;
    }
    let i = (kind - WORKER) as usize;
    COUNTS[i][op].fetch_add(1, Relaxed);
    if kind != HOUSE && ARMED.load(Relaxed) {
        let _ = FIRST_BAD_SIZE[i].compare_exchange(0, size.max(1), Relaxed, Relaxed);
        if BACKTRACE_ON.load(Relaxed) {
            // Record one backtrace per thread kind; allocations made while recording
            // pass through uncounted.
            let _ = KIND.try_with(|k| {
                k.set(BUSY);
                if let Ok(mut v) = BACKTRACES.try_lock()
                    && v.len() < 8
                {
                    let op_name = ["Rust alloc", "Rust free", "C alloc", "C free"][op];
                    v.push(format!("{op_name} of {size} bytes on {}:\n{}", ["worker", "device"][i], std::backtrace::Backtrace::force_capture()));
                }
                k.set(kind);
            });
        }
    }
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(kind_for_alloc(), RUST_ALLOC, l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        note(kind_for_alloc(), RUST_ALLOC, l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        note(kind_for_free(), RUST_FREE, l.size());
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        note(kind_for_alloc(), RUST_ALLOC, n);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

// ------------------------------------------------------------------ C runtime hooks

/// Windows + GNU toolchain: the executable's import slots for the CRT allocation
/// functions are pointed at counting wrappers (Rubber Band calls `_aligned_malloc`
/// through `__imp__aligned_malloc`; libstdc++'s `operator new` calls `malloc`).
#[cfg(all(windows, target_env = "gnu"))]
mod crt {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    type F1 = unsafe extern "C" fn(usize) -> *mut c_void;
    type F2 = unsafe extern "C" fn(usize, usize) -> *mut c_void;
    type Re = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
    type Fr = unsafe extern "C" fn(*mut c_void);

    unsafe extern "C" {
        #[link_name = "__imp_malloc"]
        static mut IMP_MALLOC: F1;
        #[link_name = "__imp_calloc"]
        static mut IMP_CALLOC: F2;
        #[link_name = "__imp_realloc"]
        static mut IMP_REALLOC: Re;
        #[link_name = "__imp_free"]
        static mut IMP_FREE: Fr;
        #[link_name = "__imp__aligned_malloc"]
        static mut IMP_ALIGNED_MALLOC: F2;
        #[link_name = "__imp__aligned_free"]
        static mut IMP_ALIGNED_FREE: Fr;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old: *mut u32) -> i32;
    }

    static REAL: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];

    unsafe extern "C" fn malloc_w(n: usize) -> *mut c_void {
        super::note(super::kind_for_alloc(), super::C_ALLOC, n);
        let f: F1 = unsafe { std::mem::transmute(REAL[0].load(Relaxed)) };
        unsafe { f(n) }
    }
    unsafe extern "C" fn calloc_w(a: usize, b: usize) -> *mut c_void {
        super::note(super::kind_for_alloc(), super::C_ALLOC, a.saturating_mul(b));
        let f: F2 = unsafe { std::mem::transmute(REAL[1].load(Relaxed)) };
        unsafe { f(a, b) }
    }
    unsafe extern "C" fn realloc_w(p: *mut c_void, n: usize) -> *mut c_void {
        super::note(super::kind_for_alloc(), super::C_ALLOC, n);
        let f: Re = unsafe { std::mem::transmute(REAL[2].load(Relaxed)) };
        unsafe { f(p, n) }
    }
    unsafe extern "C" fn free_w(p: *mut c_void) {
        if !p.is_null() {
            super::note(super::kind_for_free(), super::C_FREE, 0);
        }
        let f: Fr = unsafe { std::mem::transmute(REAL[3].load(Relaxed)) };
        unsafe { f(p) }
    }
    unsafe extern "C" fn aligned_malloc_w(n: usize, a: usize) -> *mut c_void {
        super::note(super::kind_for_alloc(), super::C_ALLOC, n);
        let f: F2 = unsafe { std::mem::transmute(REAL[4].load(Relaxed)) };
        unsafe { f(n, a) }
    }
    unsafe extern "C" fn aligned_free_w(p: *mut c_void) {
        if !p.is_null() {
            super::note(super::kind_for_free(), super::C_FREE, 0);
        }
        let f: Fr = unsafe { std::mem::transmute(REAL[5].load(Relaxed)) };
        unsafe { f(p) }
    }

    /// Swap one import slot; the real function is stored before the slot points at the
    /// wrapper, so a call racing with the swap always finds it.
    unsafe fn swap(slot: *mut usize, real: &AtomicUsize, wrapper: usize) {
        let mut old = 0u32;
        unsafe {
            assert!(VirtualProtect(slot.cast(), 8, 0x04 /* PAGE_READWRITE */, &mut old) != 0, "VirtualProtect");
            real.store(slot.read_volatile(), Relaxed);
            slot.write_volatile(wrapper);
            VirtualProtect(slot.cast(), 8, old, &mut old);
        }
    }

    pub fn install() -> bool {
        unsafe {
            swap((&raw mut IMP_MALLOC).cast(), &REAL[0], malloc_w as F1 as usize);
            swap((&raw mut IMP_CALLOC).cast(), &REAL[1], calloc_w as F2 as usize);
            swap((&raw mut IMP_REALLOC).cast(), &REAL[2], realloc_w as Re as usize);
            swap((&raw mut IMP_FREE).cast(), &REAL[3], free_w as Fr as usize);
            swap((&raw mut IMP_ALIGNED_MALLOC).cast(), &REAL[4], aligned_malloc_w as F2 as usize);
            swap((&raw mut IMP_ALIGNED_FREE).cast(), &REAL[5], aligned_free_w as Fr as usize);
        }
        true
    }
}

#[cfg(not(all(windows, target_env = "gnu")))]
mod crt {
    pub fn install() -> bool {
        false
    }
}

// ------------------------------------------------------------------ helpers

type Counts = [[u64; 4]; 3];

fn counts() -> Counts {
    std::array::from_fn(|k| std::array::from_fn(|o| COUNTS[k][o].load(Relaxed)))
}

fn delta(a: &Counts, b: &Counts) -> Counts {
    std::array::from_fn(|k| std::array::from_fn(|o| b[k][o] - a[k][o]))
}

fn tmp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-rt-alloc").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_for(secs: f64, what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed().as_secs_f64() < secs, "timed out after {secs} s waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

struct Phases {
    rows: Vec<(String, Counts)>,
    last: Counts,
}

impl Phases {
    fn start() -> Phases {
        ARMED.store(true, Relaxed);
        Phases { rows: Vec::new(), last: counts() }
    }
    fn end(&mut self, name: &str) {
        let now = counts();
        self.rows.push((name.to_string(), delta(&self.last, &now)));
        self.last = now;
    }
    fn finish(self, label: &str, engine: &Engine) -> Vec<(String, Counts)> {
        ARMED.store(false, Relaxed);
        let d = engine.diagnostics();
        println!("== {label}: device {:?}, {} callbacks, underruns {}, device starved {}, min lead {:?} ms, longest callback {:.1} us, longest worker block {:.2} ms, Rubber Band built on the render thread {}, cache misses {}",
            d.device, d.callbacks, d.underruns, d.device_starved, d.min_lead_ms.map(|x| (x * 10.0).round() / 10.0), d.max_callback_us, d.worker_max_block_ms, d.inline_constructions, d.cache_misses);
        println!("   {:<34} {:>22} {:>22} {:>22}", "phase", "worker R+/R-/C+/C-", "device R+/R-/C+/C-", "house R+/R-/C+/C-");
        for (name, c) in &self.rows {
            let f = |k: usize| format!("{}/{}/{}/{}", c[k][0], c[k][1], c[k][2], c[k][3]);
            println!("   {:<34} {:>22} {:>22} {:>22}", name, f(0), f(1), f(2));
        }
        self.rows
    }
}

fn params(semitones: i32) -> MixParams {
    MixParams { semitones, ..MixParams::default() }
}

/// Play, key changes, seeks, mixer, pause, song change, all counted from the steady state.
fn run_realtime_phases(engine: &Engine, song_a: &PathBuf, song_b: &PathBuf) -> Vec<(String, Counts)> {
    engine.load(song_a, params(0)).unwrap();
    engine.play();
    wait_for(10.0, "playing", || engine.snapshot().state == PlaybackState::Playing && engine.snapshot().position_s > 0.3);
    sleep_ms(500);

    let mut ph = Phases::start();
    sleep_ms(2000);
    ph.end("play, key 0 (2 s)");

    // Output tap (practice mode, src/tap.rs): attach a reader (the first call allocates the
    // ring, here on the control thread), read it on an analysis thread while the song
    // plays, seeks and the master volume changes (at or under -60 dB, for the real-device
    // run); then detach. The device callback copies into the tap only while attached.
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let tap_reader = {
        let mut r = engine.output_tap();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("jk-practice-test".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; 2 * 1024];
                let mut frames = 0u64;
                while !stop.load(Relaxed) {
                    while let Some(p) = r.read(&mut buf) {
                        frames += p.frames as u64;
                    }
                    sleep_ms(5);
                }
                (frames, r.lost())
            })
            .unwrap()
    };
    sleep_ms(1000);
    engine.seek(8.0);
    sleep_ms(400);
    let m = if engine.snapshot().device.as_ref().is_some_and(|d| d.name.contains("null device")) { 0.3 } else { 0.001 };
    engine.set_master(m);
    sleep_ms(200);
    engine.set_master(0.0);
    sleep_ms(400);
    ph.end("output tap read (2 s, seek, 2 master)");
    stop.store(true, Relaxed);
    let (tap_frames, tap_lost) = tap_reader.join().unwrap();
    println!("   output tap: {tap_frames} frames read on the analysis thread, {tap_lost} lost");
    assert!(tap_frames > 20_000 && tap_lost == 0, "output tap: {tap_frames} frames read, {tap_lost} lost");
    sleep_ms(300);
    ph.end("output tap detached (0.3 s)");

    let mut p = params(0);
    // Every key from -12 to +12 (each pitch ratio sets up Rubber Band's resampler for
    // its own rational ratio), ending on +3.
    for k in [2, -5, 7, 0, -12, 12, -1, 1, -2, -3, 4, -4, 5, 6, -6, -7, 8, -8, 9, -9, 10, -10, 11, -11, 3] {
        p.semitones = k;
        engine.set_params(p);
        wait_for(10.0, "key heard", || engine.diagnostics().audible_semitones == k);
        sleep_ms(250);
    }
    ph.end("25 key changes (every key -12..+12)");

    for s in [12.0, 3.5, 25.0, 0.5] {
        engine.seek(s);
        sleep_ms(400);
    }
    ph.end("4 seeks at key +3");

    let steps: [fn(&mut MixParams); 8] = [
        |p| p.guide_vocal = 0.6,
        |p| p.backing_on = false,
        |p| {
            p.backing_on = true;
            p.backing_level = 0.4
        },
        |p| p.drums = 0.3,
        |p| p.other = 0.8,
        |p| p.shift_drums = true,
        |p| p.shift_drums = false,
        |p| *p = MixParams { semitones: p.semitones, ..MixParams::default() },
    ];
    for f in steps {
        f(&mut p);
        engine.set_params(p);
        sleep_ms(200);
    }
    for m in [0.2, 0.0, 1.0] {
        engine.set_master(m);
        sleep_ms(150);
    }
    ph.end("8 mixer + 3 master changes");

    engine.pause();
    sleep_ms(300);
    engine.play();
    sleep_ms(500);
    ph.end("pause + resume");

    engine.load(song_b, params(-2)).unwrap();
    engine.play();
    wait_for(10.0, "second song playing", || engine.snapshot().state == PlaybackState::Playing && engine.snapshot().position_s > 0.3);
    sleep_ms(500);
    ph.end("song change while playing (key -2)");

    ph.finish("real-time key mode", engine)
}

/// Prerender key mode: the worker plays from the key cache through lock-free feeds.
fn run_prerender_phases(engine: &Engine, song: &PathBuf, cache: &PathBuf) -> Vec<(String, Counts)> {
    engine.set_key_cache(Some(cache), 1 << 30).unwrap();
    engine.set_key_mode(KeyMode::Prerender);
    engine.load(song, params(0)).unwrap();
    engine.play();
    wait_for(10.0, "playing", || engine.snapshot().state == PlaybackState::Playing && engine.snapshot().position_s > 0.3);
    sleep_ms(500);

    let mut ph = Phases::start();
    let mut p = params(4);
    engine.set_params(p);
    wait_for(120.0, "cached key +4 heard", || engine.snapshot().key.preparing.is_none() && engine.diagnostics().audible_semitones == 4);
    sleep_ms(1000);
    ph.end("key +4 prepared, then cached play");

    for s in [6.0, 1.0, 12.0] {
        engine.seek(s);
        sleep_ms(500);
    }
    ph.end("3 seeks on the cached key");

    p.guide_vocal = 0.5;
    engine.set_params(p);
    let t = Instant::now();
    while engine.snapshot().key.preparing.is_none() && t.elapsed() < Duration::from_millis(500) {
        sleep_ms(5);
    }
    wait_for(120.0, "new mix prepared", || engine.snapshot().key.preparing.is_none());
    sleep_ms(800);
    p.drums = 0.4; // stays live in prerender mode
    engine.set_params(p);
    sleep_ms(400);
    ph.end("mix change (re-render) + live drum level");

    p.semitones = 0;
    engine.set_params(p);
    wait_for(10.0, "key 0", || engine.diagnostics().audible_semitones == 0);
    sleep_ms(500);
    ph.end("back to key 0 (bypass)");
    ph.finish("prerender key mode", engine)
}

fn check(label: &str, rows: &[(String, Counts)]) {
    for (name, c) in rows {
        for (k, who) in [(0usize, "render worker"), (1, "device thread")] {
            assert_eq!(
                c[k],
                [0, 0, 0, 0],
                "{label} / {name}: the {who} allocated or freed (Rust alloc, Rust free, C alloc, C free = {:?}; first size {} bytes). Re-run with JK_RT_ALLOC_BACKTRACE=1 for a backtrace.",
                c[k],
                FIRST_BAD_SIZE[k].load(Relaxed)
            );
        }
    }
}

#[test]
fn the_real_time_path_never_allocates_or_frees() {
    let c_hooked = crt::install();
    if std::env::var_os("JK_RT_ALLOC_BACKTRACE").is_some() {
        BACKTRACE_ON.store(true, Relaxed);
    }
    let dir = tmp_dir("songs");
    let song_a = dir.join("a.jksong");
    let song_b = dir.join("b.jksong");
    jk_engine::demo::make_synth_song(&song_a, 30.0, 44_100, 7).unwrap();
    jk_engine::demo::make_synth_song(&song_b, 20.0, 44_100, 11).unwrap();

    // Positive control 1: the counters see the device thread. The capture engine's null
    // device appends every buffer to a Vec on that thread, which must be counted.
    {
        let (engine, _cap) = Engine::new_with_capture(48_000, false, 0.0).unwrap();
        engine.load(&song_a, params(0)).unwrap();
        engine.play();
        let before = counts();
        let t = Instant::now();
        let mut d = delta(&before, &counts());
        while d[1][RUST_ALLOC] == 0 && t.elapsed() < Duration::from_secs(5) {
            sleep_ms(50);
            d = delta(&before, &counts());
        }
        assert!(d[1][RUST_ALLOC] > 0, "positive control: the capture Vec growth on the device thread was not counted ({d:?})");
        println!("positive control (capture Vec on the device thread): {} Rust allocations counted in {:.2} s", d[1][RUST_ALLOC], t.elapsed().as_secs_f64());
    }

    // The real test: no capture, null device at 48 kHz (44.1 -> 48 kHz resampling).
    let mut all = Vec::new();
    {
        let engine = Engine::new(EngineConfig { device_id: Some("null:48000".into()), exclusive: false, master: 0.0 }).unwrap();
        let rows = run_realtime_phases(&engine, &song_a, &song_b);
        all.push(("null:48000, real time".to_string(), rows));
    }
    {
        let engine = Engine::new(EngineConfig { device_id: Some("null:44100".into()), exclusive: false, master: 0.0 }).unwrap();
        // The 20 s song: a slow (debug) build renders it to the end within the timeouts.
        let rows = run_prerender_phases(&engine, &song_b, &tmp_dir("cache"));
        all.push(("null:44100, prerender".to_string(), rows));
    }
    if std::env::var_os("JK_RT_ALLOC_REAL_DEVICE").is_some() {
        // Master 0: the device plays digital silence.
        let engine = Engine::new(EngineConfig { device_id: None, exclusive: false, master: 0.0 }).unwrap();
        let rows = run_realtime_phases(&engine, &song_a, &song_b);
        all.push(("default device (master 0), real time".to_string(), rows));
    }

    // The instrument worked: every thread kind was seen, the house thread's Rust and
    // (when hooked) C++ allocations were counted during key changes.
    for (i, (_, who)) in KINDS.iter().enumerate() {
        assert!(SEEN[i].load(Relaxed), "the {who} thread was never classified: the counters would see nothing");
    }
    let key_rows: Vec<&Counts> = all.iter().flat_map(|(_, r)| r.iter()).filter(|(n, _)| n.contains("key")).map(|(_, c)| c).collect();
    let house_rust: u64 = key_rows.iter().map(|c| c[2][RUST_ALLOC]).sum();
    let house_c: u64 = key_rows.iter().map(|c| c[2][C_ALLOC]).sum();
    println!("positive control (house thread during key phases): {house_rust} Rust allocations, {house_c} C/C++ allocations; C runtime hooked: {c_hooked}");
    assert!(house_rust > 0, "positive control: no Rust allocation counted on the house thread");
    if c_hooked {
        assert!(house_c > 0, "positive control: building Rubber Band on the house thread made no counted C/C++ allocation");
    }

    let bts = BACKTRACES.lock().unwrap();
    for b in bts.iter() {
        println!("{b}");
    }
    drop(bts);
    for (label, rows) in &all {
        check(label, rows);
    }
}
