//! Housekeeping while the menu is idle: following changes to Popper's
//! database and keeping the keyboard focus.

use super::*;

impl App {
    /// Re-reads Popper's database on a worker thread (see `poll_reload`).
    pub(crate) fn start_reload(&mut self) {
        if self.reload.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let popper = self.popper_dir.clone();
        let spawned = std::thread::Builder::new().name("db-reload".into()).spawn(move || {
            let _ = tx.send(Library::load(&popper));
        });
        match spawned {
            Ok(_) => self.reload = Some(rx),
            Err(e) => log::warn!("Couldn't start the database reload: {e}"),
        }
    }

    /// Notices when Popper's database file changes (tables added in Popper's
    /// setup or VPin Studio, playlists edited) and re-reads it once it settles.
    pub(crate) fn check_db_changed(&mut self) {
        if self.last_db_check.elapsed() < DB_CHECK_EVERY {
            return;
        }
        self.last_db_check = Instant::now();
        let seen = db_mtime(&self.popper_dir);
        if seen != self.db_seen {
            self.db_seen = seen;
            self.db_dirty_since = Some(Instant::now());
        } else if self.db_dirty_since.is_some_and(|t| t.elapsed() >= DB_SETTLE) && self.menu.is_none() {
            self.db_dirty_since = None;
            self.start_reload();
        }
    }

