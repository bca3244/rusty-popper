//! Access to Popper's PUPDatabase.db (Games, Emulators, Playlists).
//!
//! Reading uses a temp copy opened read-only, so a running PinUpMenu or VPin
//! Studio is never locked. The only writes are play statistics, through
//! [`StatsWriter`], which backs the DB up first.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// Set for SQL playlists (PlayListType 1).
    pub sql: Option<String>,
}

/// One row of PinUPFunctions: a menu action and its keyboard / joystick binding.
#[derive(Debug, Clone)]
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

pub struct PopperDb {
    conn: Connection,
    _copy: PathBuf,
}

impl PopperDb {
    pub fn open_copy(live: &Path) -> Result<Self> {
        let copy = std::env::temp_dir().join(format!("rusty-popper-{}.db", std::process::id()));
        std::fs::copy(live, &copy)
            .with_context(|| format!("copying {} to {}", live.display(), copy.display()))?;
        let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { conn, _copy: copy })
    }

    pub fn emulators(&self) -> Result<Vec<Emulator>> {
        let mut st = self
            .conn
            .prepare("SELECT EMUID, EmuName, DirMedia, DirGames, DirRoms, EmuLaunchDir, LaunchScript, PostScript, KeepDisplays FROM Emulators WHERE Visible = 1 ORDER BY EMUID")?;
        let rows = st.query_map([], |r| {
            Ok(Emulator {
                id: r.get(0)?,
                name: r.get(1)?,
                media_dir: opt_text(r.get(2)?).map(PathBuf::from),
                games_dir: opt_text(r.get(3)?).map(PathBuf::from),
                roms_dir: opt_text(r.get(4)?).map(PathBuf::from),
                launch_dir: opt_text(r.get(5)?).map(PathBuf::from),
                launch_script: opt_text(r.get(6)?).unwrap_or_default(),
                post_script: opt_text(r.get(7)?).unwrap_or_default(),
                keep_displays: opt_text(r.get(8)?)
                    .map(|s| s.split(',').filter_map(|n| n.trim().parse().ok()).collect())
                    .unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn games(&self) -> Result<Vec<Game>> {
        let mut st = self.conn.prepare(
            "SELECT GameID, EMUID, GameName, COALESCE(GameDisplay, GameName), GameFileName,
                    Manufact, GameYear, NumPlayers, ROM, ALTEXE, LaunchCustomVar, CUSTOM2, CUSTOM3, AltRunMode, TAGS
             FROM Games WHERE Visible = 1 ORDER BY COALESCE(GameDisplay, GameName) COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Game {
                id: r.get(0)?,
                emu_id: r.get(1)?,
                name: r.get(2)?,
                display: r.get(3)?,
                file_name: r.get(4)?,
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
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Visible playlists in Popper's display order.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut st = self.conn.prepare(
            "SELECT PlayListID, PlayName, PlayListType, PlayListSQL FROM PlayLists
             WHERE Visible = 1 ORDER BY DisplayOrder, PlayListID",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Playlist {
                id: r.get(0)?,
                name: opt_text(r.get(1)?).unwrap_or_else(|| "Playlist".into()),
                sql: if opt_int(r.get(2)?) == Some(1) { opt_text(r.get(3)?) } else { None },
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
                let mut st = self.conn.prepare(sql).with_context(|| format!("playlist {:?} query", pl.name))?;
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
                let mut st = self.conn.prepare(
                    "SELECT GameID FROM PlayListDetails WHERE PlayListID = ?1 AND COALESCE(Visible, 1) = 1
                     ORDER BY DisplayOrder, UniqueID",
                )?;
                let rows = st.query_map([pl.id], |r| r.get::<_, i64>(0))?;
                for id in rows {
                    ids.push(id?);
                }
            }
        }
        Ok(ids)
    }

    /// Popper's menu rotation (GlobalSettings.ThumbRotate, degrees clockwise):
    /// how the wheel is turned to face the player on a rotated playfield.
    pub fn menu_rotation(&self) -> Result<i32> {
        let v: rusqlite::types::Value = self.conn.query_row("SELECT ThumbRotate FROM GlobalSettings LIMIT 1", [], |r| r.get(0))?;
        Ok(opt_int(v).unwrap_or(0) as i32)
    }

    pub fn functions(&self) -> Result<Vec<Function>> {
        let mut st = self.conn.prepare(
            "SELECT fIndex, Descript, CntrlCodes, JoyCodes, Active FROM PinUPFunctions ORDER BY fIndex",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Function {
                index: r.get(0)?,
                name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                key: opt_int(r.get(2)?).filter(|k| *k > 0),
                // Popper stores 0-based buttons, with 99 standing for button 0 and -1 for none.
                joy_button: opt_int(r.get(3)?).filter(|b| *b >= 0).map(|b| if b == 99 { 0 } else { b }),
                active: opt_int(r.get(4)?).unwrap_or(0) != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

impl Drop for PopperDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self._copy);
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
}