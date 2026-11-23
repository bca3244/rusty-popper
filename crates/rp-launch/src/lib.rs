//! Runs per-emulator LaunchScript/PostScript and tracks the game process.
//!
//! Popper's scripts are Windows batch files with `[PLACEHOLDER]` tokens
//! (case-insensitive). We fill them in the same way Popper does, run them
//! hidden, then wait for the emulator's game process to finish before running
//! the post script. The frontend can ask for the game to be closed: that runs
//! the post script early (it is Popper's "close" script), and kills whatever
//! is still running a while later.

use anyhow::{Context, Result};
use rp_db::{Emulator, Game};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Helper processes that live in an emulator folder but don't mean a game is
/// still running (killed by post scripts, resident loaders, Steam's own
/// services). [`LaunchJob::ignore`] adds to this list.
pub const DEFAULT_IGNORED: &[&str] = &[
    "dmdext.exe",
    "fploader.exe",
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "steamerrorreporter.exe",
    "gameoverlayui.exe",
];

/// How long to wait for the game process to show up after the launch script.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(90);
/// A launch script that itself runs this long without a trackable process is
/// taken to be the game (a script that runs the emulator without START).
const BLOCKING_SCRIPT: Duration = Duration::from_secs(20);
/// The game counts as closed once no game process has been seen for this long
/// (a launcher stub may exit a moment before the real game starts).
const EXIT_CONFIRM: Duration = Duration::from_millis(1200);
/// After a close request, how long the post script gets before the game's
/// processes are killed.
const KILL_AFTER: Duration = Duration::from_secs(20);
/// The same, when the emulator has no post script to close it.
const KILL_AFTER_NO_SCRIPT: Duration = Duration::from_secs(2);
/// How often the whole process list is rescanned while a game runs (in
/// between, only the known game processes are checked).
const FULL_SCAN_EVERY: Duration = Duration::from_secs(2);
/// The longest a post (close) script may run before it is ended.
const POST_SCRIPT_LIMIT: Duration = Duration::from_secs(30);
/// After the game's processes were killed, how long to wait for them to go
/// before returning to the menu anyway.
const GIVE_UP_AFTER_KILL: Duration = Duration::from_secs(5);
/// After a close request during loading, how long to keep looking for game
/// processes that only show up now (and end them).
const LATE_START_WATCH: Duration = Duration::from_secs(10);

/// Everything needed to launch one game.
#[derive(Debug, Clone)]
pub struct LaunchJob {
    pub popper_dir: PathBuf,
    pub emulator: Emulator,
    pub game: Game,
    /// Extra process names (like `helper.exe`) that never count as the game.
    pub ignore: Vec<String>,
}

/// Where a running launch is, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Launch script running / waiting for the game window.
    Launching = 0,
    /// The game process is running.
    Playing = 1,
    /// The game exited (or is being closed); post script running.
    Exiting = 2,
}

/// Shared between the worker running a [`LaunchJob`] and the UI.
#[derive(Debug, Default)]
pub struct Control {
    phase: AtomicU8,
    stop: AtomicBool,
    /// Process ids of the game, as last seen.
    pids: Mutex<Vec<u32>>,
}

impl Control {
    pub fn phase(&self) -> Phase {
        match self.phase.load(Ordering::Relaxed) {
            1 => Phase::Playing,
            2 => Phase::Exiting,
            _ => Phase::Launching,
        }
    }

