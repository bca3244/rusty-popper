//! Minimal libmpv binding, loaded at runtime from libmpv-2.dll so no import
//! library is needed at build time. Uses the render API: mpv draws each
//! screen's video into our OpenGL framebuffer, and we draw the UI on top.

use anyhow::{anyhow, bail, Result};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::Arc;

type MpvHandle = c_void;
type RenderCtx = c_void;

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct OpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

const RENDER_PARAM_INVALID: c_int = 0;
const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const RENDER_PARAM_OPENGL_FBO: c_int = 3;
const RENDER_PARAM_FLIP_Y: c_int = 4;
/// Off: mpv must not sleep inside render() waiting for a frame's display time;
/// we pace frames ourselves with vsync and drive several render contexts.
const RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
/// Bit returned by mpv_render_context_update when a new video frame is waiting.
const RENDER_UPDATE_FRAME: u64 = 1;

pub const EVENT_SHUTDOWN: i32 = 1;
/// The reply to a `command_async`, carrying its tag.
pub const EVENT_COMMAND_REPLY: i32 = 5;
/// A newly loaded file is open (its first frame follows with PLAYBACK_RESTART).
pub const EVENT_FILE_LOADED: i32 = 8;
/// A file ended or was stopped.
pub const EVENT_END_FILE: i32 = 7;
/// The video's size or format changed (or video was turned on or off).
pub const EVENT_VIDEO_RECONFIG: i32 = 17;
/// Playback (re)started after loading or seeking: the new file has frames.
pub const EVENT_PLAYBACK_RESTART: i32 = 21;

struct Api {
    _lib: Library,
    create: unsafe extern "C" fn() -> *mut MpvHandle,
    set_option_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    initialize: unsafe extern "C" fn(*mut MpvHandle) -> c_int,
    command: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int,
    command_async: unsafe extern "C" fn(*mut MpvHandle, u64, *const *const c_char) -> c_int,
    get_property_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    wait_event: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut MpvEvent,
    terminate_destroy: unsafe extern "C" fn(*mut MpvHandle),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    render_create: unsafe extern "C" fn(*mut *mut RenderCtx, *mut MpvHandle, *mut RenderParam) -> c_int,
    render_render: unsafe extern "C" fn(*mut RenderCtx, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(*mut RenderCtx),
    render_update: unsafe extern "C" fn(*mut RenderCtx) -> u64,
}

#[derive(Clone)]
pub struct Mpv(Arc<Api>);

impl Mpv {
    pub fn load(dll: &Path) -> Result<Self> {
        unsafe {
            let lib = Library::new(dll).map_err(|e| anyhow!("loading {}: {e}", dll.display()))?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes())
                        .map_err(|e| anyhow!("{}: {e}", $name))?
                };
            }
            let api = Api {
                create: sym!("mpv_create"),
                set_option_string: sym!("mpv_set_option_string"),
                initialize: sym!("mpv_initialize"),
                command: sym!("mpv_command"),
                command_async: sym!("mpv_command_async"),
                get_property_string: sym!("mpv_get_property_string"),
                free: sym!("mpv_free"),
                wait_event: sym!("mpv_wait_event"),
                terminate_destroy: sym!("mpv_terminate_destroy"),
                error_string: sym!("mpv_error_string"),
                render_create: sym!("mpv_render_context_create"),
                render_render: sym!("mpv_render_context_render"),
                render_free: sym!("mpv_render_context_free"),
                render_update: sym!("mpv_render_context_update"),
                _lib: lib,
            };
            Ok(Mpv(Arc::new(api)))
        }
    }

    fn err(&self, code: c_int) -> String {
        unsafe { CStr::from_ptr((self.0.error_string)(code)).to_string_lossy().into_owned() }
    }

    /// Creates a player that renders through the render API (`vo=libmpv`).
    /// `options` must all be accepted; `optional` ones are skipped (logged) if
    /// this libmpv build doesn't know them.
    pub fn player(&self, options: &[(&str, String)], optional: &[(&str, &str)]) -> Result<Player> {
        let h = unsafe { (self.0.create)() };
        if h.is_null() {
            bail!("mpv_create failed");
        }
        let p = Player { api: self.clone(), h, render: std::ptr::null_mut(), last_tag: std::cell::Cell::new(0) };
        p.set_option("vo", "libmpv")?;
        for (k, v) in options {
            p.set_option(k, v)?;
        }
        for (k, v) in optional {
            if let Err(e) = p.set_option(k, v) {
                log::info!("Skipping {e:#}");
            }
        }
        let rc = unsafe { (self.0.initialize)(h) };
        if rc < 0 {
            bail!("mpv_initialize: {}", self.err(rc));
        }
        Ok(p)
    }
}

/// Resolves GL function pointers for mpv.
pub type GetProcAddress = dyn Fn(&CStr) -> *const c_void;

/// The resolver as handed to mpv: mpv keeps a pointer to the inner box, which
/// lives on the heap, so it stays valid when the `Loader` itself is moved.
/// Must stay alive as long as any render context created with it.
pub type Loader = Box<Box<GetProcAddress>>;

unsafe extern "C" fn get_proc_trampoline(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let f = unsafe { &*(ctx as *const Box<GetProcAddress>) };
    f(unsafe { CStr::from_ptr(name) }) as *mut c_void
}

pub struct Player {
    api: Mpv,
    h: *mut MpvHandle,
    render: *mut RenderCtx,
    /// The tag given to the last `show`, so its reply can be recognised.
    last_tag: std::cell::Cell<u64>,
}

