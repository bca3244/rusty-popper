//! User settings from `rusty-popper.toml` next to the exe (all optional).
//!
//! ```toml
//! [wheel]
//! style = "arc"   # layout template: "arc", "flow", "minimal" or "vertical"
//! # Options → Layout; each defaults to the template's value.
//! # size = 80          # main wheel size, percent (60-150)
//! # position = "bottom" # or "left", "right"
//! # side_wheels = 2    # wheels shown each side of the main one (0-5)
//! # spacing = 100      # gap between the wheels, percent (50-200)
//! # arc = true         # curve the row of wheels
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
//!
//! [remote]
//! enabled = false   # phone remote web page (also in Options)
//! port = 8642
//! # pin = '1234'
//! # address = '192.168.1.20'
//! ```

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WheelStyle {
    #[default]
    Arc,
    Flow,
    Minimal,
    Vertical,
}

impl WheelStyle {
    /// The next style in the Options cycle.
    pub fn toggled(self) -> Self {
        match self {
            WheelStyle::Arc => WheelStyle::Flow,
            WheelStyle::Flow => WheelStyle::Minimal,
            WheelStyle::Minimal => WheelStyle::Vertical,
            WheelStyle::Vertical => WheelStyle::Arc,
        }
    }

    /// The name in rusty-popper.toml.
    pub fn key(self) -> &'static str {
        match self {
            WheelStyle::Arc => "arc",
            WheelStyle::Flow => "flow",
            WheelStyle::Minimal => "minimal",
            WheelStyle::Vertical => "vertical",
        }
    }

    /// The name in Options.
    pub fn label(self) -> &'static str {
        match self {
            WheelStyle::Arc => "Arc",
            WheelStyle::Flow => "Cover Flow",
            WheelStyle::Minimal => "Minimal",
            WheelStyle::Vertical => "Vertical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "arc" => Some(WheelStyle::Arc),
            "flow" => Some(WheelStyle::Flow),
            "minimal" => Some(WheelStyle::Minimal),
            "vertical" => Some(WheelStyle::Vertical),
            _ => None,
        }
    }
}

/// Where the row of wheels runs: along the bottom, or down one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WheelPosition {
    Bottom,
    Left,
    Right,
}

impl WheelPosition {
    pub fn toggled(self) -> Self {
        match self {
            WheelPosition::Bottom => WheelPosition::Left,
            WheelPosition::Left => WheelPosition::Right,
            WheelPosition::Right => WheelPosition::Bottom,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            WheelPosition::Bottom => "bottom",
            WheelPosition::Left => "left",
            WheelPosition::Right => "right",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            WheelPosition::Bottom => "Bottom",
            WheelPosition::Left => "Left",
            WheelPosition::Right => "Right",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "bottom" => Some(WheelPosition::Bottom),
            "left" => Some(WheelPosition::Left),
            "right" => Some(WheelPosition::Right),
            _ => None,
        }
    }
}

/// Main wheel sizes (percent) that Options → Layout cycles through.
pub const SIZE_CHOICES: &[u32] = &[60, 70, 80, 90, 100, 110, 120, 130, 140, 150];
/// The most wheels shown on each side of the main one.
pub const MAX_SIDE_WHEELS: u32 = 5;
/// Wheel spacings (percent of the template's) that Options → Layout cycles through.
pub const SPACING_CHOICES: &[u32] = &[50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 175, 200];

/// How the wheel is laid out (Options → Layout). A template (the wheel
/// style) sets all of these; each can then be changed on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Main wheel size, percent (one of SIZE_CHOICES).
    pub size: u32,
    pub position: WheelPosition,
    /// Wheels shown on each side of the main one (0..=MAX_SIDE_WHEELS).
    pub side_wheels: u32,
    /// How far apart the wheels are, percent (one of SPACING_CHOICES).
    pub spacing: u32,
    /// Whether the row of wheels curves (round a big wheel off the screen edge).
    pub arc: bool,
}

impl WheelStyle {
    /// The layout this style starts from.
    pub fn template(self) -> Layout {
        let (size, position, side_wheels, arc) = match self {
            WheelStyle::Arc => (90, WheelPosition::Bottom, 2, true),
            WheelStyle::Flow => (130, WheelPosition::Bottom, 2, false),
            WheelStyle::Minimal => (120, WheelPosition::Bottom, 1, false),
            WheelStyle::Vertical => (100, WheelPosition::Right, 3, false),
        };
        Layout { size, position, side_wheels, spacing: 100, arc }
    }
}

