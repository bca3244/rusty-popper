//! Menu input: keyboard (winit) plus joystick buttons (winmm, the same
//! 1-based button numbers Popper stores in PinUPFunctions.JoyCodes).

use rp_db::Function;
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Prev,
    Next,
    PagePrev,
    PageNext,
    /// Jump to the previous / next starting letter.
    LetterPrev,
    LetterNext,
    Start,
    Quit,
    /// Open (or close) the exit menu.
    Menu,
    /// Open the playlist picker.
    Playlists,
    ToggleStyle,
}

impl Action {
    /// Browsing actions auto-repeat while held.
    fn repeats(self) -> bool {
        matches!(self, Action::Prev | Action::Next | Action::PagePrev | Action::PageNext | Action::LetterPrev | Action::LetterNext)
    }

    fn from_popper(name: &str) -> Option<Action> {
        Some(match name {
            "Game Prior" => Action::Prev,
            "Game Next" => Action::Next,
            "Page Prior" => Action::PagePrev,
            "Page Next" => Action::PageNext,
            "List Next" | "List Prior" => Action::Playlists,
            "Game Start" | "_Game Start Alt" => Action::Start,
            "Menu/System Exit" | "System Menu" | "Exit Emulators" => Action::Menu,
            _ => return None,
        })
    }
}

pub struct Bindings {
    keys: Vec<(KeyCode, Action)>,
    joy: Vec<(u32, Action)>,
}

impl Bindings {
    /// Built-in defaults plus whatever Popper has bound in PinUPFunctions.
    /// `joy_overrides` (from rusty-popper.toml) replace any Popper binding on
    /// the same button.
    pub fn new(functions: &[Function], joy_overrides: &[(u32, Action)]) -> Self {
        let mut keys = vec![
            (KeyCode::ArrowLeft, Action::Prev),
            (KeyCode::ShiftLeft, Action::Prev),
            (KeyCode::ArrowRight, Action::Next),
            (KeyCode::ShiftRight, Action::Next),
            (KeyCode::ArrowUp, Action::LetterPrev),
            (KeyCode::ArrowDown, Action::LetterNext),
            (KeyCode::ControlLeft, Action::LetterPrev),
            (KeyCode::ControlRight, Action::LetterNext),
            (KeyCode::Enter, Action::Start),
            (KeyCode::NumpadEnter, Action::Start),
            (KeyCode::Escape, Action::Menu),
            (KeyCode::KeyP, Action::Playlists),
            (KeyCode::KeyQ, Action::Quit),
            (KeyCode::KeyW, Action::ToggleStyle),
        ];
        let mut joy = Vec::new();
        for f in functions.iter().filter(|f| f.active) {
            let Some(action) = Action::from_popper(&f.name) else { continue };
            if let Some(code) = f.key.and_then(vk_to_keycode) {
                keys.push((code, action));
            }
            if let Some(b) = f.joy_button.filter(|b| (1..=32).contains(b)) {
                joy.push((b as u32, action));
            }
        }
        for &(b, action) in joy_overrides.iter().filter(|(b, _)| (1..=32).contains(b)) {
            joy.retain(|(jb, _)| *jb != b);
            joy.push((b, action));
        }
        // Popper often binds one button to several functions (e.g. joy 9 for
        // both System Menu and Exit Emulators); fire each action once.
        let mut uniq_joy: Vec<(u32, Action)> = Vec::new();
        for j in joy {
            if !uniq_joy.contains(&j) {
                uniq_joy.push(j);
            }
        }
        let mut uniq_keys: Vec<(KeyCode, Action)> = Vec::new();
        for k in keys {
            if !uniq_keys.iter().any(|u| u.0 == k.0) {
                uniq_keys.push(k);
            }
        }
        Bindings { keys: uniq_keys, joy: uniq_joy }
    }

    pub fn key(&self, code: KeyCode, repeat: bool) -> Option<Action> {
        let a = self.keys.iter().find(|(k, _)| *k == code).map(|(_, a)| *a)?;
        (!repeat || a.repeats()).then_some(a)
    }

