//! What the buttons do in the menu: browsing, playlists, letter jumps, the
//! exit and options menus, and attract mode.

use super::*;

impl App {
    /// Switches to playlist `n`, staying on the current game if it's in it.
    pub(crate) fn set_playlist(&mut self, n: usize) {
        let view = self.lib.view(n).to_vec();
        let Some(g) = self.gfx.as_mut() else { return };
        let current = self.view.get(g.wheel.selected()).copied();
        let pos = current.and_then(|c| view.iter().position(|&i| i == c)).unwrap_or(0);
        self.playlist = if n < self.lib.views.len() { n } else { 0 };
        self.view = view;
        g.wheel = wheel::Wheel::new(self.view.len().max(1), pos);
        g.wheel.info_t = 0.0;
        if let Some(&item) = self.view.get(pos) {
            if item != g.shown {
                g.shown = item;
                g.swapped_at = (Instant::now() - g.t0).as_secs_f64();
                for o in &g.outs {
                    o.show(self.lib.items[item].media(o.screen.slot).as_deref());
                }
            }
        }
        let name = self.lib.playlist_name(self.playlist).to_string();
        log::info!("Playlist: {name} ({} entries)", self.view.len());
        let _ = std::fs::write(state_path(), name);
    }

    /// Moves to the first game of the next (`dir` > 0) or previous letter
    /// group. A playlist that isn't alphabetical moves by a page instead.
    pub(crate) fn jump_letter(&mut self, dir: i64) {
        let Some(g) = self.gfx.as_mut() else { return };
        let n = self.view.len();
        if n == 0 {
            return;
        }
        let cur = g.wheel.selected();
        if !library::is_alphabetical(&self.view, self.lib.game_count) {
            g.wheel.step(dir.signum() * PAGE_STEP.min(n as i64 - 1).max(1));
            return;
        }
        // Playlist entries form one group (shown as a star) after the games.
        let letters: Vec<char> = self
            .view
            .iter()
            .map(|&i| {
                let it = &self.lib.items[i];
                if it.is_playlist() { '*' } else { library::letter_of(&it.title) }
            })
            .collect();
        let pos = library::letter_jump(&letters, cur, dir);
        g.wheel.step(library::shortest_steps(cur, pos, n));
        self.letter = Some((letters[pos], Instant::now()));
    }

