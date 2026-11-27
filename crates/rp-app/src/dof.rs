//! Optional DirectOutput Framework (DOF) support: lights the cabinet's toys
//! from the menu the way PinUP Popper's menu does, so the "PinUP Menu" table
//! (ROM `pinupmenu`) from the DOF Config Tool works unchanged. No lighting
//! setup of our own.
//!
//! What Popper's menu script (PuPMenuScript.pup) does, and so do we:
//! - start: `Init("B2SServer", "", "pinupmenu")` on its COM object;
//! - each new selection: the last random event goes to 0 and a new random
//!   `E901`-`E990` goes to 1 and stays there;
//! - wheel prior / next: `E800` / `E801` pulsed (1, then 0 at once);
//!   page next / prior: `E804` / `E805`. Unlike Popper we pulse at most
//!   once per Rate Cap (Options → DOF; any of these events): a held flipper
//!   steps the wheel up to ~28 times a second, and extra pulses are dropped,
//!   not queued;
//! - game launch: `Finish()`, so the table's own DOF can use the hardware
//!   (the launch waits a moment for it); back from the game: `Init` again.
//!   Exit (and a crash): `Finish()`.
//!
//! The COM object is PinUP's out-of-process `PinUPDOF.DOFLink` bridge when
//! it's registered (as Popper uses; it works whatever our bitness), else
//! DOF's own in-process `DirectOutput.ComObject` (needs its x64 registration).
//! Everything runs on a worker thread: DOF's `Init` reads its config files
//! and can take a second or two. If neither object is there, DOF stays off
//! quietly (one line in the log); a new selection tries again now and then
//! (PinUpDOF.exe started late, the bridge restarted).

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Popper's menu events (`E` + 799 + its button index).
pub const PRIOR: u32 = 800;
pub const NEXT: u32 = 801;
pub const PAGE_NEXT: u32 = 804;
pub const PAGE_PRIOR: u32 = 805;

/// Popper picks `floor(random * 90) + 1 + 900`.
const RANDOM_FIRST: u32 = 901;
const RANDOM_COUNT: u32 = 90;

/// Rate Cap: the shortest time between two wheel/page pulses (of any
/// event), in ms; ones sooner are dropped, so fast scrolling doesn't hammer
/// the toys (and DOF) on every step. 0 is Off: every step pulses.
pub const RATE_CAP_CHOICES: &[u64] = &[0, 50, 100, 150, 200, 250, 300, 350, 400, 450, 500];
pub const DEFAULT_RATE_CAP_MS: u64 = 100;

/// Without a session it should have, the worker tries to start one again
/// at most this often (on a new selection).
const RETRY_EVERY: Duration = Duration::from_secs(30);
/// How long a launch waits for the menu's session to end before the table starts.
pub const FINISH_WAIT: Duration = Duration::from_secs(2);

/// The worker's queue, for ending the session from the panic hook.
static WORKER: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);

enum Cmd {
    /// Start a DOF session (if there isn't one).
    Start,
    /// End the DOF session (if there is one); the sender, if any, is told
    /// when that's done.
    Finish(Option<Sender<()>>),
    /// A new selection: hold a new random event.
    Select,
    Pulse(u32),
    /// Finish and stop the thread; the sender is told when that's done.
    Quit(Sender<()>),
}

/// The menu's side of DOF: what it wants, sent to the worker.
pub struct Dof {
    pub enabled: bool,
    /// A game (ours or another program's) is using the cabinet: no session.
    suspended: bool,
    tx: Option<Sender<Cmd>>,
    /// Rate Cap, in ms (see [`RATE_CAP_CHOICES`]).
    pub rate_cap_ms: u64,
    /// When the last pulse was sent, for the Rate Cap.
    last_pulse: Option<Instant>,
}

impl Dof {
    pub fn new(enabled: bool, rate_cap_ms: u64) -> Dof {
        let mut d = Dof { enabled: false, suspended: false, tx: None, rate_cap_ms: snap_rate_cap(rate_cap_ms), last_pulse: None };
        d.set_enabled(enabled);
        d
    }

    fn active(&self) -> bool {
        self.enabled && !self.suspended
    }

