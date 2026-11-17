// SPDX-License-Identifier: GPL-3.0-or-later
//! A debounced action on its own thread: "save about a second after the last change".
//!
//! Used for the app settings file and for the song file's settings slot
//! (docs/APP_API.md section 3: "saved ... debounced about 1 s, flushed on song change
//! and exit"). A slider dragged at 60 changes per second then causes one write, not
//! sixty fsyncs on a USB stick.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Action = Arc<dyn Fn() + Send + Sync>;

struct State {
    /// When the pending action should run; `None` = nothing pending.
    due: Option<Instant>,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Held while the action runs, so the worker and a flush never run it at once.
    running: Mutex<()>,
    action: Action,
    delay: Duration,
}

/// Runs `action` once, `delay` after the most recent [`Debouncer::touch`].
pub struct Debouncer {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Debouncer {
    pub fn new(name: &str, delay: Duration, action: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                due: None,
                stop: false,
            }),
            wake: Condvar::new(),
            running: Mutex::new(()),
            action: Arc::new(action),
            delay,
        });
        let s = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name(format!("debounce-{name}"))
            .spawn(move || worker_loop(&s))
            .expect("spawn debounce thread");
        Debouncer {
            shared,
            worker: Mutex::new(Some(worker)),
        }
    }

    /// Something changed: (re)start the quiet period.
    pub fn touch(&self) {
        let mut st = lock(&self.shared.state);
        st.due = Some(Instant::now() + self.shared.delay);
        self.shared.wake.notify_all();
    }

    /// True when a change is waiting to be written.
    pub fn pending(&self) -> bool {
        lock(&self.shared.state).due.is_some()
    }

    /// Runs the pending action now, on the caller's thread, and waits for it. Does
    /// nothing when nothing is pending. Also waits for a run the worker has already
    /// started, so after `flush` returns everything touched before it is written.
    pub fn flush(&self) {
        let _running = lock(&self.shared.running);
        let was_pending = lock(&self.shared.state).due.take().is_some();
        if was_pending {
            (self.shared.action)();
        }
    }

    /// Flushes and stops the worker thread.
    pub fn shutdown(&self) {
        self.flush();
        lock(&self.shared.state).stop = true;
        self.shared.wake.notify_all();
        if let Some(w) = lock(&self.worker).take() {
            let _ = w.join();
        }
    }
}

impl Drop for Debouncer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic in an action must not wedge saving forever.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn worker_loop(s: &Shared) {
    let mut st = lock(&s.state);
    loop {
        if st.stop {
            return;
        }
        match st.due {
            None => st = s.wake.wait(st).unwrap_or_else(|e| e.into_inner()),
            Some(due) => {
                let now = Instant::now();
                if now < due {
                    st = s
                        .wake
                        .wait_timeout(st, due - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                    continue;
                }
                // Due: take the running lock first (a flush may be in progress), then
                // re-check that the change is still pending.
                drop(st);
                {
                    let _running = lock(&s.running);
                    let still_due = {
                        let mut st = lock(&s.state);
                        match st.due {
                            Some(d) if d <= Instant::now() => {
                                st.due = None;
                                true
                            }
                            _ => false,
                        }
                    };
                    if still_due {
                        (s.action)();
                    }
                }
                st = lock(&s.state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn many_touches_make_one_run_after_the_quiet_period() {
        let runs = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&runs);
        let d = Debouncer::new("t1", Duration::from_millis(150), move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        for _ in 0..20 {
            d.touch();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runs.load(Ordering::SeqCst), 0, "ran before the quiet period");
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(!d.pending());
    }

    #[test]
    fn flush_runs_now_and_only_when_pending() {
        let runs = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&runs);
        let d = Debouncer::new("t2", Duration::from_secs(30), move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        d.flush();
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        d.touch();
        d.flush();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        d.flush();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        d.touch();
        drop(d); // shutdown flushes
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }
}
