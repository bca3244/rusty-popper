//! Everything read from Popper's database, ready for the wheel: the games
//! with their media folders, the playlists, and each playlist's wheel order.
//!
//! A `Library` is a snapshot. It is loaded at startup and again whenever the
//! database file changes (new tables, edited playlists), on a worker thread.

use crate::wheel;
use anyhow::{bail, Result};
use rp_config::{self as config, Slot};
use rp_db::{self as db, Emulator, Game};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The media folders the menu looks in, listed once per emulator when the
/// database is read: its screens, the info / help cards, game audio, wheels.
const INDEXED: &[&str] = &["Topper", "DMD", "BackGlass", "PlayField", "Menu", "GameInfo", "GameHelp", "Audio", "Wheel"];

/// What a wheel entry is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Game { game: Box<Game>, emu: Box<Emulator> },
    /// A playlist shown as a wheel: 0 = all games, n = `playlists[n - 1]`.
    Playlist(usize),
}

/// A wheel entry with its resolved media.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub title: String,
    /// File name (without extension) of this entry's media and wheel image.
    pub media_name: String,
    pub media_dir: PathBuf,
    pub wheel_png: Option<PathBuf>,
    pub kind: Kind,
    /// Where its media files are.
    pub media_files: MediaFiles,
}

/// How an item's media is found: from the listing of its media folders
/// made with the snapshot, or (once that may be out of date) on disk, each
/// lookup remembered. Never part of an item's identity: two snapshots of the
/// same database compare equal whatever is on disk.
#[derive(Default, Clone)]
pub struct MediaFiles {
    index: Option<Arc<config::MediaIndex>>,
    cache: [OnceLock<Option<PathBuf>>; 11],
}

impl PartialEq for MediaFiles {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for MediaFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaFiles")
    }
}

impl Item {
    pub fn media(&self, slot: Slot) -> Option<PathBuf> {
        if self.media_name.is_empty() {
            return None;
        }
        let cell = &self.media_files.cache[slot as usize];
        match &self.media_files.index {
            // A miss is checked on disk once (remembered): media added since
            // the listing (say by VPin Studio, with no database change)
            // still shows on the first visit.
            Some(ix) => ix.find(slot.media_folder(), &self.media_name, config::MEDIA_EXTS).or_else(|| cell.get_or_init(|| config::find_media(&self.media_dir, slot, &self.media_name)).clone()),
            None => cell.get_or_init(|| config::find_media(&self.media_dir, slot, &self.media_name)).clone(),
        }
    }

    /// The game's own audio (`Audio\<name>.mp3` and the like), if it has some.
    pub fn audio(&self) -> Option<PathBuf> {
        if self.is_playlist() || self.media_name.is_empty() {
            return None;
        }
        match &self.media_files.index {
            Some(ix) => ix.find("Audio", &self.media_name, crate::gameaudio::AUDIO_EXTS),
            None => crate::gameaudio::find(&self.media_dir, &self.media_name),
        }
    }

    pub fn is_playlist(&self) -> bool {
        matches!(self.kind, Kind::Playlist(_))
    }

    pub fn game_id(&self) -> Option<i64> {
        match &self.kind {
            Kind::Game { game, .. } => Some(game.id),
            Kind::Playlist(_) => None,
        }
    }

    /// The emulator's name (empty for a playlist entry).
    pub fn emu_name(&self) -> &str {
        match &self.kind {
            Kind::Game { emu, .. } => &emu.name,
            Kind::Playlist(_) => "",
        }
    }

    /// Screen numbers that keep showing media while this game runs.
    pub fn keep_displays(&self) -> &[u32] {
        match &self.kind {
            Kind::Game { emu, .. } => &emu.keep_displays,
            Kind::Playlist(_) => &[],
        }
    }