impl Player {
    fn set_option(&self, k: &str, v: &str) -> Result<()> {
        let (ck, cv) = (CString::new(k)?, CString::new(v)?);
        let rc = unsafe { (self.api.0.set_option_string)(self.h, ck.as_ptr(), cv.as_ptr()) };
        if rc < 0 {
            bail!("mpv option {k}={v}: {}", self.api.err(rc));
        }
        Ok(())
    }

    /// Attaches an OpenGL render context. The GL context must be current.
    /// `loader` must outlive the player.
    pub fn init_gl(&mut self, loader: &Loader) -> Result<()> {
        // The trampoline needs the (thin) address of the inner box itself.
        #[allow(clippy::borrowed_box)]
        let inner: &Box<GetProcAddress> = loader;
        let mut init = OpenGlInitParams {
            get_proc_address: get_proc_trampoline,
            get_proc_address_ctx: inner as *const Box<GetProcAddress> as *mut c_void,
        };
        let api_type = CString::new("opengl")?;
        let mut params = [
            RenderParam { kind: RENDER_PARAM_API_TYPE, data: api_type.as_ptr() as *mut c_void },
            RenderParam { kind: RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
            RenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let rc = unsafe { (self.api.0.render_create)(&mut self.render, self.h, params.as_mut_ptr()) };
        if rc < 0 {
            bail!("mpv_render_context_create: {}", self.api.err(rc));
        }
        Ok(())
    }

    /// Draws the current video frame into framebuffer 0. The GL context for
    /// this screen must be current.
    pub fn render(&self, w: i32, h: i32) {
        self.render_to(0, w, h);
    }

    /// Draws the current video frame into framebuffer object `fbo` (0 = the window).
    pub fn render_to(&self, fbo: i32, w: i32, h: i32) {
        if self.render.is_null() {
            return;
        }
        // The window (fbo 0) has a bottom-left origin, so mpv flips for it; a
        // texture we sample with femtovg wants the unflipped image.
        let mut flip: c_int = if fbo == 0 { 1 } else { 0 };
        let mut block: c_int = 0;
        let mut fbo = OpenGlFbo { fbo, w, h, internal_format: 0 };
        let mut params = [
            RenderParam { kind: RENDER_PARAM_OPENGL_FBO, data: &mut fbo as *mut _ as *mut c_void },
            RenderParam { kind: RENDER_PARAM_FLIP_Y, data: &mut flip as *mut _ as *mut c_void },
            RenderParam { kind: RENDER_PARAM_BLOCK_FOR_TARGET_TIME, data: &mut block as *mut _ as *mut c_void },
            RenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        unsafe { (self.api.0.render_render)(self.render, params.as_mut_ptr()) };
    }

    /// True when mpv has a new video frame to draw since the last render.
    /// Must be called before rendering; it also lets mpv process its updates.
    pub fn has_new_frame(&self) -> bool {
        if self.render.is_null() {
            return false;
        }
        unsafe { (self.api.0.render_update)(self.render) & RENDER_UPDATE_FRAME != 0 }
    }

    /// Reads a property as a string (None if unavailable).
    pub fn get_string(&self, name: &str) -> Option<String> {
        let c = CString::new(name).ok()?;
        let p = unsafe { (self.api.0.get_property_string)(self.h, c.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() };
        unsafe { (self.api.0.free)(p as *mut c_void) };
        Some(s)
    }

    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a)).collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe { (self.api.0.command)(self.h, ptrs.as_ptr()) };
        if rc < 0 {
            bail!("mpv command {args:?}: {}", self.api.err(rc));
        }
        Ok(())
    }

    /// Queues a command without waiting for mpv's core thread to process it.
    /// Failures arrive later as events.
    pub fn command_async(&self, args: &[&str]) -> Result<()> {
        self.command_tagged(0, args)
    }

    /// `command_async` whose reply event carries `tag`.
    fn command_tagged(&self, tag: u64, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a)).collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe { (self.api.0.command_async)(self.h, tag, ptrs.as_ptr()) };
        if rc < 0 {
            bail!("mpv command {args:?}: {}", self.api.err(rc));
        }
        Ok(())
    }

    pub fn set_pause(&self, paused: bool) -> Result<()> {
        self.command(&["set", "pause", if paused { "yes" } else { "no" }])
    }

    /// Loads a file in a loop, or stops (black) when there is none, without
    /// waiting. Returns the tag its `EVENT_COMMAND_REPLY` will carry: events
    /// before that reply still belong to the previous file.
    pub fn show(&self, media: Option<&Path>) -> Result<u64> {
        let tag = self.last_tag.get() + 1;
        self.last_tag.set(tag);
        match media {
            Some(m) => self.command_tagged(tag, &["loadfile", &m.to_string_lossy()])?,
            None => self.command_tagged(tag, &["stop"])?,
        }
        Ok(tag)
    }

    /// Drains pending events without blocking; returns (event id, reply tag) pairs.
    pub fn poll_events(&self) -> Vec<(i32, u64)> {
        let mut ids = Vec::new();
        loop {
            let ev = unsafe { &*(self.api.0.wait_event)(self.h, 0.0) };
            if ev.event_id == 0 {
                break;
            }
            ids.push((ev.event_id, ev.reply_userdata));
            if ev.event_id == EVENT_SHUTDOWN {
                break;
            }
        }
        ids
    }

    /// Frees the render context. The GL context must be current.
    pub fn free_gl(&mut self) {
        if !self.render.is_null() {
            unsafe { (self.api.0.render_free)(self.render) };
            self.render = std::ptr::null_mut();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // free_gl() should have been called with the context current; if not,
        // mpv still requires the render context to be gone before destroy.
        self.free_gl();
        unsafe { (self.api.0.terminate_destroy)(self.h) };
    }
}
