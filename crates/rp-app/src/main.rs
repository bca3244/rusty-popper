//! Rusty Popper: game wheel over the playfield video, with every cabinet
//! screen showing the selected game's media.
//!
//! Usage:
//!   rusty-popper [--popper DIR] [--mpv PATH\libmpv-2.dll] [--game TEXT]
//!                [--wheel arc|flow|minimal|vertical] [--seconds N] [--list] [--dry-run]
//!                [--print-launch]
//!
//! Browse with Left/Right (or Popper's bound buttons), W switches wheel style,
//! Esc (or Popper's exit buttons) opens the exit menu; Q quits at once.
//! Everything printed also goes to rusty-popper.log next to the exe.

// A GUI program: no console window on the cabinet. Started from a terminal,
// output still goes there (see win::attach_parent_console).
#![windows_subsystem = "windows"]

mod assets;
mod discover;
mod dof;
mod errscreen;
mod gameaudio;
mod input;
mod library;
mod logging;
mod menu;
mod mpv;
mod overlay;
mod perf;
mod reload;
mod remote;
mod render;
mod session;
mod settings;
mod sfx;
mod sysload;
mod updater;
mod video;
mod wheel;
mod win;

use anyhow::{anyhow, bail, Context, Result};
use femtovg::{renderer::OpenGl, Canvas, ImageFlags};
use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, PossiblyCurrentGlContext};
use glutin::display::{Display, GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use library::{Item, Kind, Library};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rp_config::{self as config, Screen, Slot};
use rp_db as db;
use settings::{Settings, WheelStyle};
use std::cell::Cell;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

/// The always-on menu screens; GameInfo/GameHelp/etc. are on-demand overlays.
const LIVE: &[Slot] = &[Slot::Topper, Slot::Dmd, Slot::BackGlass, Slot::PlayField, Slot::FullDmd];

/// Plays shorter than this (a table that failed to load, a mis-press) aren't
/// counted in Popper's statistics.
const MIN_PLAY_SECS: u64 = 20;
/// How many entries Page Prior/Next move in a playlist that isn't alphabetical.
const PAGE_STEP: i64 = 5;
/// How often to look whether Popper's database file has changed.
const DB_CHECK_EVERY: Duration = Duration::from_secs(15);
/// The database must have been left alone this long before it is re-read.
const DB_SETTLE: Duration = Duration::from_secs(5);
/// After a game, how long to keep taking the focus back from whatever has it.
const RECLAIM_AFTER_GAME: Duration = Duration::from_secs(10);
/// While a game runs, how often to look at the exit button.
const PLAY_POLL: Duration = Duration::from_millis(50);
/// A frame never takes less than this, so a display without vsync (or a
/// covered window) can't turn the render loop into a busy loop.
const MIN_FRAME: Duration = Duration::from_millis(4);
/// This long after the last button press the menu counts as still (its
/// animations are over), and draws only when the video has a new frame.
const STILL_AFTER: Duration = Duration::from_millis(1200);
/// While the menu is still, how often to look for a new video frame or input.
const STILL_POLL: Duration = Duration::from_millis(2);
/// Time per frame for turning loaded logos into textures.
const LOGO_UPLOAD_BUDGET: Duration = Duration::from_millis(3);
/// A secondary screen with nothing new is still redrawn this often.
const REDRAW_EVERY: Duration = Duration::from_secs(1);
/// This many failed frames in a row (about ten seconds) and the menu restarts itself.
const MAX_FRAME_ERRORS: u32 = 200;
/// How many times in a row the menu restarts itself before giving up.
const MAX_RESTARTS: u32 = 3;
/// A menu that ran this long before failing wasn't part of a row of failures:
/// its restart counts as the first.
const HEALTHY_UPTIME: Duration = Duration::from_secs(300);
/// If the game's window hasn't been seen by then, the game counts as on
/// screen this long after its process appeared.
const GAME_UP_TIMEOUT: Duration = Duration::from_secs(60);
/// The same for the playfield's "Launching" text: if no game window has
/// covered the playfield by then, it switches to "Exiting" anyway. Big
/// tables can take over a minute to load on a first run (textures, shaders).
const TABLE_UP_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a restart or shutdown from the phone waits for a game to close.
const POWER_WAIT: Duration = Duration::from_secs(30);

/// How many restarts in a row led to this run (`--restarted`).
static RESTARTED: AtomicU32 = AtomicU32::new(0);
/// Set while the menu's windows are up: a crash then restarts the menu.
static MENU_UP: AtomicBool = AtomicBool::new(false);
static STARTED: OnceLock<Instant> = OnceLock::new();
/// This run counted as a try of a freshly installed update.
static PROBATION_ACTIVE: AtomicBool = AtomicBool::new(false);
/// A fresh copy has already been started, so this one must not roll back or
/// show an error box over it.
static SELF_RESTARTED: AtomicBool = AtomicBool::new(false);
/// The menu got as far as showing (so a later error is a stop, not a failed start).
static MENU_RAN: AtomicBool = AtomicBool::new(false);

struct Args {
    popper: Option<PathBuf>,
    mpv: Option<PathBuf>,
    game: Option<String>,
    wheel: Option<WheelStyle>,
    seconds: Option<u64>,
    list: bool,
    dry_run: bool,
    print_launch: bool,
    /// How many times in a row the menu has restarted itself after a failure.
    restarted: u32,
}

fn parse_args() -> Result<Args> {
    let mut a = Args { popper: None, mpv: None, game: None, wheel: None, seconds: None, list: false, dry_run: false, print_launch: false, restarted: 0 };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--popper" => a.popper = Some(val()?.into()),
            "--mpv" => a.mpv = Some(val()?.into()),
            "--game" => a.game = Some(val()?),
            "--wheel" => {
                let v = val()?;
                a.wheel = Some(WheelStyle::parse(&v).ok_or_else(|| anyhow!("--wheel must be arc, flow, minimal or vertical, not {v}"))?);
            }
            "--seconds" => a.seconds = Some(val()?.parse()?),
            "--restarted" => a.restarted = val()?.parse()?,
            "--list" => a.list = true,
            "--dry-run" => a.dry_run = true,
            "--print-launch" => a.print_launch = true,
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(a)
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default()
}

/// libmpv-2.dll: `--mpv`, then `[paths] mpv_dll`, then the RUSTY_POPPER_MPV
/// environment variable, then next to the exe.
fn find_mpv_dll(explicit: Option<PathBuf>, configured: Option<PathBuf>) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = explicit.into_iter().chain(configured).collect();
    candidates.extend(std::env::var_os("RUSTY_POPPER_MPV").map(PathBuf::from));
    candidates.push(exe_dir().join("libmpv-2.dll"));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("libmpv-2.dll not found: put it next to the exe, set [paths] mpv_dll in rusty-popper.toml, or pass --mpv"))
}

