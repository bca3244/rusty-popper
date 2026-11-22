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
    Ok(doc.to_string())
}

/// Writes the editable values into rusty-popper.toml. The new contents go to
/// a temp file that then replaces the old one, so a power cut mid-save can't
/// leave a half-written settings file.
pub fn save(path: &Path, v: &Editable) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let new = updated(&text, v)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, new)?;
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
    fn save_keeps_comments_and_other_keys() {
        let before = "# my notes\n[playfield]\nui_rotation = 270 # turned\n\n[wheel]\nstyle = \"arc\"\n";
        let v = Editable { style: WheelStyle::Flow, attract_enabled: false, attract_idle_secs: 60, attract_interval_secs: 6, volume: 40 };
        let after = updated(before, &v).unwrap();
        assert!(after.contains("# my notes") && after.contains("ui_rotation = 270 # turned"));
        let s = Settings::parse(&after).unwrap();
        assert_eq!(s.wheel.style, WheelStyle::Flow);
        assert_eq!(s.playfield.ui_rotation, Some(270));
        assert!(!s.attract.enabled);
        assert_eq!((s.attract.idle_secs, s.attract.interval_secs, s.audio.volume), (60, 6, Some(40)));
    }
}
