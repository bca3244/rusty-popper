//! Offscreen video targets and the cover-flow slide transition.
//!
//! Each screen's video is rendered by mpv into a texture (via an FBO) instead
//! of straight to the window, so it can be moved and composited. On a game
//! change in cover-flow mode the outgoing frame is copied to a second texture
//! and slid out while the new video slides in.

use anyhow::{anyhow, Result};
use femtovg::{renderer::OpenGl, Canvas, ImageFlags, ImageId, Paint, Path, PixelFormat};
use glow::HasContext;
use std::time::Instant;

/// How long a slide takes.
pub const SLIDE_SECS: f32 = 0.45;

pub struct Target {
    pub live: ImageId,
    live_fbo: glow::NativeFramebuffer,
    snap: ImageId,
    snap_fbo: glow::NativeFramebuffer,
    w: i32,
    h: i32,
}

pub struct Slide {
    /// +1: new video comes in from the player's right (moving to the next game).
    dir: f32,
    started: Instant,
    /// The new video has produced a frame (until then its side stays black).
    ready: bool,
}

fn fbo_for(gl: &glow::Context, canvas: &Canvas<OpenGl>, img: ImageId) -> Result<glow::NativeFramebuffer> {
    let tex = canvas.get_native_texture(img).map_err(|e| anyhow!("video texture: {e:?}"))?;
    unsafe {
        let fbo = gl.create_framebuffer().map_err(|e| anyhow!("create framebuffer: {e}"))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            return Err(anyhow!("video framebuffer incomplete (0x{status:x})"));
        }
        Ok(fbo)
    }
}

impl Target {
    pub fn new(gl: &glow::Context, canvas: &mut Canvas<OpenGl>, w: i32, h: i32) -> Result<Self> {
        let mk = |c: &mut Canvas<OpenGl>| {
            c.create_image_empty(w as usize, h as usize, PixelFormat::Rgba8, ImageFlags::empty())
                .map_err(|e| anyhow!("video image: {e:?}"))
        };
        let live = mk(canvas)?;
        let snap = mk(canvas)?;
        Ok(Target { live_fbo: fbo_for(gl, canvas, live)?, snap_fbo: fbo_for(gl, canvas, snap)?, live, snap, w, h })
    }

    pub fn fbo_id(&self) -> i32 {
        self.live_fbo.0.get() as i32
    }

    /// Copies the current frame to the snapshot texture.
    pub fn freeze(&self, gl: &glow::Context) {
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.live_fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.snap_fbo));
            gl.blit_framebuffer(0, 0, self.w, self.h, 0, 0, self.w, self.h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.live_fbo);
            gl.delete_framebuffer(self.snap_fbo);
        }
    }
}

impl Slide {
    /// Starts moving at once, in step with the wheel.
    pub fn new(dir: i64) -> Self {
        Slide { dir: if dir < 0 { -1.0 } else { 1.0 }, started: Instant::now(), ready: false }
    }

    /// The new video has its first frame: show it.
    pub fn video_ready(&mut self) {
        self.ready = true;
    }

    /// Progress 0..1. None when finished.
    fn progress(&self) -> Option<f32> {
        let p = self.started.elapsed().as_secs_f32() / SLIDE_SECS;
        (p < 1.0).then_some(p)
    }
}

/// Resets GL state femtovg leaves behind that mpv's renderer is sensitive to.
pub fn reset_for_mpv(gl: &glow::Context) {
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::STENCIL_TEST);
        gl.disable(glow::BLEND);
        gl.color_mask(true, true, true, true);
        gl.use_program(None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
    }
}

fn ease(t: f32) -> f32 {
    // ease-in-out cubic
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

fn draw_image(canvas: &mut Canvas<OpenGl>, img: ImageId, x: f32, y: f32, w: f32, h: f32) {
    let mut p = Path::new();
    p.rect(x, y, w, h);
    canvas.fill_path(&p, &Paint::image(img, x, y, w, h, 0.0, 1.0));
}

/// Draws a screen's video (device pixels, canvas already sized to `w`x`h`).
/// `axis` is the device-space direction of the player's "right" for this
/// screen. Clears `slide` once it finishes.
pub fn composite(canvas: &mut Canvas<OpenGl>, t: &Target, slide: &mut Option<Slide>, axis: (f32, f32)) {
    let (w, h) = (t.w as f32, t.h as f32);
    let Some(s) = slide.as_mut() else {
        draw_image(canvas, t.live, 0.0, 0.0, w, h);
        return;
    };
    let Some(p) = s.progress() else {
        *slide = None;
        draw_image(canvas, t.live, 0.0, 0.0, w, h);
        return;
    };
    let dist = axis.0.abs() * w + axis.1.abs() * h;
    let e = ease(p);
    let out = -s.dir * e * dist;
    let inn = s.dir * (1.0 - e) * dist;
    canvas.save();
    draw_image(canvas, t.snap, axis.0 * out, axis.1 * out, w, h);
    if s.ready {
        draw_image(canvas, t.live, axis.0 * inn, axis.1 * inn, w, h);
    }
    canvas.restore();
}
