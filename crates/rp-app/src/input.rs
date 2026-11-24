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
    /// Show / hide the game's info card (Popper's "Game Info/Flyer").
    Info,
    /// Show / hide the game's help card (Popper's "Game Help").
    Help,
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
            "Game Info/Flyer" => Action::Info,
            "Game Help" => Action::Help,
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
    /// Keys (Windows virtual-key codes) Popper binds to "Exit Emulators".
    exit_keys: Vec<i32>,
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
        let mut exit_keys = Vec::new();
        for f in functions.iter().filter(|f| f.active) {
            let Some(action) = Action::from_popper(&f.name) else { continue };
            if let Some(vk) = f.key.filter(|vk| f.name == EXIT_EMULATORS && (1..255).contains(vk)) {
                exit_keys.push(vk as i32);
            }
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
            (KeyCode::KeyI, Action::Info),
            (KeyCode::KeyH, Action::Help),
        ]);
        if keyboard_quit {
            keys.push((KeyCode::KeyQ, Action::Quit));
        }
        for &(b, action) in joy_overrides.iter().filter(|(b, _)| *b < 32) {
            joy.retain(|(jb, _)| *jb != b);
            // A button set to open the exit menu also closes a running game;
            // any other use takes the button away from "Exit Emulators".
            exit_joy.retain(|jb| *jb != b);
            if action == Action::Menu {
                exit_joy.push(b);
            }
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
        Bindings { keys: uniq_keys, joy: uniq_joy, exit_joy, exit_keys }
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
    /// A rescan running on a helper thread.
    scanning: Option<std::sync::mpsc::Receiver<Vec<u32>>>,
}

/// The winmm joystick ids that answer.
fn scan_ids() -> Vec<u32> {
    (0..16).filter(|id| Joysticks::read(*id).is_some()).collect()
}

/// How often to look for joysticks: often while none is known, rarely once
/// one is (asking winmm about absent ids can stall for a moment).
const SCAN_EVERY_NONE: Duration = Duration::from_secs(5);
const SCAN_EVERY_SOME: Duration = Duration::from_secs(60);

const REPEAT_DELAY: Duration = Duration::from_millis(400);

/// Time between repeats while a browsing button is held: it speeds up the
/// longer the button stays down, so long lists go by quickly.
fn repeat_every(held_for: Duration) -> Duration {
    match held_for.as_millis() {
        0..=1499 => Duration::from_millis(90),
        1500..=2999 => Duration::from_millis(55),
        _ => Duration::from_millis(35),
    }
}

/// Whether a held button that last fired at `fired` (pressed at `since`)
/// repeats now, and if so the new "last fired" time. Repeats keep an even
/// beat rather than drifting with the frame rate; after a stall it restarts
/// from now instead of firing a burst.
fn repeat_due(since: Instant, fired: Instant, now: Instant) -> Option<Instant> {
    if now - since < REPEAT_DELAY {
        return None;
    }
    let every = repeat_every(now - since);
    // The first repeat comes right at the delay.
    let next = if fired == since { since + REPEAT_DELAY } else { fired + every };
    if now < next {
        return None;
    }
    Some(if now - next > every { now } else { next })
}

/// Auto-repeat for keyboard browsing keys on our own beat (the same as the
/// joystick's), instead of the Windows key-repeat setting.
#[derive(Default)]
pub struct KeyRepeat {
    held: Vec<(KeyCode, i32, Action, Instant, Instant)>, // (key, virtual key, action, pressed at, last fired)
}

impl KeyRepeat {
    /// A key went down (not a Windows repeat).
    pub fn pressed(&mut self, code: KeyCode, action: Action) {
        if !action.repeats() || self.held.iter().any(|h| h.0 == code) {
            return;
        }
        // The most specific Windows key code for it, to check it's still down.
        let vk = (0..=0xFF).rev().find(|&vk| vk_to_keycode(vk) == Some(code)).unwrap_or(0) as i32;
        let now = Instant::now();
        self.held.push((code, vk, action, now, now));
    }

    pub fn released(&mut self, code: KeyCode) {
        self.held.retain(|h| h.0 != code);
    }

    pub fn clear(&mut self) {
        self.held.clear();
    }

    /// Adds the repeats that are due.
    pub fn tick(&mut self, out: &mut Vec<Action>) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        let now = Instant::now();
        // A release can be missed (focus moved between our windows): ask Windows.
        self.held.retain(|h| h.1 == 0 || unsafe { GetAsyncKeyState(h.1) } < 0);
        for h in self.held.iter_mut() {
            if let Some(fired) = repeat_due(h.3, h.4, now) {
                h.4 = fired;
                out.push(h.2);
            }
        }
    }
}

impl Joysticks {
    pub fn new() -> Self {
        let mut j = Joysticks { ids: Vec::new(), last_scan: Instant::now(), held: Vec::new(), exit_down: false, scanning: None };
        let t = Instant::now();
        j.ids = scan_ids();
        crate::perf::note("Joystick scan", t, 5);
        j
    }