    fn send(&mut self, cmd: Cmd) {
        if self.tx.is_none() {
            let (tx, rx) = channel();
            match std::thread::Builder::new().name("dof".into()).spawn(move || worker(rx)) {
                Ok(_) => {
                    if let Ok(mut w) = WORKER.lock() {
                        *w = Some(tx.clone());
                    }
                    self.tx = Some(tx);
                }
                Err(e) => {
                    log::warn!("DOF: couldn't start its thread: {e}");
                    return;
                }
            }
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// Turns DOF on or off (Options).
    pub fn set_enabled(&mut self, on: bool) {
        let was = self.active();
        self.enabled = on;
        self.changed(was, None);
    }

    /// Ends the session while a game runs, and starts it again after.
    pub fn suspend(&mut self, suspended: bool) {
        let was = self.active();
        self.suspended = suspended;
        self.changed(was, None);
    }

    /// Ends the session for a game. The receiver (if a session was going)
    /// hears when it has ended, so the table's DOF doesn't start alongside ours.
    pub fn suspend_for_game(&mut self) -> Option<Receiver<()>> {
        let was = self.active();
        self.suspended = true;
        let (done_tx, done_rx) = channel();
        self.changed(was, Some(done_tx));
        (was && self.tx.is_some()).then_some(done_rx)
    }

    fn changed(&mut self, was: bool, done: Option<Sender<()>>) {
        match (was, self.active()) {
            (false, true) => self.send(Cmd::Start),
            (true, false) => self.send(Cmd::Finish(done)),
            _ => {}
        }
    }

    /// After `shutdown`: starts the session again if DOF is still wanted
    /// (an update that couldn't be installed after all).
    pub fn resume(&mut self) {
        if self.active() && self.tx.is_none() {
            self.send(Cmd::Start);
        }
    }

    /// The wheel settled on a different game or playlist.
    pub fn select(&mut self) {
        if self.active() {
            self.send(Cmd::Select);
        }
    }

    /// One of Popper's menu events (`PRIOR`, `NEXT`, ...), dropped if
    /// another pulse went less than the Rate Cap ago.
    pub fn pulse(&mut self, event: u32) {
        let gap = Duration::from_millis(self.rate_cap_ms);
        if self.active() && throttle(&mut self.last_pulse, gap, Instant::now()) {
            self.send(Cmd::Pulse(event));
        }
    }

    /// Ends the session (turning the toys off) before the menu exits, waiting
    /// a moment for it.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Ok(mut w) = WORKER.lock() {
                w.take();
            }
            quit(&tx);
        }
    }
}

/// The same from the panic hook, which can't reach the menu's `Dof`: the
/// toys mustn't stay lit, and the copy started next opens DOF itself.
pub fn shutdown_all() {
    // try_lock: never wait in the panic hook.
    let tx = WORKER.try_lock().ok().and_then(|mut w| w.take());
    if let Some(tx) = tx {
        quit(&tx);
    }
}

/// Stops the worker (ending the session), waiting a moment for it.
fn quit(tx: &Sender<Cmd>) {
    let (done_tx, done_rx) = channel();
    if tx.send(Cmd::Quit(done_tx)).is_ok() {
        let _ = done_rx.recv_timeout(Duration::from_secs(3));
    }
}

/// A Rate Cap from the settings file as the nearest choice (a hand-typed
/// 120 becomes 100, 1000 becomes 500), so the menus can show it.
fn snap_rate_cap(ms: u64) -> u64 {
    *RATE_CAP_CHOICES.iter().min_by_key(|&&c| c.abs_diff(ms)).unwrap_or(&DEFAULT_RATE_CAP_MS)
}

/// Whether a pulse may go at `now` (recording it if so): not if the last
/// one went less than `gap` ago.
fn throttle(last: &mut Option<Instant>, gap: Duration, now: Instant) -> bool {
    if last.is_some_and(|at| now.saturating_duration_since(at) < gap) {
        return false;
    }
    *last = Some(now);
    true
}

/// Whether it's time to try for a session again (last tried at `last`).
fn retry_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= RETRY_EVERY)
}

