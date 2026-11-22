//! A game session: launching, what is drawn while the game loads, runs and
//! closes, the exit button, and coming back to the menu.

use super::*;
use crate::render::face_player;

impl App {
    /// One tick while a game is loading, running or closing: follow the
    /// worker, watch the exit button, and redraw when it's time.
    pub(crate) fn frame_playing(&mut self, el: &ActiveEventLoop) -> Result<()> {
        use rp_launch::Phase;
        let Some(p) = self.playing.as_mut() else { return Ok(()) };
        match p.rx.try_recv() {
            Ok(result) => {
                // After a game that ran (or was closed by the player) the menu
                // takes the focus back; if nothing ever started, whatever the
                // player switched to meanwhile keeps it.
                let ran = p.ctl.stop_requested() || matches!(result, Ok(rp_launch::Outcome::Finished { .. }));
                match result {
                    Ok(outcome) => log::info!("Back from game ({outcome:?})"),
                    Err(e) => log::error!("Launch failed: {e:#}"),
                }
                self.return_from_game(el, ran);
                return Ok(());
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("The game thread ended without a result");
                self.return_from_game(el, true);
                return Ok(());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        let mut phase = p.ctl.phase();
        // The emulator's process shows up long before the table does. Until
        // a window of the game covers one of our screens, this is still
        // "Launching". (Other programs' windows don't count: VPin Studio's
        // overlay, say, takes the focus for a moment at every launch.)
        if phase == Phase::Playing && !p.game_up {
            let seen = *p.process_since.get_or_insert_with(Instant::now);
            let rects: Vec<(i32, i32, i32, i32)> = self.screens.iter().map(|s| (s.x, s.y, s.width, s.height)).collect();
            let (fg, pid) = win::foreground_of(&rects);
            let pids = p.ctl.game_pids();
            let game_window = fg == win::Foreground::Other && (pids.is_empty() || pids.contains(&pid));
            if game_window || seen.elapsed() >= GAME_UP_TIMEOUT {
                log::info!("Game on screen after {:.1}s", p.since.elapsed().as_secs_f32());
                p.game_up = true;
            } else {
                phase = Phase::Launching;
            }
        }
        if let Some(g) = self.gfx.as_mut() {
            // Popper's "Exit Emulators" button closes the game, whichever
            // window has the focus (ignored for a moment after Start).
            let pressed = g.joysticks.exit_pressed(&self.bindings);
            if pressed && p.since.elapsed() >= Duration::from_secs(1) && !p.ctl.stop_requested() {
                log::info!("Exit button pressed: closing the game");
                p.ctl.request_stop();
            }
        }
        if phase == Phase::Exiting && p.exiting_since.is_none() {
            p.exiting_since = Some(Instant::now());
        }
        let t = p.exiting_since.unwrap_or(p.since).elapsed().as_secs_f32();
        let want_paused = phase == Phase::Playing;
        if want_paused != p.others_paused {
            p.others_paused = want_paused;
            if let Some(g) = self.gfx.as_mut() {
                let keep = self.lib.items[p.index].keep_displays();
                for o in g.outs.iter().filter(|o| !keep.contains(&(o.screen.slot as u32))) {
                    let _ = o.player.set_pause(want_paused);
                }
                // The game is up: hand it the video memory the wheel logos use
                // (all but this game's, which the Exiting screen shows).
                if want_paused && !p.logos_released {
                    p.logos_released = true;
                    g.release_logos(Some(p.index));
                }
            }
        }
        // Animate while the table loads or closes, and while a screen the
        // emulator keeps is playing video; otherwise barely tick behind the game.
        let render_every = match phase {
            Phase::Playing if !p.kept_video => Duration::from_secs(1),
            _ => Duration::from_millis(33),
        };
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + render_every.min(PLAY_POLL)));
        if p.last_render.is_some_and(|at| at.elapsed() + Duration::from_millis(5) < render_every) {
            return Ok(());
        }
        p.last_render = Some(Instant::now());
        let index = p.index;
        self.render_launching(index, phase, t)
    }