    pub fn entry(&self) -> wheel::Entry {
        match &self.kind {
            Kind::Playlist(_) => wheel::Entry { title: self.title.clone(), subtitle: String::new(), chips: vec!["Playlist".into()], logo: None },
            Kind::Game { game, emu } => {
                let subtitle = match (&game.manufacturer, game.year) {
                    (Some(m), Some(y)) => format!("{}  ·  {y}", m.to_uppercase()),
                    (Some(m), None) => m.to_uppercase(),
                    (None, Some(y)) => y.to_string(),
                    (None, None) => String::new(),
                };
                // Popper's tags (VR, nFozzy, Fleep...), then the emulator.
                let mut chips: Vec<String> = game.tags.clone();
                chips.push(emu.name.clone());
                wheel::Entry { title: self.title.clone(), subtitle, chips, logo: None }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Library {
    /// `items[..game_count]` are games (alphabetical); the rest are playlist entries.
    pub items: Vec<Item>,
    pub game_count: usize,
    pub playlists: Vec<db::Playlist>,
    /// `views[n]`: item indices of playlist `n` (0 = all games) in wheel order.
    pub views: Vec<Vec<usize>>,
    pub emulator_count: usize,
    pub functions: Vec<db::Function>,
    /// Popper's ThumbRotate, if it could be read.
    pub menu_rotation: Option<i32>,
}

fn find_wheel(files: &MediaFiles, media_dir: &Path, name: &str) -> Option<PathBuf> {
    match &files.index {
        Some(ix) => ix.find("Wheel", name, config::WHEEL_EXTS),
        None => config::find_wheel(media_dir, name),
    }
}

/// Wheel order for one playlist: its games, then the other playlists as wheel
/// entries, so they sit where the wheel wraps from the last game to the first.
/// Inside a playlist the "All games" (go back) wheel comes last, right before
/// the first game, so one step back from the games reaches it.
fn view_for(n: usize, games: Vec<usize>, items: &[Item], game_count: usize) -> Vec<usize> {
    let mut view = games;
    let others = |i: &usize| !matches!(items[*i].kind, Kind::Playlist(p) if p == n || p == 0);
    view.extend((game_count..items.len()).filter(others));
    if n != 0 {
        view.extend((game_count..items.len()).filter(|&i| items[i].kind == Kind::Playlist(0)));
    }
    view
}

impl Library {
    /// Forgets the media folder listings (media may have been added or
    /// removed, e.g. by a tool run while a game played): media is looked up
    /// on disk until `take_media` brings fresh ones.
    pub fn forget_media(&mut self) {
        for it in &mut self.items {
            it.media_files = MediaFiles::default();
        }
    }

    /// Takes the media folder listings of a fresh snapshot of the same database.
    pub fn take_media(&mut self, fresh: Library) {
        if fresh.items != self.items {
            return;
        }
        for (it, new) in self.items.iter_mut().zip(fresh.items) {
            it.media_files = new.media_files;
        }
    }

    /// Reads a consistent snapshot of `<popper>\PUPDatabase.db`.
    pub fn load(popper: &Path) -> Result<Library> {
        let db = db::PopperDb::open_copy(&popper.join("PUPDatabase.db"))?;
        let emus = db.emulators()?;
        let media_dir_of = |emu: &Emulator| emu.media_dir.clone().unwrap_or_else(|| popper.join("POPMedia").join(&emu.name));
        let default_media = popper.join("POPMedia").join("Default");
        // Each media folder is listed once, all emulators' at the same time.
        let mut dirs: Vec<PathBuf> = emus.iter().map(media_dir_of).chain([default_media.clone()]).collect();
        dirs.sort();
        dirs.dedup();
        let indexes: Vec<(PathBuf, Option<Arc<config::MediaIndex>>)> = std::thread::scope(|s| {
            let reading: Vec<_> = dirs
                .iter()
                .map(|d| (d, std::thread::Builder::new().name("media-list".into()).spawn_scoped(s, move || config::MediaIndex::read(d, INDEXED))))
                .collect();
            reading
                .into_iter()
                .map(|(d, h)| {
                    let ix = match h {
                        // A listing that panicked leaves the lookups to the disk.
                        Ok(h) => h.join().ok(),
                        Err(_) => Some(config::MediaIndex::read(d, INDEXED)),
                    };
                    (d.clone(), ix.map(Arc::new))
                })
                .collect()
        });
        let files = |dir: &Path| MediaFiles { index: indexes.iter().find(|(d, _)| d == dir).and_then(|(_, ix)| ix.clone()), ..MediaFiles::default() };
        let mut items: Vec<Item> = db
            .games()?
            .into_iter()
            .filter_map(|game| {
                let emu = emus.iter().find(|e| e.id == game.emu_id)?; // hidden emulators drop out
                let media_dir = media_dir_of(emu);
                let media_files = files(&media_dir);
                Some(Item {
                    title: game.display.clone(),
                    media_name: game.name.clone(),
                    wheel_png: find_wheel(&media_files, &media_dir, &game.name),
                    media_dir,
                    kind: Kind::Game { game: Box::new(game), emu: Box::new(emu.clone()) },
                    media_files,
                })
            })
            .collect();
        if items.is_empty() {
            bail!("no visible games");
        }
        let game_count = items.len();
        let playlists = db.playlists().unwrap_or_else(|e| {
            log::warn!("Couldn't read playlists: {e:#}");
            Vec::new()
        });

        // Playlists ride on the wheel too, after the games: "All games"
        // (Popper's go-back wheel) and each playlist with its Popper wheel and videos.
        let default_files = files(&default_media);
        for n in 0..=playlists.len() {
            let (title, logo) = match n {
                0 => ("All games".to_string(), "goback".to_string()),
                n => (playlists[n - 1].name.clone(), playlists[n - 1].logo.clone().unwrap_or_default()),
            };
            items.push(Item {
                title,
                wheel_png: if logo.is_empty() { None } else { find_wheel(&default_files, &default_media, &logo) },
                media_name: logo,
                media_dir: default_media.clone(),
                kind: Kind::Playlist(n),
                media_files: default_files.clone(),
            });
        }

        let by_id: std::collections::HashMap<i64, usize> = items[..game_count].iter().enumerate().filter_map(|(i, it)| Some((it.game_id()?, i))).collect();
        let mut views = vec![view_for(0, (0..game_count).collect(), &items, game_count)];
        for (i, pl) in playlists.iter().enumerate() {
            let games = match db.playlist_games(pl) {
                Ok(ids) => ids.iter().filter_map(|id| by_id.get(id).copied()).collect(),
                Err(e) => {
                    log::warn!("Playlist {:?}: {e:#}", pl.name);
                    Vec::new()
                }
            };
            views.push(view_for(i + 1, games, &items, game_count));
        }
        Ok(Library {
            items,
            game_count,
            views,
            emulator_count: emus.len(),
            functions: db.functions().unwrap_or_else(|e| {
                log::warn!("Couldn't read Popper's key and button setup: {e:#}");
                Vec::new()
            }),
            menu_rotation: db.menu_rotation().ok(),
            playlists,
        })
    }

    /// Item indices for playlist `n` (0, or anything out of range, = all games).
    pub fn view(&self, n: usize) -> &[usize] {
        self.views.get(n).unwrap_or(&self.views[0])
    }

    pub fn playlist_name(&self, n: usize) -> &str {
        match n.checked_sub(1).and_then(|i| self.playlists.get(i)) {
            Some(p) => &p.name,
            None => "All games",
        }
    }

    /// The playlist with this name (0 = all games when there is none).
    pub fn playlist_named(&self, name: &str) -> usize {
        self.playlists.iter().position(|p| p.name == name).map_or(0, |i| i + 1)
    }

    /// What differs from an earlier snapshot, in words for the log, e.g.
    /// "playlist contents changed (Most Played)" or "2 games added".
    pub fn changes_since(&self, old: &Library) -> String {
        let ids = |l: &Library| l.items[..l.game_count].iter().filter_map(Item::game_id).collect::<std::collections::HashSet<_>>();
        let (was, now) = (ids(old), ids(self));
        let (added, removed) = (now.difference(&was).count(), was.difference(&now).count());
        let games = |n: usize| if n == 1 { "1 game".to_string() } else { format!("{n} games") };
        let mut parts = Vec::new();
        if added > 0 {
            parts.push(format!("{} added", games(added)));
        }
        if removed > 0 {
            parts.push(format!("{} removed", games(removed)));
        }
        if added == 0 && removed == 0 && old.items[..old.game_count] != self.items[..self.game_count] {
            parts.push("game details changed".into());
        }
        if old.playlists != self.playlists {
            parts.push(format!("playlists edited (now {})", self.playlists.len()));
        } else if old.items == self.items {
            // Same entries, so view indices compare like for like.
            let names: Vec<&str> = (1..self.views.len()).filter(|&n| old.views.get(n) != self.views.get(n)).map(|n| self.playlist_name(n)).collect();
            if !names.is_empty() {
                parts.push(format!("playlist contents changed ({})", names.join(", ")));
            }
        }
        if old.functions != self.functions {
            parts.push("button setup changed".into());
        }
        if parts.is_empty() {
            parts.push("settings changed".into());
        }
        parts.join(", ")
    }

    /// Whether playlist `n`'s view starts with a game (i.e. it isn't empty).
    pub fn has_games(&self, n: usize) -> bool {
        self.view(n).first().is_some_and(|&i| i < self.game_count)
    }
}

/// Group a title by its first letter; digits and symbols share '#'.
pub fn letter_of(title: &str) -> char {
    match title.trim_start().chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

/// Where a jump-by-letter lands: the first entry of the next (`dir` > 0) or
/// previous letter group, given each wheel position's group letter. Going
/// back from inside a group first goes to that group's start.
pub fn letter_jump(letters: &[char], cur: usize, dir: i64) -> usize {
    let n = letters.len();
    if n == 0 || cur >= n {
        return cur;
    }
    let here = letters[cur];
    let mut pos = cur;
    if dir > 0 {
        for _ in 0..n {
            pos = (pos + 1) % n;
            if letters[pos] != here {
                break;
            }
        }
    } else {
        // Step back into the previous group (or stay in this one), then to its start.
        if letters[(cur + n - 1) % n] != here {
            pos = (pos + n - 1) % n;
        }
        let target = letters[pos];
        for _ in 0..n {
            let prev = (pos + n - 1) % n;
            if letters[prev] != target || prev == cur {
                break;
            }
            pos = prev;
        }
    }
    pos
}

/// Whether a view's games are in alphabetical order (item indices ascend,
/// since items are sorted by title). Jumping by letter only makes sense then;
/// a hand-ordered playlist pages instead.
pub fn is_alphabetical(view: &[usize], game_count: usize) -> bool {
    let games: Vec<usize> = view.iter().copied().filter(|&i| i < game_count).collect();
    games.windows(2).all(|w| w[0] < w[1])
}

/// The shortest way round a wheel of `n` entries from `from` to `to`, in steps.
pub fn shortest_steps(from: usize, to: usize, n: usize) -> i64 {
    let n = n as i64;
    let mut d = to as i64 - from as i64;
    if d > n / 2 {
        d -= n;
    } else if d < -(n / 2) {
        d += n;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_group_digits_and_symbols() {
        assert_eq!(["attack", " Zed", "24", "'Round", ""].map(letter_of), ['A', 'Z', '#', '#', '#']);
    }

    #[test]
    fn letter_jump_moves_between_group_starts() {
        //                         0    1    2    3    4    5    6
        let l: Vec<char> = "#AABBC*".chars().collect();
        assert_eq!(letter_jump(&l, 1, 1), 3); // A -> first B
        assert_eq!(letter_jump(&l, 2, 1), 3);
        assert_eq!(letter_jump(&l, 6, 1), 0); // wraps
        assert_eq!(letter_jump(&l, 4, -1), 3); // inside B -> start of B
        assert_eq!(letter_jump(&l, 3, -1), 1); // start of B -> start of A
        assert_eq!(letter_jump(&l, 0, -1), 6); // wraps back
        let one: Vec<char> = "AAA".chars().collect();
        assert_eq!(letter_jump(&one, 1, 1), 1);
        assert_eq!(letter_jump(&[], 0, 1), 0);
    }

    #[test]
    fn go_back_wheel_sits_right_before_the_games() {
        // Two games-to-be (stood in for by playlist items: view_for only looks
        // at kinds past game_count), then All games and three playlists.
        let item = |k| Item { title: String::new(), media_name: String::new(), media_dir: PathBuf::new(), wheel_png: None, kind: k, media_files: MediaFiles::default() };
        let items: Vec<Item> = [9, 9, 0, 1, 2, 3].into_iter().map(|n| item(Kind::Playlist(n))).collect();
        // In playlist 2: its games, the other playlists, then All games (wrapping to game 0).
        assert_eq!(view_for(2, vec![1, 0], &items, 2), [1, 0, 3, 5, 2]);
        // In All games: the games, then every playlist.
        assert_eq!(view_for(0, vec![0, 1], &items, 2), [0, 1, 3, 4, 5]);
    }

    #[test]
    fn change_log_names_what_changed() {
        let item = |k| Item { title: String::new(), media_name: String::new(), media_dir: PathBuf::new(), wheel_png: None, kind: k, media_files: MediaFiles::default() };
        let pl = |id: i64, name: &str| db::Playlist { id, name: name.into(), sql: None, logo: None };
        let lib = || Library {
            items: (0..3).map(|n| item(Kind::Playlist(n))).collect(),
            game_count: 0,
            playlists: vec![pl(1, "Favorites"), pl(2, "Most Played")],
            views: vec![vec![1, 2], vec![0, 2], vec![0, 1]],
            emulator_count: 1,
            functions: Vec::new(),
            menu_rotation: None,
        };
        let old = lib();
        let mut new = lib();
        new.views[2] = vec![1, 0];
        assert_eq!(new.changes_since(&old), "playlist contents changed (Most Played)");
        let mut new = lib();
        new.playlists.pop();
        assert_eq!(new.changes_since(&old), "playlists edited (now 1)");
        let mut new = lib();
        new.menu_rotation = Some(90);
        assert_eq!(new.changes_since(&old), "settings changed");
    }

    #[test]
    fn alphabetical_views_are_recognised() {
        assert!(is_alphabetical(&[0, 2, 5, 9, 8], 8)); // 9 and 8 are playlist entries
        assert!(!is_alphabetical(&[4, 1, 6, 9], 8));
        assert!(is_alphabetical(&[9], 8));
    }

    #[test]
    fn shortest_way_round() {
        assert_eq!(shortest_steps(1, 3, 10), 2);
        assert_eq!(shortest_steps(1, 9, 10), -2);
        assert_eq!(shortest_steps(9, 0, 10), 1);
        assert_eq!(shortest_steps(0, 5, 10), 5);
    }
}