fn db_mtime(popper: &Path) -> Option<SystemTime> {
    std::fs::metadata(popper.join("PUPDatabase.db")).and_then(|m| m.modified()).ok()
}

/// Whether a media file is a video (as opposed to a still image).
fn is_video(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    !matches!(ext.as_str(), "png" | "jpg" | "jpeg")
}

fn main() {
    win::attach_parent_console();
    logging::init(&exe_dir().join("rusty-popper.log"));
    STARTED.get_or_init(Instant::now);
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
        log::logger().flush();
        // A crash of the menu itself (not of a helper thread, which is coped
        // with) would bare the desktop: start a fresh copy instead.
        if std::thread::current().name() == Some("main") && MENU_UP.swap(false, Ordering::Relaxed) {
            restart_self();
            log::logger().flush();
            std::process::exit(101);
        }
    }));
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            log::error!("{e:#}");
            log::logger().flush();
            // A freshly installed update that can't start goes straight back
            // to the previous version.
            let handed_over = SELF_RESTARTED.load(Ordering::Relaxed);
            if !handed_over && PROBATION_ACTIVE.load(Ordering::Relaxed) && updater::on_probation(&exe_dir()) {
                if let Err(r) = updater::rollback(&exe_dir(), &format!("{e:#}")) {
                    log::error!("Rollback failed: {r:#}");
                } else {
                    log::logger().flush();
                    std::process::exit(1);
                }
            }
            // Started at logon there's no console, so say what went wrong on
            // screen rather than leaving a bare desktop.
            if !handed_over {
                let title = if MENU_RAN.load(Ordering::Relaxed) { "Rusty Popper stopped" } else { "Rusty Popper couldn't start" };
                let rot = Settings::load(&exe_dir().join("rusty-popper.toml")).playfield.ui_rotation;
                errscreen::show(&exe_dir(), title, &format!("{e:#}"), rot);
            }
            1
        }
    };
    log::logger().flush();
    std::process::exit(code);
}

