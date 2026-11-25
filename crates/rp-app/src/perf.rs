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

/// Keeps the menu from drawing faster than the playfield can show when the
/// driver doesn't honour vsync. While vsync works the swap already waits,
/// and this only checks once a second that it still does.
pub struct Pacer {
    /// Whether vsync was found not to limit the frame rate.
    unpaced: bool,
    window_start: Instant,
    frames: u32,
    /// When the next frame may start (only while `unpaced`, sleeping).
    next: Instant,
    /// Whether waiting for the compositor (DwmFlush) paces frames.
    dwm_ok: bool,
    dwm_frames: u32,
    dwm_since: Instant,
}

#[cfg(windows)]
fn dwm_flush() {
    unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
}

#[cfg(not(windows))]
fn dwm_flush() {}

impl Pacer {
    pub fn new() -> Pacer {
        let now = Instant::now();
        Pacer { unpaced: false, window_start: now, frames: 0, next: now, dwm_ok: true, dwm_frames: 0, dwm_since: now }
    }

    /// Call right after the playfield's swap. `every_refresh`: the menu is
    /// moving, so it presents every refresh (while it's still and presents
    /// only new video frames, the rate says nothing about vsync).
    pub fn after_swap(&mut self, period: Duration, every_refresh: bool) {
        let now = Instant::now();
        if !self.unpaced {
            if !every_refresh {
                self.window_start = now;
                self.frames = 0;
            }
            self.frames += 1;
            let elapsed = now - self.window_start;
            if elapsed >= Duration::from_secs(1) {
                let expected = elapsed.as_secs_f64() / period.as_secs_f64();
                // Well over the refresh rate: the swap isn't waiting.
                if f64::from(self.frames) > expected * 1.1 {
                    log::info!(
                        "perf: vsync isn't limiting frames ({} in {:.1} s at {:.0} Hz); pacing them instead",
                        self.frames,
                        elapsed.as_secs_f64(),
                        1.0 / period.as_secs_f64()
                    );
                    self.unpaced = true;
                    self.next = now;
                }
                self.window_start = now;
                self.frames = 0;
            }
            // Never spin, even before the check above has run.
            std::thread::sleep(Duration::from_millis(1));
            return;
        }
        // Wait for the desktop compositor's next frame: a kernel wait that
        // lines frames up with the display. If that doesn't hold frames back
        // either, keep a steady beat of one frame per refresh by sleeping;
        // after a slow frame, start again from now rather than catching up.
        if self.dwm_ok {
            dwm_flush();
            let now = Instant::now();
            self.dwm_frames += 1;
            if self.dwm_frames == 1 {
                self.dwm_since = now;
            } else if now - self.dwm_since >= Duration::from_secs(2) {
                let expected = (now - self.dwm_since).as_secs_f64() / period.as_secs_f64();
                if f64::from(self.dwm_frames - 1) > expected * 1.1 {
                    log::info!("perf: waiting for the compositor doesn't pace frames either; sleeping instead");
                    self.dwm_ok = false;
                    self.next = now;
                }
                self.dwm_frames = 1;
                self.dwm_since = now;
            }
            return;
        }
        self.next += period;
        if self.next > now {
            std::thread::sleep(self.next - now);
        } else if now - self.next > period {
            self.next = now;
        }
    }
}