impl Layout {
    /// A size from the file, snapped to the nearest choice.
    fn snap_size(v: u32) -> u32 {
        *SIZE_CHOICES.iter().min_by_key(|c| c.abs_diff(v)).unwrap()
    }
    fn snap_spacing(v: u32) -> u32 {
        *SPACING_CHOICES.iter().min_by_key(|c| c.abs_diff(v)).unwrap()
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WheelSettings {
    /// The layout template.
    #[serde(default)]
    pub style: WheelStyle,
    // A value that can't be read (a typo, a wrong type) falls back to the
    // template instead of throwing away the whole settings file.
    #[serde(default, deserialize_with = "lenient_u32")]
    pub size: Option<u32>,
    #[serde(default, deserialize_with = "lenient_position")]
    pub position: Option<WheelPosition>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub side_wheels: Option<u32>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub spacing: Option<u32>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub arc: Option<bool>,
}

fn lenient_u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    let v = toml::Value::deserialize(d)?;
    Ok(v.as_integer().and_then(|n| u32::try_from(n).ok()))
}

fn lenient_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    let v = toml::Value::deserialize(d)?;
    Ok(v.as_bool())
}

fn lenient_position<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<WheelPosition>, D::Error> {
    let v = toml::Value::deserialize(d)?;
    Ok(v.as_str().and_then(WheelPosition::parse))
}

impl WheelSettings {
    /// The template's layout with any values the file sets.
    pub fn layout(&self) -> Layout {
        let t = self.style.template();
        Layout {
            size: self.size.map_or(t.size, Layout::snap_size),
            position: self.position.unwrap_or(t.position),
            side_wheels: self.side_wheels.map_or(t.side_wheels, |n| n.min(MAX_SIDE_WHEELS)),
            spacing: self.spacing.map_or(t.spacing, Layout::snap_spacing),
            arc: self.arc.unwrap_or(t.arc),
        }
    }
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

/// `[dof]`: light the cabinet's toys from the menu with DOF, using the DOF
/// Config Tool's "PinUP Menu" table (ROM pinupmenu), as Popper's menu does.
#[derive(Debug, Deserialize)]
pub struct DofSettings {
    /// Off unless turned on (Options or here).
    #[serde(default)]
    pub enabled: bool,
    /// Rate Cap: the fewest ms between two wheel/page pulses (0 = off, up to 500).
    #[serde(default = "default_rate_cap")]
    pub rate_cap_ms: u64,
}

fn default_rate_cap() -> u64 {
    crate::dof::DEFAULT_RATE_CAP_MS
}

impl Default for DofSettings {
    fn default() -> Self {
        DofSettings { enabled: false, rate_cap_ms: default_rate_cap() }
    }
}

/// `[remote]`: the phone remote, a small web page on the home network.
#[derive(Debug, Deserialize)]
pub struct RemoteSettings {
    /// Off unless turned on (Options or here).
    #[serde(default)]
    pub enabled: bool,
    /// Fixed, so the link and QR code never change.
    #[serde(default = "default_remote_port")]
    pub port: u16,
    /// Optional: when set, the page only answers with `?pin=` matching (the
    /// QR code includes it). Letters and digits; `pin = 1234` works too.
    #[serde(default, deserialize_with = "string_or_number")]
    pub pin: Option<String>,
    /// Optional: the host the QR code links to (default: this machine's
    /// address on the home network).
    pub address: Option<String>,
}

/// A PIN written as text or as a bare number.
fn string_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Pin {
        Text(String),
        Number(u64),
    }
    Ok(Option::<Pin>::deserialize(d)?.map(|p| match p {
        Pin::Text(t) => t,
        Pin::Number(n) => n.to_string(),
    }))
}

fn default_remote_port() -> u16 {
    crate::remote::DEFAULT_PORT
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings { enabled: false, port: default_remote_port(), pin: None, address: None }
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
    #[serde(default)]
    pub dof: DofSettings,
    #[serde(default)]
    pub remote: RemoteSettings,
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

#[derive(Debug, Deserialize)]
pub struct AudioSettings {
    /// The playfield video's sound. Off keeps `volume` for when it's back on.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Playfield video volume, 0-100. Default: PinUpPlayer.ini's playfield volume.
    pub volume: Option<i32>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings { enabled: true, volume: None }
    }
}

/// Values the in-menu Options screen can change.
pub struct Editable {
    pub style: WheelStyle,
    pub layout: Layout,
    pub attract_enabled: bool,
    pub attract_idle_secs: u64,
    pub attract_interval_secs: u64,
    pub video_sound: bool,
    pub volume: i32,
    pub sound_enabled: bool,
    pub sound_volume: i32,
    pub game_audio: bool,
    pub game_audio_volume: i32,
    pub dof: bool,
    pub dof_rate_cap_ms: u64,
    pub remote: bool,
}