fn worker(rx: Receiver<Cmd>) {
    let _com = com::Apartment::enter();
    let mut link: Option<com::Link> = None;
    let mut held: Option<String> = None;
    let mut warned = false;
    let mut rng = seed();
    // A session is wanted (Start, no Finish since), and when one was last tried.
    let mut wanted = false;
    let mut tried: Option<Instant> = None;
    let mut open = |link: &mut Option<com::Link>, held: &mut Option<String>, tried: &mut Option<Instant>| {
        *tried = Some(Instant::now());
        match com::Link::open() {
            Ok((l, name)) => {
                log::info!("DOF: session started via {name}");
                *link = Some(l);
                *held = None;
            }
            Err(e) => {
                // Not installed or not registered: stay off, quietly.
                if !warned {
                    log::info!("DOF: not available ({e:#}); lights stay off");
                    warned = true;
                }
            }
        }
    };
    for cmd in rx {
        // Lost (or never got) the session it should have: try again now and then.
        if matches!(cmd, Cmd::Select) && wanted && link.is_none() && retry_due(tried, Instant::now()) {
            open(&mut link, &mut held, &mut tried);
        }
        let result = match cmd {
            Cmd::Start => {
                wanted = true;
                if link.is_none() {
                    open(&mut link, &mut held, &mut tried);
                }
                Ok(())
            }
            Cmd::Finish(done) => {
                wanted = false;
                let r = finish(&mut link);
                if let Some(d) = done {
                    let _ = d.send(());
                }
                r
            }
            Cmd::Select => match &link {
                Some(l) => {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    let e = format!("E{}", RANDOM_FIRST + (rng % RANDOM_COUNT as u64) as u32);
                    let r = held.take().map_or(Ok(()), |h| l.set(&h, 0)).and_then(|()| l.set(&e, 1));
                    held = Some(e);
                    r
                }
                None => Ok(()),
            },
            Cmd::Pulse(n) => match &link {
                Some(l) => {
                    let e = format!("E{n}");
                    l.set(&e, 1).and_then(|()| l.set(&e, 0))
                }
                None => Ok(()),
            },
            Cmd::Quit(done) => {
                let _ = finish(&mut link);
                let _ = done.send(());
                return;
            }
        };
        if let Err(e) = result {
            // The bridge went away (PinUpDOF.exe closed, say): drop it; the
            // next Start tries again.
            log::warn!("DOF: {e:#}; session dropped");
            link = None;
        }
    }
    let _ = finish(&mut link);
}

fn finish(link: &mut Option<com::Link>) -> anyhow::Result<()> {
    match link.take() {
        Some(l) => {
            log::info!("DOF: session finished");
            l.finish()
        }
        None => Ok(()),
    }
}

fn seed() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    t | 1
}

mod com {
    use anyhow::{Context, Result};
    use windows::core::{GUID, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, DISPATCH_METHOD, DISPPARAMS,
    };
    use windows::Win32::System::Variant::VARIANT;

    /// Popper's own bridge first, then DOF's in-process object.
    const PROG_IDS: &[&str] = &["PinUPDOF.DOFLink", "DirectOutput.ComObject"];
    const LOCALE_USER_DEFAULT: u32 = 0x0400;

    /// COM for this thread, for as long as it lives.
    pub struct Apartment(bool);

    impl Apartment {
        pub fn enter() -> Apartment {
            Apartment(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    /// A DOF session for the `pinupmenu` table.
    pub struct Link(IDispatch);

    impl Link {
        pub fn open() -> Result<(Link, &'static str)> {
            let mut last = anyhow::anyhow!("no DOF COM object registered");
            for &id in PROG_IDS {
                match Self::create(id) {
                    Ok(l) => match l.call("Init", &["B2SServer".into(), "".into(), "pinupmenu".into()]) {
                        Ok(()) => return Ok((l, id)),
                        Err(e) => last = e.context(format!("{id}.Init")),
                    },
                    Err(e) => last = e,
                }
            }
            Err(last)
        }

        fn create(prog_id: &str) -> Result<Link> {
            let clsid: GUID = unsafe { CLSIDFromProgID(&HSTRING::from(prog_id)) }.with_context(|| format!("{prog_id} not registered"))?;
            let d: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER) }.with_context(|| format!("couldn't create {prog_id}"))?;
            Ok(Link(d))
        }

        /// Creates each COM object and looks up its methods, without
        /// starting a session (nothing lights up). For checking a cabinet.
        #[cfg(test)]
        pub fn probe() -> Vec<(&'static str, Result<()>)> {
            PROG_IDS
                .iter()
                .map(|&id| {
                    let r = Self::create(id).and_then(|l| {
                        for m in ["Init", "Finish", "UpdateNamedTableElement"] {
                            l.id_of(m)?;
                        }
                        Ok(())
                    });
                    (id, r)
                })
                .collect()
        }

        fn id_of(&self, method: &str) -> Result<i32> {
            let name = HSTRING::from(method);
            let names = [PCWSTR(name.as_ptr())];
            let mut id = 0i32;
            unsafe { self.0.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, LOCALE_USER_DEFAULT, &mut id) }.with_context(|| format!("no {method} method"))?;
            Ok(id)
        }

