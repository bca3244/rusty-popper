//! Setting up the windows, GL and players, and drawing a frame of the menu.

use super::*;

impl App {
    /// Extra clockwise turn of the menu overlay (Popper's ThumbRotate unless overridden).
    pub(crate) fn ui_rotation(&self) -> i32 {
        self.ui_rotation_override.or(self.lib.menu_rotation).unwrap_or(0).rem_euclid(360)
    }

    pub(crate) fn init(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let init_start = Instant::now();
        // Always start in All games, on its first game (A-Z) or the --game
        // match. Nothing about the last session is remembered; the file that
        // used to remember the playlist goes.
        let _ = std::fs::remove_file(exe_dir().join("last-playlist.txt"));
        self.playlist = 0;
        self.view = self.lib.view(0).to_vec();
        let start_pos = self.view.iter().position(|&i| i == self.start).unwrap_or(0);
        self.start = self.view[start_pos];
        log::info!("Menu rotation: {} deg", self.ui_rotation());
        if let Some(pf) = self.screens.iter().find(|s| s.slot == Slot::PlayField) {
            // So an error on a later start can be shown the right way up.
            let rot = (pf.rotation + self.ui_rotation()).rem_euclid(360);
            errscreen::remember(&exe_dir(), errscreen::Placement { x: pf.x, y: pf.y, w: pf.width, h: pf.height, rot });
        }
        // Playfield first: its window creates the GL display and config.
        let mut order: Vec<Screen> = self.screens.clone();
        order.sort_by_key(|s| (s.slot != Slot::PlayField) as u8);

        // Start every player on its own thread while the windows are made:
        // starting mpv takes a while, and only the GL part needs this thread.
        let players: Vec<_> = order
            .iter()
            .map(|screen| {
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
                    ("volume", if screen.slot == Slot::PlayField { self.video_volume() } else { screen.volume }.to_string()),
                ];
                let mpv = self.mpv.clone();
                let slot = screen.slot;
                std::thread::Builder::new().name("mpv-start".into()).spawn(move || {
                    // Don't scan each video's folder for subtitles, audio tracks or cover art.
                    // Plain scaling: the videos are shown at (or near) their own size,
                    // where mpv's sharper filters cost a lot at 4K and show little.
                    let optional = [
                        ("sub-auto", "no"),
                        ("audio-file-auto", "no"),
                        ("cover-art-auto", "no"),
                        ("scale", "bilinear"),
                        ("dscale", "bilinear"),
                        ("dither", "no"),
                        ("correct-downscaling", "no"),
                        ("sigmoid-upscaling", "no"),
                    ];
                    mpv.player(&opts, &optional).with_context(|| format!("{slot:?}"))
                })
            })
            .collect();

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
        // A multisampled window can't take a framebuffer copy (glBlitFramebuffer).
        let blit_ok = gl_config.num_samples() == 0;

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
        let loader: mpv::Loader = Box::new(Box::new(move |name| d2.get_proc_address(name)));

        let start_item = &self.lib.items[self.start];
        let mut outs = Vec::new();
        for (((screen, window), surface), player) in order.into_iter().zip(windows).zip(surfaces).zip(players) {
            context.make_current(&surface)?;
            // Only the playfield waits for vsync, so three swaps don't triple the frame time.
            // (The interval belongs to the shared context, so frame() sets it again
            // before each swap; this is just the starting state.)
            let interval = if screen.slot == Slot::PlayField { SwapInterval::Wait(NonZeroU32::new(1).unwrap()) } else { SwapInterval::DontWait };
            let _ = surface.set_swap_interval(&context, interval);
            let mut player = player
                .map_err(|e| anyhow!("starting the {:?} player: {e}", screen.slot))?
                .join()
                .map_err(|_| anyhow!("starting the {:?} player panicked", screen.slot))??;
            player.init_gl(&loader)?;
            // A new frame wakes the (otherwise sleeping) still menu.
            let waker = self.waker.clone();
            player.on_update(Arc::new(move || waker.frame_ready()));
            window.set_cursor_visible(false);
            let out = ScreenOut {
                screen,
                window,
                surface,
                player,
                target: None,
                slide: None,
                file_loaded: Cell::new(false),
                load_tag: Cell::new(0),
                load_acked: Cell::new(false),
                wants_video: Cell::new(false),
                loaded_at: Cell::new(None),
                hwdec_logged: Cell::new(false),
                dirty: Cell::new(true),
                drawn_at: Cell::new(Instant::now()),
                had_card: Cell::new(false),
                loaded: RefCell::new(None),
                ended: Cell::new(false),
                vid_off: Cell::new(false),
            };
            out.load(start_item.media(out.screen.slot).as_deref());
            outs.push(out);
        }

