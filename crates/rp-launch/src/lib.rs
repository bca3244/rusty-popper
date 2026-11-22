//! Runs per-emulator LaunchScript/PostScript and tracks the game process.
//!
//! Popper's scripts are Windows batch files with `[PLACEHOLDER]` tokens
//! (case-insensitive). We fill them in the same way Popper does, run them
//! hidden, then wait for the emulator's game process to finish before running
//! the post script.

use anyhow::{Context, Result};
use rp_db::{Emulator, Game};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Helper processes that live in an emulator folder but don't mean a game is
/// still running (killed by post scripts, or resident loaders).
const IGNORED: &[&str] = &["dmdext.exe", "fploader.exe"];

/// How long to wait for the game process to show up after the launch script.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(90);

/// Everything needed to launch one game.
#[derive(Debug, Clone)]
pub struct LaunchJob {
    pub popper_dir: PathBuf,
    pub emulator: Emulator,
    pub game: Game,
}

/// Where a running launch is, for the UI (stored in an [AtomicU8]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Launch script running / waiting for the game window.
    Launching = 0,
    /// The game process is running.
    Playing = 1,
    /// The game exited; post script running.
    Exiting = 2,
}

impl Phase {
    pub fn load(a: &AtomicU8) -> Phase {
        match a.load(Ordering::Relaxed) {
            1 => Phase::Playing,
            2 => Phase::Exiting,
            _ => Phase::Launching,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The game ran and exited after this many seconds.
    Finished { secs: u64 },
    /// No game process appeared in time (bad script, missing table...).
    NeverStarted,}

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

    /// Writes the filled script to a temp .bat and runs it hidden, waiting
    /// for the batch itself (which normally STARTs the emulator and returns).
    fn run_script(&self, label: &str, script: &str) -> Result<()> {
        if script.trim().is_empty() {
            return Ok(());
        }
        let bat = std::env::temp_dir().join(format!("rusty-popper-{label}-{}.bat", std::process::id()));
        // Batch files want CRLF line endings.
        let body = self.fill(script).replace("\r\n", "\n").replace('\n', "\r\n");
        std::fs::write(&bat, body).with_context(|| format!("writing {}", bat.display()))?;
        let cwd = self.emulator.launch_dir.clone().unwrap_or_else(|| self.popper_dir.clone());
        let status = Command::new("cmd")
            .arg("/c")
            .arg(&bat)
            .current_dir(&cwd)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .with_context(|| format!("running {label} script"))?;
        if !status.success() {
            eprintln!("{label} script exited with {status}");
        }
        let _ = std::fs::remove_file(&bat);
        Ok(())
    }

    /// Runs the whole game: launch script, wait for the game to exit, post
    /// script. Blocks; call it from a worker thread.
    pub fn run(&self, phase: &AtomicU8) -> Result<Outcome> {
        phase.store(Phase::Launching as u8, Ordering::Relaxed);
        let launched_at = now_secs().saturating_sub(2);
        println!("Launching {} ({})", self.game.display, self.emulator.name);
        self.run_script("launch", &self.emulator.launch_script)?;

        let outcome = match &self.emulator.launch_dir {
            Some(dir) => wait_for_game(dir, launched_at, phase),
            None => {
                eprintln!("{} has no emulator folder; can't track the game process", self.emulator.name);
                Outcome::NeverStarted
            }
        };
        phase.store(Phase::Exiting as u8, Ordering::Relaxed);
        println!("Game {:?}; running post script", outcome);
        self.run_script("post", &self.emulator.post_script)?;
        Ok(outcome)
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Waits until a process started from inside `emu_dir` after `since` appears,
/// then until all such processes have exited.
fn wait_for_game(emu_dir: &Path, since: u64, phase: &AtomicU8) -> Outcome {
    let Ok(mut sys) = System::new() else { return Outcome::NeverStarted };
    let emu_dir = emu_dir.to_string_lossy().to_lowercase();
    let mut running = |sys: &mut System| -> Vec<String> {
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet));
        sys.processes()
            .values()
            .filter(|p| p.start_time() >= since)
            .filter_map(|p| {
                let exe = p.exe()?.to_string_lossy().to_lowercase();
                let name = p.name().to_string_lossy().to_lowercase();
                (exe.starts_with(&emu_dir) && !IGNORED.contains(&name.as_str())).then_some(name)
            })
            .collect()
    };

    let start = Instant::now();
    loop {
        let procs = running(&mut sys);
        if !procs.is_empty() {
            println!("Game process: {}", procs.join(", "));
            phase.store(Phase::Playing as u8, Ordering::Relaxed);
            break;
        }
        if start.elapsed() > APPEAR_TIMEOUT {
            return Outcome::NeverStarted;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let played = Instant::now();
    while !running(&mut sys).is_empty() {
        std::thread::sleep(Duration::from_millis(750));
    }
    Outcome::Finished { secs: played.elapsed().as_secs() }
}