/// `text` (the current file contents) with the editable values set, keeping
/// everything else (comments, other keys) as it was.
fn updated(text: &str, v: &Editable) -> anyhow::Result<String> {
    use toml_edit::{value, DocumentMut, Item, Table};
    let mut doc: DocumentMut = text.parse()?;
    let mut set = |table: &str, key: &str, val: toml_edit::Value| {
        // A table may also be written inline (`remote = { pin = "1234" }`):
        // keep it, only add one if there's none.
        if !doc.get(table).is_some_and(|t| t.is_table_like()) {
            doc.insert(table, Item::Table(Table::new()));
        }
        doc[table][key] = value(val);
    };
    set("wheel", "style", v.style.key().into());
    set("attract", "enabled", v.attract_enabled.into());
    set("attract", "idle_secs", (v.attract_idle_secs as i64).into());
    set("attract", "interval_secs", (v.attract_interval_secs as i64).into());
    set("audio", "enabled", v.video_sound.into());
    set("audio", "volume", (v.volume as i64).into());
    set("sound", "enabled", v.sound_enabled.into());
    set("sound", "volume", (v.sound_volume as i64).into());
    set("sound", "game_audio", v.game_audio.into());
    set("sound", "game_audio_volume", (v.game_audio_volume as i64).into());
    set("dof", "enabled", v.dof.into());
    set("dof", "rate_cap_ms", (v.dof_rate_cap_ms as i64).into());
    set("remote", "enabled", v.remote.into());
    // Only what differs from the template is written, so changing `style`
    // by hand still brings that template's whole layout.
    let t = v.style.template();
    let l = v.layout;
    let layout_keys: [(&str, Option<toml_edit::Value>); 5] = [
        ("size", (l.size != t.size).then(|| (l.size as i64).into())),
        ("position", (l.position != t.position).then(|| l.position.key().into())),
        ("side_wheels", (l.side_wheels != t.side_wheels).then(|| (l.side_wheels as i64).into())),
        ("spacing", (l.spacing != t.spacing).then(|| (l.spacing as i64).into())),
        ("arc", (l.arc != t.arc).then(|| l.arc.into())),
    ];
    for (key, val) in layout_keys {
        match val {
            Some(val) => doc["wheel"][key] = value(val),
            None => {
                if let Some(w) = doc.get_mut("wheel").and_then(|w| w.as_table_like_mut()) {
                    w.remove(key);
                }
            }
        }
    }
    Ok(doc.to_string())
}

