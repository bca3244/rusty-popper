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

/// A game on the wheel with its resolved media.
struct Item {
    game: Game,
    emu_name: String,
    media_dir: PathBuf,
    wheel_png: Option<PathBuf>,
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
        let mut chips = Vec::new();
        if let Some(p) = g.players.filter(|p| *p > 0) {
            chips.push(format!("{p} {}", if p == 1 { "player" } else { "players" }));
        }
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
            Some(Item { emu_name: emu.name.clone(), media_dir, wheel_png, game })
        })
        .collect()
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let settings = Settings::load(&exe_dir().join("rusty-popper.toml")).context("reading rusty-popper.toml")?;
    let style = args.wheel.unwrap_or(settings.wheel.style);

    let db = db::PopperDb::open_copy(&args.popper.join("PUPDatabase.db"))?;
    let emus = db.emulators()?;
    let items = load_items(&args.popper, &emus, db.games()?);
    let bindings = input::Bindings::new(&db.functions()?, &settings.input.menu_joy_buttons);
    let ui_rotation = settings.playfield.ui_rotation.unwrap_or(db.menu_rotation().unwrap_or(0)).rem_euclid(360);
    drop(db);
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

    let dll = find_mpv_dll(args.mpv)?;
    let mpv = mpv::Mpv::load(&dll)?;
    println!("libmpv: {}", dll.display());
    if args.dry_run {
        return Ok(());
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        mpv,
        screens,
        items,
        bindings,
        style,
        start,
        ui_rotation,
        deadline: args.seconds.map(|s| Instant::now() + Duration::from_secs(s)),
        gfx: None,
        error: None,
        menu: None,
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
const EXIT_MENU: &[&str] = &["Back to games", "Exit Rusty Popper", "Restart computer", "Shut down computer"];

struct ExitMenu {
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
                ("volume", screen.volume.to_string()),
            ];
            let mut player = self.mpv.player(&opts).with_context(|| format!("{:?}", screen.slot))?;
            player.init_gl(&loader)?;
            window.set_cursor_visible(false);
            let out = ScreenOut { screen, window, surface, player };
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

        outs[0].window.focus_window();
        let now = Instant::now();
        self.gfx = Some(Gfx {
            outs,
            context,
            _loader: loader,
            canvas,
            fonts,
            entries: self.items.iter().map(Item::entry).collect(),
            logos,
            wheel: wheel::Wheel::new(n, self.start),
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

    #[allow(clippy::needless_return)]
    fn act(&mut self, el: &ActiveEventLoop, action: input::Action) {
        use input::Action::*;
        let Some(g) = self.gfx.as_mut() else { return };
        // While the exit menu is open, browsing buttons move its highlight.
        if let Some(menu) = self.menu.as_mut() {
            let n = EXIT_MENU.len();
            match action {
                Prev | PagePrev | JumpPrev => menu.selected = (menu.selected + n - 1) % n,
                Next | PageNext | JumpNext => menu.selected = (menu.selected + 1) % n,
                Menu => self.menu = None,
                Quit => el.exit(),
                Start => {
                    let choice = menu.selected;
                    self.menu = None;
                    match choice {
                        1 => el.exit(),
                        2 => power(el, "/r"),
                        3 => power(el, "/s"),
                        _ => {}
                    }
                }
                ToggleStyle => {}
            }
            return;
        }
        match action {
            Menu => self.menu = Some(ExitMenu { selected: 0, opened: Instant::now() }),
            Prev => g.wheel.step(-1),
            Next => g.wheel.step(1),
            // On Brian's cabinet Popper's "Page" functions sit on the flipper
            // buttons and step one game, so they do the same here.
            PagePrev => g.wheel.step(-1),
            PageNext => g.wheel.step(1),
            JumpPrev => g.wheel.step(-10),
            JumpNext => g.wheel.step(10),
            ToggleStyle => self.style = self.style.toggled(),
            Start => println!("Start pressed on {} (launching comes in the next step)", self.items[g.wheel.selected()].game.display),
            Quit => el.exit(),
        }
    }

    fn frame(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let mut actions = Vec::new();
        {
            let Some(g) = self.gfx.as_mut() else { return Ok(()) };
            g.joysticks.poll(&self.bindings, &mut actions);
        }
        for a in actions {
            self.act(el, a);
        }
        let style = self.style;
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
        let sel = g.wheel.selected();
        if sel != g.shown && now - g.wheel.changed_at >= wheel::MEDIA_DEBOUNCE {
            g.shown = sel;
            g.swapped_at = now;
            let item = &self.items[sel];
            for o in &g.outs {
                o.show(item.media(o.screen.slot).as_deref());
            }
        }
        for o in &g.outs {
            if o.player.poll_events().contains(&mpv::EVENT_SHUTDOWN) {
                el.exit();
            }
        }

        // Secondary screens: video only.
        for o in g.outs.iter().skip(1) {
            g.context.make_current(&o.surface)?;
            o.player.render(o.screen.width, o.screen.height);
            o.surface.swap_buffers(&g.context)?;
        }
        // Playfield: video, then the wheel on top, then present (waits for vsync).
        let pf = &g.outs[0];
        g.context.make_current(&pf.surface)?;
        let (w, h) = (pf.screen.width, pf.screen.height);
        pf.player.render(w, h);
        g.canvas.set_size(w as u32, h as u32, 1.0);
        g.canvas.save();
        // Draw in the frame the player sees: on a cabinet the playfield is
        // usually mounted turned, so the menu is rotated to face them.
        let rot = (pf.screen.rotation + self.ui_rotation).rem_euclid(360);
        let (lw, lh) = if rot % 180 == 0 { (w, h) } else { (h, w) };
        if rot != 0 {
            g.canvas.translate(w as f32 / 2.0, h as f32 / 2.0);
            g.canvas.rotate((rot as f32).to_radians());
            g.canvas.translate(-(lw as f32) / 2.0, -(lh as f32) / 2.0);
        }
        let fade = if g.shown == sel { ((now - g.swapped_at) / wheel::MEDIA_FADE).min(1.0) } else { 1.0 };
        let frame = wheel::Frame { style, wheel: &g.wheel, entries: &g.entries, fonts: &g.fonts, video_fade: fade };
        wheel::draw(&mut g.canvas, lw as f32, lh as f32, &frame);
        if let Some(menu) = &self.menu {
            let view = wheel::MenuView { title: "Exit", options: EXIT_MENU, selected: menu.selected, t: menu.opened.elapsed().as_secs_f32() };
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

    fn shutdown(&mut self) {
        if let Some(mut g) = self.gfx.take() {
            for o in g.outs.iter_mut() {
                if g.context.make_current(&o.surface).is_ok() {
                    o.player.free_gl();
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
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
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
