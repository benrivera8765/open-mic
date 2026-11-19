// SPDX-License-Identifier: GPL-3.0-or-later
//! The host clock that every block of microphone sound is stamped with.
//!
//! WO2 trap 4 ("two clocks"): the microphone and the output are different
//! devices whose sample clocks drift apart, so blocks are stamped with the
//! computer's own monotonic clock, never by counting samples. On each system
//! that is the clock the audio APIs themselves use for their time stamps:
//!
//! * **Windows:** `QueryPerformanceCounter`. WASAPI's `IAudioCaptureClient::GetBuffer`
//!   reports each packet's capture time on it (`pu64QPCPosition`, 100 ns units),
//!   and `IAudioClock::GetPosition` reports the output position on it too.
//! * **macOS:** `mach_absolute_time` in nanoseconds (scaled by `mach_timebase_info`).
//!   Core Audio's `AudioTimeStamp.mHostTime` is this clock, and cpal 0.16 turns it
//!   into its `StreamInstant` with exactly this scaling (cpal-0.16.0
//!   `src/host/coreaudio/mod.rs`, `host_time_to_stream_instant`), so
//!   [`stream_instant_ns`] gives a value on this clock.
//! * **Elsewhere:** `std::time::Instant` since the first call (no backend there).
//!
//! All values are nanoseconds as `i64`; the origin is the OS's (boot on both
//! systems) and only differences matter. Reading the clock is real-time safe on
//! both systems (QPC is a user-mode read on every supported Windows; on macOS
//! `mach_absolute_time` reads the commpage).

/// Nanoseconds per second.
pub const NS_PER_SECOND: i64 = 1_000_000_000;

/// The host clock now, in nanoseconds.
pub fn host_time_ns() -> i64 {
    imp::now_ns()
}

/// Windows: a QPC value in WASAPI's 100 ns units (as `GetBuffer` returns it) on
/// this clock, in nanoseconds.
pub fn qpc_100ns_to_ns(qpc_hns: u64) -> i64 {
    (qpc_hns as i64).saturating_mul(100)
}

/// macOS / cpal: a cpal `StreamInstant` on this clock, in nanoseconds. cpal 0.16's
/// Core Audio backend builds its instants from `mHostTime` scaled to nanoseconds
/// since the host-time origin, and `StreamInstant::new(0, 0)` is that origin.
#[cfg(target_os = "macos")]
pub fn stream_instant_ns(t: &cpal::StreamInstant) -> Option<i64> {
    t.duration_since(&cpal::StreamInstant::new(0, 0))
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

    fn frequency() -> i64 {
        static FREQ: OnceLock<i64> = OnceLock::new();
        *FREQ.get_or_init(|| {
            let mut f = 0i64;
            // SAFETY: valid out pointer; cannot fail on Windows XP or later.
            let _ = unsafe { QueryPerformanceFrequency(&mut f) };
            f.max(1)
        })
    }

    pub(super) fn now_ns() -> i64 {
        let mut c = 0i64;
        // SAFETY: valid out pointer; cannot fail on Windows XP or later.
        let _ = unsafe { QueryPerformanceCounter(&mut c) };
        (i128::from(c) * 1_000_000_000 / i128::from(frequency())) as i64
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::OnceLock;

    #[repr(C)]
    #[derive(Default)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    // libSystem, available on every macOS version.
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    fn timebase() -> (u64, u64) {
        static TB: OnceLock<(u64, u64)> = OnceLock::new();
        *TB.get_or_init(|| {
            let mut info = MachTimebaseInfo::default();
            // SAFETY: valid out pointer.
            let r = unsafe { mach_timebase_info(&mut info) };
            if r != 0 || info.denom == 0 {
                (1, 1)
            } else {
                (u64::from(info.numer), u64::from(info.denom))
            }
        })
    }

    pub(super) fn now_ns() -> i64 {
        let (numer, denom) = timebase();
        // SAFETY: plain libSystem call.
        let t = unsafe { mach_absolute_time() };
        // The same scaling cpal applies to mHostTime (see the module docs).
        (u128::from(t) * u128::from(numer) / u128::from(denom)) as i64
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use std::sync::OnceLock;
    use std::time::Instant;

    pub(super) fn now_ns() -> i64 {
        static T0: OnceLock<Instant> = OnceLock::new();
        T0.get_or_init(Instant::now).elapsed().as_nanos() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_clock_is_monotonic_and_in_nanoseconds() {
        let a = host_time_ns();
        let t = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let b = host_time_ns();
        let real = t.elapsed().as_nanos() as i64;
        assert!(b > a);
        // Same length as std's clock within 5 ms (scheduler noise).
        assert!(
            (b - a - real).abs() < 5_000_000,
            "host {} vs std {}",
            b - a,
            real
        );
    }

    #[test]
    fn qpc_units() {
        assert_eq!(qpc_100ns_to_ns(12_345), 1_234_500);
    }
}