    /// Shows the selected game's info or help card on the screen Popper
    /// assigns it (if that's one of ours and it's showing), else the playfield.
    pub(crate) fn open_card(&mut self, kind: overlay::CardKind) {
        let Some(g) = self.gfx.as_mut() else { return };
        let Some(&item) = self.view.get(g.wheel.selected()) else { return };
        if self.lib.items[item].is_playlist() {
            return;
        }
        let slot = if kind == overlay::CardKind::Info { Slot::GameInfo } else { Slot::GameHelp };
        let rect = if kind == overlay::CardKind::Info { &self.card_screens.0 } else { &self.card_screens.1 };
        let out = rect
            .as_ref()
            .and_then(|r| {
                let (cx, cy) = (r.x + r.width / 2, r.y + r.height / 2);
                g.outs.iter().position(|o| {
                    let s = &o.screen;
                    cx >= s.x && cx < s.x + s.width && cy >= s.y && cy < s.y + s.height && o.window.is_visible() != Some(false)
                })
            })
            .unwrap_or(0);
        // Decoding a big PNG takes a moment: do it off the UI thread so the
        // videos keep playing, and upload it when it arrives.
        let pending = self.lib.items[item].media(slot).filter(|p| !is_video(p)).map(|p| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let im = match image::open(&p) {
                    Ok(im) => Some(im.to_rgba8()),
                    Err(e) => {
                        log::warn!("Card {}: {e}", p.display());
                        None
                    }
                };
                let _ = tx.send(im);
            });
            rx
        });
        log::info!("{:?} card for {} on {:?}: {}", kind, self.lib.items[item].title, g.outs[out].screen.slot, if pending.is_some() { "loading" } else { "none" });
        self.sounds.play(sfx::Sfx::Select);
        self.close_card();
        self.card = Some(overlay::Card { kind, image: None, opened: Instant::now(), out, pending });
    }

    /// Uploads a card image once its decode has finished.
    pub(crate) fn poll_card(&mut self) {
        let Some(card) = self.card.as_mut() else { return };
        let Some(rx) = card.pending.as_ref() else { return };
        let im = match rx.try_recv() {
            Ok(im) => im,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        card.pending = None;
        let Some(g) = self.gfx.as_mut() else { return };
        card.image = im.and_then(|im| {
            use femtovg::rgb::FromSlice;
            let (w, h) = im.dimensions();
            g.context.make_current(&g.outs[0].surface).ok()?;
            let id = g
                .canvas
                .create_image(femtovg::imgref::Img::new(im.as_raw().as_rgba(), w as usize, h as usize), femtovg::ImageFlags::empty())
                .ok()?;
            Some((id, w as f32, h as f32))
        });
        // Fade in from now, and give a missing card its full time on screen.
        card.opened = Instant::now();
    }

    pub(crate) fn close_card(&mut self) {
        if let Some(card) = self.card.take() {
            if let (Some((img, _, _)), Some(g)) = (card.image, self.gfx.as_mut()) {
                g.canvas.delete_image(img);
            }
        }
    }

    /// The Options row for updates, describing where things stand.
    pub(crate) fn update_label(&self) -> String {
        use updater::Status::*;
        let current = env!("CARGO_PKG_VERSION");
        match self.updater.status() {
            Off => format!("Check for updates (v{current})"),
            Checking => "Checking for updates…".into(),
            UpToDate => format!("Up to date (v{current})"),
            Available(r) => format!("Install update v{}", r.version),
            Downloading { version, percent } => format!("Downloading v{version}… {percent}%"),
            Staged { version, .. } => format!("Installing v{version}…"),
            Restarting(version) => format!("Restarting into v{version}…"),
            Failed(why) => format!("Update failed: {}", why.chars().take(40).collect::<String>()),
        }
    }

    /// Labels for the Options screen, with current values.
    pub(crate) fn option_labels(&self) -> Vec<String> {
        let onoff = |b: bool| if b { "On" } else { "Off" };
        let mins = |s: u64| if s.checked_rem(60) == Some(0) { format!("{} min", s / 60) } else { format!("{s} s") };
        vec![
            format!("Wheel style: {}", if self.style == WheelStyle::Arc { "Arc" } else { "Cover flow" }),
            format!("Attract mode: {}", onoff(self.attract.enabled)),
            format!("Attract starts after: {}", mins(self.attract.idle_secs)),
            format!("Time per game: {} s", self.attract.interval_secs),
            format!("Video volume: {}%", self.volume),
            format!("Sound effects: {}", onoff(self.sounds.enabled)),
            format!("Effects volume: {}%", self.sounds.volume),
            self.update_label(),
            "Back".to_string(),
        ]
    }

    /// Cycles the value on Options row `row`, applies it, and saves it.
    pub(crate) fn change_option(&mut self, row: usize) {
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
            5 => self.sounds.enabled = !self.sounds.enabled,
            7 => {
                match self.updater.status() {
                    updater::Status::Available(rel) => self.updater.download(rel),
                    updater::Status::Downloading { .. } | updater::Status::Staged { .. } | updater::Status::Restarting(_) | updater::Status::Checking => {}
                    _ => self.updater.check(),
                }
                return;
            }
            6 => {
                let v = if self.sounds.volume >= 100 { 0 } else { (self.sounds.volume / 10 + 1) * 10 };
                self.sounds.set_volume(v);
            }
            _ => return,
        }
        let v = settings::Editable {
            style: self.style,
            attract_enabled: self.attract.enabled,
            attract_idle_secs: self.attract.idle_secs,
            attract_interval_secs: self.attract.interval_secs,
            volume: self.volume,
            sound_enabled: self.sounds.enabled,
            sound_volume: self.sounds.volume,
        };
        if let Err(e) = settings::save(&exe_dir().join("rusty-popper.toml"), &v) {
            log::warn!("Couldn't save settings: {e:#}");
        }
    }

    pub(crate) fn act(&mut self, el: &ActiveEventLoop, action: input::Action) {
        use input::Action::*;
        self.last_input = Instant::now();
        if std::env::var_os("RP_DEBUG_INPUT").is_some() {
            log::info!("action {action:?}");
        }
        // While a game is loading or running the menu isn't on screen, so
        // nothing may act on it. The exit key closes the game instead (it
        // only reaches us if the game never took the keyboard focus).
        if let Some(p) = &self.playing {
            if action == Menu && !p.ctl.stop_requested() {
                log::info!("Exit pressed: closing the game");
                p.ctl.request_stop();
            }
            return;
        }
        // Any button just wakes the menu from attract mode.
        if self.attract_on {
            self.attract_on = false;
            return;
        }
        if self.gfx.is_none() {
            return;
        }
        // An info / help card closes on any button.
        if self.card.is_some() {
            self.close_card();
            return;
        }
        // While a menu is open, browsing buttons move its highlight.
        if let Some(menu) = self.menu.as_mut() {
            let n = match menu.kind {
                MenuKind::Exit => EXIT_MENU.len(),
                MenuKind::Options => OPTION_ROWS,
            };
            match action {
                Prev | PagePrev | LetterPrev => {
                    menu.selected = (menu.selected + n - 1) % n;
                    self.sounds.play(sfx::Sfx::MenuMove);
                }
                Next | PageNext | LetterNext => {
                    menu.selected = (menu.selected + 1) % n;
                    self.sounds.play(sfx::Sfx::MenuMove);
                }
                Menu => self.menu = None,
                Playlists | Info | Help => {}

                Quit => el.exit(),
                Start => {
                    let (kind, choice) = (menu.kind, menu.selected);
                    self.sounds.play(sfx::Sfx::Select);
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
            Menu => {
                self.menu = Some(ExitMenu { kind: MenuKind::Exit, selected: 0, opened: Instant::now() });
                self.sounds.play(sfx::Sfx::Select);
            }
            Info => self.open_card(overlay::CardKind::Info),
            Help => self.open_card(overlay::CardKind::Help),
            // Jump to the playlist wheels (after the games).
            Playlists => {
                let cur = g.wheel.selected();
                if let Some(pos) = self.view.iter().position(|&i| i >= self.lib.game_count) {
                    g.wheel.step(pos as i64 - cur as i64);
                }
            }
            Prev => {
                g.wheel.step(-1);
                self.sounds.play(sfx::Sfx::Move(-1));
            }
            Next => {
                g.wheel.step(1);
                self.sounds.play(sfx::Sfx::Move(1));
            }
            // Popper's Page Prior/Next (usually the magna-saves) jump by letter.
            PagePrev | LetterPrev => {
                self.jump_letter(-1);
                self.sounds.play(sfx::Sfx::Jump);
            }
            PageNext | LetterNext => {
                self.jump_letter(1);
                self.sounds.play(sfx::Sfx::Jump);
            }
            ToggleStyle => self.style = self.style.toggled(),
            Start => {
                if let Some(&item) = self.view.get(g.wheel.selected()) {
                    match self.lib.items[item].kind {
                        Kind::Playlist(n) => {
                            self.sounds.play(sfx::Sfx::Select);
                            self.set_playlist(n);
                        }
                        Kind::Game { .. } => {
                            self.sounds.play(sfx::Sfx::Launch);
                            self.launch(item);
                        }
                    }
                }
            }
            Quit => el.exit(),
        }
    }

    /// Attract mode: after a while with no input, hop to a random game every
    /// few seconds until any button is pressed.
    pub(crate) fn attract_tick(&mut self) {
        if !self.attract.enabled || self.menu.is_some() || self.card.is_some() || self.playing.is_some() || self.view.len() < 2 {
            return;
        }
        let now = Instant::now();
        if !self.attract_on {
            if now - self.last_input >= Duration::from_secs(self.attract.idle_secs) {
                log::info!("Attract mode on");
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
        let Some(g) = self.gfx.as_mut() else { return };
        let cur = g.wheel.selected();
        // Only games (they come first in the view), never the playlist wheels.
        let games = self.view.iter().take_while(|&&i| i < self.lib.game_count).count();
        if games < 2 {
            return;
        }
        let mut target = (self.rng % games as u64) as usize;
        if target == cur {
            target = (target + 1) % games;
        }
        g.wheel.step(library::shortest_steps(cur, target, self.view.len()));
    }
}
