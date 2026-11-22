//! Access to Popper's PUPDatabase.db (Games, Emulators, Playlists).
//!
//! Reading uses a temp copy opened read-only, so a running PinUpMenu or VPin
//! Studio is never locked. The only writes are play statistics, through
//! [`StatsWriter`], which backs the DB up first.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Emulator {
    pub id: i64,
    pub name: String,
    pub media_dir: Option<PathBuf>,
    /// Folder holding the table files (DirGames).
    pub games_dir: Option<PathBuf>,
    pub roms_dir: Option<PathBuf>,
    /// The emulator's own folder ([DIREMU] in scripts).
    pub launch_dir: Option<PathBuf>,
    pub launch_script: String,
    pub post_script: String,
    /// Screen numbers (Popper slots) that keep showing media while a game runs.
    pub keep_displays: Vec<u32>,
    /// Popper's "process name" for the emulator, when the Emulators table has
    /// that column and it is filled in. An extra way to spot the game process.
    pub process_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub id: i64,
    pub emu_id: i64,
    pub name: String,
    pub display: String,
    pub file_name: String,
    pub manufacturer: Option<String>,
    pub year: Option<i64>,
    pub players: Option<i64>,
    pub rom: Option<String>,
    pub alt_exe: Option<String>,
    /// LaunchCustomVar ([CUSTOM1] in scripts), CUSTOM2, CUSTOM3.
    pub custom: [Option<String>; 3],
    pub alt_mode: Option<String>,
    /// Popper's TAGS, split on commas (e.g. VR, nFozzy, Fleep).
    pub tags: Vec<String>,
}

/// A Popper playlist: either a SQL query over Games or a manual list.
#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// Set for SQL playlists (PlayListType 1).
    pub sql: Option<String>,
    /// Media name (PlayLists.Logo) under POPMedia\Default, e.g. `pl_Music`.
    pub logo: Option<String>,
}

/// One row of PinUPFunctions: a menu action and its keyboard / joystick binding.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub index: i64,
    pub name: String,
    /// Windows virtual-key code, if bound.
    pub key: Option<i64>,
    /// 0-based joystick button (Popper numbering), if bound.
    pub joy_button: Option<i64>,
    pub active: bool,
}

