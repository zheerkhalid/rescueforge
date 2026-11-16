//! Pause / stop switches and progress counters shared between a worker
//! thread and the window.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub struct Control {
    stop: AtomicBool,
    pause: AtomicBool,
}

impl Control {
    pub fn new() -> Control {
        Control::default()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn set_paused(&self, paused: bool) {
        self.pause.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    /// Called by workers between units of work. Waits while paused and
    /// returns `true` when the work should end.
    pub fn checkpoint(&self) -> bool {
        while self.is_paused() && !self.is_stopped() {
            std::thread::sleep(Duration::from_millis(60));
        }
        self.is_stopped()
    }
}

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// Files found (scan) or files finished (recovery).
    pub found: AtomicU64,
    phase: Mutex<String>,
}

impl Progress {
    pub fn new() -> Progress {
        Progress::default()
    }

    pub fn set(&self, done: u64, total: u64) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn set_phase(&self, phase: &str) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = phase.to_string();
    }

    pub fn phase(&self) -> String {
        self.phase.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            (self.done.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn pause_blocks_until_resumed_or_stopped() {
        let ctl = Arc::new(Control::new());
        assert!(!ctl.checkpoint());
        ctl.set_paused(true);
        let c2 = ctl.clone();
        let t = std::thread::spawn(move || c2.checkpoint());
        std::thread::sleep(Duration::from_millis(150));
        assert!(!t.is_finished(), "worker waits while paused");
        ctl.set_paused(false);
        assert!(!t.join().unwrap());
        ctl.set_paused(true);
        ctl.stop();
        assert!(ctl.checkpoint(), "stop wins over pause");
    }

    #[test]
    fn progress() {
        let p = Progress::new();
        assert_eq!(p.fraction(), 0.0);
        p.set(50, 200);
        assert_eq!(p.fraction(), 0.25);
        p.set(500, 200);
        assert_eq!(p.fraction(), 1.0);
        p.set_phase("Reading");
        assert_eq!(p.phase(), "Reading");
    }
}
