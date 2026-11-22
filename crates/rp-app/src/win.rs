//! Small Win32 helpers: which window has the foreground, taking it back, and
//! attaching to the console the exe was started from.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, SetForegroundWindow};

/// Who owns the foreground (keyboard focus) right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreground {
    /// One of our windows.
    Ours,
    /// Nothing, or the Windows shell (desktop, taskbar): nobody is using
    /// another program, so the menu may take the focus back.
    Shell,
    /// Another program's window covering one of our screens (a game started
    /// elsewhere, a maximized tool): that program is what's being used.
    Other,
    /// Another program's smaller window (a dialog, a notification).
    Popup,
}

/// Does a window rectangle cover the whole of screen `(x, y, w, h)`?
fn covers(r: &RECT, (x, y, w, h): (i32, i32, i32, i32)) -> bool {
    r.left <= x && r.top <= y && r.right >= x + w && r.bottom >= y + h
}

/// Who has the foreground; `screens` are our screens as `(x, y, width, height)`.
pub fn foreground(screens: &[(i32, i32, i32, i32)]) -> Foreground {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return Foreground::Shell;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        if pid == GetCurrentProcessId() {
            return Foreground::Ours;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(fg, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        match String::from_utf16_lossy(&buf[..n.min(buf.len())]).as_str() {
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" => return Foreground::Shell,
            _ => {}
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(fg, &mut r) != 0 && screens.iter().any(|s| covers(&r, *s)) {
            Foreground::Other
        } else {
            Foreground::Popup
        }
    }
}

/// Brings `hwnd` (one of our windows, as an integer handle) to the foreground.
/// Windows refuses a plain SetForegroundWindow from a background process, so
/// this briefly attaches to the foreground thread's input queue, which is
/// allowed to hand the foreground over.
pub fn force_foreground(hwnd: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        let fg = GetForegroundWindow();
        let ours = GetCurrentThreadId();
        let theirs = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
        let attached = theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, 1) != 0;
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        if attached {
            AttachThreadInput(ours, theirs, 0);
        }
    }
}

/// When started from a terminal, send output there (a GUI-subsystem exe has
/// no console of its own). Does nothing when started from Explorer or a
/// scheduled task.
pub fn attach_parent_console() {
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_must_span_the_whole_screen_to_cover_it() {
        let screen = (1920, 0, 1080, 1920);
        let r = |left, top, right, bottom| RECT { left, top, right, bottom };
        assert!(covers(&r(1920, 0, 3000, 1920), screen));
        assert!(covers(&r(1912, -8, 3008, 1928), screen)); // maximized windows overhang
        assert!(!covers(&r(2000, 100, 2600, 700), screen));
        assert!(!covers(&r(0, 0, 1920, 1080), screen)); // on another monitor
    }
}