/// SQLite columns are loosely typed; Popper stores some numbers as text.
fn opt_int(v: rusqlite::types::Value) -> Option<i64> {
    use rusqlite::types::Value::*;
    match v {
        Integer(i) => Some(i),
        Real(f) => Some(f as i64),
        Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn opt_text(v: rusqlite::types::Value) -> Option<String> {
    use rusqlite::types::Value::*;
    match v {
        Text(s) if !s.trim().is_empty() => Some(s),
        Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Collects the usable rows of a query, logging how many were skipped.
fn keep_good<T>(rows: impl Iterator<Item = rusqlite::Result<Option<T>>>, table: &str) -> rusqlite::Result<Vec<T>> {
    let mut out = Vec::new();
    let mut skipped = 0;
    for row in rows {
        match row? {
            Some(v) => out.push(v),
            None => skipped += 1,
        }
    }
    if skipped > 0 {
        log::warn!("{table}: skipped {skipped} row(s) with missing required values");
    }
    Ok(out)
}

pub struct PopperDb {
    /// `None` only while dropping (the connection must close before the copy
    /// can be deleted on Windows).
    conn: Option<Connection>,
    copy: PathBuf,
}

/// Distinguishes the temp copies one process makes (it re-reads the DB when
/// Popper's tools change it).
static COPY_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Prefix of the temp files this crate creates, for cleaning up stale ones.
pub const TEMP_PREFIX: &str = "rusty-popper-";

/// Consistent snapshot of the live DB at `dest` (VACUUM INTO waits for any
/// writer instead of copying a half-written file).
fn snapshot(live: &Path, dest: &Path) -> Result<()> {
    let _ = std::fs::remove_file(dest);
    let conn = Connection::open_with_flags(live, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", live.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy()])
        .with_context(|| format!("snapshotting {} to {}", live.display(), dest.display()))?;
    Ok(())
}

impl PopperDb {
    pub fn open_copy(live: &Path) -> Result<Self> {
        let seq = COPY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let copy = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{seq}.db", std::process::id()));
        if let Err(e) = snapshot(live, &copy) {
            // Fall back to a plain file copy (e.g. a DB SQLite refuses to vacuum).
            log::warn!("{e:#}; falling back to a file copy");
            let _ = std::fs::remove_file(&copy);
            std::fs::copy(live, &copy)
                .with_context(|| format!("copying {} to {}", live.display(), copy.display()))?;
        }
        let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { conn: Some(conn), copy })
    }

    fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("connection open until drop")
    }

    fn has_column(&self, table: &str, column: &str) -> bool {
        let found = || -> rusqlite::Result<bool> {
            let mut st = self.conn().prepare(&format!("PRAGMA table_info({table})"))?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                if r.get::<_, String>(1)?.eq_ignore_ascii_case(column) {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        found().unwrap_or(false)
    }

    pub fn emulators(&self) -> Result<Vec<Emulator>> {
        // ProcessName is not in every Popper version's schema.
        let process = if self.has_column("Emulators", "ProcessName") { "ProcessName" } else { "NULL" };
        let mut st = self.conn().prepare(&format!(
            "SELECT EMUID, EmuName, DirMedia, DirGames, DirRoms, EmuLaunchDir, LaunchScript, PostScript, KeepDisplays, {process}
             FROM Emulators WHERE Visible = 1 ORDER BY EMUID"
        ))?;
        let rows = st.query_map([], |r| {
            let Some(id) = opt_int(r.get(0)?) else { return Ok(None) };
            Ok(Some(Emulator {
                id,
                name: opt_text(r.get(1)?).unwrap_or_else(|| format!("Emulator {id}")),
                media_dir: opt_text(r.get(2)?).map(PathBuf::from),
                games_dir: opt_text(r.get(3)?).map(PathBuf::from),
                roms_dir: opt_text(r.get(4)?).map(PathBuf::from),
                launch_dir: opt_text(r.get(5)?).map(PathBuf::from),
                launch_script: opt_text(r.get(6)?).unwrap_or_default(),
                post_script: opt_text(r.get(7)?).unwrap_or_default(),
                keep_displays: opt_text(r.get(8)?)
                    .map(|s| s.split(',').filter_map(|n| n.trim().parse().ok()).collect())
                    .unwrap_or_default(),
                process_name: opt_text(r.get(9)?).map(|s| s.trim().to_string()),
            }))
        })?;
        Ok(keep_good(rows, "Emulators")?)
    }

    pub fn games(&self) -> Result<Vec<Game>> {
        let mut st = self.conn().prepare(
            "SELECT GameID, EMUID, GameName, COALESCE(NULLIF(TRIM(GameDisplay), ''), GameName), GameFileName,
                    Manufact, GameYear, NumPlayers, ROM, ALTEXE, LaunchCustomVar, CUSTOM2, CUSTOM3, AltRunMode, TAGS
             FROM Games WHERE Visible = 1
             ORDER BY COALESCE(NULLIF(TRIM(GameDisplay), ''), GameName) COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            // A row without an id, emulator, name or file can't be shown or
            // launched: skip it rather than fail the whole query.
            let (Some(id), Some(emu_id), Some(name), Some(file_name)) =
                (opt_int(r.get(0)?), opt_int(r.get(1)?), opt_text(r.get(2)?), opt_text(r.get(4)?))
            else {
                return Ok(None);
            };
            Ok(Some(Game {
                id,
                emu_id,
                display: opt_text(r.get(3)?).unwrap_or_else(|| name.clone()),
                name,
                file_name,
                manufacturer: opt_text(r.get(5)?),
                year: opt_int(r.get(6)?),
                players: opt_int(r.get(7)?),
                rom: opt_text(r.get(8)?),
                alt_exe: opt_text(r.get(9)?),
                custom: [opt_text(r.get(10)?), opt_text(r.get(11)?), opt_text(r.get(12)?)],
                alt_mode: opt_text(r.get(13)?),
                tags: opt_text(r.get(14)?)
                    .map(|t| t.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
                    .unwrap_or_default(),
            }))
        })?;
        Ok(keep_good(rows, "Games")?)
    }

    /// Visible playlists in Popper's display order.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut st = self.conn().prepare(
            "SELECT PlayListID, PlayName, PlayListType, PlayListSQL, Logo FROM PlayLists
             WHERE Visible = 1
               -- Skip Popper's root 'Home' list (PlayListParent -1): it is the menu
               -- container in Popper, and 'All games' covers going back.
               AND COALESCE(PlayListParent, 0) <> -1
             ORDER BY DisplayOrder, PlayListID",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Playlist {
                id: r.get(0)?,
                name: opt_text(r.get(1)?).unwrap_or_else(|| "Playlist".into()),
                sql: if opt_int(r.get(2)?) == Some(1) { opt_text(r.get(3)?) } else { None },
                logo: opt_text(r.get(4)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// GameIDs in a playlist, in the playlist's order. SQL playlists run their
    /// query (on the read-only copy); manual ones use PlayListDetails.
    pub fn playlist_games(&self, pl: &Playlist) -> Result<Vec<i64>> {
        let mut ids = Vec::new();
        match &pl.sql {
            Some(sql) => {
                let mut st = self.conn().prepare(sql).with_context(|| format!("playlist {:?} query", pl.name))?;
                let col = (0..st.column_count())
                    .find(|&i| st.column_name(i).is_ok_and(|n| n.eq_ignore_ascii_case("GameID")))
                    .ok_or_else(|| anyhow::anyhow!("playlist {:?} query has no GameID column", pl.name))?;
                let mut rows = st.query([])?;
                while let Some(r) = rows.next()? {
                    if let Some(id) = opt_int(r.get(col)?) {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
            }
            None => {
                let mut st = self.conn().prepare(
                    "SELECT GameID FROM PlayListDetails WHERE PlayListID = ?1 AND COALESCE(Visible, 1) = 1
                     ORDER BY DisplayOrder, UniqueID",
                )?;
                let rows = st.query_map([pl.id], |r| Ok(opt_int(r.get(0)?)))?;
                for id in rows {
                    ids.extend(id?);
                }
            }
        }
        Ok(ids)
    }

    /// Popper's menu rotation (GlobalSettings.ThumbRotate, degrees clockwise):
    /// how the wheel is turned to face the player on a rotated playfield.
    pub fn menu_rotation(&self) -> Result<i32> {
        let v: rusqlite::types::Value = self.conn().query_row("SELECT ThumbRotate FROM GlobalSettings LIMIT 1", [], |r| r.get(0))?;
        Ok(opt_int(v).unwrap_or(0) as i32)
    }

    pub fn functions(&self) -> Result<Vec<Function>> {
        let mut st = self.conn().prepare(
            "SELECT fIndex, Descript, CntrlCodes, JoyCodes, Active FROM PinUPFunctions ORDER BY fIndex",
        )?;
        let rows = st.query_map([], |r| {
            let Some(index) = opt_int(r.get(0)?) else { return Ok(None) };
            Ok(Some(Function {
                index,
                name: opt_text(r.get(1)?).unwrap_or_default(),
                key: opt_int(r.get(2)?).filter(|k| *k > 0),
                // Popper stores 0-based buttons, with 99 standing for button 0 and -1 for none.
                joy_button: opt_int(r.get(3)?).filter(|b| *b >= 0).map(|b| if b == 99 { 0 } else { b }),
                active: opt_int(r.get(4)?).unwrap_or(0) != 0,
            }))
        })?;
        Ok(keep_good(rows, "PinUPFunctions")?)
    }
}

impl Drop for PopperDb {
    fn drop(&mut self) {
        // Close first: Windows won't delete a file that is still open.
        self.conn.take();
        let _ = std::fs::remove_file(&self.copy);
    }
}

/// Writes to the live PUPDatabase.db. Only play statistics are touched, in
/// the same format Popper uses, and a backup is taken before the first write.
pub struct StatsWriter {
    live: PathBuf,
    backup_dir: PathBuf,
    backed_up: bool,
}

impl StatsWriter {
    pub fn new(live: &Path, backup_dir: &Path) -> Self {
        StatsWriter { live: live.to_path_buf(), backup_dir: backup_dir.to_path_buf(), backed_up: false }
    }

    /// Consistent copy of the live DB (VACUUM INTO), keeping the newest 10.
    pub fn backup(&mut self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.backup_dir)?;
        let name = format!("PUPDatabase-{}.db", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let dest = self.backup_dir.join(name);
        let conn = Connection::open_with_flags(&self.live, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute("VACUUM INTO ?1", [dest.to_string_lossy()])
            .with_context(|| format!("backing up to {}", dest.display()))?;
        let mut old: Vec<PathBuf> = std::fs::read_dir(&self.backup_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("PUPDatabase-")))
            .collect();
        old.sort();
        while old.len() > 10 {
            let _ = std::fs::remove_file(old.remove(0));
        }
        self.backed_up = true;
        Ok(dest)
    }

    /// Counts one play of `game_id` lasting `secs` in GamesStats, like Popper:
    /// LastPlayed in local time ("YYYY-MM-DD HH:MM:SS.mmm"), NumberPlays + 1,
    /// TimePlayedSecs + secs.
    pub fn record_play(&mut self, game_id: i64, secs: u64) -> Result<()> {
        if !self.backed_up {
            self.backup()?;
        }
        let mut conn = Connection::open(&self.live)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        let tx = conn.transaction()?;
        let updated = tx.execute(
            "UPDATE GamesStats SET LastPlayed = ?1, NumberPlays = COALESCE(NumberPlays, 0) + 1,
                    TimePlayedSecs = COALESCE(TimePlayedSecs, 0) + ?2 WHERE GameID = ?3",
            rusqlite::params![now, secs as i64, game_id],
        )?;
        if updated == 0 {
            tx.execute(
                "INSERT INTO GamesStats (GameID, LastPlayed, NumberPlays, TimePlayedSecs) VALUES (?1, ?2, 1, ?3)",
                rusqlite::params![game_id, now, secs as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_play_updates_or_inserts_and_backs_up() {
        let dir = std::env::temp_dir().join(format!("rp-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let live = dir.join("PUPDatabase.db");
        let conn = Connection::open(&live).unwrap();
        conn.execute_batch(
            "CREATE TABLE GamesStats (UniqueID INTEGER PRIMARY KEY AUTOINCREMENT, GameID INTEGER,
                 LastPlayed DATETIME, NumberPlays INTEGER DEFAULT (0), TimePlayedSecs INTEGER, HighScores TEXT);
             INSERT INTO GamesStats (GameID, LastPlayed, NumberPlays, TimePlayedSecs) VALUES (7, '2026-01-01 00:00:00.000', 3, 100);",
        )
        .unwrap();
        drop(conn);

        let mut w = StatsWriter::new(&live, &dir.join("backups"));
        w.record_play(7, 60).unwrap();
        w.record_play(9, 30).unwrap();

        let conn = Connection::open(&live).unwrap();
        let (plays, secs, last): (i64, i64, String) = conn
            .query_row("SELECT NumberPlays, TimePlayedSecs, LastPlayed FROM GamesStats WHERE GameID = 7", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((plays, secs), (4, 160));
        assert_eq!(last.len(), "2026-11-21 21:03:41.942".len());
        let (plays9, secs9): (i64, i64) =
            conn.query_row("SELECT NumberPlays, TimePlayedSecs FROM GamesStats WHERE GameID = 9", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((plays9, secs9), (1, 30));
        // Exactly one backup, taken before the first write (so it has the original row).
        let backups: Vec<_> = std::fs::read_dir(dir.join("backups")).unwrap().collect();
        assert_eq!(backups.len(), 1);
        let b = Connection::open(backups[0].as_ref().unwrap().path()).unwrap();
        let orig: i64 = b.query_row("SELECT NumberPlays FROM GamesStats WHERE GameID = 7", [], |r| r.get(0)).unwrap();
        assert_eq!(orig, 3);
        drop((conn, b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn popper_like_db(dir: &Path) -> PathBuf {
        let live = dir.join("PUPDatabase.db");
        let conn = Connection::open(&live).unwrap();
        conn.execute_batch(
            "CREATE TABLE Emulators (EMUID INTEGER, EmuName TEXT, DirMedia TEXT, DirGames TEXT, DirRoms TEXT, EmuLaunchDir TEXT,
                 LaunchScript TEXT, PostScript TEXT, KeepDisplays TEXT, Visible INTEGER);
             INSERT INTO Emulators VALUES (1, 'Visual Pinball X', NULL, 'C:\\t', NULL, 'C:\\vpx', 'START x', '', '0,2', 1);
             CREATE TABLE Games (GameID INTEGER, EMUID INTEGER, GameName TEXT, GameDisplay TEXT, GameFileName TEXT, Manufact TEXT,
                 GameYear TEXT, NumPlayers TEXT, ROM TEXT, ALTEXE TEXT, LaunchCustomVar TEXT, CUSTOM2 TEXT, CUSTOM3 TEXT,
                 AltRunMode TEXT, TAGS TEXT, Visible INTEGER);
             INSERT INTO Games (GameID, EMUID, GameName, GameDisplay, GameFileName, GameYear, TAGS, Visible)
                 VALUES (1, 1, 'Zed', '  ', 'zed.vpx', '1992', 'VR, nFozzy', 1);
             INSERT INTO Games (GameID, EMUID, GameName, GameDisplay, GameFileName, Visible) VALUES (2, 1, 'abc', 'Abc (Maker 1990)', 'abc.vpx', 1);
             INSERT INTO Games (GameID, EMUID, GameName, GameDisplay, GameFileName, Visible) VALUES (3, 1, NULL, NULL, 'broken.vpx', 1);
             INSERT INTO Games (GameID, EMUID, GameName, GameDisplay, GameFileName, Visible) VALUES (4, 1, 'NoFile', NULL, NULL, 1);
             CREATE TABLE PlayLists (PlayListID INTEGER, PlayName TEXT, PlayListType INTEGER, PlayListSQL TEXT, Logo TEXT,
                 Visible INTEGER, PlayListParent INTEGER, DisplayOrder INTEGER);
             INSERT INTO PlayLists VALUES (1, 'Home', 0, NULL, NULL, 1, -1, 0);
             INSERT INTO PlayLists VALUES (2, 'Nineties', 1, 'SELECT * FROM Games WHERE GameYear >= 1990', 'pl_90', 1, 1, 1);
             INSERT INTO PlayLists VALUES (3, 'Manual', 0, NULL, NULL, 1, 1, 2);
             CREATE TABLE PlayListDetails (UniqueID INTEGER PRIMARY KEY, PlayListID INTEGER, GameID INTEGER, Visible INTEGER, DisplayOrder INTEGER);
             INSERT INTO PlayListDetails (PlayListID, GameID, DisplayOrder) VALUES (3, 2, 1), (3, NULL, 2), (3, 1, 3);",
        )
        .unwrap();
        live
    }

    #[test]
    fn reads_tolerate_bad_rows_and_missing_columns() {
        let dir = std::env::temp_dir().join(format!("rp-db-read-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let live = popper_like_db(&dir);

        let db = PopperDb::open_copy(&live).unwrap();
        let copy = db.copy.clone();
        let emus = db.emulators().unwrap();
        assert_eq!(emus.len(), 1);
        assert_eq!(emus[0].keep_displays, vec![0, 2]);
        assert_eq!(emus[0].process_name, None); // no ProcessName column in this schema

        // Rows 3 and 4 are skipped; a blank GameDisplay falls back to GameName.
        let games = db.games().unwrap();
        assert_eq!(games.iter().map(|g| g.display.as_str()).collect::<Vec<_>>(), ["Abc (Maker 1990)", "Zed"]);
        assert_eq!(games[1].year, Some(1992));
        assert_eq!(games[1].tags, ["VR", "nFozzy"]);

        let pls = db.playlists().unwrap();
        assert_eq!(pls.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Nineties", "Manual"]);
        assert_eq!(db.playlist_games(&pls[0]).unwrap(), vec![1]);
        assert_eq!(db.playlist_games(&pls[1]).unwrap(), vec![2, 1]);

        // The temp copy goes away with the handle.
        assert!(copy.is_file());
        drop(db);
        assert!(!copy.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
