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

        self.lib = lib;
        log::info!("Popper's database changed: now {} games, {} playlists", self.lib.game_count, self.lib.playlists.len());
        self.bindings = input::Bindings::new(&self.lib.functions, &self.joy_overrides, self.keyboard_quit);
        self.playlist = self.lib.playlist_named(&playlist_name);
        if !self.lib.has_games(self.playlist) {
            self.playlist = 0;
        }
        self.view = self.lib.view(self.playlist).to_vec();
        let same = |it: &Item| match cur_game {
            Some(id) => it.game_id() == Some(id),
            None => it.is_playlist() && Some(&it.title) == cur_title.as_ref(),
        };
        let pos = self.view.iter().position(|&i| same(&self.lib.items[i])).unwrap_or(0);
        let Some(g) = self.gfx.as_mut() else { return };
        g.release_logos(None);
        g.entries = self.lib.items.iter().map(Item::entry).collect();
        g.wheel = wheel::Wheel::new(self.view.len().max(1), pos);
        let item = self.view.get(pos).copied().unwrap_or(0);
        g.shown = item;
        g.swapped_at = (Instant::now() - g.t0).as_secs_f64();
        for o in g.outs.iter_mut() {
            o.slide = None;
            o.show(self.lib.items[item].media(o.screen.slot).as_deref());
        }
        g.load_logos(&self.lib.items, item);
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
        let reclaim = now < self.reclaim_until || (fg == win::Foreground::Shell && now - since >= Duration::from_secs(1));
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
}
