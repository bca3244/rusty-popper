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

#[derive(Debug, Default, Deserialize)]
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
}

impl Settings {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.is_file() {
            return Ok(Settings::default());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
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

/// Writes the editable values into rusty-popper.toml, keeping everything
/// else in the file (comments, other keys) as it was.
pub fn save(path: &Path, v: &Editable) -> anyhow::Result<()> {
    use toml_edit::{value, DocumentMut, Item, Table};
    let text = std::fs::read_to_string(path).unwrap_or_default();
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
    std::fs::write(path, doc.to_string())?;
    Ok(())
}