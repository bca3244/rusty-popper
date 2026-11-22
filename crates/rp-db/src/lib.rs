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
    pub manufacturer: Option<String>,
    pub year: Option<i64>,
    pub players: Option<i64>,
}

/// One row of PinUPFunctions: a menu action and its keyboard / joystick binding.
#[derive(Debug, Clone)]
pub struct Function {
    pub index: i64,
    pub name: String,
    /// Windows virtual-key code, if bound.
    pub key: Option<i64>,
    /// 1-based joystick button, if bound.
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
            "SELECT GameID, EMUID, GameName, COALESCE(GameDisplay, GameName), GameFileName,
                    Manufact, GameYear, NumPlayers
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
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
                joy_button: opt_int(r.get(3)?).filter(|b| *b > 0),
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
