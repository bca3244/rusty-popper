//! Minimal libmpv binding, loaded at runtime from libmpv-2.dll so no import
//! library is needed at build time.

use anyhow::{anyhow, bail, Result};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::Arc;

type MpvHandle = c_void;

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

pub const EVENT_SHUTDOWN: i32 = 1;

struct Api {
    _lib: Library,
    create: unsafe extern "C" fn() -> *mut MpvHandle,
    set_option_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    initialize: unsafe extern "C" fn(*mut MpvHandle) -> c_int,
    command: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut MpvEvent,
    terminate_destroy: unsafe extern "C" fn(*mut MpvHandle),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
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
                wait_event: sym!("mpv_wait_event"),
                terminate_destroy: sym!("mpv_terminate_destroy"),
                error_string: sym!("mpv_error_string"),
                _lib: lib,
            };
            Ok(Mpv(Arc::new(api)))
        }
    }

    fn err(&self, code: c_int) -> String {
        unsafe { CStr::from_ptr((self.0.error_string)(code)).to_string_lossy().into_owned() }
    }

    pub fn player(&self, options: &[(&str, String)]) -> Result<Player> {
        let h = unsafe { (self.0.create)() };
        if h.is_null() {
            bail!("mpv_create failed");
        }
        let p = Player { api: self.clone(), h };
        for (k, v) in options {
            let (ck, cv) = (CString::new(*k)?, CString::new(v.as_str())?);
            let rc = unsafe { (self.0.set_option_string)(h, ck.as_ptr(), cv.as_ptr()) };
            if rc < 0 {
                bail!("mpv option {k}={v}: {}", self.err(rc));
            }
        }
        let rc = unsafe { (self.0.initialize)(h) };
        if rc < 0 {
            bail!("mpv_initialize: {}", self.err(rc));
        }
        Ok(p)
    }
}

pub struct Player {
    api: Mpv,
    h: *mut MpvHandle,
}

impl Player {
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

    /// Drains pending events without blocking; returns the ids seen.
    pub fn poll_events(&self) -> Vec<i32> {
        let mut ids = Vec::new();
        loop {
            let ev = unsafe { &*(self.api.0.wait_event)(self.h, 0.0) };
            if ev.event_id == 0 {
                break;
            }
            ids.push(ev.event_id);
            if ev.event_id == EVENT_SHUTDOWN {
                break;
            }
        }
        ids
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe { (self.api.0.terminate_destroy)(self.h) };
    }
}
