//! User settings from `rusty-popper.toml` next to the exe (all optional).
//!
//! ```toml
//! [wheel]
//! style = "arc"   # or "flow"
//!
//! [playfield]
//! ui_rotation = 270   # default: Popper's ThumbRotate
//!
//! [input]
//! # Optional: buttons are normally taken from Popper's controller setup.
//! menu_joy_buttons = []
//! letter_prev_joy_buttons = []
//! letter_next_joy_buttons = []
//! playlist_joy_buttons = []
//!
//! # Ignore cabinet buttons while another program has the focus.
//! joystick_needs_focus = true
//! # Q quits at once (handy with a keyboard; turn off if a button sends Q).
//! keyboard_quit = true
//!
//! [launch]
//! # Extra helper programs in an emulator folder that aren't the game.
//! ignore_processes = []
//!
//! [paths]
//! # popper_dir = 'C:\vPinball\PinUPSystem'
//! # mpv_dll = 'C:\somewhere\libmpv-2.dll'
//!
//! [attract]
//! enabled = true
//! idle_secs = 180
//! interval_secs = 12
//!
//! [sound]
//! enabled = true    # menu sound effects (Popper's PinUpMenu sounds)
//! volume = 30
//! # dir = 'C:\vPinball\PinUPSystem\PUPVideos\PinUpMenu\PuPSounds'
//! ```

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WheelStyle {
    #[default]
    Arc,
    Flow,
}