    pub fn describe(&self) -> String {
        let joy: Vec<String> = self.joy.iter().map(|(b, a)| format!("joy{b}={a:?}")).collect();
        format!("{} key bindings; {}", self.keys.len(), if joy.is_empty() { "no joystick bindings".into() } else { joy.join(", ") })
    }
}

/// Maps the Windows virtual-key codes Popper stores to winit key codes.
fn vk_to_keycode(vk: i64) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP,
        KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    Some(match vk {
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x30..=0x39 => DIGITS[(vk - 0x30) as usize],
        0x08 => Backspace,
        0x09 => Tab,
        0x0D => Enter,
        0x1B => Escape,
        0x20 => Space,
        0x25 => ArrowLeft,
        0x26 => ArrowUp,
        0x27 => ArrowRight,
        0x28 => ArrowDown,
        0x10 | 0xA0 => ShiftLeft,
        0xA1 => ShiftRight,
        0x11 | 0xA2 => ControlLeft,
        0xA3 => ControlRight,
        0x70..=0x7B => [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12][(vk - 0x70) as usize],
        0xBA => Semicolon,
        0xBB => Equal,
        0xBC => Comma,
        0xBD => Minus,
        0xBE => Period,
        0xBF => Slash,
        0xDB => BracketLeft,
        0xDC => Backslash,
        0xDD => BracketRight,
        0xDE => Quote,
        _ => return None,
    })
}

/// Polls joysticks through winmm and turns button presses into actions,
/// with auto-repeat for browsing.
pub struct Joysticks {
    ids: Vec<u32>,
    last_scan: Instant,
    held: Vec<(u32, u32, Instant, Instant)>, // (joy id, button, pressed at, last fired)
}

const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_EVERY: Duration = Duration::from_millis(90);

impl Joysticks {
    pub fn new() -> Self {
        let mut j = Joysticks { ids: Vec::new(), last_scan: Instant::now(), held: Vec::new() };
        j.scan();
        j
    }

    fn read(id: u32) -> Option<u32> {
        use windows_sys::Win32::Media::Multimedia::{joyGetPosEx, JOYINFOEX, JOY_RETURNBUTTONS};
        let mut info: JOYINFOEX = unsafe { std::mem::zeroed() };
        info.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
        info.dwFlags = JOY_RETURNBUTTONS as u32;
        (unsafe { joyGetPosEx(id, &mut info) } == 0).then_some(info.dwButtons)
    }

    fn scan(&mut self) {
        self.ids = (0..16).filter(|id| Self::read(*id).is_some()).collect();
        self.last_scan = Instant::now();
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    /// Marks every bound button that is down right now as already handled,
    /// so it only fires again after being released.
    pub fn swallow_held(&mut self, bindings: &Bindings) {
        let now = Instant::now();
        self.held.clear();
        for &id in &self.ids {
            let Some(buttons) = Self::read(id) else { continue };
            for &(b, _) in &bindings.joy {
                if buttons & (1 << (b - 1)) != 0 {
                    self.held.push((id, b, now, now + Duration::from_secs(3600)));
                }
            }
        }
    }

    pub fn poll(&mut self, bindings: &Bindings, out: &mut Vec<Action>) {
        if bindings.joy.is_empty() {
            return;
        }
        if self.last_scan.elapsed() > Duration::from_secs(5) {
            self.scan();
        }
        let now = Instant::now();
        let mut still_held = Vec::new();
        for &id in &self.ids {
            let Some(buttons) = Self::read(id) else { continue };
            for &(b, action) in &bindings.joy {
                if buttons & (1 << (b - 1)) == 0 {
                    continue;
                }
                match self.held.iter().find(|h| h.0 == id && h.1 == b) {
                    None => {
                        out.push(action);
                        still_held.push((id, b, now, now));
                    }
                    Some(&(_, _, since, fired)) => {
                        if action.repeats() && now - since >= REPEAT_DELAY && now - fired >= REPEAT_EVERY {
                            out.push(action);
                            still_held.push((id, b, since, now));
                        } else {
                            still_held.push((id, b, since, fired));
                        }
                    }
                }
            }
        }
        self.held = still_held;
    }
}