        context.make_current(&outs[0].surface)?;
        let renderer = unsafe { OpenGl::new_from_function_cstr(|s| display.get_proc_address(s)) }
            .map_err(|e| anyhow!("femtovg: {e:?}"))?;
        let mut canvas = Canvas::new(renderer).map_err(|e| anyhow!("femtovg canvas: {e:?}"))?;
        let font_dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into())).join("Fonts");
        let font = |canvas: &mut Canvas<OpenGl>, files: &[&str]| {
            files
                .iter()
                .find_map(|f| canvas.add_font(font_dir.join(f)).ok())
                .ok_or_else(|| anyhow!("none of the fonts {files:?} found in {}", font_dir.display()))
        };
        let fonts = wheel::Fonts {
            display: font(&mut canvas, &["bahnschrift.ttf", "segoeuib.ttf", "arialbd.ttf"])?,
            body: font(&mut canvas, &["segoeui.ttf", "arial.ttf"])?,
            mono: font(&mut canvas, &["consola.ttf", "cour.ttf"])?,
            text: Default::default(),
        };

        // Offscreen video targets (one context, so textures work on every surface).
        let gl = unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s)) };
        for o in outs.iter_mut() {
            context.make_current(&o.surface)?;
            match video::Target::new(&gl, &mut canvas, o.screen.width, o.screen.height) {
                Ok(t) => o.target = Some(t),
                Err(e) => log::warn!("{:?}: no offscreen video ({e:#}); slides disabled", o.screen.slot),
            }
        }
        context.make_current(&outs[0].surface)?;
        let refresh = outs[0].window.current_monitor().and_then(|m| m.refresh_rate_millihertz());
        let frame_period = refresh.filter(|&mhz| mhz >= 20_000).map_or(MIN_FRAME, |mhz| Duration::from_micros(1_000_000_000 / mhz as u64));
        log::info!("Playfield refresh: {}", refresh.map_or("unknown".into(), |mhz| format!("{:.2} Hz", mhz as f32 / 1000.0)));
        let now = Instant::now();
        let mut gfx = Gfx {
            pf_hwnd: hwnd_of(&outs[0].window),
            outs,
            context,
            revealed: false,
            qr: None,
            gl,
            _loader: loader,
            canvas,
            fonts,
            entries: self.lib.items.iter().map(Item::entry).collect(),
            logos: assets::idle(),
            logo_keep: Vec::new(),
            logos_steered: None,
            wheel: wheel::Wheel::new(self.view.len(), start_pos),
            joysticks: input::Joysticks::new(),
            shown: self.start,
            t0: now,
            last: now,
            perf: perf::Frames::new(),
            vsync_on: None,
            frame_period,
            pacer: perf::Pacer::new(),
            blit_ok,
        };
        // The first frame asks for the logos nearest the starting game.
        gfx.load_logos(&self.lib.items);
        perf::note("Creating windows and players", init_start, 500);
        self.gfx = Some(gfx);
        Ok(())
    }

    pub(crate) fn frame(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let frame_start = Instant::now();
        self.waker.woken();
        self.poll_remote(el);
        // A fresh update that has run for a minute has proved itself (also
        // when that minute ends mid-game: a game started from it and then a
        // power switch is no failed start).
        if !self.update_confirmed && STARTED.get().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            self.update_confirmed = true;
            updater::confirm(&exe_dir());
        }
        // While a game runs, keep our windows up behind it (black, with a
        // "Launching" screen on the playfield) so the desktop never shows.
        if self.playing.is_some() {
            return self.frame_playing(el);
        }
        if self.watch_other_launch() {
            let name = self.aside.as_ref().map(|a| a.name.clone());
            self.publish_remote(remote::OTHER, None, None, name.as_deref());
            self.game_audio.tick(None, |_| None);
            // Nothing to draw or listen to until that table closes.
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250)));
            return Ok(());
        }
        el.set_control_flow(ControlFlow::Poll);
        self.poll_reload();
        self.check_db_changed();
        let listen = self.watch_focus();
        let mut actions = Vec::new();
        {
            let Some(g) = self.gfx.as_mut() else { return Ok(()) };
            if listen {
                if self.joy_blocked {
                    // Buttons held while the other program was in front don't count.
                    self.joy_blocked = false;
                    g.joysticks.swallow_held(&self.bindings);
                }
                g.joysticks.poll(&self.bindings, &mut actions);
                self.key_repeat.tick(&mut actions);
            } else {
                self.joy_blocked = true;
                self.key_repeat.clear();
            }
        }
        for a in actions {
            self.act(el, a);
        }
        if self.playing.is_some() {
            return Ok(()); // a button just launched a game
        }
        self.attract_tick();
        if let Some(shown) = self.gfx.as_ref().map(|g| g.shown) {
            self.publish_remote(remote::MENU, Some(shown), None, None);
        }
        // DOF: a new random effect for each game or playlist the wheel settles on.
        if let Some(g) = &self.gfx {
            if self.dof_shown != Some(g.shown) {
                self.dof_shown = Some(g.shown);
                self.dof.select();
            }
        }
        // The selected game's own audio, once the wheel rests on it (attract mode included).
        let selected = self.gfx.as_ref().and_then(|g| self.view.get(g.wheel.selected()).copied());
        let lib = &self.lib;
        self.game_audio.tick(selected, |i| lib.items[i].audio());
        // An update downloaded from Options: swap it in and restart (never mid-game).
        let update_status = self.updater.status();
        if self.playing.is_none() {
            if let updater::Status::Staged { version, dir } = &update_status {
                let (version, dir) = (version.clone(), dir.clone());
                // Mark it first: el.exit() only takes effect after this frame.
                self.updater.restarting(version.clone());
                // The new version opens DOF as it starts: ours must be over.
                self.dof.shutdown();
                match updater::apply_and_restart(&dir, &exe_dir(), &version) {
                    Ok(()) => {
                        el.exit();
                        return Ok(());
                    }
                    Err(e) => {
                        log::error!("Installing the update failed: {e:#}");
                        self.updater.fail(format!("{e:#}"));
                        self.dof.resume();
                        self.dof_shown = None;
                    }
                }
            }
        }
        self.poll_card();
        // The "no card" notice closes by itself, and so does a notice.
        if self.card.as_ref().is_some_and(|c| c.expired()) {
            self.close_card();
        }
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed().as_secs_f32() > overlay::NOTICE_SECS) {
            self.notice = None;
        }
        let (style, layout) = (self.style, self.layout);
        let option_labels = match self.menu.as_ref().map(|m| m.kind) {
            Some(MenuKind::Sound) => self.sound_labels(),
            Some(MenuKind::Layout) => self.layout_labels(),
            Some(MenuKind::Attract) => self.attract_labels(),
            Some(MenuKind::Dof) => self.dof_labels(),
            Some(MenuKind::Options) => self.option_labels(),
            Some(MenuKind::Exit) | None => Vec::new(),
        };
        let ui_rotation = self.ui_rotation();
        // Menu, letter and card animations all finish within a second of the
        // button press (or the card's picture arriving) that started them.
        let animating = self.last_input.elapsed() < STILL_AFTER || self.card.as_ref().is_some_and(|c| c.opened.elapsed() < STILL_AFTER) || self.notice.is_some();
        let Some(g) = self.gfx.as_mut() else { return Ok(()) };

        let now_i = Instant::now();
        let dt = (now_i - g.last).as_secs_f32().min(0.05);
        g.last = now_i;
        let now = (now_i - g.t0).as_secs_f64();
        g.perf.start(frame_start);
        g.perf.part("input");

        // Upload any logos the loader finished, within a small time budget
        // per frame so loading never costs a dropped frame: while anything
        // moves, a few logos that are on the wheel now (so a letter jump
        // doesn't sit on placeholders) or one that isn't, and the playfield
        // is redrawn for one that shows up on a still wheel.
        let moving = animating || !g.wheel.at_rest() || g.outs.iter().any(|o| o.slide.is_some());
        let budget = if moving { MOVING_UPLOAD_BUDGET } else { LOGO_UPLOAD_BUDGET };
        let upload_start = Instant::now();
        let mut current = false;
        let mut uploaded = 0;
        let mut off_wheel = false;
        while upload_start.elapsed() < budget && !(moving && (off_wheel || uploaded >= MOVING_UPLOADS)) {
            let Some(logo) = g.logos.try_recv() else { break };
            // One the wheel has scrolled far from since it was asked for isn't wanted now.
            if !g.logo_keep.get(logo.index).copied().unwrap_or(true) {
                continue;
            }
            uploaded += 1;
            if !current {
                g.context.make_current(&g.outs[0].surface)?;
                current = true;
            }
            let shown = on_wheel(&self.view, g.wheel.selected(), logo.index);
            off_wheel |= !shown;
            if !moving && shown {
                g.outs[0].dirty.set(true);
            }
            let upload = |c: &mut Canvas<OpenGl>, img: &image::RgbaImage| {
                use femtovg::rgb::FromSlice;
                let (w, h) = img.dimensions();
                c.create_image(femtovg::imgref::Img::new(img.as_raw().as_rgba(), w as usize, h as usize), ImageFlags::empty())
            };
            let Some(entry) = g.entries.get_mut(logo.index).filter(|e| e.logo.is_none()) else { continue };
            let Ok(img) = upload(&mut g.canvas, &logo.rgba) else { continue };
            match upload(&mut g.canvas, &logo.glow) {
                Ok(glow) => {
                    let (w, h) = logo.rgba.dimensions();
                    entry.logo = Some(wheel::LogoTex { img, glow, aspect: w as f32 / h as f32, hue: logo.hue });
                }
                Err(_) => g.canvas.delete_image(img),
            }
        }

        g.perf.part("logo uploads");
        g.wheel.update(dt, now);
        // Swap media at once on a step after a pause (long enough for a Cover
        // flow slide to finish), so a single press loads straight away; while
        // scrolling fast, wait until the selection rests for a moment.
        let sel = self.view.get(g.wheel.selected()).copied().unwrap_or(g.shown);
        g.steer_logos(&self.view, g.wheel.selected());
        let settled = now - g.wheel.changed_at >= wheel::MEDIA_DEBOUNCE as f64;
        let paused = g.wheel.gap >= (wheel::MEDIA_DEBOUNCE + video::SLIDE_SECS) as f64;
        if sel != g.shown && (settled || paused) {
            g.shown = sel;
            let item = &self.lib.items[sel];
            let slide = style == WheelStyle::Flow;
            let dir = g.wheel.last_dir;
            for o in g.outs.iter_mut() {
                let media = item.media(o.screen.slot);
                // Cover flow: freeze the outgoing frame and slide it away once
                // the new video has a frame.
                if slide && media.is_some() {
                    if let Some(t) = o.target.as_mut() {
                        g.context.make_current(&o.surface)?;
                        if t.freeze(&g.gl, &mut g.canvas) {
                            o.slide = Some(video::Slide::new(dir));
                        }
                    }
                }
                o.show(media.as_deref());
            }
        }
        for o in g.outs.iter_mut() {
            if o.pump_events() {
                el.exit();
            }
        }
        g.perf.part("media swap");

        // Which screens need presenting this frame. While anything moves the
        // playfield is presented every refresh, but mpv only draws into its
        // texture when there's a new video frame. Once the menu is still, the
        // playfield too is presented only for a new video frame (so 60 fps
        // video costs 60 frames a second, not the display's 144), like a
        // secondary screen. Either way there's a redraw every second as a
        // backstop.
        let card_out = self.card.as_ref().map(|c| c.out);
        let still = !animating && g.wheel.at_rest() && g.outs.iter().all(|o| o.slide.is_none());
        let pf_every_frame = !still || g.outs[0].target.is_none();
        let present: Vec<(bool, bool)> = g
            .outs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let fresh = o.player.has_new_frame();
                let card_here = card_out == Some(i);
                let card_changed = o.had_card.replace(card_here) != card_here;
                let stale = o.drawn_at.get().elapsed() >= REDRAW_EVERY;
                let render_video = fresh || o.dirty.get() || stale || o.target.is_none();
                let every_frame = i == 0 && pf_every_frame;
                let present = every_frame || fresh || o.dirty.get() || stale || card_here || card_changed || o.slide.is_some();
                (render_video && present, present)
            })
            .collect();

        // Pass 1: every screen's video into its offscreen texture (all mpv work
        // first, with GL state reset, since femtovg's state upsets mpv). The
        // textures belong to the one shared context, so only a screen without
        // one needs its own surface current.
        for (i, (o, &(render_video, _))) in g.outs.iter().zip(&present).enumerate() {
            if !render_video {
                continue;
            }
            if i == 0 {
                // The playfield is presented every frame; this times its video.
                o.drawn_at.set(Instant::now());
            }
            match &o.target {
                Some(t) => {
                    video::reset_for_mpv(&g.gl);
                    o.player.render_to(t.fbo_id(), o.screen.width, o.screen.height);
                }
                None => {
                    g.context.make_current(&o.surface)?;
                    video::reset_for_mpv(&g.gl);
                    o.player.render(o.screen.width, o.screen.height);
                    g.context.make_current(&g.outs[0].surface)?;
                }
            }
        }
        g.perf.part("video render");
        // Pass 2: composite. The slide runs along the player's left/right: on
        // the playfield that's the menu's rotated x axis, elsewhere screen x.
        let pf_rot = (g.outs[0].screen.rotation + ui_rotation).rem_euclid(360);
        for (i, o) in g.outs.iter_mut().enumerate().skip(1) {
            if !present[i].1 {
                continue;
            }
            o.dirty.set(false);
            o.drawn_at.set(Instant::now());
            g.context.make_current(&o.surface)?;
            if g.vsync_on != Some(false) {
                g.vsync_on = Some(false);
                if let Err(e) = o.surface.set_swap_interval(&g.context, SwapInterval::DontWait) {
                    log::warn!("Turning vsync off for {:?}: {e}", o.screen.slot);
                }
            }
            let card = self.card.as_ref().filter(|c| c.out == i);
            // Just the video: copied straight to the window, without femtovg.
            let plain = card.is_none() && o.slide.is_none() && g.blit_ok;
            if let (Some(t), true) = (&o.target, plain) {
                t.copy_to_window(&g.gl);
            } else if o.target.is_some() || card.is_some() {
                g.canvas.set_size(o.screen.width as u32, o.screen.height as u32, 1.0);
                if let Some(t) = &o.target {
                    let a = (o.screen.rotation as f32).to_radians();
                    video::composite(&mut g.canvas, t, &mut o.slide, (a.cos(), a.sin()));
                }
                if let Some(card) = card {
                    overlay::draw(&mut g.canvas, o.screen.width as f32, o.screen.height as f32, card, &g.fonts);
                }
                g.canvas.flush();
            }
            o.surface.swap_buffers(&g.context)?;
        }
        g.perf.part("other screens");
        if !present[0].1 {
            // Still, and no new video frame yet: sleep until a player has one
            // (its update callback wakes the loop), a key is pressed, or it's
            // time to look at the joysticks again.
            if self.deadline.is_some_and(|d| Instant::now() >= d) {
                el.exit();
            }
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + STILL_POLL));
            return Ok(());
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
        let (lw, lh) = face_player(&mut g.canvas, w, h, pf_rot);
        let letter = self.letter.map(|(c, at)| (c, at.elapsed().as_secs_f32()));
        // The phone remote's QR code, (re)made when the remote starts or its address changes.
        match &self.remote {
            Some(r) if g.qr.as_ref().map(|q| q.version) != Some(r.url_version()) => {
                if let Some(old) = g.qr.take() {
                    g.canvas.delete_image(old.img);
                }
                let (url, version) = r.url();
                g.qr = remote::qr_rgba(&url).and_then(|(rgba, n)| {
                    use femtovg::rgb::FromSlice;
                    let img = g.canvas.create_image(femtovg::imgref::Img::new(rgba.as_rgba(), n, n), ImageFlags::NEAREST).ok()?;
                    Some(Qr { img, modules: n, version })
                });
            }
            _ => {}
        }
        let frame = wheel::Frame {
            style,
            layout,
            wheel: &g.wheel,
            entries: &g.entries,
            order: &self.view,
            playlist: self.lib.playlist_name(self.playlist),
            fonts: &g.fonts,
            letter,
            update_note: match update_status {
                updater::Status::Available(r) => Some(format!("UPDATE v{} AVAILABLE", r.version)),
                updater::Status::Downloading { percent, .. } => Some(format!("DOWNLOADING UPDATE {percent}%")),
                _ => None,
            },
            qr: g.qr.as_ref().filter(|_| self.remote.is_some()).map(|q| (q.img, q.modules)),
        };
        wheel::draw(&mut g.canvas, lw, lh, &frame);
        if let Some(menu) = &self.menu {
            let names: Vec<&str> = match menu.kind {
                MenuKind::Exit => EXIT_MENU.to_vec(),
                MenuKind::Options | MenuKind::Attract | MenuKind::Sound | MenuKind::Layout | MenuKind::Dof => option_labels.iter().map(String::as_str).collect(),
            };
            let title = match menu.kind {
                MenuKind::Exit => "Exit",
                MenuKind::Options => "Options",
                MenuKind::Attract => "Attract Mode",
                MenuKind::Sound => "Sound",
                MenuKind::Layout => "Layout",
                MenuKind::Dof => "DOF",
            };
            let view = wheel::MenuView { title, options: &names, selected: menu.selected, t: menu.opened.elapsed().as_secs_f32() };
            wheel::draw_menu(&mut g.canvas, lw, lh, &view, &g.fonts);
        }
        if let Some(card) = self.card.as_ref().filter(|c| c.out == 0) {
            overlay::draw(&mut g.canvas, lw, lh, card, &g.fonts);
        }
        if let Some((text, at)) = &self.notice {
            overlay::draw_notice(&mut g.canvas, lw, lh, text, at.elapsed().as_secs_f32(), &g.fonts);
        }
        g.canvas.restore();
        g.fonts.text.borrow_mut().end_frame(&mut g.canvas);
        g.canvas.flush();
        g.perf.part("playfield draw");
        let busy = frame_start.elapsed();
        if g.vsync_on != Some(true) {
            g.vsync_on = Some(true);
            if let Err(e) = pf.surface.set_swap_interval(&g.context, SwapInterval::Wait(NonZeroU32::new(1).unwrap())) {
                log::warn!("Turning vsync on for the playfield: {e}");
            }
        }
        pf.surface.swap_buffers(&g.context)?;
        pf.dirty.set(false);
        g.perf.end(busy);
        if !g.revealed {
            // Every screen has a frame now: show them (secondaries only with media).
            g.revealed = true;
            if let Some(t) = STARTED.get() {
                log::info!("perf: first frame {} ms after start", t.elapsed().as_millis());
            }
            let item = &self.lib.items[g.shown];
            for o in &g.outs {
                o.window.set_visible(o.screen.slot == Slot::PlayField || item.media(o.screen.slot).is_some());
            }
            g.outs[0].window.focus_window();
            // A notice from startup (settings that couldn't be read) is timed from now.
            if let Some(n) = self.notice.as_mut() {
                n.1 = Instant::now();
            }
            MENU_SHOWN.store(true, Ordering::Relaxed);
        }

        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            el.exit();
        }
        // Normally the swap above waits for vsync. If the driver ignores it
        // (seen on the cabinet), pace frames to the refresh rate ourselves.
        g.pacer.after_swap(g.frame_period, pf_every_frame);
        Ok(())
    }

    pub(crate) fn shutdown(&mut self) {
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

/// Whether item `item` is among the logos the wheel shows around view
/// position `selected` (the wheel draws at most seven each side).
fn on_wheel(view: &[usize], selected: usize, item: usize) -> bool {
    let n = view.len() as i64;
    n > 0 && (-7..=7).any(|k| view.get((selected as i64 + k).rem_euclid(n) as usize) == Some(&item))
}

/// Turns the canvas so drawing faces the player: on a cabinet the playfield
/// is usually mounted turned, so the menu is rotated `rot` degrees clockwise.
/// Returns the size of the screen as the player sees it.
pub(crate) fn face_player(canvas: &mut Canvas<OpenGl>, w: i32, h: i32, rot: i32) -> (f32, f32) {
    let (lw, lh) = if rot % 180 == 0 { (w as f32, h as f32) } else { (h as f32, w as f32) };
    if rot != 0 {
        canvas.translate(w as f32 / 2.0, h as f32 / 2.0);
        canvas.rotate((rot as f32).to_radians());
        canvas.translate(-lw / 2.0, -lh / 2.0);
    }
    (lw, lh)
}

#[cfg(test)]
mod tests {
    use super::on_wheel;

    #[test]
    fn logos_near_the_selection_count_as_on_the_wheel() {
        let view: Vec<usize> = (100..120).collect();
        assert!(on_wheel(&view, 0, 100));
        assert!(on_wheel(&view, 0, 107));
        assert!(!on_wheel(&view, 0, 108));
        // Wraps around both ends.
        assert!(on_wheel(&view, 0, 119));
        assert!(on_wheel(&view, 19, 106));
        assert!(!on_wheel(&view, 10, 999));
        assert!(!on_wheel(&[], 0, 0));
    }
}