impl WheelStyle {
    pub fn toggled(self) -> Self {
        match self {
            WheelStyle::Arc => WheelStyle::Flow,
            WheelStyle::Flow => WheelStyle::Arc,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "arc" => Some(WheelStyle::Arc),
            "flow" => Some(WheelStyle::Flow),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WheelSettings {
    #[serde(default)]
    pub style: WheelStyle,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlayfieldSettings {
    /// Degrees clockwise to turn the menu so it faces the player.
    /// Defaults to Popper's own setting (GlobalSettings.ThumbRotate).
    pub ui_rotation: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct InputSettings {
    /// Optional extra joystick buttons (Popper numbering, 0-based) that open the exit menu, on top of
    /// Popper's System Menu / Exit bindings.
    #[serde(default)]
    pub menu_joy_buttons: Vec<u32>,
    /// Buttons that jump to the previous / next starting letter.
    #[serde(default)]
    pub letter_prev_joy_buttons: Vec<u32>,
    #[serde(default)]
    pub letter_next_joy_buttons: Vec<u32>,
    /// Buttons that open the playlist picker.
    #[serde(default)]
    pub playlist_joy_buttons: Vec<u32>,
    /// Ignore joystick buttons while another program's window has the focus
    /// (a table started from VPin Studio, a setup tool).
    #[serde(default = "yes")]
    pub joystick_needs_focus: bool,
    /// Whether Q quits at once, without the exit menu.
    #[serde(default = "yes")]
    pub keyboard_quit: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        InputSettings {
            menu_joy_buttons: Vec::new(),
            letter_prev_joy_buttons: Vec::new(),
            letter_next_joy_buttons: Vec::new(),
            playlist_joy_buttons: Vec::new(),
            joystick_needs_focus: true,
            keyboard_quit: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct LaunchSettings {
    /// Process names (like `helper.exe`) in an emulator folder that don't
    /// mean a game is running, on top of the built-in list.
    #[serde(default)]
    pub ignore_processes: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PathSettings {
    /// Popper's folder (holding PUPDatabase.db). Default: C:\vPinball\PinUPSystem.
    pub popper_dir: Option<std::path::PathBuf>,
    /// libmpv-2.dll, when it isn't next to the exe.
    pub mpv_dll: Option<std::path::PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettings {
    /// Look for a newer release on GitHub at startup.
    #[serde(default = "yes")]
    pub check: bool,
    /// GitHub repository the releases come from.
    #[serde(default = "default_repo")]
    pub repo: String,
}

fn default_repo() -> String {
    "bca3244/rusty-popper".into()
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { check: true, repo: default_repo() }
    }
}

#[derive(Debug, Deserialize)]
pub struct SoundSettings {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 0-100.
    #[serde(default = "default_sound_volume")]
    pub volume: i32,
    /// Folder with fx_*.wav. Default: Popper's PUPVideos\PinUpMenu\PuPSounds.
    pub dir: Option<std::path::PathBuf>,
    /// Play the selected game's Audio media (POPMedia\<emulator>\Audio) on the wheel.
    #[serde(default = "yes")]
    pub game_audio: bool,
    /// 0-100.
    #[serde(default = "default_game_audio_volume")]
    pub game_audio_volume: i32,
}

fn default_game_audio_volume() -> i32 {
    50
}

fn default_sound_volume() -> i32 {
    30
}

impl Default for SoundSettings {
    fn default() -> Self {
        SoundSettings { enabled: true, volume: default_sound_volume(), dir: None, game_audio: true, game_audio_volume: default_game_audio_volume() }
    }
}

#[derive(Debug, Deserialize)]
pub struct AttractSettings {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Seconds without input before attract mode starts.
    #[serde(default = "default_idle")]
    pub idle_secs: u64,
    /// Seconds between games while in attract mode.
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
}

fn yes() -> bool {
    true
}
fn default_idle() -> u64 {
    180
}
fn default_interval() -> u64 {
    12
}

impl Default for AttractSettings {
    fn default() -> Self {
        AttractSettings { enabled: true, idle_secs: default_idle(), interval_secs: default_interval() }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub wheel: WheelSettings,
    #[serde(default)]
    pub playfield: PlayfieldSettings,
    #[serde(default)]
    pub input: InputSettings,
    #[serde(default)]
    pub attract: AttractSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub launch: LaunchSettings,
    #[serde(default)]
    pub paths: PathSettings,
    #[serde(default)]
    pub sound: SoundSettings,
    #[serde(default)]
    pub update: UpdateSettings,
}

impl Settings {
    fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Reads the settings file. A missing file means defaults; so does one
    /// that can't be read or parsed (with a warning), so that a typo or a
    /// half-written file never stops the cabinet from starting.
    pub fn load(path: &Path) -> Self {
        if !path.is_file() {
            return Settings::default();
        }
        let parsed = std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| Self::parse(&t));
        parsed.unwrap_or_else(|e| {
            log::warn!("Ignoring {} (using defaults): {e:#}", path.display());
            Settings::default()
        })
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct AudioSettings {
    /// Playfield video volume, 0-100. Default: PinUpPlayer.ini's playfield volume.
    pub volume: Option<i32>,
}

/// Values the in-menu Options screen can change.
pub struct Editable {
    pub style: WheelStyle,
    pub attract_enabled: bool,
    pub attract_idle_secs: u64,
    pub attract_interval_secs: u64,
    pub volume: i32,
    pub sound_enabled: bool,
    pub sound_volume: i32,
    pub game_audio: bool,
    pub game_audio_volume: i32,
}

/// `text` (the current file contents) with the editable values set, keeping
/// everything else (comments, other keys) as it was.
fn updated(text: &str, v: &Editable) -> anyhow::Result<String> {
    use toml_edit::{value, DocumentMut, Item, Table};
    let mut doc: DocumentMut = text.parse()?;
    let mut set = |table: &str, key: &str, val: toml_edit::Value| {
        if !doc.contains_table(table) {
            doc.insert(table, Item::Table(Table::new()));
        }
        doc[table][key] = value(val);
    };
    set("wheel", "style", (if v.style == WheelStyle::Arc { "arc" } else { "flow" }).into());
    set("attract", "enabled", v.attract_enabled.into());
    set("attract", "idle_secs", (v.attract_idle_secs as i64).into());
    set("attract", "interval_secs", (v.attract_interval_secs as i64).into());
    set("audio", "volume", (v.volume as i64).into());
    set("sound", "enabled", v.sound_enabled.into());
    set("sound", "volume", (v.sound_volume as i64).into());
    set("sound", "game_audio", v.game_audio.into());
    set("sound", "game_audio_volume", (v.game_audio_volume as i64).into());
    Ok(doc.to_string())
}

/// Writes the editable values into rusty-popper.toml. The new contents go to
/// a temp file that then replaces the old one, so a power cut mid-save can't
/// leave a half-written settings file.
/// The file's text, or empty if there's no file yet. Any other read error is
/// passed on, so a file we can't read is never overwritten.
fn read_existing(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(path: &Path, v: &Editable) -> anyhow::Result<()> {
    let text = read_existing(path)?;
    let new = updated(&text, v)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Remembers where Popper was found: sets `[paths] popper_dir`, keeping the
/// rest of rusty-popper.toml as it is.
pub fn save_popper_dir(path: &Path, dir: &Path) -> anyhow::Result<()> {
    use toml_edit::{value, DocumentMut, Item, Table};
    let text = read_existing(path)?;
    let mut doc: DocumentMut = text.parse()?;
    // `[paths]` may also be written inline (`paths = { mpv_dll = ".." }`).
    if !doc.get("paths").is_some_and(|p| p.is_table_like()) {
        doc.insert("paths", Item::Table(Table::new()));
    }
    doc["paths"]["popper_dir"] = value(dir.display().to_string());
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_missing_keys() {
        let s = Settings::parse("[wheel]\nstyle = \"flow\"\n[input]\nmenu_joy_buttons = [3]\n").unwrap();
        assert_eq!(s.wheel.style, WheelStyle::Flow);
        assert_eq!(s.input.menu_joy_buttons, [3]);
        assert!(s.input.joystick_needs_focus && s.input.keyboard_quit);
        assert!(s.attract.enabled);
        assert_eq!((s.attract.idle_secs, s.attract.interval_secs), (180, 12));
        assert!(s.launch.ignore_processes.is_empty() && s.paths.popper_dir.is_none());
        assert!(Settings::parse("[wheel\nstyle=").is_err());
    }

    #[test]
    fn save_popper_dir_handles_inline_paths() {
        let dir = std::env::temp_dir().join(format!("rp-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.toml");
        std::fs::write(&f, "# keep\npaths = { mpv_dll = 'C:/x/libmpv-2.dll' }\n").unwrap();
        save_popper_dir(&f, Path::new("D:/Popper")).unwrap();
        let s = Settings::parse(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(s.paths.popper_dir.as_deref(), Some(Path::new("D:/Popper")));
        assert_eq!(s.paths.mpv_dll.as_deref(), Some(Path::new("C:/x/libmpv-2.dll")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_keeps_comments_and_other_keys() {
        let before = "# my notes\n[playfield]\nui_rotation = 270 # turned\n\n[wheel]\nstyle = \"arc\"\n";
        let v = Editable { style: WheelStyle::Flow, attract_enabled: false, attract_idle_secs: 60, attract_interval_secs: 6, volume: 40, sound_enabled: false, sound_volume: 30, game_audio: false, game_audio_volume: 50 };
        let after = updated(before, &v).unwrap();
        assert!(after.contains("# my notes") && after.contains("ui_rotation = 270 # turned"));
        let s = Settings::parse(&after).unwrap();
        assert_eq!(s.wheel.style, WheelStyle::Flow);
        assert_eq!(s.playfield.ui_rotation, Some(270));
        assert!(!s.attract.enabled);
        assert_eq!((s.attract.idle_secs, s.attract.interval_secs, s.audio.volume), (60, 6, Some(40)));
    }
}
