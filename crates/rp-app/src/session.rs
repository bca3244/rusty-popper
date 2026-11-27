//! A game session: launching, what is drawn while the game loads, runs and
//! closes, the exit button, and coming back to the menu.

use super::*;
use crate::render::face_player;

impl App {
    /// One tick while a game is loading, running or closing: follow the
    /// worker, watch the exit button, and redraw when it's time.
    pub(crate) fn frame_playing(&mut self, el: &ActiveEventLoop) -> Result<()> {
        self.game_audio.tick(None, |_| None);
        use rp_launch::Phase;
        let Some(p) = self.playing.as_mut() else { return Ok(()) };
        match p.rx.try_recv() {
            Ok(result) => {
                // After a game that ran (or was closed by the player) the menu
                // takes the focus back; if nothing ever started, whatever the
                // player switched to meanwhile keeps it.
                let ran = p.ctl.stop_requested() || matches!(result, Ok(rp_launch::Outcome::Finished { .. }));
                let failed = !p.ctl.stop_requested() && !matches!(result, Ok(rp_launch::Outcome::Finished { .. }));
                let index = p.index;
                match result {
                    Ok(outcome) => log::info!("Back from game ({outcome:?})"),
                    Err(e) => log::error!("Launch failed: {e:#}"),
                }
                self.return_from_game(el, ran);
                if failed {
                    self.launch_failed(index);
                }
                return Ok(());
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("The game thread ended without a result");
                let (index, closing) = (p.index, p.ctl.stop_requested());
                self.return_from_game(el, true);
                if !closing {
                    self.launch_failed(index);
                }
                return Ok(());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        let mut phase = p.ctl.phase();
        // The emulator's process shows up long before the table does. Until
        // a window of the game covers one of our screens, this is still
        // "Launching": the other screens keep their media. (Other programs'
        // windows don't count: VPin Studio's overlay, say, takes the focus for
        // a moment at every launch.)
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
        // The playfield keeps saying "Launching" until the game's window
        // covers the playfield itself. While a table loads, its backglass or
        // DMD window (B2S, PinMAME, FlexDMD; often inside the emulator's own
        // process) can fill the backglass or DMD screen and take the focus
        // well before the table is drawn.
        if phase == Phase::Playing && !p.table_up {
            let seen = p.process_since.unwrap_or(p.since);
            let pf = self.screens.iter().find(|s| s.slot == Slot::PlayField).map(|s| (s.x, s.y, s.width, s.height));
            let pids = p.ctl.game_pids();
            let table_window = pf.and_then(win::foreground_covering).filter(|(pid, _)| pids.is_empty() || pids.contains(pid));
            if let Some((_, window)) = &table_window {
                log::info!("Table on the playfield after {:.1}s ({window})", p.since.elapsed().as_secs_f32());
                p.table_up = true;
            } else if seen.elapsed() >= TABLE_UP_TIMEOUT {
                log::info!("Table counted as on the playfield after {:.1}s (no game window seen there)", p.since.elapsed().as_secs_f32());
                p.table_up = true;
            }
        }
        let launching = phase == Phase::Launching || (phase == Phase::Playing && !p.table_up);
        let state = match phase {
            Phase::Exiting => remote::EXITING,
            _ if launching => remote::LAUNCHING,
            _ => remote::PLAYING,
        };
        let (index, since) = (p.index, p.since);
        self.publish_remote(state, Some(index), Some(since), None);
        // A restart or shutdown from the phone doesn't wait forever on a game
        // that won't close: its processes are ended (as a second Exit press
        // would), and a while later the cabinet goes down anyway.
        let Some(p) = self.playing.as_mut() else { return Ok(()) };
        if let Some((flag, at)) = self.pending_power {
            if at.elapsed() >= POWER_WAIT && !p.ctl.force_requested() {
                log::warn!("The game didn't close in time: force-closing it before {flag}");
                p.ctl.request_force();
            } else if at.elapsed() >= POWER_WAIT + POWER_FORCE_WAIT {
                log::warn!("The game still hasn't closed: {flag} anyway");
                self.pending_power = None;
                let _ = self.power_off(el, flag);
                return Ok(());
            }
        }
        let Some(p) = self.playing.as_mut() else { return Ok(()) };
        if let Some(g) = self.gfx.as_mut() {
            // Popper's "Exit Emulators" button closes the game, whichever
            // window has the focus (ignored for a moment after Start).
            let pressed = g.joysticks.exit_pressed(&self.bindings);
            if pressed && p.since.elapsed() >= Duration::from_secs(1) {
                match p.stop_pressed {
                    None if !p.ctl.stop_requested() => {
                        log::info!("Exit button pressed: closing the game");
                        p.ctl.request_stop();
                        p.stop_pressed = Some(Instant::now());
                    }
                    // Pressed again (a moment later, so not a bounce): the
                    // table is stuck, end it now.
                    Some(t) if !p.ctl.force_requested() && t.elapsed() >= FORCE_AFTER => {
                        log::info!("Exit button pressed again: force-closing the game");
                        p.ctl.request_force();
                    }
                    _ => {}
                }
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
        self.render_launching(index, phase, launching, t)
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
        // Keys held now are the game's from here on.
        self.key_repeat.clear();
        // The table's own DOF needs the hardware: end ours (as Popper does).
        let dof_done = self.dof.suspend_for_game();
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
            // Our DOF session must be over before the table opens its own
            // (it may still be starting up: Init takes a second or two).
            if let Some(done) = dof_done {
                if done.recv_timeout(dof::FINISH_WAIT).is_err() {
                    log::warn!("DOF: the menu's session didn't end in time; starting the game anyway");
                }
            }
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
            // Nothing is running: undo what was set up for the game.
            log::error!("Couldn't start the game thread: {e}");
            self.dof.suspend(false);
            if let Some(g) = self.gfx.as_mut() {
                g.load_logos(&self.lib.items, g.shown);
            }
            self.launch_failed(index);
            return;
        }
        // The launch screen covers the playfield video: stop decoding it so
        // the table loads faster (its audio keeps playing).
        if let Some(pf) = self.gfx.as_ref().map(|g| &g.outs[0]).filter(|o| !emu.keep_displays.contains(&(o.screen.slot as u32))) {
            let _ = pf.player.command_async(&["set", "vid", "no"]);
        }
        self.playing = Some(Playing {
            rx,
            ctl,
            index,
            since: Instant::now(),
            process_since: None,
            game_up: false,
            table_up: false,
            exiting_since: None,
            others_paused: false,
            kept_video,
            logos_released: false,
            last_render: None,
            stop_pressed: None,
        });
    }

    /// Restarts (`/r`) or shuts down (`/s`) the cabinet and exits, or, if
    /// Windows wouldn't take the request, says so on the wheel and stays.
    pub(crate) fn power_off(&mut self, el: &ActiveEventLoop, flag: &str) -> Result<(), String> {
        match power(flag) {
            Ok(()) => {
                el.exit();
                Ok(())
            }
            Err(e) => {
                log::error!("{e:#}");
                let what = if flag == "/r" { "Couldn't restart the cabinet" } else { "Couldn't shut down the cabinet" };
                self.notice = Some((what.into(), Instant::now()));
                Err(what.into())
            }
        }
    }

    /// Tells the player on the wheel that a game didn't start (instead of a
    /// silent return to the wheel).
    pub(crate) fn launch_failed(&mut self, index: usize) {
        let title = self.lib.items.get(index).map_or("the game", |it| it.title.as_str());
        self.notice = Some((format!("Couldn't start {title}"), Instant::now()));
    }

    /// Brings the menu back after a game: screens, video, logos and (when
    /// `take_focus`) the keyboard focus, even from another program.
    pub(crate) fn return_from_game(&mut self, el: &ActiveEventLoop, take_focus: bool) {
        self.playing = None;
        // Play statistics recorded without a backup of Popper's database: say so once.
        if let Some(why) = self.stats.lock().ok().and_then(|mut s| s.take_warning()) {
            self.notice = Some((why, Instant::now()));
        }
        self.dof.suspend(false);
        self.dof_shown = None;
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
        self.lib.forget_media();
        let Some(g) = self.gfx.as_mut() else { return };
        let item = &self.lib.items[g.shown];
        for o in g.outs.iter_mut() {
            o.slide = None;
            let _ = o.player.set_pause(false);
            if o.screen.slot == Slot::PlayField {
                let _ = o.player.command_async(&["set", "vid", "auto"]);
            }
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
        self.key_repeat.clear();
        self.joy_blocked = false;
        g.last = now;
        // The phone remote asked for a restart or shutdown during the game,
        // or Windows asked the menu to close.
        if let Some((flag, _)) = self.pending_power.take() {
            let _ = self.power_off(el, flag);
        } else if self.exit_after_game {
            el.exit();
        }
    }

    /// One frame while a game is loading, running or closing: the launch /
    /// exit screen on the playfield and the game's media (or black) elsewhere.
    /// `launching` picks the playfield's text: "Launching" until the table
    /// is on the playfield, "Exiting" after.
    pub(crate) fn render_launching(&mut self, index: usize, phase: rp_launch::Phase, launching: bool, t: f32) -> Result<()> {
        let ui_rotation = self.ui_rotation();
        let Some(g) = self.gfx.as_mut() else { return Ok(()) };
        // This screen's swaps leave the swap interval as it was set last.
        g.vsync_on = None;
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
        // Once the table is on the playfield, the screen behind it already
        // reads "Exiting", so that's what shows the moment the table closes
        // (not a stale "Launching").
        let verb = if launching { "Launching" } else { "Exiting" };
        wheel::draw_launch(&mut g.canvas, lw, lh, &g.entries[index], verb, item.emu_name(), t, &g.fonts);
        g.canvas.restore();
        g.canvas.flush();
        pf.surface.swap_buffers(&g.context)?;
        Ok(())
    }
}