    fn set(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    /// The game's process ids as last seen (empty when it can't be tracked).
    pub fn game_pids(&self) -> Vec<u32> {
        self.pids.lock().map(|p| p.clone()).unwrap_or_default()
    }

    fn set_pids(&self, known: &[Pid]) {
        if let Ok(mut p) = self.pids.lock() {
            *p = known.iter().map(|pid| pid.as_u32()).collect();
        }
    }

    /// Asks the worker to close the game (the cabinet's Exit button).
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The game ran and exited after this many seconds.
    Finished { secs: u64 },
    /// No game process appeared in time (bad script, missing table...).
    NeverStarted,
}

/// A script running hidden from a temp .bat file.
struct Script {
    child: Child,
    bat: PathBuf,
    label: &'static str,
    started: Instant,
    /// How long it ran, once it has finished.
    ran: Option<Duration>,
}

static BAT_SEQ: AtomicU32 = AtomicU32::new(0);

impl Script {
    /// True once the batch itself has exited (it normally STARTs the emulator
    /// and returns at once).
    fn finished(&mut self) -> bool {
        if self.ran.is_some() {
            return true;
        }
        match self.child.try_wait() {
            Ok(None) => false,
            done => {
                match done {
                    Ok(Some(status)) if !status.success() => log::warn!("{} script exited with {status}", self.label),
                    Err(e) => log::warn!("{} script: {e}", self.label),
                    _ => {}
                }
                self.ran = Some(self.started.elapsed());
                let _ = std::fs::remove_file(&self.bat);
                true
            }
        }
    }

    /// Waits for the script, but no longer than `limit`: after that it is
    /// ended, with whatever it started.
    fn wait_up_to(&mut self, limit: Duration) {
        while !self.finished() {
            if self.started.elapsed() >= limit {
                log::warn!("{} script still running after {}s; ending it", self.label, limit.as_secs());
                self.end();
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Ends the script and the programs it started, if it is still running.
    fn end(&mut self) {
        if self.finished() {
            return;
        }
        taskkill(&[self.child.id()]);
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.ran = Some(self.started.elapsed());
        let _ = std::fs::remove_file(&self.bat);
    }
}

/// Force-ends processes and everything they started. All are ended at once,
/// with no console window flashing up.
fn taskkill(pids: &[u32]) {
    let children: Vec<Child> = pids
        .iter()
        .filter_map(|pid| {
            let mut cmd = Command::new("taskkill.exe");
            cmd.args(["/F", "/T", "/PID"]).arg(pid.to_string());
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            cmd.spawn().map_err(|e| log::warn!("taskkill: {e}")).ok()
        })
        .collect();
    for mut c in children {
        let _ = c.wait();
    }
}

/// Lowercases a Windows path and removes what differs between spellings of
/// the same folder: `/` separators, the `\\?\` prefix and trailing slashes.
fn normalize(path: &str) -> String {
    let p = path.trim().replace('/', "\\").to_lowercase();
    let p = p.strip_prefix(r"\\?\").unwrap_or(&p);
    p.trim_end_matches('\\').to_string()
}

/// Is `exe` inside folder `dir` (both normalized)? Whole path components
/// only: `c:\vpx` does not contain `c:\vpx64\a.exe`.
fn path_under(exe: &str, dir: &str) -> bool {
    !dir.is_empty() && exe.len() > dir.len() && exe.starts_with(dir) && exe.as_bytes()[dir.len()] == b'\\'
}

fn strip_exe(name: &str) -> &str {
    name.strip_suffix(".exe").unwrap_or(name)
}

/// Decides which processes are "the game" for one emulator.
#[derive(Debug, Clone, Default)]
struct Matcher {
    /// Normalized spellings of the emulator folder (as stored, and resolved).
    dirs: Vec<String>,
    /// Popper's process name for the emulator, lowercase without `.exe`.
    process: Option<String>,
    /// Lowercase names that never count.
    ignored: Vec<String>,
}

impl Matcher {
    fn new(emulator: &Emulator, extra_ignored: &[String]) -> Self {
        let mut dirs = Vec::new();
        if let Some(dir) = &emulator.launch_dir {
            dirs.push(normalize(&dir.to_string_lossy()));
            // Also the real location, in case the folder is a junction or symlink.
            if let Ok(real) = std::fs::canonicalize(dir) {
                dirs.push(normalize(&real.to_string_lossy()));
            }
        }
        dirs.retain(|d| !d.is_empty());
        dirs.dedup();
        let process = emulator.process_name.as_deref().map(|p| strip_exe(&p.trim().to_lowercase()).to_string()).filter(|p| !p.is_empty());
        let mut ignored: Vec<String> = DEFAULT_IGNORED.iter().map(|s| s.to_string()).collect();
        ignored.extend(extra_ignored.iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()));
        Matcher { dirs, process, ignored }
    }

    /// False when nothing could ever match (no folder and no process name).
    fn can_track(&self) -> bool {
        !self.dirs.is_empty() || self.process.is_some()
    }

    fn matches(&self, exe: &str, name: &str) -> bool {
        let name = name.to_lowercase();
        if self.ignored.iter().any(|i| *i == name || strip_exe(i) == strip_exe(&name)) {
            return false;
        }
        if self.process.as_deref() == Some(strip_exe(&name)) {
            return true;
        }
        let exe = normalize(exe);
        self.dirs.iter().any(|d| path_under(&exe, d))
    }
}

/// Notices tables started by another program (VPin Studio's launcher or its
/// media recorder, say), so the menu can step out of the way.
pub struct OtherLaunches {
    sys: Option<System>,
}

impl Default for OtherLaunches {
    fn default() -> Self {
        Self::new()
    }
}

impl OtherLaunches {
    pub fn new() -> Self {
        OtherLaunches { sys: System::new().map_err(|e| log::warn!("can't list processes: {e}")).ok() }
    }

    /// The program name of process `pid` if it belongs to one of `emulators`
    /// (found the same way as a game we start ourselves).
    pub fn emulator_process(&mut self, pid: u32, emulators: &[Emulator], extra_ignored: &[String]) -> Option<String> {
        let sys = self.sys.as_mut()?;
        let p = Pid::from_u32(pid);
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::nothing().with_exe(UpdateKind::Always));
        let proc = sys.process(p)?;
        let exe = proc.exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let name = proc.name().to_string_lossy().into_owned();
        emulators.iter().any(|e| Matcher::new(e, extra_ignored).matches(&exe, &name)).then_some(name)
    }

    /// Whether process `pid` is still running.
    pub fn alive(&mut self, pid: u32) -> bool {
        let Some(sys) = self.sys.as_mut() else { return false };
        let p = Pid::from_u32(pid);
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::nothing());
        sys.process(p).is_some()
    }
}

/// Finds and follows the game's processes.
struct Tracker {
    sys: Option<System>,
    matcher: Matcher,
    /// Only processes started at or after this time (Unix seconds) count.
    since: u64,
    known: Vec<Pid>,
}

impl Tracker {
    fn new(matcher: Matcher, since: u64) -> Self {
        let sys = System::new().map_err(|e| log::warn!("can't list processes: {e}")).ok();
        Tracker { sys, matcher, since, known: Vec::new() }
    }