fn run() -> Result<()> {
    let args = parse_args()?;
    RESTARTED.store(args.restarted, Ordering::Relaxed);
    log::info!("Rusty Popper {} starting", env!("CARGO_PKG_VERSION"));
    // Only a real menu start counts as a try of a new version (not --list etc.).
    let ui_run = !(args.list || args.dry_run || args.print_launch);
    if ui_run {
        match updater::start_probation(&exe_dir()) {
            updater::Probation::GiveUp => match updater::rollback(&exe_dir(), "the new version kept failing to start") {
                Ok(()) => return Ok(()),
                // Nothing to go back to: carry on with this version.
                Err(e) => log::error!("Rollback failed, staying on {}: {e:#}", env!("CARGO_PKG_VERSION")),
            },
            updater::Probation::OnProbation => PROBATION_ACTIVE.store(true, Ordering::Relaxed),
            updater::Probation::None => {}
        }
    }
    let settings = Settings::load(&exe_dir().join("rusty-popper.toml"));
    let style = args.wheel.unwrap_or(settings.wheel.style);
    let found = discover::find_popper(args.popper.clone(), settings.paths.popper_dir.clone())?;
    let popper = found.dir;

    // libmpv is a big DLL: load it while the database is read.
    let (mpv_arg, mpv_conf) = (args.mpv.clone(), settings.paths.mpv_dll.clone());
    let mpv_loading = std::thread::spawn(move || -> Result<(PathBuf, mpv::Mpv)> {
        let dll = find_mpv_dll(mpv_arg, mpv_conf)?;
        let mpv = mpv::Mpv::load(&dll)?;
        Ok((dll, mpv))
    });
    rp_launch::remove_stale_temp_files();
    let db_seen = db_mtime(&popper);
    let t = Instant::now();
    let lib = Library::load(&popper).with_context(|| format!("reading Popper's database in {}", popper.display()))?;
    perf::note("Reading Popper's database", t, 300);
    let mut joy_overrides: Vec<(u32, input::Action)> = Vec::new();
    let inp = &settings.input;
    for (buttons, action) in [
        (&inp.menu_joy_buttons, input::Action::Menu),
        (&inp.letter_prev_joy_buttons, input::Action::LetterPrev),
        (&inp.letter_next_joy_buttons, input::Action::LetterNext),
        (&inp.playlist_joy_buttons, input::Action::Playlists),
    ] {
        joy_overrides.extend(buttons.iter().map(|b| (*b, action)));
    }
    let bindings = input::Bindings::new(&lib.functions, &joy_overrides, inp.keyboard_quit);
    log::info!("Playlists: {}", lib.playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
    log::info!("DB: {} visible emulators, {} games on the wheel", lib.emulator_count, lib.game_count);
    log::info!("Input: {}", bindings.describe());

    if args.list {
        for it in &lib.items[..lib.game_count] {
            let Kind::Game { game, emu } = &it.kind else { continue };
            log::info!("  [{}] {} ({}){}", emu.name, it.title, game.file_name, if it.wheel_png.is_none() { "  [no wheel image]" } else { "" });
        }
        return Ok(());
    }

    let start = match &args.game {
        Some(q) => {
            let q = q.to_lowercase();
            lib.items[..lib.game_count]
                .iter()
                .position(|it| it.title.to_lowercase().contains(&q) || it.media_name.to_lowercase().contains(&q))
                .ok_or_else(|| anyhow!("no visible game matches {q:?}"))?
        }
        None => 0,
    };

    let all_screens = config::read_screens(&popper.join("PinUpPlayer.ini"))?;
    // Where Popper shows the info / help cards (used to pick which of our screens draws them).
    let card_screen = |slot: Slot| all_screens.iter().find(|s| s.slot == slot && s.width > 0 && s.height > 0).cloned();
    let card_screens = (card_screen(Slot::GameInfo), card_screen(Slot::GameHelp));
    let screens: Vec<Screen> = all_screens.into_iter().filter(|s| LIVE.contains(&s.slot) && s.width > 0 && s.height > 0).collect();
    if !screens.iter().any(|s| s.slot == Slot::PlayField) {
        bail!("PinUpPlayer.ini has no PlayField screen");
    }
    log::info!("Wheel style: {style:?}; start: {}", lib.items[start].title);
    for s in &screens {
        let m = lib.items[start].media(s.slot);
        log::info!("  {:?}: {}x{} at {},{} rot {} -> {}", s.slot, s.width, s.height, s.x, s.y, s.rotation, m.as_ref().map_or("(none)".into(), |p| p.display().to_string()));
    }

    if args.print_launch {
        if let Kind::Game { game, emu } = &lib.items[start].kind {
            let job = rp_launch::LaunchJob { popper_dir: popper.clone(), emulator: (**emu).clone(), game: (**game).clone(), ignore: Vec::new() };
            log::info!("--- launch script ---\n{}\n--- post script ---\n{}", job.fill(&emu.launch_script), job.fill(&emu.post_script));
        }
        return Ok(());
    }
    let t = Instant::now();
    let (dll, mpv) = mpv_loading.join().map_err(|_| anyhow!("loading libmpv panicked"))??;
    perf::note("Waiting for libmpv", t, 100);
    log::info!("libmpv: {}", dll.display());
    if args.dry_run {
        return Ok(());
    }

    if found.searched && ui_run {
        // Remember it so the next start goes straight there.
        match settings::save_popper_dir(&exe_dir().join("rusty-popper.toml"), &popper) {
            Ok(()) => log::info!("Saved popper_dir = {} to rusty-popper.toml", popper.display()),
            Err(e) => log::warn!("Couldn't save popper_dir to rusty-popper.toml: {e:#}"),
        }
    }

    let volume = settings.audio.volume.unwrap_or_else(|| screens.iter().find(|s| s.slot == Slot::PlayField).map_or(100, |s| s.volume)).clamp(0, 100);
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let now = Instant::now();
    let game_audio = gameaudio::GameAudio::new(&mpv, settings.sound.game_audio, settings.sound.game_audio_volume);
    let mut app = App {
        game_audio,
        dof: dof::Dof::new(settings.dof.enabled),
        dof_shown: None,
        mpv,
        screens,
        card_screens,
        card: None,
        notice: None,
        updater: updater::Updater::new(settings.update.check, &settings.update.repo, &exe_dir()),
        update_confirmed: false,
        sounds: sfx::Sounds::load(
            &settings.sound.dir.clone().unwrap_or_else(|| popper.join("PUPVideos").join("PinUpMenu").join("PuPSounds")),
            settings.sound.enabled,
            settings.sound.volume,
        ),
        lib,
        bindings,
        key_repeat: input::KeyRepeat::default(),
        joy_overrides,
        keyboard_quit: inp.keyboard_quit,
        joy_needs_focus: inp.joystick_needs_focus,
        ignore_processes: settings.launch.ignore_processes.clone(),
        style,
        start,
        ui_rotation_override: settings.playfield.ui_rotation,
        deadline: args.seconds.map(|s| now + Duration::from_secs(s)),
        gfx: None,
        error: None,
        frame_errors: 0,
        restart: false,
        menu: None,
        popper_dir: popper.clone(),
        playing: None,
        playlist: 0,
        view: Vec::new(),
        attract: settings.attract,
        volume,
        video_sound: settings.audio.enabled,
        last_input: now,
        attract_on: false,
        next_attract: now,
        rng: (SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)) | 1,
        letter: None,
        stats: Arc::new(std::sync::Mutex::new(db::StatsWriter::new(&popper.join("PUPDatabase.db"), &exe_dir().join("backups")))),
        reload: None,
        db_seen,
        db_dirty_since: None,
        last_db_check: now,
        unfocused_since: None,
        last_reclaim: now,
        reclaim_until: now + Duration::from_secs(5),
        joy_blocked: false,
        others: rp_launch::OtherLaunches::new(),
        aside: None,
        last_aside_check: now,
        quiet_until: now,
        remote: None,
        remote_cfg: settings.remote,
        remote_error: None,
        pending_power: None,
    };
    if app.remote_cfg.enabled {
        // A copy this one replaces (a restart, an update) may still hold the port.
        let replacing = args.restarted > 0 || PROBATION_ACTIVE.load(Ordering::Relaxed);
        app.set_remote(true, if replacing { Duration::from_secs(4) } else { Duration::ZERO });
    }
    MENU_UP.store(true, Ordering::Relaxed);
    MENU_RAN.store(true, Ordering::Relaxed);
    let ran = event_loop.run_app(&mut app);
    MENU_UP.store(false, Ordering::Relaxed);
    ran?;
    if app.restart {
        restart_self();
    }
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Starts a fresh copy of the menu after rendering broke for good (a display
/// driver reset, say) or the menu crashed, so the cabinet isn't left on the
/// desktop.
fn restart_self() {
    let healthy = STARTED.get().is_some_and(|t| t.elapsed() >= HEALTHY_UPTIME);
    let restarted = if healthy { 0 } else { RESTARTED.load(Ordering::Relaxed) };
    if restarted >= MAX_RESTARTS {
        log::error!("Rendering keeps failing after {restarted} restarts; giving up");
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    // The same arguments, with the restart count replaced.
    let passed = updater::passthrough_args();
    log::warn!("Restarting the menu (attempt {})", restarted + 1);
    std::thread::sleep(Duration::from_secs(2));
    match std::process::Command::new(exe).args(passed).arg("--restarted").arg((restarted + 1).to_string()).spawn() {
        Ok(_) => SELF_RESTARTED.store(true, Ordering::Relaxed),
        Err(e) => log::error!("Couldn't restart: {e}"),
    }
}

/// The menu out of the way of a table another program started.
struct Aside {
    pid: u32,
    name: String,
    /// Which of our windows were showing, to restore them.
    visible: Vec<bool>,
}

/// One cabinet screen: its window, GL surface and mpv player.
struct ScreenOut {
    screen: Screen,
    window: Window,
    surface: Surface<WindowSurface>,
    player: mpv::Player,
    /// Offscreen texture mpv renders into (None if it couldn't be created:
    /// then mpv draws straight to the window and there's no slide).
    target: Option<video::Target>,
    slide: Option<video::Slide>,
    /// Whether the file asked for by the last `show` has been opened. Until
    /// then a "playback restarted" event still belongs to the previous video.
    file_loaded: Cell<bool>,
    /// Tag of the last `show`, and whether mpv has replied to it yet: until
    /// it has, a "file loaded" event is still the previous file's.
    load_tag: Cell<u64>,
    load_acked: Cell<bool>,
    /// When the last `show` was asked for, to log how long the switch took.
    loaded_at: Cell<Option<Instant>>,
    /// Whether the decoder mpv picked has been logged yet.
    hwdec_logged: Cell<bool>,
    /// Whether the file asked for by the last `show` is a video.
    wants_video: Cell<bool>,
    /// Must be redrawn even without a new video frame (new file, shown again,
    /// a card just closed). Secondary screens skip frames with nothing new.
    dirty: Cell<bool>,
    /// When this screen was last presented, for a periodic safety redraw.
    drawn_at: Cell<Instant>,
    /// Whether a card was on this screen last frame.
    had_card: Cell<bool>,
}

impl ScreenOut {
    /// Plays `media`, or for a secondary screen with nothing to show, hides
    /// its window (like Popper does) so it doesn't cover what's beneath it.
    /// Topper and DMD rectangles can overlap other screens on some cabinets.
    fn show(&self, media: Option<&Path>) {
        self.load(media);
        if self.screen.slot != Slot::PlayField {
            self.window.set_visible(media.is_some());
        }
    }

    /// Plays `media` without showing or hiding the window.
    fn load(&self, media: Option<&Path>) {
        self.file_loaded.set(false);
        self.load_acked.set(false);
        self.dirty.set(true);
        self.loaded_at.set(media.map(|_| Instant::now()));
        self.wants_video.set(media.is_some_and(is_video));
        match self.player.show(media) {
            Ok(tag) => self.load_tag.set(tag),
            Err(e) => {
                log::warn!("{:?}: {e}", self.screen.slot);
                self.load_acked.set(true);
            }
        }
    }

    /// Drains the player's events. Returns true if mpv is shutting down.
    fn pump_events(&mut self) -> bool {
        let mut shutdown = false;
        for (ev, tag) in self.player.poll_events() {
            match ev {
                mpv::EVENT_SHUTDOWN => shutdown = true,
                mpv::EVENT_COMMAND_REPLY if tag != 0 && tag == self.load_tag.get() => self.load_acked.set(true),
                mpv::EVENT_FILE_LOADED if self.load_acked.get() => {
                    self.file_loaded.set(true);
                    self.dirty.set(true);
                }
                // Draw once more even without a new frame, so a stopped or
                // reconfigured video doesn't leave its last picture up.
                mpv::EVENT_END_FILE | mpv::EVENT_VIDEO_RECONFIG => self.dirty.set(true),
                mpv::EVENT_PLAYBACK_RESTART if self.file_loaded.get() => {
                    if let Some(s) = self.slide.as_mut() {
                        s.video_ready();
                    }
                    if let Some(at) = self.loaded_at.take() {
                        let ms = at.elapsed().as_millis();
                        if ms >= perf::SLOW_SWITCH_MS {
                            log::info!("perf: {:?} media took {ms} ms to start", self.screen.slot);
                        }
                    }
                    // Logged for the first video (a still image says nothing).
                    if self.wants_video.get() && !self.hwdec_logged.replace(true) {
                        let dec = self.player.get_string("hwdec-current").unwrap_or_else(|| "?".into());
                        log::info!("{:?}: video decoder hwdec={dec}", self.screen.slot);
                    }
                }
                _ => {}
            }
        }
        shutdown
    }
}

// Field order matters: fields drop top to bottom, and everything that frees
// GL objects (players, canvas) must go while the context still exists.
struct Gfx {
    outs: Vec<ScreenOut>, // the playfield is outs[0]
    canvas: Canvas<OpenGl>,
    fonts: wheel::Fonts,
    entries: Vec<wheel::Entry>,
    logos: assets::Loader,
    wheel: wheel::Wheel,
    joysticks: input::Joysticks,
    shown: usize,    // index whose media is on the screens
    t0: Instant,
    last: Instant,
    perf: perf::Frames,
    /// Whether the shared context's swap interval is set to wait for vsync
    /// (None: unknown). It's per context, not per window, so it is switched
    /// on just for the playfield's swap and off for the other screens'.
    vsync_on: Option<bool>,
    /// The playfield's refresh period: frames are never shorter than this,
    /// even if the driver ignores vsync.
    frame_period: Duration,
    pacer: perf::Pacer,
    /// The playfield window's handle, for taking the focus back.
    pf_hwnd: isize,
    gl: glow::Context,
    _loader: mpv::Loader,
    context: PossiblyCurrentContext,
    /// Whether the windows have been shown (after the first frame).
    revealed: bool,
    /// The phone remote's QR code texture, while the remote is on.
    qr: Option<Qr>,
}

/// The phone remote's QR code, as uploaded for drawing.
struct Qr {
    img: femtovg::ImageId,
    /// Pixels (modules) per side.
    modules: usize,
    /// The remote's URL version it was made from.
    version: u64,
}

impl Gfx {
    /// Frees the wheel logo textures (all but `keep`) and stops the loader,
    /// e.g. to give the video memory to a running game.
    fn release_logos(&mut self, keep: Option<usize>) {
        self.logos = assets::idle();
        if self.context.make_current(&self.outs[0].surface).is_err() {
            return;
        }
        for (i, e) in self.entries.iter_mut().enumerate() {
            if Some(i) == keep {
                continue;
            }
            if let Some(l) = e.logo.take() {
                self.canvas.delete_image(l.img);
                self.canvas.delete_image(l.glow);
            }
        }
    }

    /// (Re)starts loading the logos that aren't in memory, nearest `near` first.
    fn load_logos(&mut self, items: &[Item], near: usize) {
        let paths = items.iter().zip(&self.entries).map(|(it, e)| if e.logo.is_some() { None } else { it.wheel_png.clone() }).collect();
        self.logos = assets::spawn_loader(paths, near);
    }
}

/// A second Exit press this long after the first force-closes the game
/// (sooner, a table may still be saving as it closes normally).
const FORCE_AFTER: Duration = Duration::from_secs(3);

/// Exit menu entries. "Back" is first and selected on open so a stray press
/// of Start never reboots or shuts down the cabinet.
const EXIT_MENU: &[&str] = &["Back to games", "Options", "Exit Rusty Popper", "Restart cabinet", "Shut down cabinet"];

/// Choices the Options screen cycles through with Start.
const ATTRACT_IDLE_CHOICES: &[u64] = &[60, 120, 180, 300, 600];
const ATTRACT_INTERVAL_CHOICES: &[u64] = &[6, 12, 20, 30];
const OPTION_ROWS: usize = 7; // style, attract (submenu), sound (submenu), DOF, phone remote, update, back
/// The Options rows, in the order `option_labels` lists them (Back is last).
const STYLE_ROW: usize = 0;
/// The rows that open the Attract mode and Sound submenus.
const ATTRACT_ROW: usize = 1;
const SOUND_ROW: usize = 2;
const DOF_ROW: usize = 3;
const REMOTE_ROW: usize = 4;
const UPDATE_ROW: usize = 5;

/// The rows of Options → Attract mode (see `App::attract_rows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttractRow {
    Toggle,
    StartsAfter,
    TimePerGame,
    Back,
}

/// The rows of Options → Sound (see `App::sound_rows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoundRow {
    Video,
    VideoVolume,
    Effects,
    EffectsVolume,
    GameAudio,
    GameAudioVolume,
    Back,
}

fn next_choice(choices: &[u64], cur: u64) -> u64 {
    let i = choices.iter().position(|&c| c > cur).unwrap_or(0);
    choices[i]
}

/// The game currently running.
struct Playing {
    rx: Receiver<Result<rp_launch::Outcome>>,
    /// Phase reported by, and close requests sent to, the worker.
    ctl: Arc<rp_launch::Control>,
    index: usize,
    since: Instant,
    /// When the game's process was first seen.
    process_since: Option<Instant>,
    /// The game's window is on screen (its process appears well before that,
    /// while the table still loads).
    game_up: bool,
    /// A game window covers the playfield (the table is drawn): the
    /// playfield's text behind it switches from "Launching" to "Exiting".
    table_up: bool,
    /// When the game was seen to exit (restarts the screen's animation).
    exiting_since: Option<Instant>,
    /// Screens not in KeepDisplays are paused (and black) only while the game is on screen.
    others_paused: bool,
    /// A screen the emulator keeps is showing a video, so it needs real frame rates.
    kept_video: bool,
    logos_released: bool,
    /// When the Exit button first asked the game to close.
    stop_pressed: Option<Instant>,
    last_render: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Exit,
    Options,
    /// Options → Attract mode: on/off and its timings.
    Attract,
    /// Options → Sound: every volume and sound setting.
    Sound,
}

struct ExitMenu {
    kind: MenuKind,
    selected: usize,
    opened: Instant,
}

/// Restarts (`/r`) or shuts down (`/s`) Windows, then exits.
fn power(el: &ActiveEventLoop, flag: &str) {
    log::info!("Running shutdown {flag} /t 0");
    use std::os::windows::process::CommandExt;
    // No console window flashing up.
    if let Err(e) = std::process::Command::new("shutdown").args([flag, "/t", "0"]).creation_flags(rp_launch::CREATE_NO_WINDOW).spawn() {
        log::error!("shutdown {flag} failed: {e}");
    }
    el.exit();
}

struct App {
    mpv: mpv::Mpv,
    screens: Vec<Screen>,
    /// Popper's GameInfo / GameHelp screen rectangles, if configured.
    card_screens: (Option<Screen>, Option<Screen>),
    /// The info / help card being shown.
    card: Option<overlay::Card>,
    /// A short message over the wheel ("Couldn't start ...") and when it appeared.
    notice: Option<(String, Instant)>,
    sounds: sfx::Sounds,
    updater: updater::Updater,
    /// Whether a fresh update has been confirmed as working (or there was none).
    update_confirmed: bool,
    /// The games and playlists, as last read from Popper's database.
    lib: Library,
    bindings: input::Bindings,
    /// Browsing keys held down, repeated on our own beat.
    key_repeat: input::KeyRepeat,
    /// From rusty-popper.toml; kept to rebuild `bindings` when the database changes.
    joy_overrides: Vec<(u32, input::Action)>,
    keyboard_quit: bool,
    joy_needs_focus: bool,
    ignore_processes: Vec<String>,
    style: WheelStyle,
    start: usize,
    /// `[playfield] ui_rotation`, overriding Popper's ThumbRotate.
    ui_rotation_override: Option<i32>,
    deadline: Option<Instant>,
    gfx: Option<Gfx>,
    error: Option<anyhow::Error>,
    /// Frames that failed in a row.
    frame_errors: u32,
    /// Set when the menu should start a fresh copy of itself on exit.
    restart: bool,
    menu: Option<ExitMenu>,
    popper_dir: PathBuf,
    /// The game currently running, if any: finishes when the worker sends.
    playing: Option<Playing>,
    /// Records plays in Popper's GamesStats (shared with the game thread).
    stats: Arc<std::sync::Mutex<db::StatsWriter>>,
    /// 0 = all games, n = playlists[n - 1].
    playlist: usize,
    /// Item indices in the current playlist, in wheel order.
    view: Vec<usize>,
    attract: settings::AttractSettings,
    /// Playfield video volume (0-100), adjustable in Options; kept while
    /// `video_sound` is off.
    volume: i32,
    /// Whether the playfield video plays its sound.
    video_sound: bool,
    last_input: Instant,
    attract_on: bool,
    next_attract: Instant,
    rng: u64,
    /// Letter shown after a jump-by-letter, and when.
    letter: Option<(char, Instant)>,
    /// A re-read of the database in progress on a worker thread.
    reload: Option<Receiver<Result<Library>>>,
    /// Modification time of the database file when last looked at.
    db_seen: Option<SystemTime>,
    /// When the database file was seen to change (it is re-read once it settles).
    db_dirty_since: Option<Instant>,
    last_db_check: Instant,
    /// Since when none of our windows has had the focus.
    unfocused_since: Option<Instant>,
    last_reclaim: Instant,
    /// Until then, take the focus back even from another program.
    reclaim_until: Instant,
    /// Joystick input was being ignored (another program had the focus).
    joy_blocked: bool,
    /// The selected game's Audio media, played after a short rest on it.
    game_audio: gameaudio::GameAudio,
    /// Cabinet toys through DOF (optional; off by default).
    dof: dof::Dof,
    /// The item DOF last heard was selected.
    dof_shown: Option<usize>,
    /// Spots tables started by other programs.
    others: rp_launch::OtherLaunches,
    /// Set while such a table is on screen and the menu has stepped aside.
    aside: Option<Aside>,
    last_aside_check: Instant,
    /// No taking the focus back from the desktop before this.
    quiet_until: Instant,
    /// The phone remote's web server, while it's on.
    remote: Option<remote::Remote>,
    /// `[remote]` from rusty-popper.toml (`enabled` follows Options).
    remote_cfg: settings::RemoteSettings,
    /// Why the phone remote couldn't start, for Options.
    remote_error: Option<String>,
    /// A restart (`/r`) or shutdown (`/s`) from the phone, waiting for the
    /// game to close, and when it was asked for.
    pending_power: Option<(&'static str, Instant)>,
}

/// The app icon as a PNG (also the phone remote's page icon).
pub(crate) static ICON_PNG: &[u8] = include_bytes!("../assets/rusty-popper-256.png");

/// The app icon (copper-ring RP wheel) for the windows' title/Alt-Tab icon.
fn window_icon() -> Option<winit::window::Icon> {
    // Decoded once and shared by every window.
    static RGBA: OnceLock<Option<(Vec<u8>, u32, u32)>> = OnceLock::new();
    let (rgba, w, h) = RGBA
        .get_or_init(|| {
            let img = image::load_from_memory(ICON_PNG).ok()?.to_rgba8();
            let (w, h) = img.dimensions();
            Some((img.into_raw(), w, h))
        })
        .as_ref()?;
    winit::window::Icon::from_rgba(rgba.clone(), *w, *h).ok()
}

fn window_attrs(s: &Screen) -> winit::window::WindowAttributes {
    Window::default_attributes()
        .with_title(format!("Rusty Popper {:?}", s.slot))
        .with_window_icon(window_icon())
        .with_decorations(false)
        .with_resizable(false)
        .with_position(PhysicalPosition::new(s.x, s.y))
        .with_inner_size(PhysicalSize::new(s.width as u32, s.height as u32))
        .with_skip_taskbar(true)
        // Shown after the first frame: an undrawn window shows up white.
        .with_visible(false)
}

fn hwnd_of(window: &Window) -> isize {
    match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() {
            if let Err(e) = self.init(el) {
                self.error = Some(e);
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            // Synthetic presses are winit replaying held keys when focus moves
            // between our windows (e.g. a screen shown/hidden by a media swap);
            // acting on them made one Start press both switch playlist and launch.
            // Windows' own key repeats are ignored: held browsing keys repeat
            // on the same beat as joystick buttons (see KeyRepeat).
            WindowEvent::KeyboardInput { event, is_synthetic, .. } if event.state == ElementState::Pressed && !is_synthetic && !event.repeat => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(a) = self.bindings.key(code, false) {
                        self.key_repeat.pressed(code, a);
                        self.act(el, a);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if event.state == ElementState::Released => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.key_repeat.released(code);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.frame(el) {
            Ok(()) => self.frame_errors = 0,
            // A failed frame (a display mode change, a driver hiccup) is
            // retried rather than ending the menu and baring the desktop.
            Err(e) => {
                self.frame_errors += 1;
                if self.frame_errors % 100 == 1 {
                    log::error!("Frame failed ({} in a row): {e:#}", self.frame_errors);
                }
                if self.frame_errors >= MAX_FRAME_ERRORS && self.playing.is_none() {
                    // It isn't coming back: start afresh.
                    self.error = Some(e.context("rendering kept failing"));
                    self.restart = true;
                    el.exit();
                } else {
                    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50)));
                }
            }
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        MENU_UP.store(false, Ordering::Relaxed);
        // Frees the port at once, for a copy started by a restart or update.
        self.remote = None;
        self.dof.shutdown();
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_choices_cycle() {
        assert_eq!(next_choice(ATTRACT_IDLE_CHOICES, 60), 120);
        assert_eq!(next_choice(ATTRACT_IDLE_CHOICES, 600), 60);
        assert_eq!(next_choice(ATTRACT_IDLE_CHOICES, 200), 300); // an off-list value moves to the next step up
        assert_eq!(next_choice(ATTRACT_INTERVAL_CHOICES, 999), 6);
    }

    #[test]
    fn stills_are_not_video() {
        assert!(is_video(Path::new(r"C:\m\Topper\a.mp4")));
        assert!(is_video(Path::new("a.GIF")));
        assert!(!is_video(Path::new("a.PNG")));
        assert!(!is_video(Path::new("a.jpg")));
    }
}
