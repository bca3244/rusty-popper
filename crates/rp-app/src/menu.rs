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
        update_label_for(&self.updater.status())
    }

    /// Labels for the Options screen, with current values.
    pub(crate) fn option_labels(&self) -> Vec<String> {
        let labels = vec![
            format!("Wheel style: {}", self.style.label()),
            "Attract mode \u{203a}".to_string(),
            "Sound \u{203a}".to_string(),
            format!("DOF: {}", if self.dof.enabled { "On" } else { "Off" }),
            match (&self.remote_error, self.remote.is_some()) {
                (Some(why), _) => format!("Phone remote: can't start ({why})"),
                // The address, for typing in when the QR code won't scan.
                (None, true) => format!("Phone remote: On \u{b7} {}", self.remote.as_ref().map_or(String::new(), |r| remote::short_url(&r.url().0).to_string())),
                (None, false) => "Phone remote: Off".to_string(),
            },
            self.update_label(),
            "Back".to_string(),
        ];
        debug_assert_eq!(labels.len(), OPTION_ROWS);
        labels
    }

    /// Labels for Options → Attract mode, with current values.
    pub(crate) fn attract_labels(&self) -> Vec<String> {
        let onoff = |b: bool| if b { "On" } else { "Off" };
        let mins = |s: u64| if s.checked_rem(60) == Some(0) { format!("{} min", s / 60) } else { format!("{s} s") };
        vec![
            format!("Attract mode: {}", onoff(self.attract.enabled)),
            format!("Starts after: {}", mins(self.attract.idle_secs)),
            format!("Time per game: {} s", self.attract.interval_secs),
            "Back".to_string(),
        ]
    }

    /// Labels for Options → Sound, with current values.
    pub(crate) fn sound_labels(&self) -> Vec<String> {
        let onoff = |b: bool| if b { "On" } else { "Off" };
        vec![
            format!("Video volume: {}%", self.volume),
            format!("Sound effects: {}", onoff(self.sounds.enabled)),
            format!("Effects volume: {}%", self.sounds.volume),
            format!("Game audio: {}", onoff(self.game_audio.enabled)),
            format!("Game audio volume: {}%", self.game_audio.volume),
            "Back".to_string(),
        ]
    }

    /// Cycles the value on Options row `row`, applies it, and saves it.
    pub(crate) fn change_option(&mut self, row: usize) {
        match row {
            STYLE_ROW => self.style = self.style.toggled(),
            DOF_ROW => {
                let on = !self.dof.enabled;
                self.dof.set_enabled(on);
                self.dof_shown = None;
            }
            REMOTE_ROW => {
                let on = !self.remote_cfg.enabled;
                self.set_remote(on, Duration::ZERO);
            }
            UPDATE_ROW => {
                match self.updater.status() {
                    updater::Status::Available(rel) => self.updater.download(rel),
                    updater::Status::Downloading { .. } | updater::Status::Staged { .. } | updater::Status::Restarting(_) | updater::Status::Checking => {}
                    _ => self.updater.check(),
                }
                return;
            }
            _ => return,
        }
        self.save_settings();
    }

    /// Cycles the value on Options → Attract mode row `row`, applies it, and saves it.
    pub(crate) fn change_attract(&mut self, row: usize) {
        match row {
            0 => self.attract.enabled = !self.attract.enabled,
            1 => self.attract.idle_secs = next_choice(ATTRACT_IDLE_CHOICES, self.attract.idle_secs),
            2 => self.attract.interval_secs = next_choice(ATTRACT_INTERVAL_CHOICES, self.attract.interval_secs),
            _ => return,
        }
        self.save_settings();
    }

    /// Cycles the value on Options → Sound row `row`, applies it, and saves it.
    pub(crate) fn change_sound(&mut self, row: usize) {
        match row {
            0 => {
                self.volume = next_volume(self.volume);
                if let Some(g) = &self.gfx {
                    let _ = g.outs[0].player.command(&["set", "volume", &self.volume.to_string()]);
                }
            }
            1 => self.sounds.enabled = !self.sounds.enabled,
            3 => self.game_audio.set_enabled(!self.game_audio.enabled),
            4 => {
                self.game_audio.set_volume(next_volume(self.game_audio.volume));
            }
            2 => {
                self.sounds.set_volume(next_volume(self.sounds.volume));
            }
            _ => return,
        }
        self.save_settings();
    }

    /// The Options settings, for the phone remote's Settings card.
    pub(crate) fn settings_view(&self) -> remote::SettingsView {
        use updater::Status::*;
        // Read once, so the label, state and button can't disagree.
        let status = self.updater.status();
        remote::SettingsView {
            style: self.style.key(),
            attract: self.attract.enabled,
            attract_idle: self.attract.idle_secs,
            attract_interval: self.attract.interval_secs,
            idle_choices: ATTRACT_IDLE_CHOICES,
            interval_choices: ATTRACT_INTERVAL_CHOICES,
            volume: self.volume,
            sfx: self.sounds.enabled,
            sfx_volume: self.sounds.volume,
            game_audio: self.game_audio.enabled,
            game_audio_volume: self.game_audio.volume,
            dof: self.dof.enabled,
            update: update_label_for(&status),
            update_state: match &status {
                Off => "idle",
                Checking => "checking",
                UpToDate => "none",
                Available(_) => "available",
                Downloading { .. } | Staged { .. } | Restarting(_) => "busy",
                Failed(_) => "failed",
            },
            update_action: match &status {
                Available(_) => "install",
                Checking | Downloading { .. } | Staged { .. } | Restarting(_) => "",
                _ => "check",
            },
        }
    }

    /// Changes one setting from the phone remote (`key` as in SettingsView),
    /// applies it and saves it, as Options does.
    pub(crate) fn set_from_remote(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bad = || format!("Can't set {key} to {value}");
        let on = || match value {
            "on" | "true" | "1" => Ok(true),
            "off" | "false" | "0" => Ok(false),
            _ => Err(bad()),
        };
        let percent = || value.parse::<i32>().ok().filter(|v| (0..=100).contains(v)).ok_or_else(bad);
        let choice = |choices: &[u64]| value.parse::<u64>().ok().filter(|v| choices.contains(v)).ok_or_else(bad);
        match key {
            "style" => self.style = WheelStyle::parse(value).ok_or_else(bad)?,
            "attract" => self.attract.enabled = on()?,
            "attract_idle" => self.attract.idle_secs = choice(ATTRACT_IDLE_CHOICES)?,
            "attract_interval" => self.attract.interval_secs = choice(ATTRACT_INTERVAL_CHOICES)?,
            "volume" => {
                self.volume = percent()?;
                if let Some(g) = &self.gfx {
                    let _ = g.outs[0].player.command(&["set", "volume", &self.volume.to_string()]);
                }
            }
            "sfx" => self.sounds.enabled = on()?,
            "sfx_volume" => self.sounds.set_volume(percent()?),
            "game_audio" => self.game_audio.set_enabled(on()?),
            "game_audio_volume" => self.game_audio.set_volume(percent()?),
            "dof" => {
                self.dof.set_enabled(on()?);
                self.dof_shown = None;
            }
            // Installs only when the menu is up (never mid-game), as from Options.
            "update" => {
                match (value, self.updater.status()) {
                    ("install", updater::Status::Available(rel)) => self.updater.download(rel),
                    ("check", updater::Status::Checking | updater::Status::Downloading { .. } | updater::Status::Staged { .. } | updater::Status::Restarting(_)) => {
                        return Err("An update is already in progress".into());
                    }
                    ("check", _) => self.updater.check(),
                    _ => return Err("No update to install".into()),
                }
                return Ok(());
            }
            _ => return Err(format!("Unknown setting {key}")),
        }
        self.save_settings();
        Ok(())
    }

    fn save_settings(&self) {
        let v = settings::Editable {
            style: self.style,
            attract_enabled: self.attract.enabled,
            attract_idle_secs: self.attract.idle_secs,
            attract_interval_secs: self.attract.interval_secs,
            volume: self.volume,
            sound_enabled: self.sounds.enabled,
            sound_volume: self.sounds.volume,
            game_audio: self.game_audio.enabled,
            game_audio_volume: self.game_audio.volume,
            dof: self.dof.enabled,
            remote: self.remote_cfg.enabled,
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
        // Leaving attract mode: a browsing button (flippers, magna-saves)
        // moves the wheel on from the game it's showing straight away.
        // Anything else (Start, Exit, Info...) only wakes the menu, so
        // a stray press never launches a game nobody picked.
        if self.attract_on {
            self.attract_on = false;
            if !matches!(action, Prev | Next | PagePrev | PageNext | LetterPrev | LetterNext) {
                return;
            }
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
                MenuKind::Attract => ATTRACT_ROWS,
                MenuKind::Sound => SOUND_ROWS,
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
                    match (kind, choice) {
                        (MenuKind::Options, ATTRACT_ROW) => {
                            self.menu = Some(ExitMenu { kind: MenuKind::Attract, selected: 0, opened: Instant::now() });
                            return;
                        }
                        (MenuKind::Attract, c) if c < ATTRACT_ROWS - 1 => {
                            self.change_attract(c);
                            return;
                        }
                        (MenuKind::Attract, _) => {
                            self.menu = Some(ExitMenu { kind: MenuKind::Options, selected: ATTRACT_ROW, opened: Instant::now() });
                            return;
                        }
                        (MenuKind::Options, SOUND_ROW) => {
                            self.menu = Some(ExitMenu { kind: MenuKind::Sound, selected: 0, opened: Instant::now() });
                            return;
                        }
                        (MenuKind::Options, c) if c < OPTION_ROWS - 1 => {
                            self.change_option(c);
                            return;
                        }
                        (MenuKind::Sound, c) if c < SOUND_ROWS - 1 => {
                            self.change_sound(c);
                            return;
                        }
                        // Back from Sound returns to Options, on the Sound row.
                        (MenuKind::Sound, _) => {
                            self.menu = Some(ExitMenu { kind: MenuKind::Options, selected: SOUND_ROW, opened: Instant::now() });
                            return;
                        }
                        _ => {}
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
                self.dof.pulse(dof::PRIOR);
                g.wheel.step(-1);
                self.sounds.play(sfx::Sfx::Move(-1));
            }
            Next => {
                self.dof.pulse(dof::NEXT);
                g.wheel.step(1);
                self.sounds.play(sfx::Sfx::Move(1));
            }
            // Popper's Page Prior/Next (usually the magna-saves) jump by letter.
            PagePrev | LetterPrev => {
                self.dof.pulse(dof::PAGE_PRIOR);
                self.jump_letter(-1);
                self.sounds.play(sfx::Sfx::Jump);
            }
            PageNext | LetterNext => {
                self.dof.pulse(dof::PAGE_NEXT);
                self.jump_letter(1);
                self.sounds.play(sfx::Sfx::Jump);
            }
            // Saved like the Options row, so a later save of any other
            // setting (here or from the phone) doesn't save it by surprise.
            ToggleStyle => {
                self.style = self.style.toggled();
                self.save_settings();
            }
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

/// The Options row for updates, describing where `status` stands.
fn update_label_for(status: &updater::Status) -> String {
    use updater::Status::*;
    let current = env!("CARGO_PKG_VERSION");
    match status {
        Off => format!("Check for updates (v{current})"),
        Checking => "Checking for updates…".into(),
        UpToDate => format!("No new updates (v{current})"),
        Available(r) => format!("Install update v{}", r.version),
        Downloading { version, percent } => format!("Downloading v{version}… {percent}%"),
        Staged { version, .. } => format!("Installing v{version}…"),
        Restarting(version) => format!("Restarting into v{version}…"),
        Failed(why) => format!("Update failed: {}", why.chars().take(40).collect::<String>()),
    }
}

/// The next step of a volume that Options cycles in 10s (100 wraps to 0).
fn next_volume(v: i32) -> i32 {
    if v >= 100 {
        0
    } else {
        (v / 10 + 1) * 10
    }
}
