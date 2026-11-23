//! The "couldn't start" screen: a full-screen notice on the playfield, turned
//! to face the player like the menu, instead of a sideways Windows message box.
//!
//! It's drawn with plain GDI because it has to work when nothing else does
//! (no Popper, no libmpv, no GL). Where to draw it comes from `last-screen.txt`,
//! which every good start writes; before the first good start it falls back to
//! the primary monitor and `[playfield] ui_rotation` from rusty-popper.toml.
//! Any key, mouse click or joystick button closes it.

use std::cell::RefCell;
use std::path::Path;

/// Where the playfield is and how the menu is turned on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Clockwise turn of the text, as the menu uses (0, 90, 180, 270).
    pub rot: i32,
}

impl Placement {
    fn parse(s: &str) -> Option<Placement> {
        let v: Vec<i32> = s.split_whitespace().map(|p| p.parse().ok()).collect::<Option<_>>()?;
        match v[..] {
            [x, y, w, h, rot] if w > 0 && h > 0 => Some(Placement { x, y, w, h, rot: rot.rem_euclid(360) / 90 * 90 }),
            _ => None,
        }
    }

    fn to_line(self) -> String {
        format!("{} {} {} {} {}", self.x, self.y, self.w, self.h, self.rot)
    }
}

fn state_file(exe_dir: &Path) -> std::path::PathBuf {
    exe_dir.join("last-screen.txt")
}

/// Called on every good start, so a later failure knows where to draw.
pub fn remember(exe_dir: &Path, p: Placement) {
    let path = state_file(exe_dir);
    if std::fs::read_to_string(&path).ok().and_then(|s| Placement::parse(&s)) != Some(p) {
        let _ = std::fs::write(path, p.to_line());
    }
}

struct State {
    title: Vec<u16>,
    text: Vec<u16>,
    hint: Vec<u16>,
    placement: Placement,
    /// Joystick buttons held when the screen opened (a press must be new).
    held: Vec<(u32, u32)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Shows the error until a button is pressed. Skipped when RP_NO_DIALOG is
/// set (automated runs) or a console is attached (the message went there).
pub fn show(exe_dir: &Path, title: &str, text: &str, fallback_rot: Option<i32>) {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    if std::env::var_os("RP_NO_DIALOG").is_some() || !unsafe { GetConsoleWindow() }.is_null() {
        return;
    }
    unsafe {
        // Real pixels, so the window lands exactly on the playfield.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let placement = std::fs::read_to_string(state_file(exe_dir)).ok().and_then(|s| Placement::parse(&s)).unwrap_or_else(|| Placement {
        x: 0,
        y: 0,
        w: unsafe { GetSystemMetrics(SM_CXSCREEN) },
        h: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        rot: fallback_rot.unwrap_or(0).rem_euclid(360) / 90 * 90,
    });
    let held = (0..16).filter_map(|id| crate::input::Joysticks::read(id).map(|b| (id, b))).collect();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            title: wide(title),
            text: wide(text),
            hint: wide("Press any button to close  \u{2022}  details in rusty-popper.log next to rusty-popper.exe"),
            placement,
            held,
        })
    });
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("RustyPopperError\0");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let name = wide("Rusty Popper couldn't start\0");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            class.as_ptr(),
            name.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            placement.x,
            placement.y,
            placement.w,
            placement.h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        ShowCursor(0);
        SetForegroundWindow(hwnd);
        SetTimer(hwnd, 1, 50, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        ShowCursor(1);
    }
    STATE.with(|s| s.borrow_mut().take());
}

