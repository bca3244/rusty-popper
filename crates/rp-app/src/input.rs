//! Menu input: keyboard (winit) plus joystick buttons (winmm, the same
//! 0-based button numbers Popper stores in PinUPFunctions.JoyCodes, where 99
//! means button 0).

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
    /// Buttons Popper binds to "Exit Emulators": they close a running game.
    exit_joy: Vec<u32>,
}

/// The Popper function that closes the running game.
const EXIT_EMULATORS: &str = "Exit Emulators";

impl Bindings {
    /// Whatever Popper has bound in PinUPFunctions, plus built-in keyboard
    /// defaults for keys Popper leaves free. `joy_overrides` (from
    /// rusty-popper.toml) replace any Popper binding on the same button.
    /// `keyboard_quit` enables Q as an immediate quit.
    pub fn new(functions: &[Function], joy_overrides: &[(u32, Action)], keyboard_quit: bool) -> Self {
        let mut keys = Vec::new();
        let mut joy = Vec::new();
        let mut exit_joy = Vec::new();
        for f in functions.iter().filter(|f| f.active) {
            let Some(action) = Action::from_popper(&f.name) else { continue };
            if let Some(code) = f.key.and_then(vk_to_keycode) {
                keys.push((code, action));
            }
            if let Some(b) = f.joy_button.filter(|b| (0..32).contains(b)) {
                joy.push((b as u32, action));
                if f.name == EXIT_EMULATORS && !exit_joy.contains(&(b as u32)) {
                    exit_joy.push(b as u32);
                }
            }
        }
        // Popper's own key bindings come first, so they win over the defaults.
        keys.extend([
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
            (KeyCode::KeyW, Action::ToggleStyle),
        ]);
        if keyboard_quit {
            keys.push((KeyCode::KeyQ, Action::Quit));
        }
        for &(b, action) in joy_overrides.iter().filter(|(b, _)| *b < 32) {
            joy.retain(|(jb, _)| *jb != b);
            exit_joy.retain(|jb| *jb != b);
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
        Bindings { keys: uniq_keys, joy: uniq_joy, exit_joy }
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
    /// Whether an exit button was down at the last [`exit_pressed`](Self::exit_pressed) check.
    exit_down: bool,
}

/// How often to look for joysticks: often while none is known, rarely once
/// one is (asking winmm about absent ids can stall for a moment).
const SCAN_EVERY_NONE: Duration = Duration::from_secs(5);
const SCAN_EVERY_SOME: Duration = Duration::from_secs(60);

const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_EVERY: Duration = Duration::from_millis(90);

impl Joysticks {
    pub fn new() -> Self {
        let mut j = Joysticks { ids: Vec::new(), last_scan: Instant::now(), held: Vec::new(), exit_down: false };
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
                if buttons & (1 << b) != 0 {
                    self.held.push((id, b, now, now + Duration::from_secs(3600)));
                }
            }
        }
    }

    fn rescan_if_due(&mut self) {
        let every = if self.ids.is_empty() { SCAN_EVERY_NONE } else { SCAN_EVERY_SOME };
        if self.last_scan.elapsed() > every {
            self.scan();
        }
    }

    fn exit_down_now(&self, bindings: &Bindings) -> bool {
        let mask = bindings.exit_joy.iter().fold(0u32, |m, b| m | (1 << b));
        mask != 0 && self.ids.iter().any(|&id| Self::read(id).is_some_and(|buttons| buttons & mask != 0))
    }

    /// Call when a game starts: an exit button already held doesn't count.
    pub fn arm_exit(&mut self, bindings: &Bindings) {
        self.exit_down = self.exit_down_now(bindings);
    }

    /// While a game runs: true when an "Exit Emulators" button has just been
    /// pressed. Works whichever window has the focus.
    pub fn exit_pressed(&mut self, bindings: &Bindings) -> bool {
        if bindings.exit_joy.is_empty() {
            return false;
        }
        self.rescan_if_due();
        let down = self.exit_down_now(bindings);
        let pressed = down && !self.exit_down;
        self.exit_down = down;
        pressed
    }

    pub fn poll(&mut self, bindings: &Bindings, out: &mut Vec<Action>) {
        if bindings.joy.is_empty() {
            return;
        }
        self.rescan_if_due();
        let now = Instant::now();
        let mut still_held = Vec::new();
        for &id in &self.ids {
            let Some(buttons) = Self::read(id) else { continue };
            for &(b, action) in &bindings.joy {
                if buttons & (1 << b) == 0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn f(index: i64, name: &str, key: Option<i64>, joy: Option<i64>) -> Function {
        Function { index, name: name.into(), key, joy_button: joy, active: true }
    }

    #[test]
    fn popper_bindings_win_over_defaults_and_overrides_win_over_popper() {
        let functions = [
            f(1, "Game Prior", Some(0xA0), Some(4)),        // left shift, as by default
            f(2, "Page Next", Some(0x27), Some(6)),         // right arrow: a default says Next
            f(3, "System Menu", None, Some(9)),
            f(4, "Exit Emulators", Some(0x51), Some(9)),    // Q
            f(5, "Game Start", Some(0x0D), Some(7)),
            Function { active: false, ..f(6, "Game Next", Some(0x41), Some(5)) },
        ];
        let b = Bindings::new(&functions, &[(6, Action::Playlists)], true);
        assert_eq!(b.key(KeyCode::ArrowRight, false), Some(Action::PageNext));
        assert_eq!(b.key(KeyCode::ShiftLeft, false), Some(Action::Prev));
        assert_eq!(b.key(KeyCode::KeyQ, false), Some(Action::Menu)); // Popper's use of Q beats quit
        assert_eq!(b.key(KeyCode::KeyA, false), None); // inactive function
        assert_eq!(b.key(KeyCode::Enter, true), None); // Start doesn't auto-repeat
        assert_eq!(b.key(KeyCode::ArrowLeft, true), Some(Action::Prev));
        // Button 9 is bound twice to Menu: one binding. Button 6 was overridden.
        assert_eq!(b.joy, [(4, Action::Prev), (9, Action::Menu), (7, Action::Start), (6, Action::Playlists)]);
        assert_eq!(b.exit_joy, [9]);
    }

    #[test]
    fn quit_key_can_be_turned_off() {
        assert_eq!(Bindings::new(&[], &[], true).key(KeyCode::KeyQ, false), Some(Action::Quit));
        assert_eq!(Bindings::new(&[], &[], false).key(KeyCode::KeyQ, false), None);
    }
}