    fn can_track(&self) -> bool {
        self.sys.is_some() && self.matcher.can_track()
    }

    /// Looks through every process; returns the names of the game's.
    fn scan(&mut self) -> Vec<String> {
        let Some(sys) = self.sys.as_mut() else { return Vec::new() };
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet));
        // A process seen in its first moments can have no readable path yet,
        // and an empty one is remembered: ask again for those.
        let pathless: Vec<Pid> = sys
            .processes()
            .iter()
            .filter(|(_, p)| p.start_time() >= self.since && p.exe().is_none_or(|e| e.as_os_str().is_empty()))
            .map(|(pid, _)| *pid)
            .collect();
        if !pathless.is_empty() {
            sys.refresh_processes_specifics(ProcessesToUpdate::Some(&pathless), false, ProcessRefreshKind::nothing().with_exe(UpdateKind::Always));
        }
        let mut names = Vec::new();
        self.known.clear();
        for (pid, p) in sys.processes() {
            if p.start_time() < self.since {
                continue;
            }
            // The path can't be read for a process running elevated; its name
            // can still match Popper's process name.
            let exe = p.exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
            let name = p.name().to_string_lossy();
            if self.matcher.matches(&exe, &name) {
                self.known.push(*pid);
                names.push(name.to_lowercase());
            }
        }
        names
    }