unsafe extern "system" fn wnd_proc(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, wp: usize, lp: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            DestroyWindow(hwnd);
            0
        }
        WM_TIMER => {
            let pressed = STATE.with(|s| {
                let mut s = s.borrow_mut();
                let Some(st) = s.as_mut() else { return false };
                let mut pressed = false;
                for (id, held) in st.held.iter_mut() {
                    let now = crate::input::Joysticks::read(*id).unwrap_or(0);
                    pressed |= now & !*held != 0;
                    // A button let go counts again next time it's pressed.
                    *held &= now;
                }
                pressed
            });
            if pressed {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// COLORREF from 0xRRGGBB.
const fn rgb(c: u32) -> u32 {
    ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff)
}

unsafe fn paint(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DrawIconEx, LoadImageW, DI_NORMAL, IMAGE_ICON, LR_DEFAULTCOLOR};
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    STATE.with(|s| {
        let s = s.borrow();
        let Some(st) = s.as_ref() else { return };
        let p = st.placement;
        let bg = CreateSolidBrush(rgb(0x0d1014));
        FillRect(dc, &RECT { left: 0, top: 0, right: p.w, bottom: p.h }, bg);
        DeleteObject(bg);

        // Turn the drawing to face the player, like render::face_player.
        let (lw, lh) = if p.rot % 180 == 0 { (p.w, p.h) } else { (p.h, p.w) };
        SetGraphicsMode(dc, GM_ADVANCED);
        let a = (p.rot as f32).to_radians();
        let (sin, cos) = a.sin_cos();
        let (cx, cy) = (p.w as f32 / 2.0, p.h as f32 / 2.0);
        let (hx, hy) = (lw as f32 / 2.0, lh as f32 / 2.0);
        let xf = XFORM { eM11: cos, eM12: sin, eM21: -sin, eM22: cos, eDx: cx - (hx * cos - hy * sin), eDy: cy - (hx * sin + hy * cos) };
        SetWorldTransform(dc, &xf);
        SetBkMode(dc, TRANSPARENT as _);

        // Sizes scale with the short side, so it reads the same on any screen.
        let unit = lw.min(lh) as f32 / 1080.0;
        let px = |v: f32| (v * unit).round() as i32;
        let face = wide("Segoe UI\0");
        let font = |size: f32, weight: i32| {
            CreateFontW(-px(size), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _, CLEARTYPE_QUALITY as _, (DEFAULT_PITCH | FF_SWISS) as _, face.as_ptr())
        };
        let (title_font, text_font, hint_font) = (font(60.0, 700), font(34.0, 400), font(24.0, 400));
        let margin = px(90.0);
        let width = lw - 2 * margin;
        let flags = DT_CENTER | DT_WORDBREAK | DT_NOPREFIX;
        // How tall the wrapped message is, to centre the whole block.
        let measure = |font: HFONT, text: &[u16]| {
            SelectObject(dc, font);
            let mut r = RECT { left: 0, top: 0, right: width, bottom: 0 };
            DrawTextW(dc, text.as_ptr() as *mut u16, text.len() as i32, &mut r, flags | DT_CALCRECT);
            r.bottom
        };
        let icon_size = px(220.0);
        let (title_h, text_h) = (measure(title_font, &st.title), measure(text_font, &st.text));
        let gap = px(48.0);
        let total = icon_size + gap + title_h + px(28.0) + px(6.0) + px(36.0) + text_h;
        let mut y = ((lh - total) / 2).max(margin);

        let icon = LoadImageW(GetModuleHandleW(std::ptr::null()), std::ptr::without_provenance::<u16>(1), IMAGE_ICON, icon_size, icon_size, LR_DEFAULTCOLOR);
        if !icon.is_null() {
            DrawIconEx(dc, (lw - icon_size) / 2, y, icon, icon_size, icon_size, 0, std::ptr::null_mut(), DI_NORMAL);
        }
        y += icon_size + gap;

        let draw = |font: HFONT, color: u32, text: &[u16], y: i32| {
            SelectObject(dc, font);
            SetTextColor(dc, rgb(color));
            let mut r = RECT { left: margin, top: y, right: margin + width, bottom: lh };
            DrawTextW(dc, text.as_ptr() as *mut u16, text.len() as i32, &mut r, flags);
        };
        draw(title_font, 0xf2f4f8, &st.title, y);
        y += title_h + px(28.0);
        // A short copper rule under the title, the icon's colour.
        let copper = CreateSolidBrush(rgb(0xc8743a));
        FillRect(dc, &RECT { left: (lw - px(160.0)) / 2, top: y, right: (lw + px(160.0)) / 2, bottom: y + px(6.0) }, copper);
        DeleteObject(copper);
        y += px(6.0) + px(36.0);
        draw(text_font, 0xc9ced8, &st.text, y);
        draw(hint_font, 0x7d8594, &st.hint, lh - margin - px(30.0));

        for f in [title_font, text_font, hint_font] {
            DeleteObject(f);
        }
    });
    EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trips_and_rejects_junk() {
        let p = Placement { x: 0, y: 0, w: 3840, h: 2160, rot: 270 };
        assert_eq!(Placement::parse(&p.to_line()), Some(p));
        assert_eq!(Placement::parse("1 2 3 4 -90").map(|p| p.rot), Some(270));
        assert_eq!(Placement::parse("1 2 0 4 0"), None);
        assert_eq!(Placement::parse("1 2 3"), None);
        assert_eq!(Placement::parse("a b c d e"), None);
    }
}