    /// Shows the launch screen and runs the selected game's emulator scripts
    /// on a worker thread.
    pub(crate) fn launch(&mut self, index: usize) {
        if self.playing.is_some() {
            return;
        }
        let item = &self.lib.items[index];
        let Kind::Game { game, emu } = &item.kind else { return };
        // RP_NO_LAUNCH=1: log instead of starting a table (for automated tests).
        if std::env::var_os("RP_NO_LAUNCH").is_some() {
            log::info!("(RP_NO_LAUNCH) would launch {}", item.title);
            return;
        }
        let Some(g) = self.gfx.as_mut() else { return };
        let job = rp_launch::LaunchJob { popper_dir: self.popper_dir.clone(), emulator: (**emu).clone(), game: (**game).clone(), ignore: self.ignore_processes.clone() };
        // Make sure every screen shows this game's media while it loads.
        if g.shown != index {
            g.shown = index;
            for o in g.outs.iter_mut() {
                o.slide = None;
                o.show(item.media(o.screen.slot).as_deref());
            }
        }
        let kept_video = g.outs.iter().any(|o| emu.keep_displays.contains(&(o.screen.slot as u32)) && item.media(o.screen.slot).is_some_and(|m| is_video(&m)));
        g.joysticks.arm_exit(&self.bindings);
        // Logos still being loaded would only pile up while the game runs.
        g.logos = assets::idle();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctl = Arc::new(rp_launch::Control::default());
        let worker_ctl = ctl.clone();
        let stats = self.stats.clone();
        let game_id = game.id;
        let spawned = std::thread::Builder::new().name("game".into()).spawn(move || {
            let result = job.run(&worker_ctl);
            match &result {
                Ok(rp_launch::Outcome::Finished { secs }) if *secs >= MIN_PLAY_SECS => {
                    let recorded = stats.lock().map_err(|_| anyhow!("stats lock poisoned")).and_then(|mut s| s.record_play(game_id, *secs));
                    match recorded {
                        Ok(()) => log::info!("Recorded play of game {game_id} ({secs}s)"),
                        Err(e) => log::warn!("Couldn't record play stats: {e:#}"),
                    }
                }
                Ok(rp_launch::Outcome::Finished { secs }) => log::info!("Not counting a {secs}s play of game {game_id}"),
                _ => {}
            }
            let _ = tx.send(result);
        });
        if let Err(e) = spawned {
            log::error!("Couldn't start the game thread: {e}");
            return;
        }
        self.playing = Some(Playing {
            rx,
            ctl,
            index,
            since: Instant::now(),
            process_since: None,
            game_up: false,
            exiting_since: None,
            others_paused: false,
            kept_video,
            logos_released: false,
            last_render: None,
        });
    }

    /// Brings the menu back after a game: screens, video, logos and (when
    /// `take_focus`) the keyboard focus, even from another program.
    pub(crate) fn return_from_game(&mut self, el: &ActiveEventLoop, take_focus: bool) {
        self.playing = None;
        el.set_control_flow(ControlFlow::Poll);
        let now = Instant::now();
        // Time in the game isn't idle time: attract mode waits its full delay again.
        self.last_input = now;
        if take_focus {
            self.reclaim_until = now + RECLAIM_AFTER_GAME;
        }
        // Our own statistics write changed the file; pick up anything else
        // that changed while the game ran, too.
        self.db_seen = db_mtime(&self.popper_dir);
        self.db_dirty_since = None;
        self.last_db_check = now;
        // A re-read begun before the statistics were written would be stale.
        self.reload = None;
        self.start_reload();
        let Some(g) = self.gfx.as_mut() else { return };
        let item = &self.lib.items[g.shown];
        for o in g.outs.iter_mut() {
            o.slide = None;
            let _ = o.player.set_pause(false);
            o.show(item.media(o.screen.slot).as_deref());
        }
        g.load_logos(&self.lib.items, g.shown);
        if take_focus {
            win::force_foreground(g.pf_hwnd);
        } else {
            win::request_foreground(g.pf_hwnd);
        }
        self.last_reclaim = now;
        // Buttons still held from the game (e.g. Exit) must not fire in the menu.
        g.joysticks.swallow_held(&self.bindings);
        self.joy_blocked = false;
        g.last = now;
    }

    /// One frame while a game is loading, running or closing: the launch /
    /// exit screen on the playfield and the game's media (or black) elsewhere.
    pub(crate) fn render_launching(&mut self, index: usize, phase: rp_launch::Phase, t: f32) -> Result<()> {
        let ui_rotation = self.ui_rotation();
        let Some(g) = self.gfx.as_mut() else { return Ok(()) };
        let item = &self.lib.items[index];
        let keep = item.keep_displays();
        for o in g.outs.iter_mut() {
            o.pump_events();
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
        let (lw, lh) = face_player(&mut g.canvas, w, h, (pf.screen.rotation + ui_rotation).rem_euclid(360));
        // Once the game is on screen, the screen behind it already reads
        // "Exiting", so that's what shows the moment the table closes (not a
        // stale "Launching").
        let verb = if phase == rp_launch::Phase::Launching { "Launching" } else { "Exiting" };
        wheel::draw_launch(&mut g.canvas, lw, lh, &g.entries[index], verb, item.emu_name(), t, &g.fonts);
        g.canvas.restore();
        g.canvas.flush();
        pf.surface.swap_buffers(&g.context)?;
        Ok(())
    }
}