/// The file's text, or empty if there's no file yet. Any other read error is
/// passed on, so a file we can't read is never overwritten.
fn read_existing(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

/// Writes the editable values into rusty-popper.toml. The new contents go to
/// a temp file that then replaces the old one, so a power cut mid-save can't
/// leave a half-written settings file.
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
        assert!(s.audio.enabled, "video sound is on by default");
        assert!(!s.dof.enabled, "DOF is off by default");
        assert_eq!(s.dof.rate_cap_ms, 200);
        assert!(!s.remote.enabled && s.remote.pin.is_none(), "the phone remote is off, with no PIN, by default");
        assert_eq!(s.remote.port, crate::remote::DEFAULT_PORT);
        assert_eq!(Settings::parse("[remote]\npin = 1234\n").unwrap().remote.pin.as_deref(), Some("1234"));
        assert_eq!(Settings::parse("[remote]\npin = 'ab12'\n").unwrap().remote.pin.as_deref(), Some("ab12"));
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
        let v = Editable { style: WheelStyle::Flow, layout: Layout { size: 60, position: WheelPosition::Left, side_wheels: 4, spacing: 70, arc: true }, attract_enabled: false, attract_idle_secs: 60, attract_interval_secs: 6, video_sound: false, volume: 40, sound_enabled: false, sound_volume: 30, game_audio: false, game_audio_volume: 50, dof: true, dof_rate_cap_ms: 350, remote: true };
        let after = updated(before, &v).unwrap();
        assert!(after.contains("# my notes") && after.contains("ui_rotation = 270 # turned"));
        let s = Settings::parse(&after).unwrap();
        assert_eq!(s.wheel.style, WheelStyle::Flow);
        assert_eq!(s.wheel.layout(), Layout { size: 60, position: WheelPosition::Left, side_wheels: 4, spacing: 70, arc: true });
        assert_eq!(s.playfield.ui_rotation, Some(270));
        assert!(!s.attract.enabled);
        assert_eq!((s.attract.idle_secs, s.attract.interval_secs, s.audio.volume), (60, 6, Some(40)));
        assert!(s.dof.enabled);
        assert_eq!(s.dof.rate_cap_ms, 350);
        assert!(s.remote.enabled);
        assert!(!s.audio.enabled);
    }

    #[test]
    fn save_keeps_inline_tables() {
        let before = "remote = { enabled = false, pin = '1234', port = 9000 }\nsound = { volume = 10 }\n";
        let v = Editable {
            video_sound: true,
            style: WheelStyle::Arc,
            layout: WheelStyle::Arc.template(),
            attract_enabled: true,
            attract_idle_secs: 180,
            attract_interval_secs: 12,
            volume: 50,
            sound_enabled: true,
            sound_volume: 30,
            game_audio: true,
            game_audio_volume: 50,
            dof: false,
            dof_rate_cap_ms: 200,
            remote: true,
        };
        let s = Settings::parse(&updated(before, &v).unwrap()).unwrap();
        assert!(s.remote.enabled);
        assert_eq!((s.remote.pin.as_deref(), s.remote.port), (Some("1234"), 9000));
        assert_eq!(s.sound.volume, 30);
    }

    #[test]
    fn every_wheel_style_round_trips() {
        let mut style = WheelStyle::Arc;
        for _ in 0..4 {
            assert_eq!(WheelStyle::parse(style.key()), Some(style));
            let v = Editable { style, layout: style.template(), attract_enabled: true, attract_idle_secs: 180, attract_interval_secs: 12, video_sound: true, volume: 50, sound_enabled: true, sound_volume: 30, game_audio: true, game_audio_volume: 50, dof: false, dof_rate_cap_ms: 200, remote: false };
            let s = Settings::parse(&updated("[wheel]\nstyle = \"arc\"\n", &v).unwrap()).unwrap();
            assert_eq!(s.wheel.style, style);
            assert_eq!(s.wheel.layout(), style.template());
            style = style.toggled();
        }
        assert_eq!(style, WheelStyle::Arc, "the Options cycle visits all four styles");
        assert_eq!(Settings::parse("[wheel]\nstyle = \"vertical\"\n").unwrap().wheel.style, WheelStyle::Vertical);
    }

    #[test]
    fn layout_defaults_to_the_template() {
        // An older file (style only) gets the template's layout.
        let s = Settings::parse("[wheel]\nstyle = \"vertical\"\n").unwrap();
        assert_eq!(s.wheel.layout(), WheelStyle::Vertical.template());
        assert_eq!(s.wheel.layout().position, WheelPosition::Right);
        // Values the file sets win; out-of-range ones are brought into range.
        let s = Settings::parse("[wheel]\nstyle = \"arc\"\nsize = 133\nposition = \"Left\"\nside_wheels = 9\nspacing = 180\narc = false\n").unwrap();
        assert_eq!(s.wheel.layout(), Layout { size: 130, position: WheelPosition::Left, side_wheels: MAX_SIDE_WHEELS, spacing: 175, arc: false });
        // Unreadable values fall back to the template, keeping the rest of the file.
        let s = Settings::parse("[wheel]\nstyle = \"minimal\"\nsize = -10\nposition = \"up\"\nside_wheels = 1.5\narc = \"on\"\n[dof]\nenabled = true\n").unwrap();
        assert_eq!(s.wheel.layout(), WheelStyle::Minimal.template());
        assert!(s.dof.enabled);
        // Only changes from the template are saved: editing `style` later brings its layout.
        let mut v = Editable { style: WheelStyle::Arc, layout: WheelStyle::Arc.template(), attract_enabled: true, attract_idle_secs: 180, attract_interval_secs: 12, video_sound: true, volume: 50, sound_enabled: true, sound_volume: 30, game_audio: true, game_audio_volume: 50, dof: false, dof_rate_cap_ms: 200, remote: false };
        let text = updated("[wheel]\nstyle = \"arc\"\nsize = 60\n", &v).unwrap();
        assert!(!text.contains("size") && !text.contains("position") && !text.contains("arc ="), "{text}");
        v.layout.position = WheelPosition::Left;
        let text = updated(&text, &v).unwrap();
        assert!(text.contains("position = \"left\"") && !text.contains("size"), "{text}");
        let edited = text.replace("style = \"arc\"", "style = \"vertical\"");
        assert_eq!(Settings::parse(&edited).unwrap().wheel.layout(), Layout { position: WheelPosition::Left, ..WheelStyle::Vertical.template() });
        for t in [WheelStyle::Arc, WheelStyle::Flow, WheelStyle::Minimal, WheelStyle::Vertical] {
            let l = t.template();
            assert!(SIZE_CHOICES.contains(&l.size) && SPACING_CHOICES.contains(&l.spacing) && l.side_wheels <= MAX_SIDE_WHEELS);
        }
        let mut p = WheelPosition::Bottom;
        for _ in 0..3 {
            assert_eq!(WheelPosition::parse(p.key()), Some(p));
            p = p.toggled();
        }
        assert_eq!(p, WheelPosition::Bottom);
    }
}