    /// Takes a finished re-read of the database, if it differs from what's shown.
    pub(crate) fn poll_reload(&mut self) {
        let Some(rx) = &self.reload else { return };
        let result = match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(anyhow!("reload thread ended without a result")),
        };
        self.reload = None;
        // A read that got no button setup (the table was locked or mid-change)
        // must not leave the cabinet without buttons: keep the one we have.
        let result = result.map(|mut lib| {
            if lib.functions.is_empty() {
                lib.functions.clone_from(&self.lib.functions);
            }
            lib
        });
        match result {
            Ok(lib) if lib != self.lib => self.apply_library(lib),
            Ok(_) => {}
            // Keep showing what we have (the file may be mid-change; the next
            // change to it triggers another try).
            Err(e) => log::warn!("Couldn't re-read Popper's database: {e:#}"),
        }
    }

    /// Swaps in a freshly read library, staying on the same playlist and game
    /// where they still exist.
    pub(crate) fn apply_library(&mut self, lib: Library) {
        let current = self.gfx.as_ref().and_then(|g| self.view.get(g.wheel.selected())).map(|&i| &self.lib.items[i]);
        let cur_game = current.and_then(Item::game_id);
        let cur_title = current.map(|it| it.title.clone());
        let playlist_name = self.lib.playlist_name(self.playlist).to_string();

        // Often only a playlist's contents changed (our own statistics write
        // reorders "Most Played"): then the logos and, where possible, the
        // wheel and the videos are left alone.
        let same_items = lib.items == self.lib.items;
        self.lib = lib;
        log::info!("Popper's database changed: now {} games, {} playlists", self.lib.game_count, self.lib.playlists.len());
        self.bindings = input::Bindings::new(&self.lib.functions, &self.joy_overrides, self.keyboard_quit);
        self.playlist = self.lib.playlist_named(&playlist_name);
        if !self.lib.has_games(self.playlist) {
            self.playlist = 0;
        }
        let view = self.lib.view(self.playlist).to_vec();
        let same_view = same_items && view == self.view;
        self.view = view;
        let same = |it: &Item| match cur_game {
            Some(id) => it.game_id() == Some(id),
            None => it.is_playlist() && Some(&it.title) == cur_title.as_ref(),
        };
        let pos = self.view.iter().position(|&i| same(&self.lib.items[i])).unwrap_or(0);
        let Some(g) = self.gfx.as_mut() else { return };
        if !same_items {
            g.release_logos(None);
            g.entries = self.lib.items.iter().map(Item::entry).collect();
        }
        if !same_view {
            g.wheel = wheel::Wheel::new(self.view.len().max(1), pos);
        }
        let item = self.view.get(pos).copied().unwrap_or(0);
        if !same_items || (!same_view && item != g.shown) {
            g.shown = item;
            g.swapped_at = (Instant::now() - g.t0).as_secs_f64();
            for o in g.outs.iter_mut() {
                o.slide = None;
                o.show(self.lib.items[item].media(o.screen.slot).as_deref());
            }
        }
        if !same_items {
            g.load_logos(&self.lib.items, item);
        }
    }

    /// Keeps the keyboard focus on the menu: right after a game it is taken
    /// back from whatever has it; at other times only when nothing else is in
    /// use (the desktop or taskbar got it). Returns whether joystick buttons
    /// should be listened to.
    pub(crate) fn watch_focus(&mut self) -> bool {
        let rects: Vec<(i32, i32, i32, i32)> = self.screens.iter().map(|s| (s.x, s.y, s.width, s.height)).collect();
        let fg = win::foreground(&rects);
        let now = Instant::now();
        if fg == win::Foreground::Ours {
            self.unfocused_since = None;
            return true;
        }
        let since = *self.unfocused_since.get_or_insert(now);
        let reclaim = now < self.reclaim_until || (fg == win::Foreground::Shell && now - since >= Duration::from_secs(1) && now >= self.quiet_until);
        if reclaim && now - self.last_reclaim >= Duration::from_millis(500) {
            self.last_reclaim = now;
            if let Some(g) = &self.gfx {
                win::force_foreground(g.pf_hwnd);
            }
        }
        // Another program covering a screen (a table started from VPin Studio,
        // a maximized tool) is being used: the cabinet buttons are meant for
        // it, not us. A small window in front (a notification, a dialog)
        // doesn't stop the buttons, or the cabinet would seem dead.
        fg != win::Foreground::Other || !self.joy_needs_focus
    }

    /// The emulators of the games on the wheel.
    fn emulators(&self) -> Vec<rp_db::Emulator> {
        let mut out: Vec<rp_db::Emulator> = Vec::new();
        for it in &self.lib.items[..self.lib.game_count] {
            if let Kind::Game { emu, .. } = &it.kind {
                if !out.iter().any(|e| e.id == emu.id) {
                    out.push((**emu).clone());
                }
            }
        }
        out
    }

    /// While a table that another program started (VPin Studio's launcher
    /// or its media recorder, say) is on screen, the menu hides its windows
    /// and pauses its videos, as if Popper's menu had been closed: recordings
    /// then show the table, not our videos. When that table closes, the menu
    /// comes back. Returns whether the menu is stepped aside.
    pub(crate) fn watch_other_launch(&mut self) -> bool {
        let now = Instant::now();
        if now - self.last_aside_check < Duration::from_millis(500) {
            return self.aside.is_some();
        }
        self.last_aside_check = now;
        if let Some(a) = &self.aside {
            if self.others.alive(a.pid) {
                return true;
            }
            let a = self.aside.take().expect("checked above");
            log::info!("{} has closed: menu back", a.name);
            if let Some(g) = self.gfx.as_mut() {
                for (o, visible) in g.outs.iter().zip(&a.visible) {
                    o.window.set_visible(*visible);
                    let _ = o.player.set_pause(false);
                }
            }
            self.last_input = now;
            self.attract_on = false;
            // Don't grab the focus straight back: VPin Studio's recorder may be
            // about to start the next table, which needs to come to the front.
            self.quiet_until = now + Duration::from_secs(10);
            // Buttons held for the table don't count for the menu.
            self.joy_blocked = true;
            return false;
        }
        let rects: Vec<(i32, i32, i32, i32)> = self.screens.iter().map(|s| (s.x, s.y, s.width, s.height)).collect();
        let (fg, pid) = win::foreground_of(&rects);
        if fg != win::Foreground::Other {
            return false;
        }
        let emulators = self.emulators();
        let Some(name) = self.others.emulator_process(pid, &emulators, &self.ignore_processes) else { return false };
        log::info!("{name} (started by another program) is on screen: menu steps aside");
        self.close_card();
        let Some(g) = self.gfx.as_mut() else { return false };
        let visible = g.outs.iter().map(|o| o.window.is_visible() != Some(false)).collect();
        for o in &g.outs {
            let _ = o.player.set_pause(true);
            o.window.set_visible(false);
        }
        self.aside = Some(Aside { pid, name, visible });
        true
    }
}