    pub(crate) fn read(id: u32) -> Option<u32> {
        use windows_sys::Win32::Media::Multimedia::{joyGetPosEx, JOYINFOEX, JOY_RETURNBUTTONS};
        let mut info: JOYINFOEX = unsafe { std::mem::zeroed() };
        info.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
        info.dwFlags = JOY_RETURNBUTTONS as u32;
        (unsafe { joyGetPosEx(id, &mut info) } == 0).then_some(info.dwButtons)
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

    /// Looks for joysticks again now and then, on a helper thread: asking
    /// winmm about absent ids can stall, and that mustn't hold up a frame.
    fn rescan_if_due(&mut self) {
        if let Some(rx) = &self.scanning {
            match rx.try_recv() {
                Ok(ids) => {
                    if ids != self.ids {
                        log::info!("Joysticks found: {}", ids.len());
                    }
                    self.ids = ids;
                    self.scanning = None;
                    self.last_scan = Instant::now();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.scanning = None;
                    self.last_scan = Instant::now();
                }
            }
            return;
        }
        let every = if self.ids.is_empty() { SCAN_EVERY_NONE } else { SCAN_EVERY_SOME };
        if self.last_scan.elapsed() > every {
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("joystick-scan".into()).spawn(move || {
                let t = Instant::now();
                let ids = scan_ids();
                crate::perf::note("Joystick scan", t, 5);
                let _ = tx.send(ids);
            });
            match spawned {
                Ok(_) => self.scanning = Some(rx),
                Err(_) => self.last_scan = Instant::now(),
            }
        }
    }

    fn exit_down_now(&self, bindings: &Bindings) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        // The keyboard is asked directly: the game has the focus, so no key
        // events reach our windows.
        if bindings.exit_keys.iter().any(|&vk| unsafe { GetAsyncKeyState(vk) } < 0) {
            return true;
        }
        let mask = bindings.exit_joy.iter().fold(0u32, |m, b| m | (1 << b));
        mask != 0 && self.ids.iter().any(|&id| Self::read(id).is_some_and(|buttons| buttons & mask != 0))
    }

    /// Call when a game starts: an exit button already held doesn't count.
    pub fn arm_exit(&mut self, bindings: &Bindings) {
        self.exit_down = self.exit_down_now(bindings);
    }

    /// While a game runs: true when an "Exit Emulators" button or key has
    /// just been pressed. Works whichever window has the focus.
    pub fn exit_pressed(&mut self, bindings: &Bindings) -> bool {
        if bindings.exit_joy.is_empty() && bindings.exit_keys.is_empty() {
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
                    Some(&(_, _, since, fired)) => match repeat_due(since, fired, now).filter(|_| action.repeats()) {
                        Some(next) => {
                            out.push(action);
                            still_held.push((id, b, since, next));
                        }
                        None => still_held.push((id, b, since, fired)),
                    },
                }
            }
        }
        self.held = still_held;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_keep_an_even_beat_and_speed_up() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // Nothing before the delay; the first repeat lands on it.
        assert_eq!(repeat_due(t0, t0, ms(399)), None);
        assert_eq!(repeat_due(t0, t0, ms(405)), Some(ms(400)));
        // Then every 90 ms from the last beat, not from when the frame came.
        assert_eq!(repeat_due(t0, ms(400), ms(489)), None);
        assert_eq!(repeat_due(t0, ms(400), ms(495)), Some(ms(490)));
        // Faster after 1.5 s and 3 s.
        assert_eq!(repeat_due(t0, ms(1600), ms(1655)), Some(ms(1655)));
        assert_eq!(repeat_due(t0, ms(3100), ms(3135)), Some(ms(3135)));
        // After a long stall it restarts from now instead of bursting.
        assert_eq!(repeat_due(t0, ms(400), ms(900)), Some(ms(900)));
    }

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
        assert_eq!(b.exit_keys, [0x51]);
    }

    #[test]
    fn overrides_decide_which_buttons_close_a_game() {
        let functions = [f(1, "Exit Emulators", None, Some(9)), f(2, "Game Start", None, Some(7))];
        // Button 9 stays the exit button when the override also says Menu...
        assert_eq!(Bindings::new(&functions, &[(9, Action::Menu), (3, Action::Menu)], true).exit_joy, [9, 3]);
        // ...and stops being one when it is given another job.
        assert_eq!(Bindings::new(&functions, &[(9, Action::Playlists)], true).exit_joy, [] as [u32; 0]);
    }

    #[test]
    fn quit_key_can_be_turned_off() {
        assert_eq!(Bindings::new(&[], &[], true).key(KeyCode::KeyQ, false), Some(Action::Quit));
        assert_eq!(Bindings::new(&[], &[], false).key(KeyCode::KeyQ, false), None);
    }
}
