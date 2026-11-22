//! Parses PinUpPlayer.ini screen layout and resolves the POPMedia tree.
//!
//! The DB `Screens` table can hold stale defaults; PinUpPlayer.ini
//! `[INFO]`..`[INFO10]` is what PinUpDisplay actually uses.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Media extensions Popper accepts, in lookup order.
pub const MEDIA_EXTS: &[&str] = &["mp4", "f4v", "mkv", "avi", "mov", "apng", "gif", "png", "jpg"];

/// Finds the wheel logo `<emu media dir>\Wheel\<GameName>.png|apng`.
pub fn find_wheel(emu_media_dir: &Path, game_name: &str) -> Option<PathBuf> {
    ["png", "apng"]
        .iter()
        .map(|ext| emu_media_dir.join("Wheel").join(format!("{game_name}.{ext}")))
        .find(|p| p.is_file())
}

/// Finds `<emu media dir>\<slot folder>\<GameName>.<ext>`.
pub fn find_media(emu_media_dir: &Path, slot: Slot, game_name: &str) -> Option<PathBuf> {
    MEDIA_EXTS
        .iter()
        .map(|ext| emu_media_dir.join(slot.media_folder()).join(format!("{game_name}.{ext}")))
        .find(|p| p.is_file())
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
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
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
    fn rotation_accepts_quarter_turns_and_degrees() {
        assert_eq!([0, 1, 2, 3, 90, 180, 270, 45].map(rotation_degrees), [0, 90, 180, 270, 90, 180, 270, 0]);
    }
}
