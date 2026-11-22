//! Read-only access to Popper's PUPDatabase.db (Games, Emulators, Playlists).
//!
//! The live DB is copied to a temp file first and the copy is opened read-only,
//! so a running PinUpMenu/VPin-Studio is never locked or modified.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Emulator {
    pub id: i64,
    pub name: String,
    pub media_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: i64,
    pub emu_id: i64,
    pub name: String,
    pub display: String,
    pub file_name: String,
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
            .prepare("SELECT EMUID, EmuName, DirMedia FROM Emulators WHERE Visible = 1 ORDER BY EMUID")?;
        let rows = st.query_map([], |r| {
            Ok(Emulator {
                id: r.get(0)?,
                name: r.get(1)?,
                media_dir: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn games(&self) -> Result<Vec<Game>> {
        let mut st = self.conn.prepare(
            "SELECT GameID, EMUID, GameName, COALESCE(GameDisplay, GameName), GameFileName
             FROM Games WHERE Visible = 1 ORDER BY COALESCE(GameDisplay, GameName) COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Game {
                id: r.get(0)?,
                emu_id: r.get(1)?,
                name: r.get(2)?,
                display: r.get(3)?,
                file_name: r.get(4)?,
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
