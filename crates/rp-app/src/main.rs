//! Rusty Popper: game wheel over the playfield video, with every cabinet
//! screen showing the selected game's media.
//!
//! Usage:
//!   rusty-popper [--popper DIR] [--mpv PATH\libmpv-2.dll] [--game TEXT]
//!                [--wheel arc|flow] [--seconds N] [--list] [--dry-run]
//!
//! Browse with Left/Right (or Popper's bound buttons), W switches wheel style,
//! Esc (or Popper's exit buttons) opens the exit menu; Q quits at once.

mod assets;
mod input;
mod mpv;
mod settings;
mod video;
mod wheel;

use anyhow::{anyhow, bail, Context, Result};
use femtovg::{renderer::OpenGl, Canvas, ImageFlags};
use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, PossiblyCurrentGlContext};
use glutin::display::{Display, GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use rp_config::{self as config, Screen, Slot};
use rp_db::{self as db, Emulator, Game};
use settings::{Settings, WheelStyle};
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

/// The always-on menu screens; GameInfo/GameHelp/etc. are on-demand overlays.
const LIVE: &[Slot] = &[Slot::Topper, Slot::Dmd, Slot::BackGlass, Slot::PlayField, Slot::FullDmd];

struct Args {
    popper: PathBuf,
    mpv: Option<PathBuf>,
    game: Option<String>,
    wheel: Option<WheelStyle>,
    seconds: Option<u64>,
    list: bool,
    dry_run: bool,
    print_launch: bool,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        popper: PathBuf::from(r"C:\vPinball\PinUPSystem"),
        mpv: None,
        game: None,
        wheel: None,
        seconds: None,
        list: false,
        dry_run: false,
        print_launch: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--popper" => a.popper = val()?.into(),
            "--mpv" => a.mpv = Some(val()?.into()),
            "--game" => a.game = Some(val()?),
            "--wheel" => {
                let v = val()?;
                a.wheel = Some(WheelStyle::parse(&v).ok_or_else(|| anyhow!("--wheel must be arc or flow, not {v}"))?);
            }
            "--seconds" => a.seconds = Some(val()?.parse()?),
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

fn find_mpv_dll(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p);
    }
    candidates.push(exe_dir().join("libmpv-2.dll"));
    candidates.push(PathBuf::from(r"D:\Claude\deps\mpv\libmpv-2.dll"));
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| anyhow!("libmpv-2.dll not found; pass --mpv"))
}

/// A wheel entry with its resolved media: a game, or (when `playlist` is set)
/// a playlist shown as a wheel, whose media lives in POPMedia\Default.
struct Item {
    game: Game,
    emu: Emulator,
    emu_name: String,
    media_dir: PathBuf,
    wheel_png: Option<PathBuf>,
    /// For playlist entries: 0 = all games, n = playlists[n - 1].
    playlist: Option<usize>,
}

impl Item {
    fn media(&self, slot: Slot) -> Option<PathBuf> {
        config::find_media(&self.media_dir, slot, &self.game.name)
    }

    fn entry(&self) -> wheel::Entry {
        let g = &self.game;
        let subtitle = match (&g.manufacturer, g.year) {
            (Some(m), Some(y)) => format!("{}  ·  {y}", m.to_uppercase()),
            (Some(m), None) => m.to_uppercase(),
            (None, Some(y)) => y.to_string(),
            (None, None) => String::new(),
        };
        // Popper's tags (VR, nFozzy, Fleep...), then the emulator.
        if self.playlist.is_some() {
            return wheel::Entry { title: g.display.clone(), subtitle: String::new(), chips: vec!["Playlist".into()], logo: None };
        }
        let mut chips: Vec<String> = g.tags.clone();
        chips.push(self.emu_name.clone());
        wheel::Entry { title: g.display.clone(), subtitle, chips, logo: None }
    }
}

