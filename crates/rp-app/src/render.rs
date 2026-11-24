//! Setting up the windows, GL and players, and drawing a frame of the menu.

use super::*;

impl App {
    /// Extra clockwise turn of the menu overlay (Popper's ThumbRotate unless overridden).
    pub(crate) fn ui_rotation(&self) -> i32 {
        self.ui_rotation_override.or(self.lib.menu_rotation).unwrap_or(0).rem_euclid(360)
    }

    pub(crate) fn init(&mut self, el: &ActiveEventLoop) -> Result<()> {
        // Start in the last playlist used (if it still exists), on the --game
        // match or that playlist's first game.
        let last = std::fs::read_to_string(state_path()).unwrap_or_default();
        self.playlist = self.lib.playlist_named(last.trim());
        self.view = self.lib.view(self.playlist).to_vec();
        if !self.lib.has_games(self.playlist) || (self.start != 0 && !self.view.contains(&self.start)) {
            self.playlist = 0;
            self.view = self.lib.view(0).to_vec();
        }
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
        let loader: mpv::Loader = Box::new(Box::new(move |name| d2.get_proc_address(name)));

        let start_item = &self.lib.items[self.start];
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
            let out = ScreenOut { screen, window, surface, player, target: None, slide: None, file_loaded: Cell::new(false) };
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
        let now = Instant::now();
        let mut gfx = Gfx {
            pf_hwnd: hwnd_of(&outs[0].window),
            outs,
            context,
            revealed: false,
            gl,
            _loader: loader,
            canvas,
            fonts,
            entries: self.lib.items.iter().map(Item::entry).collect(),
            logos: assets::idle(),
            wheel: wheel::Wheel::new(self.view.len(), start_pos),
            joysticks: input::Joysticks::new(),
            shown: self.start,
            swapped_at: -10.0,
            t0: now,
            last: now,
        };
        // Load logos nearest the starting game first.
        gfx.load_logos(&self.lib.items, self.start);
        log::info!("Joysticks found: {}", gfx.joysticks.count());
        self.gfx = Some(gfx);
        Ok(())
    }

