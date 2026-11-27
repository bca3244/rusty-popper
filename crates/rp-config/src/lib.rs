//! Parses PinUpPlayer.ini screen layout and resolves the POPMedia tree.
//!
//! The DB `Screens` table can hold stale defaults; PinUpPlayer.ini
//! `[INFO]`..`[INFO10]` is what PinUpDisplay actually uses.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Media extensions Popper accepts, in lookup order.
pub const MEDIA_EXTS: &[&str] = &["mp4", "f4v", "mkv", "avi", "mov", "apng", "gif", "png", "jpg"];

/// Wheel logo extensions, in lookup order.
pub const WHEEL_EXTS: &[&str] = &["png", "apng"];

/// Finds the wheel logo `<emu media dir>\Wheel\<GameName>.png|apng`.
pub fn find_wheel(emu_media_dir: &Path, game_name: &str) -> Option<PathBuf> {
    find_in(emu_media_dir, "Wheel", game_name, WHEEL_EXTS)
}

/// Finds `<emu media dir>\<slot folder>\<GameName>.<ext>`.
pub fn find_media(emu_media_dir: &Path, slot: Slot, game_name: &str) -> Option<PathBuf> {
    find_in(emu_media_dir, slot.media_folder(), game_name, MEDIA_EXTS)
}

/// Finds `<emu media dir>\<folder>\<name>.<ext>` for the first of `exts` on disk.
pub fn find_in(emu_media_dir: &Path, folder: &str, name: &str, exts: &[&str]) -> Option<PathBuf> {
    let dir = emu_media_dir.join(folder);
    exts.iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|p| p.is_file())
}

/// The file names in some of an emulator's media folders, read once (one
/// directory listing each), so finding a game's media doesn't probe the disk
/// for every extension. Names match case-insensitively, as on Windows.
#[derive(Debug, Default)]
pub struct MediaIndex {
    dir: PathBuf,
    /// Lower-case folder names that were read.
    folders: Vec<String>,
    /// Lower-case `<folder>\<file name>` of every file in them.
    files: std::collections::HashSet<String>,
}

impl MediaIndex {
    /// Lists `folders` under `emu_media_dir` (a missing folder has no files).
    pub fn read(emu_media_dir: &Path, folders: &[&str]) -> MediaIndex {
        let mut files = std::collections::HashSet::new();
        for folder in folders {
            let Ok(list) = std::fs::read_dir(emu_media_dir.join(folder)) else { continue };
            let key = folder.to_lowercase();
            for entry in list.flatten() {
                // A link counts if it leads to a file, as `is_file` says.
                let file = entry.file_type().is_ok_and(|t| t.is_file() || (t.is_symlink() && entry.path().is_file()));
                if file {
                    files.insert(format!("{key}\\{}", entry.file_name().to_string_lossy().to_lowercase()));
                }
            }
        }
        MediaIndex { dir: emu_media_dir.to_path_buf(), folders: folders.iter().map(|f| f.to_lowercase()).collect(), files }
    }

    /// Like [`find_in`], from the listing (on disk for a folder not listed).
    pub fn find(&self, folder: &str, name: &str, exts: &[&str]) -> Option<PathBuf> {
        let key = folder.to_lowercase();
        if !self.folders.contains(&key) {
            return find_in(&self.dir, folder, name, exts);
        }
        let stem = format!("{key}\\{}.", name.to_lowercase());
        let ext = exts.iter().find(|ext| self.files.contains(&format!("{stem}{}", ext.to_lowercase())))?;
        Some(self.dir.join(folder).join(format!("{name}.{ext}")))
    }
}

/// Popper screen slots. The index matches the `[INFOn]` section (`[INFO]` = 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Topper = 0,
    Dmd = 1,
    BackGlass = 2,
    PlayField = 3,
    Music = 4,
    FullDmd = 5,
    GameSelect = 6,
    Loading = 7,
    Other2 = 8,
    GameInfo = 9,
    GameHelp = 10,
}

impl Slot {
    pub fn from_index(i: u32) -> Option<Slot> {
        use Slot::*;
        Some(match i {
            0 => Topper,
            1 => Dmd,
            2 => BackGlass,
            3 => PlayField,
            4 => Music,
            5 => FullDmd,
            6 => GameSelect,
            7 => Loading,
            8 => Other2,
            9 => GameInfo,
            10 => GameHelp,
            _ => return None,
        })
    }

