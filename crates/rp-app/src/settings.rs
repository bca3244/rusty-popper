//! User settings from `rusty-popper.toml` next to the exe (all optional).
//!
//! ```toml
//! [wheel]
//! style = "arc"   # or "flow"
//!
//! [playfield]
//! ui_rotation = 270   # default: Popper's ThumbRotate
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
pub struct Settings {
    #[serde(default)]
    pub wheel: WheelSettings,
    #[serde(default)]
    pub playfield: PlayfieldSettings,
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