        pub fn set(&self, element: &str, value: i32) -> Result<()> {
            self.call("UpdateNamedTableElement", &[element.into(), value.into()])
        }

        pub fn finish(self) -> Result<()> {
            self.call("Finish", &[])
        }

        /// Calls `method` with `args` through IDispatch.
        fn call(&self, method: &str, args: &[VARIANT]) -> Result<()> {
            let id = self.id_of(method)?;
            // IDispatch takes the arguments last to first.
            let mut rev: Vec<VARIANT> = args.iter().rev().cloned().collect();
            let params = DISPPARAMS { rgvarg: rev.as_mut_ptr(), rgdispidNamedArgs: std::ptr::null_mut(), cArgs: rev.len() as u32, cNamedArgs: 0 };
            let mut result = VARIANT::default();
            unsafe { self.0.Invoke(id, &GUID::zeroed(), LOCALE_USER_DEFAULT, DISPATCH_METHOD, &params, Some(&mut result), None, None) }.with_context(|| format!("{method} failed"))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_events_stay_in_popper_range() {
        for r in [0u64, 1, 89, 90, 12345, u64::MAX] {
            let n = RANDOM_FIRST + (r % RANDOM_COUNT as u64) as u32;
            assert!((901..=990).contains(&n));
        }
    }

    #[test]
    fn disabled_sends_nothing() {
        let mut d = Dof::new(false, DEFAULT_RATE_CAP_MS);
        d.select();
        d.pulse(NEXT);
        d.suspend(true);
        d.suspend(false);
        assert!(d.tx.is_none());
    }

    #[test]
    fn lost_sessions_are_retried_now_and_then() {
        let t = Instant::now();
        assert!(retry_due(None, t));
        assert!(!retry_due(Some(t), t + RETRY_EVERY - Duration::from_millis(1)));
        assert!(retry_due(Some(t), t + RETRY_EVERY));
    }

    #[test]
    fn rate_cap_snaps_to_a_choice() {
        assert_eq!([0, 1, 120, 130, 200, 1000].map(snap_rate_cap), [0, 0, 100, 150, 200, 500]);
    }

    #[test]
    fn pulses_are_throttled() {
        let mut last = None;
        let t = Instant::now();
        let ms = Duration::from_millis;
        let gap = ms(200);
        assert!(throttle(&mut last, gap, t));
        assert!(!throttle(&mut last, gap, t + ms(35)));
        assert!(!throttle(&mut last, gap, t + gap - ms(1)));
        assert!(throttle(&mut last, gap, t + gap));
        assert!(!throttle(&mut last, gap, t + gap + ms(100)));
        // Held flipper at 35 ms steps for 2 s: 10 pulses at 200 ms, every
        // step with the cap off (0), 4 at 500 ms.
        for (cap, want) in [(200, 10), (0, 58), (500, 4)] {
            let mut last = None;
            assert_eq!((0..58).filter(|i| throttle(&mut last, ms(cap), t + ms(35 * i))).count(), want, "{cap} ms");
        }
    }

    /// On a cabinet: `cargo test -p rusty-popper dof_objects -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dof_objects() {
        let _com = com::Apartment::enter();
        for (id, r) in com::Link::probe() {
            println!("{id}: {}", match r { Ok(()) => "OK".to_string(), Err(e) => format!("{e:#}") });
        }
    }
}