    /// The POPMedia sub-folder holding this screen's per-game media.
    pub fn media_folder(self) -> &'static str {
        use Slot::*;
        match self {
            Topper => "Topper",
            Dmd => "DMD",
            BackGlass => "BackGlass",
            PlayField => "PlayField",
            Music => "Audio",
            FullDmd => "Menu",
            GameSelect => "GameSelect",
            Loading => "Loading",
            Other2 => "Other2",
            GameInfo => "GameInfo",
            GameHelp => "GameHelp",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Screen {
    pub slot: Slot,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Degrees clockwise: 0, 90, 180 or 270.
    pub rotation: i32,
    pub volume: i32,
}

pub fn read_screens(ini_path: &Path) -> Result<Vec<Screen>> {
    let bytes = std::fs::read(ini_path).with_context(|| format!("reading {}", ini_path.display()))?;
    Ok(screens_from_ini(&decode_ini(&bytes)))
}

/// INI files written by Windows tools may be UTF-8 (with or without a BOM),
/// UTF-16 LE with a BOM, or ANSI. Section and key names are ASCII, so a lossy
/// decode of anything else is enough.
fn decode_ini(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

fn screens_from_ini(text: &str) -> Vec<Screen> {
    let sections = parse_ini(text);

    let mut screens = Vec::new();
    for i in 0..=10u32 {
        let name = if i == 0 { "INFO".to_string() } else { format!("INFO{i}") };
        let Some(sec) = sections.get(&name) else { continue };
        let get = |k: &str| sec.get(&k.to_ascii_lowercase()).and_then(|v| v.parse::<i32>().ok());
        let (Some(x), Some(y), Some(width), Some(height)) =
            (get("ScreenXPos"), get("ScreenYPos"), get("ScreenWidth"), get("ScreenHeight"))
        else {
            continue;
        };
        screens.push(Screen {
            slot: Slot::from_index(i).unwrap(),
            x,
            y,
            width,
            height,
            rotation: rotation_degrees(get("ScreenRotation").unwrap_or(0)),
            volume: get("volume").unwrap_or(100),
        });
    }
    screens
}

/// PinUpPlayer.ini stores rotation either as 0..3 quarter turns or as degrees.
fn rotation_degrees(v: i32) -> i32 {
    match v {
        1 => 90,
        2 => 180,
        3 => 270,
        90 | 180 | 270 => v,
        _ => 0,
    }
}

/// Minimal INI parser: section and key names are case-insensitive (keys lowercased).
fn parse_ini(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim().to_ascii_uppercase();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.entry(current.clone())
                .or_default()
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INI: &str = "[INFO]\r\nScreenXPos=0\r\nScreenYPos=0\r\nScreenWidth=1920\r\nScreenHeight=1080\r\n\r\n; comment\r\n[info3]\r\nscreenxpos = -1080\r\nSCREENYPOS=0\r\nScreenWidth=1080\r\nScreenHeight=1920\r\nScreenRotation=3\r\nvolume=40\r\n[INFO5]\r\nScreenXPos=x\r\n";

    fn check(screens: &[Screen]) {
        assert_eq!(screens.len(), 2); // INFO5 has no usable rectangle
        assert_eq!(screens[0].slot, Slot::Topper);
        assert_eq!((screens[0].rotation, screens[0].volume), (0, 100));
        let pf = &screens[1];
        assert_eq!(pf.slot, Slot::PlayField);
        assert_eq!((pf.x, pf.width, pf.height, pf.rotation, pf.volume), (-1080, 1080, 1920, 270, 40));
    }

    #[test]
    fn parses_screens_case_insensitively() {
        check(&screens_from_ini(INI));
    }

    #[test]
    fn decodes_bom_utf16_and_ansi() {
        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend(INI.as_bytes());
        check(&screens_from_ini(&decode_ini(&utf8_bom)));

        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(INI.encode_utf16().flat_map(u16::to_le_bytes));
        check(&screens_from_ini(&decode_ini(&utf16)));

        // A stray ANSI byte (e.g. an accented comment) must not fail the read.
        let mut ansi = b"; caf\xE9\r\n".to_vec();
        ansi.extend(INI.as_bytes());
        check(&screens_from_ini(&decode_ini(&ansi)));
    }

    #[test]
    fn media_index_finds_what_the_disk_probe_finds() {
        let dir = std::env::temp_dir().join(format!("rp-config-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for f in ["PlayField", "Wheel", "Topper", "Topper/Sub.mp4"] {
            std::fs::create_dir_all(dir.join(f)).unwrap();
        }
        // Both formats there: the earlier one in MEDIA_EXTS wins, as on disk.
        for f in ["PlayField/Attack (1980).png", "PlayField/Attack (1980).mp4", "PlayField/v1.2 Game.MKV", "Wheel/Zed.apng", "Other2/Zed.mp4"] {
            std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let ix = MediaIndex::read(&dir, &["PlayField", "Wheel", "Topper", "BackGlass"]);
        let pf = |name: &str| ix.find("PlayField", name, MEDIA_EXTS);
        assert_eq!(pf("Attack (1980)"), Some(dir.join("PlayField").join("Attack (1980).mp4")));
        assert_eq!(pf("Attack (1980)"), find_media(&dir, Slot::PlayField, "Attack (1980)"));
        // Any case, like Windows (the path is spelled as asked, as the probe does).
        assert_eq!(pf("V1.2 game"), Some(dir.join("PlayField").join("V1.2 game.mkv")));
        assert_eq!(pf("Attack"), None);
        assert_eq!(ix.find("Wheel", "Zed", WHEEL_EXTS), Some(dir.join("Wheel").join("Zed.apng")));
        // A folder is not a file; a folder that isn't there has nothing.
        assert_eq!(ix.find("Topper", "Sub", MEDIA_EXTS), None);
        assert_eq!(ix.find("BackGlass", "Zed", MEDIA_EXTS), None);
        // A folder that wasn't listed is looked up on disk.
        assert_eq!(ix.find("Other2", "Zed", MEDIA_EXTS), Some(dir.join("Other2").join("Zed.mp4")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_accepts_quarter_turns_and_degrees() {
        assert_eq!([0, 1, 2, 3, 90, 180, 270, 45].map(rotation_degrees), [0, 90, 180, 270, 90, 180, 270, 0]);
    }
}
