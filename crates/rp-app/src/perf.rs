//! Cheap timing notes in the log, written only when something is slow, so
//! the cabinet's log shows where time goes without filling up.

use std::time::{Duration, Instant};

/// A media switch slower than this (show to first frame) is logged.
pub const SLOW_SWITCH_MS: u128 = 300;
/// A frame slower than this is counted as a hitch.
const HITCH: Duration = Duration::from_millis(25);
/// How often a summary is written (only if there were hitches).
const REPORT_EVERY: Duration = Duration::from_secs(60);

/// Logs `what` with its duration if it took at least `ms` milliseconds.
pub fn note(what: &str, since: Instant, ms: u64) {
    let took = since.elapsed();
    if took >= Duration::from_millis(ms) {
        log::info!("perf: {what} took {} ms", took.as_millis());
    }
}

/// Per-frame timings for the menu: counts hitches and remembers the worst
/// frame and which part of it was slowest.
pub struct Frames {
    since: Instant,
    frames: u32,
    hitches: u32,
    worst: Duration,
    worst_part: (&'static str, Duration),
    /// The current frame's slowest part so far.
    cur_part: (&'static str, Duration),
    mark: Instant,
}

impl Frames {
    pub fn new() -> Frames {
        let now = Instant::now();
        Frames { since: now, frames: 0, hitches: 0, worst: Duration::ZERO, worst_part: ("", Duration::ZERO), cur_part: ("", Duration::ZERO), mark: now }
    }

    /// Starts timing a frame's parts from `at` (when the frame began).
    pub fn start(&mut self, at: Instant) {
        self.mark = at;
        self.cur_part = ("", Duration::ZERO);
    }

    /// Ends the part named `name` (started at the previous `part` or `start`).
    pub fn part(&mut self, name: &'static str) {
        let now = Instant::now();
        let d = now - self.mark;
        self.mark = now;
        if d > self.cur_part.1 {
            self.cur_part = (name, d);
        }
    }

    /// Records a whole frame's time (excluding the vsync wait when `busy` is
    /// measured before the swap) and writes the summary now and then.
    pub fn end(&mut self, busy: Duration) {
        self.frames += 1;
        if busy >= HITCH {
            self.hitches += 1;
        }
        if busy > self.worst {
            self.worst = busy;
            self.worst_part = self.cur_part;
        }
        if self.since.elapsed() >= REPORT_EVERY {
            if self.hitches > 0 {
                log::info!(
                    "perf: {} frames in {} s, {} over {} ms; worst {} ms (slowest part: {} {} ms)",
                    self.frames,
                    self.since.elapsed().as_secs(),
                    self.hitches,
                    HITCH.as_millis(),
                    self.worst.as_millis(),
                    self.worst_part.0,
                    self.worst_part.1.as_millis()
                );
            }
            *self = Frames::new();
        }
    }
}