    pub(crate) fn frame(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let frame_start = Instant::now();
        // While a game runs, keep our windows up behind it (black, with a
        // "Launching" screen on the playfield) so the desktop never shows.
        if self.playing.is_some() {
            return self.frame_playing(el);
        }
        if self.watch_other_launch() {
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
            } else {
                self.joy_blocked = true;
            }
        }
        for a in actions {
            self.act(el, a);
        }
        if self.playing.is_some() {
            return Ok(()); // a button just launched a game
        }
        self.attract_tick();
        // DOF: a new random effect for each game or playlist the wheel settles on.
        if let Some(g) = &self.gfx {
            if self.dof_shown != Some(g.shown) {
                self.dof_shown = Some(g.shown);
                self.dof.select();
            }
        }
        // The selected game's own audio, once the wheel rests on it (not in attract mode).
        let selected = if self.attract_on { None } else { self.gfx.as_ref().and_then(|g| self.view.get(g.wheel.selected()).copied()) };
        let lib = &self.lib;
        self.game_audio.tick(selected, |i| {
            let it = &lib.items[i];
            if it.is_playlist() { None } else { gameaudio::find(&it.media_dir, &it.media_name) }
        });
        // A fresh update that has run for a minute has proved itself.
        if !self.update_confirmed && STARTED.get().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            self.update_confirmed = true;
            updater::confirm(&exe_dir());
        }
        // An update downloaded from Options: swap it in and restart (never mid-game).
        if self.playing.is_none() {
            if let updater::Status::Staged { version, dir } = self.updater.status() {
                // Mark it first: el.exit() only takes effect after this frame.
                self.updater.restarting(version.clone());
                match updater::apply_and_restart(&dir, &exe_dir(), &version) {
                    Ok(()) => {
                        el.exit();
                        return Ok(());
                    }
                    Err(e) => {
                        log::error!("Installing the update failed: {e:#}");
                        self.updater.fail(format!("{e:#}"));
                    }
                }
            }
        }
        self.poll_card();
        // The "no card" notice closes by itself.
        if self.card.as_ref().is_some_and(|c| c.expired()) {
            self.close_card();
        }
        let style = self.style;
        let option_labels = match self.menu.as_ref().map(|m| m.kind) {
            Some(MenuKind::Sound) => self.sound_labels(),
            Some(MenuKind::Attract) => self.attract_labels(),
            _ => self.option_labels(),
        };
        let ui_rotation = self.ui_rotation();
        let Some(g) = self.gfx.as_mut() else { return Ok(()) };

        let now_i = Instant::now();
        let dt = (now_i - g.last).as_secs_f32().min(0.05);
        g.last = now_i;
        let now = (now_i - g.t0).as_secs_f64();

        // Upload any logos the loader finished (a few per frame to avoid hitches).
        g.context.make_current(&g.outs[0].surface)?;
        for logo in g.logos.try_iter().take(4) {
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

        g.wheel.update(dt, now);
        // Swap media once the selection has rested for a moment.
        let sel = self.view.get(g.wheel.selected()).copied().unwrap_or(g.shown);
        if sel != g.shown && now - g.wheel.changed_at >= wheel::MEDIA_DEBOUNCE as f64 {
            g.shown = sel;
            g.swapped_at = now;
            let item = &self.lib.items[sel];
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
            if o.pump_events() {
                el.exit();
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
        let pf_rot = (g.outs[0].screen.rotation + ui_rotation).rem_euclid(360);
        for (i, o) in g.outs.iter_mut().enumerate().skip(1) {
            g.context.make_current(&o.surface)?;
            let card = self.card.as_ref().filter(|c| c.out == i);
            if o.target.is_some() || card.is_some() {
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
        // Arc fades the new video in from black; cover flow slides it instead.
        let fade = if style == WheelStyle::Flow || g.shown != sel { 1.0 } else { ((now - g.swapped_at) as f32 / wheel::MEDIA_FADE).min(1.0) };
        let letter = self.letter.map(|(c, at)| (c, at.elapsed().as_secs_f32()));
        let frame = wheel::Frame {
            style,
            wheel: &g.wheel,
            entries: &g.entries,
            order: &self.view,
            playlist: self.lib.playlist_name(self.playlist),
            fonts: &g.fonts,
            video_fade: fade,
            letter,
            update_note: match self.updater.status() {
                updater::Status::Available(r) => Some(format!("UPDATE v{} AVAILABLE", r.version)),
                updater::Status::Downloading { percent, .. } => Some(format!("DOWNLOADING UPDATE {percent}%")),
                _ => None,
            },
        };
        wheel::draw(&mut g.canvas, lw, lh, &frame);
        if let Some(menu) = &self.menu {
            let names: Vec<&str> = match menu.kind {
                MenuKind::Exit => EXIT_MENU.to_vec(),
                MenuKind::Options | MenuKind::Attract | MenuKind::Sound => option_labels.iter().map(String::as_str).collect(),
            };
            let title = match menu.kind {
                MenuKind::Exit => "Exit",
                MenuKind::Options => "Options",
                MenuKind::Attract => "Attract mode",
                MenuKind::Sound => "Sound",
            };
            let view = wheel::MenuView { title, options: &names, selected: menu.selected, t: menu.opened.elapsed().as_secs_f32() };
            wheel::draw_menu(&mut g.canvas, lw, lh, &view, &g.fonts);
        }
        if let Some(card) = self.card.as_ref().filter(|c| c.out == 0) {
            overlay::draw(&mut g.canvas, lw, lh, card, &g.fonts);
        }
        g.canvas.restore();
        g.canvas.flush();
        pf.surface.swap_buffers(&g.context)?;
        if !g.revealed {
            // Every screen has a frame now: show them (secondaries only with media).
            g.revealed = true;
            let item = &self.lib.items[g.shown];
            for o in &g.outs {
                o.window.set_visible(o.screen.slot == Slot::PlayField || item.media(o.screen.slot).is_some());
            }
            g.outs[0].window.focus_window();
        }

        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            el.exit();
        }
        // Normally the swap above waits for vsync; if it didn't, don't spin.
        if let Some(short) = MIN_FRAME.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(short);
        }
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