fn load_items(popper: &std::path::Path, emus: &[Emulator], games: Vec<Game>) -> Vec<Item> {
    games
        .into_iter()
        .filter_map(|game| {
            let emu = emus.iter().find(|e| e.id == game.emu_id)?; // hidden emulators drop out
            let media_dir = emu.media_dir.clone().unwrap_or_else(|| popper.join("POPMedia").join(&emu.name));
            let wheel_png = config::find_wheel(&media_dir, &game.name);
            Some(Item { emu_name: emu.name.clone(), emu: emu.clone(), media_dir, wheel_png, game, playlist: None })
        })
        .collect()
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let settings = Settings::load(&exe_dir().join("rusty-popper.toml")).context("reading rusty-popper.toml")?;
    let style = args.wheel.unwrap_or(settings.wheel.style);

    let db = db::PopperDb::open_copy(&args.popper.join("PUPDatabase.db"))?;
    let emus = db.emulators()?;
    let mut items = load_items(&args.popper, &emus, db.games()?);
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
    let bindings = input::Bindings::new(&db.functions()?, &joy_overrides);
    let ui_rotation = settings.playfield.ui_rotation.unwrap_or(db.menu_rotation().unwrap_or(0)).rem_euclid(360);
    let playlists = db.playlists().unwrap_or_else(|e| {
        eprintln!("Couldn't read playlists: {e:#}");
        Vec::new()
    });
    println!("Playlists: {}", playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
    println!("Menu rotation: {ui_rotation} deg");
    println!("DB: {} visible emulators, {} games on the wheel", emus.len(), items.len());
    println!("Input: {}", bindings.describe());

    if args.list {
        for it in &items {
            println!("  [{}] {} ({}){}", it.emu_name, it.game.display, it.game.file_name, if it.wheel_png.is_none() { "  [no wheel image]" } else { "" });
        }
        return Ok(());
    }
    if items.is_empty() {
        bail!("no visible games");
    }

    let start = match &args.game {
        Some(q) => {
            let q = q.to_lowercase();
            items
                .iter()
                .position(|it| it.game.display.to_lowercase().contains(&q) || it.game.name.to_lowercase().contains(&q))
                .ok_or_else(|| anyhow!("no visible game matches {q:?}"))?
        }
        None => 0,
    };

    let screens: Vec<Screen> = config::read_screens(&args.popper.join("PinUpPlayer.ini"))?
        .into_iter()
        .filter(|s| LIVE.contains(&s.slot) && s.width > 0 && s.height > 0)
        .collect();
    if !screens.iter().any(|s| s.slot == Slot::PlayField) {
        bail!("PinUpPlayer.ini has no PlayField screen");
    }
    println!("Wheel style: {style:?}; start: {}", items[start].game.display);
    for s in &screens {
        let m = items[start].media(s.slot);
        println!("  {:?}: {}x{} at {},{} rot {} -> {}", s.slot, s.width, s.height, s.x, s.y, s.rotation, m.as_ref().map_or("(none)".into(), |p| p.display().to_string()));
    }

    if args.print_launch {
        let it = &items[start];
        let job = rp_launch::LaunchJob { popper_dir: args.popper.clone(), emulator: it.emu.clone(), game: it.game.clone() };
        println!("--- launch script ---
{}
--- post script ---
{}", job.fill(&it.emu.launch_script), job.fill(&it.emu.post_script));
        return Ok(());
    }
    let dll = find_mpv_dll(args.mpv)?;
    let mpv = mpv::Mpv::load(&dll)?;
    println!("libmpv: {}", dll.display());
    if args.dry_run {
        return Ok(());
    }

    // Playlists ride on the wheel too, after the games: "All games" (Popper's
    // go-back wheel) and each playlist with its Popper wheel and videos.
    let game_count = items.len();
    let default_media = args.popper.join("POPMedia").join("Default");
    let template = items[0].emu.clone();
    for n in 0..=playlists.len() {
        let (name, logo) = match n {
            0 => ("All games".to_string(), Some("goback".to_string())),
            n => (playlists[n - 1].name.clone(), playlists[n - 1].logo.clone()),
        };
        let logo = logo.unwrap_or_default();
        let mut game = items[0].game.clone();
        game.id = -1 - n as i64;
        game.name = logo.clone();
        game.display = name;
        game.tags.clear();
        items.push(Item {
            wheel_png: config::find_wheel(&default_media, &logo),
            media_dir: default_media.clone(),
            emu_name: String::new(),
            emu: template.clone(),
            game,
            playlist: Some(n),
        });
    }
    let volume = settings.audio.volume.unwrap_or_else(|| screens.iter().find(|s| s.slot == Slot::PlayField).map_or(100, |s| s.volume)).clamp(0, 100);
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        mpv,
        screens,
        items,
        game_count,
        bindings,
        style,
        start,
        ui_rotation,
        deadline: args.seconds.map(|s| Instant::now() + Duration::from_secs(s)),
        gfx: None,
        error: None,
        menu: None,
        popper_dir: args.popper.clone(),
        playing: None,
        db,
        playlists,
        playlist: 0,
        view: Vec::new(),
        attract: settings.attract,
        volume,
        last_input: Instant::now(),
        attract_on: false,
        next_attract: Instant::now(),
        rng: (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)) | 1,
        letter: None,
        stats: std::sync::Arc::new(std::sync::Mutex::new(db::StatsWriter::new(
            &args.popper.join("PUPDatabase.db"),
            &exe_dir().join("backups"),
        ))),
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(())
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
}

impl ScreenOut {
    /// Plays `media`, or for a secondary screen with nothing to show, hides
    /// its window (like Popper does) so it doesn't cover what's beneath it.
    /// Topper and DMD rectangles can overlap other screens on some cabinets.
    fn show(&self, media: Option<&std::path::Path>) {
        if let Err(e) = self.player.show(media) {
            eprintln!("{:?}: {e}", self.screen.slot);
        }
        if self.screen.slot != Slot::PlayField {
            self.window.set_visible(media.is_some());
        }
    }
}

struct Gfx {
    outs: Vec<ScreenOut>, // the playfield is outs[0]
    context: PossiblyCurrentContext,
    gl: glow::Context,
    _loader: Box<mpv::GetProcAddress>,
    canvas: Canvas<OpenGl>,
    fonts: wheel::Fonts,
    entries: Vec<wheel::Entry>,
    logos: Receiver<assets::Logo>,
    wheel: wheel::Wheel,
    joysticks: input::Joysticks,
    shown: usize,      // index whose media is on the screens
    swapped_at: f32,   // when that media was loaded (for the fade-in)
    t0: Instant,
    last: Instant,
}

/// Exit menu entries. "Back" is first and selected on open so a stray press
/// of Start never reboots or shuts down the cabinet.
const EXIT_MENU: &[&str] = &["Back to games", "Options", "Exit Rusty Popper", "Restart computer", "Shut down computer"];

/// Choices the Options screen cycles through with Start.
const ATTRACT_IDLE_CHOICES: &[u64] = &[60, 120, 180, 300, 600];
const ATTRACT_INTERVAL_CHOICES: &[u64] = &[6, 12, 20, 30];
const OPTION_ROWS: usize = 6; // style, attract, idle, interval, volume, back

fn next_choice(choices: &[u64], cur: u64) -> u64 {
    let i = choices.iter().position(|&c| c >= cur).map(|i| i + 1).unwrap_or(0);
    choices[i % choices.len()]
}

struct Playing {
    rx: Receiver<Result<rp_launch::Outcome>>,
    index: usize,
    since: Instant,
    phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// When the game was seen to exit (restarts the screen's animation).
    exiting_since: Option<Instant>,
    /// Screens not in KeepDisplays are paused (and black) only while the game itself runs.
    others_paused: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Exit,
    Options,
}

struct ExitMenu {
    kind: MenuKind,
    selected: usize,
    opened: Instant,
}

/// Restarts (`/r`) or shuts down (`/s`) Windows, then exits.
fn power(el: &ActiveEventLoop, flag: &str) {
    println!("Running shutdown {flag} /t 0");
    if let Err(e) = std::process::Command::new("shutdown").args([flag, "/t", "0"]).spawn() {
        eprintln!("shutdown {flag} failed: {e}");
    }
    el.exit();
}

struct App {
    mpv: mpv::Mpv,
    screens: Vec<Screen>,
    items: Vec<Item>,
    bindings: input::Bindings,
    style: WheelStyle,
    start: usize,
    /// Extra clockwise turn of the menu overlay (Popper's ThumbRotate).
    ui_rotation: i32,
    deadline: Option<Instant>,
    gfx: Option<Gfx>,
    error: Option<anyhow::Error>,
    menu: Option<ExitMenu>,
    popper_dir: PathBuf,
    /// The game currently running, if any: finishes when the worker sends.
    playing: Option<Playing>,
    /// Records plays in Popper's GamesStats (shared with the game thread).
    stats: std::sync::Arc<std::sync::Mutex<db::StatsWriter>>,
    /// Read-only copy of Popper's DB, kept for running playlist queries.
    db: db::PopperDb,
    playlists: Vec<db::Playlist>,
    /// items[..game_count] are games; the rest are playlist entries.
    game_count: usize,
    /// 0 = all games, n = playlists[n - 1].
    playlist: usize,
    /// Item indices in the current playlist, in wheel order.
    view: Vec<usize>,
    attract: settings::AttractSettings,
    /// Playfield video volume (0-100), adjustable in Options.
    volume: i32,
    last_input: Instant,
    attract_on: bool,
    next_attract: Instant,
    rng: u64,
    /// Letter shown after a jump-by-letter, and when.
    letter: Option<(char, Instant)>,
}

/// The file next to the exe that remembers the last playlist.
fn state_path() -> PathBuf {
    exe_dir().join("last-playlist.txt")
}

/// Group a title by its first letter; digits and symbols share '#'.
fn letter_of(title: &str) -> char {
    match title.trim_start().chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

fn window_attrs(s: &Screen) -> winit::window::WindowAttributes {
    Window::default_attributes()
        .with_title(format!("Rusty Popper {:?}", s.slot))
        .with_decorations(false)
        .with_resizable(false)
        .with_position(PhysicalPosition::new(s.x, s.y))
        .with_inner_size(PhysicalSize::new(s.width as u32, s.height as u32))
        .with_skip_taskbar(true)
}

impl App {
    fn init(&mut self, el: &ActiveEventLoop) -> Result<()> {
        // Start in the last playlist used (if it still exists), on the --game
        // match or that playlist's first game.
        let last = std::fs::read_to_string(state_path()).unwrap_or_default();
        let saved = self.playlists.iter().position(|p| p.name == last.trim()).map(|i| i + 1).unwrap_or(0);
        self.playlist = saved;
        self.view = self.playlist_view(saved);
        let has_games = self.view.first().is_some_and(|&i| i < self.game_count);
        if !has_games || (self.start != 0 && !self.view.contains(&self.start)) {
            self.playlist = 0;
            self.view = self.playlist_view(0);
        }
        let start_pos = self.view.iter().position(|&i| i == self.start).unwrap_or(0);
        self.start = self.view[start_pos];
        // Playfield first: its window creates the GL display and config.
        let mut order: Vec<Screen> = self.screens.clone();
        order.sort_by_key(|s| (s.slot != Slot::PlayField) as u8);

        let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_stencil_size(8);
        let (pf_window, gl_config) = DisplayBuilder::new()
            .with_window_attributes(Some(window_attrs(&order[0])))
            .build(el, template, |configs| {
                configs
                    .reduce(|a, b| if b.num_samples() < a.num_samples() { b } else { a })
                    .expect("no GL config")
            })
            .map_err(|e| anyhow!("creating GL display: {e}"))?;
        let pf_window = pf_window.ok_or_else(|| anyhow!("no playfield window"))?;
        let display: Display = gl_config.display();

        let mut windows = vec![pf_window];
        for s in &order[1..] {
            windows.push(glutin_winit::finalize_window(el, window_attrs(s), &gl_config)?);
        }

        let raw = windows[0].window_handle()?.as_raw();
        let ctx_attrs = ContextAttributesBuilder::new().build(Some(raw));
        let not_current = unsafe { display.create_context(&gl_config, &ctx_attrs)? };

        let mut surfaces = Vec::new();
        for w in &windows {
            let attrs = w.build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())?;
            surfaces.push(unsafe { display.create_window_surface(&gl_config, &attrs)? });
        }
        let context = not_current.make_current(&surfaces[0])?;

        // mpv resolves GL symbols through the glutin display.
        let d2 = display.clone();
        let loader: Box<mpv::GetProcAddress> = Box::new(move |name| d2.get_proc_address(name) as *const c_void);

        let start_item = &self.items[self.start];
        let mut outs = Vec::new();
        for ((screen, window), surface) in order.into_iter().zip(windows).zip(surfaces) {
            context.make_current(&surface)?;
            // Only the playfield waits for vsync, so three swaps don't triple the frame time.
            let interval = if screen.slot == Slot::PlayField { SwapInterval::Wait(NonZeroU32::new(1).unwrap()) } else { SwapInterval::DontWait };
            let _ = surface.set_swap_interval(&context, interval);
            let audio = screen.slot == Slot::PlayField;
            let opts = vec![
                ("config", "no".to_string()),
                ("terminal", "no".to_string()),
                ("keepaspect", "no".to_string()),
                ("video-rotate", screen.rotation.to_string()),
                ("loop-file", "inf".to_string()),
                ("image-display-duration", "inf".to_string()),
                ("hwdec", "auto-safe".to_string()),
                ("osd-level", "0".to_string()),
                ("aid", if audio { "auto" } else { "no" }.to_string()),
                ("volume", if screen.slot == Slot::PlayField { self.volume } else { screen.volume }.to_string()),
            ];
            let mut player = self.mpv.player(&opts).with_context(|| format!("{:?}", screen.slot))?;
            player.init_gl(&loader)?;
            window.set_cursor_visible(false);
            let out = ScreenOut { screen, window, surface, player, target: None, slide: None };
            out.show(start_item.media(out.screen.slot).as_deref());
            outs.push(out);
        }

        context.make_current(&outs[0].surface)?;
        let renderer = unsafe { OpenGl::new_from_function_cstr(|s| display.get_proc_address(s) as *const c_void) }
            .map_err(|e| anyhow!("femtovg: {e:?}"))?;
        let mut canvas = Canvas::new(renderer).map_err(|e| anyhow!("femtovg canvas: {e:?}"))?;
        let font = |canvas: &mut Canvas<OpenGl>, files: &[&str]| {
            files
                .iter()
                .find_map(|f| canvas.add_font(format!(r"C:\Windows\Fonts\{f}")).ok())
                .ok_or_else(|| anyhow!("none of the fonts {files:?} found"))
        };
        let fonts = wheel::Fonts {
            display: font(&mut canvas, &["bahnschrift.ttf", "segoeuib.ttf", "arialbd.ttf"])?,
            body: font(&mut canvas, &["segoeui.ttf", "arial.ttf"])?,
            mono: font(&mut canvas, &["consola.ttf", "cour.ttf"])?,
        };

        // Load logos nearest the starting game first.
        let n = self.items.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|i| {
            let d = (*i as i64 - self.start as i64).rem_euclid(n as i64);
            d.min(n as i64 - d)
        });
        let logos = assets::spawn_loader(self.items.iter().map(|i| i.wheel_png.clone()).collect(), order);

        // Offscreen video targets (one context, so textures work on every surface).
        let gl = unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s) as *const c_void) };
        for o in outs.iter_mut() {
            context.make_current(&o.surface)?;
            match video::Target::new(&gl, &mut canvas, o.screen.width, o.screen.height) {
                Ok(t) => o.target = Some(t),
                Err(e) => eprintln!("{:?}: no offscreen video ({e:#}); slides disabled", o.screen.slot),
            }
        }
        context.make_current(&outs[0].surface)?;
        outs[0].window.focus_window();
        let now = Instant::now();
        self.gfx = Some(Gfx {
            outs,
            context,
            gl,
            _loader: loader,
            canvas,
            fonts,
            entries: self.items.iter().map(Item::entry).collect(),
            logos,
            wheel: wheel::Wheel::new(self.view.len(), start_pos),
            joysticks: input::Joysticks::new(),
            shown: self.start,
            swapped_at: -10.0,
            t0: now,
            last: now,
        });
        if let Some(g) = &self.gfx {
            println!("Joysticks found: {}", g.joysticks.count());
        }
        Ok(())
    }

    /// Item indices for playlist `n` (0 = all games), in wheel order.
    /// The playlist's games, then the other playlists as wheel entries, so
    /// they sit where the wheel wraps from the last game back to the first.
    fn playlist_view(&self, n: usize) -> Vec<usize> {
        let n = if n > self.playlists.len() { 0 } else { n };
        let mut view: Vec<usize> = if n == 0 {
            (0..self.game_count).collect()
        } else {
            let pl = &self.playlists[n - 1];
            match self.db.playlist_games(pl) {
                Ok(ids) => ids.iter().filter_map(|id| self.items[..self.game_count].iter().position(|it| it.game.id == *id)).collect(),
                Err(e) => {
                    eprintln!("Playlist {:?}: {e:#}", pl.name);
                    Vec::new()
                }
            }
        };
        view.extend((self.game_count..self.items.len()).filter(|&i| self.items[i].playlist != Some(n)));
        view
    }

    fn playlist_name(&self) -> &str {
        match self.playlist {
            0 => "All games",
            n => &self.playlists[n - 1].name,
        }
    }

    /// Switches to playlist `n`, staying on the current game if it's in it.
    fn set_playlist(&mut self, n: usize) {
        let view = self.playlist_view(n);
        let Some(g) = self.gfx.as_mut() else { return };
        let current = self.view.get(g.wheel.selected()).copied();
        let pos = current.and_then(|c| view.iter().position(|&i| i == c)).unwrap_or(0);
        self.playlist = n;
        self.view = view;
        g.wheel = wheel::Wheel::new(self.view.len().max(1), pos);
        g.wheel.info_t = 0.0;
        if let Some(&item) = self.view.get(pos) {
            if item != g.shown {
                g.shown = item;
                g.swapped_at = (Instant::now() - g.t0).as_secs_f32();
                for o in &g.outs {
                    o.show(self.items[item].media(o.screen.slot).as_deref());
                }
            }
        }
        let name = self.playlist_name().to_string();
        println!("Playlist: {name} ({} games)", self.view.len());
        let _ = std::fs::write(state_path(), name);
    }

    /// Moves to the first game of the next (`dir` > 0) or previous letter group.
    /// Going back from inside a group first goes to that group's start.
    fn jump_letter(&mut self, dir: i64) {
        let Some(g) = self.gfx.as_mut() else { return };
        let n = self.view.len();
        if n == 0 {
            return;
        }
        // Playlist entries form one group (shown as a star) after the games.
        let letter = |pos: usize| {
            let it = &self.items[self.view[pos % n]];
            if it.playlist.is_some() { '*' } else { letter_of(&it.game.display) }
        };
        let cur = g.wheel.selected();
        let here = letter(cur);
        let mut pos = cur;
        if dir > 0 {
            for _ in 0..n {
                pos = (pos + 1) % n;
                if letter(pos) != here {
                    break;
                }
            }
        } else {
            let at_group_start = letter((cur + n - 1) % n) != here;
            // Step back into the previous group (or stay in this one), then to its start.
            if at_group_start {
                pos = (pos + n - 1) % n;
            }
            let target = letter(pos);
            for _ in 0..n {
                let prev = (pos + n - 1) % n;
                if letter(prev) != target || prev == cur {
                    break;
                }
                pos = prev;
            }
        }
        // Shortest way round the wheel.
        let mut d = pos as i64 - cur as i64;
        if d > n as i64 / 2 {
            d -= n as i64;
        } else if d < -(n as i64 / 2) {
            d += n as i64;
        }
        g.wheel.step(d);
        self.letter = Some((letter(pos), Instant::now()));
    }

    /// Labels for the Options screen, with current values.
    fn option_labels(&self) -> Vec<String> {
        let onoff = |b: bool| if b { "On" } else { "Off" };
        let mins = |s: u64| if s % 60 == 0 { format!("{} min", s / 60) } else { format!("{s} s") };
        vec![
            format!("Wheel style: {}", if self.style == WheelStyle::Arc { "Arc" } else { "Cover flow" }),
            format!("Attract mode: {}", onoff(self.attract.enabled)),
            format!("Attract starts after: {}", mins(self.attract.idle_secs)),
            format!("Time per game: {} s", self.attract.interval_secs),
            format!("Video volume: {}%", self.volume),
            "Back".to_string(),
        ]
    }

    /// Cycles the value on Options row `row`, applies it, and saves it.
    fn change_option(&mut self, row: usize) {
        match row {
            0 => self.style = self.style.toggled(),
            1 => self.attract.enabled = !self.attract.enabled,
            2 => self.attract.idle_secs = next_choice(ATTRACT_IDLE_CHOICES, self.attract.idle_secs),
            3 => self.attract.interval_secs = next_choice(ATTRACT_INTERVAL_CHOICES, self.attract.interval_secs),
            4 => {
                self.volume = if self.volume >= 100 { 0 } else { (self.volume / 10 + 1) * 10 };
                if let Some(g) = &self.gfx {
                    let _ = g.outs[0].player.command(&["set", "volume", &self.volume.to_string()]);
                }
            }
            _ => return,
        }
        let v = settings::Editable {
            style: self.style,
            attract_enabled: self.attract.enabled,
            attract_idle_secs: self.attract.idle_secs,
            attract_interval_secs: self.attract.interval_secs,
            volume: self.volume,
        };
        if let Err(e) = settings::save(&exe_dir().join("rusty-popper.toml"), &v) {
            eprintln!("Couldn't save settings: {e:#}");
        }
    }

    fn act(&mut self, el: &ActiveEventLoop, action: input::Action) {
        use input::Action::*;
        self.last_input = Instant::now();
        if std::env::var_os("RP_DEBUG_INPUT").is_some() {
            println!("action {action:?}");
        }
        // Any button just wakes the menu from attract mode.
        if self.attract_on {
            self.attract_on = false;
            return;
        }
        if self.gfx.is_none() {
            return;
        }
        // While a menu is open, browsing buttons move its highlight.
        if let Some(menu) = self.menu.as_mut() {
            let n = match menu.kind {
                MenuKind::Exit => EXIT_MENU.len(),
                MenuKind::Options => OPTION_ROWS,
            };
            match action {
                Prev | PagePrev | LetterPrev => menu.selected = (menu.selected + n - 1) % n,
                Next | PageNext | LetterNext => menu.selected = (menu.selected + 1) % n,
                Menu => self.menu = None,
                Playlists => {}

                Quit => el.exit(),
                Start => {
                    let (kind, choice) = (menu.kind, menu.selected);
                    // Options rows change in place; the menu stays open.
                    if kind == MenuKind::Options && choice < OPTION_ROWS - 1 {
                        self.change_option(choice);
                        return;
                    }
                    self.menu = None;
                    match (kind, choice) {
                        (MenuKind::Exit, 1) => {
                            self.menu = Some(ExitMenu { kind: MenuKind::Options, selected: 0, opened: Instant::now() });
                        }
                        (MenuKind::Exit, 2) => el.exit(),
                        (MenuKind::Exit, 3) => power(el, "/r"),
                        (MenuKind::Exit, 4) => power(el, "/s"),
                        _ => {}
                    }
                }
                ToggleStyle => {}
            }
            return;
        }
        let g = self.gfx.as_mut().unwrap();
        match action {
            Menu => self.menu = Some(ExitMenu { kind: MenuKind::Exit, selected: 0, opened: Instant::now() }),
            // Jump to the playlist wheels (after the games).
            Playlists => {
                let cur = g.wheel.selected();
                if let Some(pos) = self.view.iter().position(|&i| i >= self.game_count) {
                    g.wheel.step(pos as i64 - cur as i64);
                }
            }
            Prev => g.wheel.step(-1),
            Next => g.wheel.step(1),
            // Popper's Page Prior/Next (usually the magna-saves) jump by letter.
            PagePrev | LetterPrev => self.jump_letter(-1),
            PageNext | LetterNext => self.jump_letter(1),
            ToggleStyle => self.style = self.style.toggled(),
            Start => {
                if let Some(&item) = self.view.get(g.wheel.selected()) {
                    match self.items[item].playlist {
                        Some(n) => self.set_playlist(n),
                        None => self.launch(item),
                    }
                }
            }
            Quit => el.exit(),
        }
    }

    /// Attract mode: after a while with no input, hop to a random game every
    /// few seconds until any button is pressed.
    fn attract_tick(&mut self) {
        if !self.attract.enabled || self.menu.is_some() || self.playing.is_some() || self.view.len() < 2 {
            return;
        }
        let now = Instant::now();
        if !self.attract_on {
            if now - self.last_input >= Duration::from_secs(self.attract.idle_secs) {
                println!("Attract mode on");
                self.attract_on = true;
                self.next_attract = now;
            } else {
                return;
            }
        }
        if now < self.next_attract {
            return;
        }
        self.next_attract = now + Duration::from_secs(self.attract.interval_secs.max(3));
        // xorshift64: good enough to pick a game.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let n = self.view.len() as i64;
        let Some(g) = self.gfx.as_mut() else { return };
        let cur = g.wheel.selected() as i64;
        // Only games (they come first in the view), never the playlist wheels.
        let games = self.view.iter().take_while(|&&i| i < self.game_count).count() as i64;
        if games < 2 {
            return;
        }
        let mut target = (self.rng % games as u64) as i64;
        if target == cur {
            target = (target + 1) % games;
        }
        let mut d = target - cur;
        if d > n / 2 {
            d -= n;
        } else if d < -(n / 2) {
            d += n;
        }
        g.wheel.step(d);
    }
    fn frame(&mut self, el: &ActiveEventLoop) -> Result<()> {
        // While a game runs, keep our windows up behind it (black, with a
        // "Launching" screen on the playfield) so the desktop never shows.
        let mut launching: Option<(usize, rp_launch::Phase, f32)> = None;
        if let Some(p) = self.playing.as_mut() {
            match p.rx.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(outcome) => println!("Back from game ({outcome:?})"),
                        Err(e) => eprintln!("Launch failed: {e:#}"),
                    }
                    self.playing = None;
                    self.return_from_game();
                    el.set_control_flow(ControlFlow::Poll);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.playing = None;
                    self.return_from_game();
                    el.set_control_flow(ControlFlow::Poll);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    use rp_launch::Phase;
                    let phase = Phase::load(&p.phase);
                    if phase == Phase::Exiting && p.exiting_since.is_none() {
                        p.exiting_since = Some(Instant::now());
                    }
                    let t = p.exiting_since.unwrap_or(p.since).elapsed().as_secs_f32();
                    let want_paused = phase == Phase::Playing;
                    if want_paused != p.others_paused {
                        p.others_paused = want_paused;
                        if let Some(g) = self.gfx.as_ref() {
                            let keep = &self.items[p.index].emu.keep_displays;
                            for o in g.outs.iter().filter(|o| !keep.contains(&(o.screen.slot as u32))) {
                                let _ = o.player.set_pause(want_paused);
                            }
                        }
                    }
                    // Animate while the table loads or closes; barely tick behind the game.
                    let every = match phase {
                        Phase::Playing => 1000,
                        Phase::Launching if t > 15.0 => 1000,
                        _ => 33,
                    };
                    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(every)));
                    launching = Some((p.index, phase, t));
                }
            }
        }
        if let Some((index, phase, t)) = launching {
            return self.render_launching(index, phase, t);
        }
        let mut actions = Vec::new();
        {
            let Some(g) = self.gfx.as_mut() else { return Ok(()) };
            g.joysticks.poll(&self.bindings, &mut actions);
        }
        for a in actions {
            self.act(el, a);
        }
        self.attract_tick();
        let style = self.style;
        let option_labels = self.option_labels();
        let Some(g) = self.gfx.as_mut() else { return Ok(()) };

        let now_i = Instant::now();
        let dt = (now_i - g.last).as_secs_f32().min(0.05);
        g.last = now_i;
        let now = (now_i - g.t0).as_secs_f32();

        // Upload any logos the loader finished (a few per frame to avoid hitches).
        for logo in g.logos.try_iter().take(4) {
            let upload = |c: &mut Canvas<OpenGl>, img: &image::RgbaImage| {
                use femtovg::rgb::FromSlice;
                let (w, h) = img.dimensions();
                c.create_image(femtovg::imgref::Img::new(img.as_raw().as_rgba(), w as usize, h as usize), ImageFlags::empty())
            };
            g.context.make_current(&g.outs[0].surface)?;
            if let (Ok(img), Ok(glow)) = (upload(&mut g.canvas, &logo.rgba), upload(&mut g.canvas, &logo.glow)) {
                let (w, h) = logo.rgba.dimensions();
                g.entries[logo.index].logo = Some(wheel::LogoTex { img, glow, aspect: w as f32 / h as f32, hue: logo.hue });
            }
        }

        g.wheel.update(dt, now);
        // Swap media once the selection has rested for a moment.
        let sel = self.view.get(g.wheel.selected()).copied().unwrap_or(g.shown);
        if sel != g.shown && now - g.wheel.changed_at >= wheel::MEDIA_DEBOUNCE {
            g.shown = sel;
            g.swapped_at = now;
            let item = &self.items[sel];
            let slide = style == WheelStyle::Flow;
            let dir = g.wheel.last_dir;
            for o in g.outs.iter_mut() {
                let media = item.media(o.screen.slot);
                // Cover flow: freeze the outgoing frame and slide it away once
                // the new video has a frame.
                if slide && media.is_some() {
                    if let Some(t) = &o.target {
                        g.context.make_current(&o.surface)?;
                        t.freeze(&g.gl);
                        o.slide = Some(video::Slide::new(dir));
                    }
                }
                o.show(media.as_deref());
            }
        }
        for o in g.outs.iter_mut() {
            let events = o.player.poll_events();
            if events.contains(&mpv::EVENT_SHUTDOWN) {
                el.exit();
            }
            if events.contains(&mpv::EVENT_PLAYBACK_RESTART) {
                if let Some(s) = o.slide.as_mut() {
                    s.video_ready();
                }
            }
        }

        // Pass 1: every screen's video into its offscreen texture (all mpv work
        // first, with GL state reset, since femtovg's state upsets mpv).
        for o in &g.outs {
            g.context.make_current(&o.surface)?;
            video::reset_for_mpv(&g.gl);
            match &o.target {
                Some(t) => o.player.render_to(t.fbo_id(), o.screen.width, o.screen.height),
                None => o.player.render(o.screen.width, o.screen.height),
            }
        }
        // Pass 2: composite. The slide runs along the player's left/right: on
        // the playfield that's the menu's rotated x axis, elsewhere screen x.
        let pf_rot = (g.outs[0].screen.rotation + self.ui_rotation).rem_euclid(360);
        for o in g.outs.iter_mut().skip(1) {
            g.context.make_current(&o.surface)?;
            if let Some(t) = &o.target {
                g.canvas.set_size(o.screen.width as u32, o.screen.height as u32, 1.0);
                let a = (o.screen.rotation as f32).to_radians();
                video::composite(&mut g.canvas, t, &mut o.slide, (a.cos(), a.sin()));
                g.canvas.flush();
            }
            o.surface.swap_buffers(&g.context)?;
        }
        // Playfield: video, then the wheel on top, then present (waits for vsync).
        let (w, h) = (g.outs[0].screen.width, g.outs[0].screen.height);
        g.context.make_current(&g.outs[0].surface)?;
        g.canvas.set_size(w as u32, h as u32, 1.0);
        {
            let pf = &mut g.outs[0];
            if let Some(t) = &pf.target {
                let a = (pf_rot as f32).to_radians();
                video::composite(&mut g.canvas, t, &mut pf.slide, (a.cos(), a.sin()));
            }
        }
        let pf = &g.outs[0];
        g.canvas.save();
        // Draw in the frame the player sees: on a cabinet the playfield is
        // usually mounted turned, so the menu is rotated to face them.
        let rot = pf_rot;
        let (lw, lh) = if rot % 180 == 0 { (w, h) } else { (h, w) };
        if rot != 0 {
            g.canvas.translate(w as f32 / 2.0, h as f32 / 2.0);
            g.canvas.rotate((rot as f32).to_radians());
            g.canvas.translate(-(lw as f32) / 2.0, -(lh as f32) / 2.0);
        }
        // Arc fades the new video in from black; cover flow slides it instead.
        let fade = if style == WheelStyle::Flow { 1.0 } else if g.shown == sel { ((now - g.swapped_at) / wheel::MEDIA_FADE).min(1.0) } else { 1.0 };
        let playlist_name = match self.playlist {
            0 => "All games",
            n => self.playlists[n - 1].name.as_str(),
        };
        let letter = self.letter.map(|(c, at)| (c, at.elapsed().as_secs_f32()));
        let frame = wheel::Frame {
            style,
            wheel: &g.wheel,
            entries: &g.entries,
            order: &self.view,
            playlist: playlist_name,
            fonts: &g.fonts,
            video_fade: fade,
            letter,
        };
        wheel::draw(&mut g.canvas, lw as f32, lh as f32, &frame);
        if let Some(menu) = &self.menu {
            let names: Vec<&str> = match menu.kind {
                MenuKind::Exit => EXIT_MENU.to_vec(),
                MenuKind::Options => option_labels.iter().map(String::as_str).collect(),
            };
            let title = match menu.kind {
                MenuKind::Exit => "Exit",
                MenuKind::Options => "Options",
            };
            let view = wheel::MenuView { title, options: &names, selected: menu.selected, t: menu.opened.elapsed().as_secs_f32() };
            wheel::draw_menu(&mut g.canvas, lw as f32, lh as f32, &view, &g.fonts);
        }
        g.canvas.restore();
        g.canvas.flush();
        pf.surface.swap_buffers(&g.context)?;

        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            el.exit();
        }
        Ok(())
    }

    /// Shows the launch screen, pauses video and runs the selected game's
    /// emulator scripts on a worker thread.
    fn launch(&mut self, index: usize) {
        if self.playing.is_some() {
            return;
        }
        // RP_NO_LAUNCH=1: log instead of starting a table (for automated tests).
        if std::env::var_os("RP_NO_LAUNCH").is_some() {
            println!("(RP_NO_LAUNCH) would launch {}", self.items[index].game.display);
            return;
        }
        let Some(g) = self.gfx.as_mut() else { return };
        let item = &self.items[index];
        let job = rp_launch::LaunchJob { popper_dir: self.popper_dir.clone(), emulator: item.emu.clone(), game: item.game.clone() };
        // Make sure every screen shows this game's media while it loads.
        if g.shown != index {
            g.shown = index;
            for o in &g.outs {
                o.show(item.media(o.screen.slot).as_deref());
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let phase = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let worker_phase = phase.clone();
        let stats = self.stats.clone();
        let game_id = item.game.id;
        std::thread::Builder::new()
            .name("game".into())
            .spawn(move || {
                let result = job.run(&worker_phase);
                if let Ok(rp_launch::Outcome::Finished { secs }) = &result {
                    let recorded = stats
                        .lock()
                        .map_err(|_| anyhow!("stats lock poisoned"))
                        .and_then(|mut s| s.record_play(game_id, *secs));
                    match recorded {
                        Ok(()) => println!("Recorded play of game {game_id} ({secs}s)"),
                        Err(e) => eprintln!("Couldn't record play stats: {e:#}"),
                    }
                }
                let _ = tx.send(result);
            })
            .expect("spawn game thread");
        self.playing = Some(Playing { rx, index, since: Instant::now(), phase, exiting_since: None, others_paused: false });
    }

    /// Brings the menu back after a game: screens, video and focus.
    fn return_from_game(&mut self) {
        let Some(g) = self.gfx.as_mut() else { return };
        let item = &self.items[g.shown];
        for o in &g.outs {
            let _ = o.player.set_pause(false);
            o.show(item.media(o.screen.slot).as_deref());
        }
        g.outs[0].window.focus_window();
        // Buttons still held from the game (e.g. Exit) must not fire in the menu.
        g.joysticks.swallow_held(&self.bindings);
        g.last = Instant::now();
    }

    /// One frame while a game is loading, running or closing: the launch /
    /// exit screen on the playfield and the game's media (or black) elsewhere.
    fn render_launching(&mut self, index: usize, phase: rp_launch::Phase, t: f32) -> Result<()> {
        let Some(g) = self.gfx.as_mut() else { return Ok(()) };
        let keep = &self.items[index].emu.keep_displays;
        for o in &g.outs {
            o.player.poll_events();
        }
        // While loading and exiting every screen shows the game's media; while
        // the game runs, screens it doesn't keep go black behind it.
        // mpv draws first: femtovg leaves GL state that mpv's renderer trips on.
        let black = |o: &ScreenOut| phase == rp_launch::Phase::Playing && !keep.contains(&(o.screen.slot as u32));
        for o in g.outs.iter().skip(1).filter(|o| !black(o)) {
            g.context.make_current(&o.surface)?;
            video::reset_for_mpv(&g.gl);
            o.player.render(o.screen.width, o.screen.height);
            o.surface.swap_buffers(&g.context)?;
        }
        for o in g.outs.iter().skip(1).filter(|o| black(o)) {
            g.context.make_current(&o.surface)?;
            wheel::clear_black(&mut g.canvas, o.screen.width as u32, o.screen.height as u32);
            o.surface.swap_buffers(&g.context)?;
        }
        let pf = &g.outs[0];
        g.context.make_current(&pf.surface)?;
        let (w, h) = (pf.screen.width, pf.screen.height);
        g.canvas.set_size(w as u32, h as u32, 1.0);
        g.canvas.save();
        let rot = (pf.screen.rotation + self.ui_rotation).rem_euclid(360);
        let (lw, lh) = if rot % 180 == 0 { (w, h) } else { (h, w) };
        if rot != 0 {
            g.canvas.translate(w as f32 / 2.0, h as f32 / 2.0);
            g.canvas.rotate((rot as f32).to_radians());
            g.canvas.translate(-(lw as f32) / 2.0, -(lh as f32) / 2.0);
        }
        // Once the game is up, the screen behind it already reads "Exiting", so
        // that's what shows the moment the table closes (not a stale "Launching").
        let verb = if phase == rp_launch::Phase::Launching { "Launching" } else { "Exiting" };
        wheel::draw_launch(&mut g.canvas, lw as f32, lh as f32, &g.entries[index], verb, &self.items[index].emu_name, t, &g.fonts);
        g.canvas.restore();
        g.canvas.flush();
        pf.surface.swap_buffers(&g.context)?;
        Ok(())
    }

    fn shutdown(&mut self) {
        if let Some(mut g) = self.gfx.take() {
            for o in g.outs.iter_mut() {
                if g.context.make_current(&o.surface).is_ok() {
                    o.player.free_gl();
                    if let Some(t) = &o.target {
                        t.delete(&g.gl);
                    }
                }
            }
        }
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
            WindowEvent::KeyboardInput { event, is_synthetic, .. } if event.state == ElementState::Pressed && !is_synthetic => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(a) = self.bindings.key(code, event.repeat) {
                        self.act(el, a);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Err(e) = self.frame(el) {
            self.error = Some(e);
            el.exit();
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.shutdown();
    }
}