    /// Cheap check that some already-known game process is still alive.
    fn known_alive(&mut self) -> bool {
        let Some(sys) = self.sys.as_mut() else { return false };
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&self.known), true, ProcessRefreshKind::nothing());
        self.known.retain(|pid| sys.process(*pid).is_some());
        !self.known.is_empty()
    }

    /// Ends every game process (and what it started). Returns whether there were any.
    fn kill_all(&mut self) -> bool {
        let names = self.scan();
        if names.is_empty() {
            return false;
        }
        log::warn!("Game didn't close; ending {}", names.join(", "));
        let pids: Vec<u32> = self.known.iter().map(|pid| pid.as_u32()).collect();
        taskkill(&pids);
        true
    }
}

impl LaunchJob {
    fn game_full_path(&self) -> PathBuf {
        match &self.emulator.games_dir {
            Some(d) => d.join(&self.game.file_name),
            None => PathBuf::from(&self.game.file_name),
        }
    }

    /// Popper's script tokens and their values for this game.
    pub fn tokens(&self) -> Vec<(&'static str, String)> {
        let s = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        let mut start_dir = self.popper_dir.display().to_string();
        if !start_dir.ends_with('\\') {
            start_dir.push('\\');
        }
        let file = Path::new(&self.game.file_name);
        vec![
            ("STARTDIR", start_dir),
            ("DIREMU", s(&self.emulator.launch_dir)),
            ("DIRGAME", s(&self.emulator.games_dir)),
            ("DIRROM", s(&self.emulator.roms_dir)),
            ("GAMEFULLNAME", self.game_full_path().display().to_string()),
            ("GAMENAME", file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            ("GAMEEXT", file.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            ("GAMEFILE", self.game.file_name.clone()),
            ("ROM", o(&self.game.rom)),
            ("ALTEXE", o(&self.game.alt_exe)),
            ("ALTMODE", o(&self.game.alt_mode)),
            ("CUSTOM1", o(&self.game.custom[0])),
            ("CUSTOM2", o(&self.game.custom[1])),
            ("CUSTOM3", o(&self.game.custom[2])),
            ("RECMODE", "0".into()),
        ]
    }

    /// Fills `[TOKEN]` placeholders (case-insensitive); unknown tokens are left as-is.
    pub fn fill(&self, script: &str) -> String {
        self.fill_noting(script, &mut Vec::new())
    }

    /// Like [`fill`](Self::fill), also collecting bracketed names that look
    /// like Popper tokens (capitals, digits, `_`, `?`) but aren't known.
    fn fill_noting(&self, script: &str, unknown: &mut Vec<String>) -> String {
        let tokens = self.tokens();
        let mut out = String::with_capacity(script.len());
        let mut rest = script;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            match after.find(']') {
                Some(close) => {
                    let name = &after[..close];
                    match tokens.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
                        Some((_, v)) => out.push_str(v),
                        None => {
                            let token_like = name.len() >= 3
                                && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '?');
                            if token_like && !unknown.iter().any(|u| u == name) {
                                unknown.push(name.to_string());
                            }
                            out.push('[');
                            out.push_str(name);
                            out.push(']');
                        }
                    }
                    rest = &after[close + 1..];
                }
                None => {
                    out.push_str(&rest[open..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The .bat file contents for `script`: tokens filled, CRLF line endings,
    /// and UTF-8 switched on when a name needs it (cmd otherwise reads the
    /// file in the OEM code page and mangles accented file names).
    fn batch_body(&self, label: &str, script: &str) -> String {
        let mut unknown = Vec::new();
        let filled = self.fill_noting(script, &mut unknown);
        if !unknown.is_empty() {
            log::warn!("{label} script for {} uses tokens Rusty Popper doesn't fill: [{}]", self.emulator.name, unknown.join("], ["));
        }
        let body = filled.replace("\r\n", "\n").replace('\n', "\r\n");
        if body.is_ascii() {
            body
        } else {
            format!("@chcp 65001 >nul\r\n{body}")
        }
    }

    /// Writes the filled script to a temp .bat and starts it hidden.
    fn start_script(&self, label: &'static str, script: &str) -> Result<Option<Script>> {
        if script.trim().is_empty() {
            return Ok(None);
        }
        let seq = BAT_SEQ.fetch_add(1, Ordering::Relaxed);
        let bat = std::env::temp_dir().join(format!("{}{label}-{}-{seq}.bat", rp_db::TEMP_PREFIX, std::process::id()));
        std::fs::write(&bat, self.batch_body(label, script)).with_context(|| format!("writing {}", bat.display()))?;
        let cwd = self.emulator.launch_dir.clone().unwrap_or_else(|| self.popper_dir.clone());
        let mut cmd = Command::new("cmd");
        cmd.arg("/c").arg(&bat).current_dir(&cwd);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match cmd.spawn() {
            Ok(child) => Ok(Some(Script { child, bat, label, started: Instant::now(), ran: None })),
            Err(e) => {
                let _ = std::fs::remove_file(&bat);
                Err(e).with_context(|| format!("running {label} script in {}", cwd.display()))
            }
        }
    }

    /// Runs the post (close) script and waits for it.
    fn run_post_script(&self) {
        match self.start_script("post", &self.emulator.post_script) {
            Ok(Some(mut s)) => s.wait_up_to(POST_SCRIPT_LIMIT),
            Ok(None) => {}
            Err(e) => log::warn!("{e:#}"),
        }
    }

    /// Runs the whole game: launch script, wait for the game to exit, post
    /// script. Blocks; call it from a worker thread.
    pub fn run(&self, ctl: &Control) -> Result<Outcome> {
        ctl.set(Phase::Launching);
        let launched_at = now_secs().saturating_sub(2);
        log::info!("Launching {} ({})", self.game.display, self.emulator.name);
        let mut script = self.start_script("launch", &self.emulator.launch_script)?;
        let mut tracker = Tracker::new(Matcher::new(&self.emulator, &self.ignore), launched_at);
        if !tracker.can_track() {
            log::warn!("{} has no emulator folder or process name; can't track the game process", self.emulator.name);
        }

        let (outcome, post_ran) = self.wait_for_game(&mut script, &mut tracker, ctl);
        ctl.set(Phase::Exiting);
        log::info!("Game {outcome:?}");
        if !post_ran {
            self.run_post_script();
        }
        // Tidy up the launch script's temp file if it has finished by now.
        if let Some(s) = script.as_mut() {
            s.finished();
        }
        Ok(outcome)
    }

    /// Waits for the game to appear and then to go away. Returns the outcome
    /// and whether the post script has already been run (on a close request).
    fn wait_for_game(&self, script: &mut Option<Script>, tracker: &mut Tracker, ctl: &Control) -> (Outcome, bool) {
        let start = Instant::now();
        // Phase 1: until a game process shows up.
        loop {
            let script_ran = match script.as_mut() {
                Some(s) => s.finished().then(|| s.ran.unwrap_or_default()),
                None => Some(Duration::ZERO),
            };
            let procs = tracker.scan();
            if !procs.is_empty() {
                log::info!("Game process: {}", procs.join(", "));
                ctl.set_pids(&tracker.known);
                break;
            }
            if ctl.stop_requested() {
                return (self.close_while_loading(script, tracker, ctl, start), true);
            }
            match script_ran {
                // The script itself ran for a good while: it was the game.
                Some(ran) if ran >= BLOCKING_SCRIPT => return (Outcome::Finished { secs: ran.as_secs() }, false),
                Some(ran) if !tracker.can_track() || start.elapsed() > ran + APPEAR_TIMEOUT => return (Outcome::NeverStarted, false),
                // Still running with nothing to track: show it as playing.
                None if start.elapsed() >= BLOCKING_SCRIPT => ctl.set(Phase::Playing),
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        // Phase 2: until every game process has gone.
        ctl.set(Phase::Playing);
        let played = Instant::now();
        let mut last_full = Instant::now();
        let mut gone_since: Option<Instant> = None;
        let mut stop_at: Option<Instant> = None;
        let mut killed_at: Option<Instant> = None;
        let has_post = !self.emulator.post_script.trim().is_empty();
        loop {
            if ctl.stop_requested() && stop_at.is_none() {
                log::info!("Close requested; running post script");
                stop_at = Some(Instant::now());
                ctl.set(Phase::Exiting);
                self.run_post_script();
            }
            if let Some(at) = stop_at {
                let limit = if has_post { KILL_AFTER } else { KILL_AFTER_NO_SCRIPT };
                if killed_at.is_none() && at.elapsed() >= limit {
                    killed_at = Some(Instant::now());
                    tracker.kill_all();
                }
            }
            if killed_at.is_some_and(|at| at.elapsed() >= GIVE_UP_AFTER_KILL) {
                log::warn!("Some game processes couldn't be ended; returning to the menu anyway");
                break;
            }
            // Mostly just check the processes already found; rescan everything
            // now and then (new child processes) and before believing it's over.
            let alive = if gone_since.is_none() && last_full.elapsed() < FULL_SCAN_EVERY && tracker.known_alive() {
                true
            } else {
                last_full = Instant::now();
                let found = !tracker.scan().is_empty();
                ctl.set_pids(&tracker.known);
                found
            };
            if alive {
                gone_since = None;
            } else if gone_since.get_or_insert_with(Instant::now).elapsed() >= EXIT_CONFIRM {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        (Outcome::Finished { secs: played.elapsed().as_secs() }, stop_at.is_some())
    }

    /// A close request while the game was still loading: runs the post
    /// (close) script, then ends whatever the launch script started, including
    /// game processes that only show up in the next few seconds.
    fn close_while_loading(&self, script: &mut Option<Script>, tracker: &mut Tracker, ctl: &Control, start: Instant) -> Outcome {
        log::info!("Close requested while loading; running post script");
        ctl.set(Phase::Exiting);
        self.run_post_script();
        // A launch script still running after this long was being shown as the game.
        let blocking = script.as_mut().is_some_and(|s| !s.finished()) && start.elapsed() >= BLOCKING_SCRIPT;
        if let Some(s) = script.as_mut() {
            s.end();
        }
        let watch = Instant::now();
        let mut quiet_since: Option<Instant> = None;
        while watch.elapsed() < LATE_START_WATCH {
            if tracker.kill_all() {
                quiet_since = None;
            } else if quiet_since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(3) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if blocking {
            Outcome::Finished { secs: start.elapsed().as_secs() }
        } else {
            Outcome::NeverStarted
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Deletes temp files (.db copies, .bat scripts) left by earlier runs that
/// didn't exit cleanly. Files of a running instance are skipped: its pid is
/// in the name, and Windows refuses to delete what is open anyway.
pub fn remove_stale_temp_files() {
    let own = format!("{}-", std::process::id());
    let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(rp_db::TEMP_PREFIX) else { continue };
        let ours = rest.ends_with(".db") || rest.ends_with(".bat");
        // Names are "<pid>.db", "<pid>-<n>.db" or "<label>-<pid>[-<n>].bat".
        let current = rest.starts_with(&own) || rest.contains(&format!("-{own}")) || rest.contains(&format!("-{}.", std::process::id()));
        if ours && !current {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> LaunchJob {
        LaunchJob {
            popper_dir: PathBuf::from(r"C:\vPinball\PinUPSystem"),
            emulator: Emulator {
                id: 1,
                name: "Visual Pinball X".into(),
                media_dir: None,
                games_dir: Some(PathBuf::from(r"C:\vPinball\Tables")),
                roms_dir: None,
                launch_dir: Some(PathBuf::from(r"C:\vPinball\VisualPinball")),
                launch_script: String::new(),
                post_script: String::new(),
                keep_displays: vec![],
                process_name: None,
            },
            game: Game {
                id: 7,
                emu_id: 1,
                name: "Attack".into(),
                display: "Attack".into(),
                file_name: "Attack from Mars.vpx".into(),
                manufacturer: None,
                year: None,
                players: None,
                rom: Some("afm_113b".into()),
                alt_exe: None,
                custom: [None, None, None],
                alt_mode: None,
                tags: vec![],
            },
            ignore: vec!["Helper.EXE".into()],
        }
    }

    #[test]
    fn fills_tokens_case_insensitively_and_keeps_unknown_ones() {
        let j = job();
        let mut unknown = Vec::new();
        let out = j.fill_noting(r#"cd /d "[DIREMU]" & START "" x.exe "[gamename]" [GAMEEXT] [Rom] [ALTEXE]|[STARTDIR]x [NOPE] [a] [open"#, &mut unknown);
        assert_eq!(out, r#"cd /d "C:\vPinball\VisualPinball" & START "" x.exe "Attack from Mars" vpx afm_113b |C:\vPinball\PinUPSystem\x [NOPE] [a] [open"#);
        assert_eq!(unknown, ["NOPE"]);
        // The joined path uses the host separator, so only check its ends here.
        let full = j.fill("[GAMEFULLNAME]");
        assert!(full.starts_with(r"C:\vPinball\Tables") && full.ends_with("Attack from Mars.vpx"));
    }

    #[test]
    fn batch_body_uses_crlf_and_utf8_only_when_needed() {
        let mut j = job();
        assert_eq!(j.batch_body("launch", "echo [ROM]\necho b\r\n"), "echo afm_113b\r\necho b\r\n");
        j.game.rom = Some("café".into());
        assert_eq!(j.batch_body("launch", "echo [ROM]"), "@chcp 65001 >nul\r\necho café");
    }

    #[test]
    fn paths_match_on_whole_components_in_any_spelling() {
        assert_eq!(normalize(r"\\?\C:/vPinball/VisualPinball\\"), r"c:\vpinball\visualpinball");
        let dir = normalize(r"C:\vPinball\VisualPinball\");
        assert!(path_under(&normalize(r"c:\VPINBALL\visualpinball\VPinballX64.exe"), &dir));
        assert!(path_under(&normalize(r"C:/vPinball/VisualPinball/sub/x.exe"), &dir));
        assert!(!path_under(&normalize(r"C:\vPinball\VisualPinballX\VPinballX.exe"), &dir));
        assert!(!path_under(&dir, &dir));
        assert!(!path_under(&normalize(r"C:\x.exe"), ""));
    }

    #[test]
    fn matcher_uses_folder_process_name_and_ignore_list() {
        let j = job();
        let m = Matcher::new(&j.emulator, &j.ignore);
        assert!(m.can_track());
        assert!(m.matches(r"C:\vPinball\VisualPinball\VPinballX64.exe", "VPinballX64.exe"));
        assert!(!m.matches(r"C:\vPinball\VisualPinball\dmdext.exe", "DMDExt.exe"));
        assert!(!m.matches(r"C:\vPinball\VisualPinball\helper.exe", "helper.exe"));
        assert!(!m.matches(r"C:\Other\game.exe", "game.exe"));

        let mut emu = j.emulator.clone();
        emu.launch_dir = None;
        assert!(!Matcher::new(&emu, &[]).can_track());
        emu.process_name = Some("Pinball FX.exe".into());
        let m = Matcher::new(&emu, &[]);
        assert!(m.can_track());
        assert!(m.matches(r"D:\Games\PinballFX\pinball fx.exe", "Pinball FX.exe"));
        assert!(!m.matches(r"D:\Games\PinballFX\other.exe", "other.exe"));
    }

    #[test]
    fn control_reports_phase_and_stop() {
        let c = Control::default();
        assert_eq!(c.phase(), Phase::Launching);
        c.set(Phase::Playing);
        assert_eq!(c.phase(), Phase::Playing);
        assert!(!c.stop_requested());
        c.request_stop();
        assert!(c.stop_requested());
        assert!(c.game_pids().is_empty());
        c.set_pids(&[Pid::from_u32(42)]);
        assert_eq!(c.game_pids(), [42]);
    }
}
