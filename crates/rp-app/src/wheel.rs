//! Game wheel state and drawing (arc wheel and cover flow), drawn with
//! femtovg over the playfield video. Layout numbers are in a 1080-px-tall
//! design space (matching the Wheel Style Lab mockup) and scaled to the screen.

use crate::assets::{GLOW_INNER, GLOW_SIZE};
use crate::settings::WheelStyle;
use femtovg::{renderer::OpenGl, Align, Baseline, Canvas, Color, FontId, ImageId, Paint, Path};
use std::f32::consts::PI;

pub const DESIGN_H: f32 = 1080.0;
pub const DESIGN_W: f32 = 1920.0;
/// Delay after the selection settles before media on the screens is swapped.
pub const MEDIA_DEBOUNCE: f32 = 0.16;
/// Fade-in time of the new playfield video after a swap.
pub const MEDIA_FADE: f32 = 0.45;
const SPRING: f32 = 11.0;
const INFO_FADE: f32 = 0.35;

pub struct Fonts {
    pub display: FontId,
    pub body: FontId,
    pub mono: FontId,
}

pub struct LogoTex {
    pub img: ImageId,
    pub glow: ImageId,
    pub aspect: f32, // width / height
    pub hue: f32,
}

pub struct Entry {
    pub title: String,
    pub subtitle: String, // "MAKER  ·  YEAR"
    pub chips: Vec<String>,
    pub logo: Option<LogoTex>,
}

pub struct Wheel {
    pub target: i64,
    pub pos: f32,
    n: i64,
    pub info_t: f32,
    last_sel: usize,
    pub changed_at: f32,
}

fn modi(a: i64, n: i64) -> i64 {
    ((a % n) + n) % n
}
fn clamp(v: f32, a: f32, b: f32) -> f32 {
    v.max(a).min(b)
}
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - clamp(t, 0.0, 1.0)).powi(3)
}

impl Wheel {
    pub fn new(n: usize, start: usize) -> Self {
        Wheel { target: start as i64, pos: start as f32, n: n.max(1) as i64, info_t: 1.0, last_sel: start, changed_at: 0.0 }
    }

    pub fn selected(&self) -> usize {
        modi(self.target, self.n) as usize
    }

    pub fn step(&mut self, d: i64) {
        self.target += d;
        // Keep the spring from replaying a long spin if many steps queue up.
        self.pos = self.pos.clamp(self.target as f32 - 6.0, self.target as f32 + 6.0);
    }

    /// Advances the animation. Returns true when the selection changed.
    pub fn update(&mut self, dt: f32, now: f32) -> bool {
        let k = 1.0 - (-dt * SPRING).exp();
        self.pos += (self.target as f32 - self.pos) * k;
        if (self.target as f32 - self.pos).abs() < 0.0005 {
            self.pos = self.target as f32;
        }
        self.info_t += dt;
        let s = self.selected();
        if s != self.last_sel {
            self.last_sel = s;
            self.info_t = 0.0;
            self.changed_at = now;
            return true;
        }
        false
    }
}

/// Everything the wheel needs to draw one frame.
pub struct Frame<'a> {
    pub style: WheelStyle,
    pub wheel: &'a Wheel,
    pub entries: &'a [Entry],
    pub fonts: &'a Fonts,
    /// 0 = black, 1 = video fully visible (fade after a media swap).
    pub video_fade: f32,
}

type Cv = Canvas<OpenGl>;

/// Draws the overlay for a `w`x`h` px screen. The canvas must already be sized.
pub fn draw(cv: &mut Cv, w: f32, h: f32, f: &Frame) {
    // The design is 1080 units on the short side, so a portrait playfield gets
    // the same element sizes as a landscape one (1080 wide, extra height above
    // the wheel); horizontal spacing is compressed in draw_flow.
    let s = (h / DESIGN_H).min(w / DESIGN_H);
    let (vw, dh) = (w / s, h / s);
    cv.save();
    cv.scale(s, s);

    if f.video_fade < 1.0 {
        fill_rect(cv, 0.0, 0.0, vw, dh, Color::rgbaf(0.0, 0.0, 0.0, 1.0 - ease_out(f.video_fade)));
    }
    // Darken the top for the info text and the bottom behind the wheel.
    let mut p = Path::new();
    p.rect(0.0, 0.0, vw, 450.0);
    cv.fill_path(&p, &Paint::linear_gradient(0.0, 0.0, 0.0, 450.0, Color::rgbaf(0.012, 0.016, 0.03, 0.7), Color::rgbaf(0.012, 0.016, 0.03, 0.0)));
    let mut p = Path::new();
    p.rect(0.0, dh - 430.0, vw, 430.0);
    cv.fill_path(&p, &Paint::linear_gradient(0.0, dh - 430.0, 0.0, dh, Color::rgbaf(0.016, 0.024, 0.04, 0.0), Color::rgbaf(0.016, 0.024, 0.04, 0.6)));

    if !f.entries.is_empty() {
        match f.style {
            WheelStyle::Arc => draw_arc(cv, vw, dh, f),
            WheelStyle::Flow => draw_flow(cv, vw, dh, f),
        }
        draw_info(cv, vw, f);
    }
    cv.restore();
}

fn fill_rect(cv: &mut Cv, x: f32, y: f32, w: f32, h: f32, c: Color) {
    let mut p = Path::new();
    p.rect(x, y, w, h);
    cv.fill_path(&p, &Paint::color(c));
}

fn hue_color(hue: f32, s: f32, l: f32, a: f32) -> Color {
    Color::hsla(hue / 360.0, s, l, a)
}

fn entry_hue(e: &Entry) -> f32 {
    e.logo.as_ref().map_or(195.0, |l| l.hue)
}

/// Soft elliptical glow under the selected slot.
fn slot_glow(cv: &mut Cv, x: f32, y: f32, rx: f32, ry: f32, hue: f32, a: f32) {
    cv.save();
    cv.translate(x, y);
    cv.scale(1.0, ry / rx);
    let mut p = Path::new();
    p.circle(0.0, 0.0, rx);
    cv.fill_path(&p, &Paint::radial_gradient(0.0, 0.0, 0.0, rx, hue_color(hue, 1.0, 0.6, 0.35 * a), hue_color(hue, 1.0, 0.5, 0.0)));
    cv.restore();
}

/// Size of a logo fitted into a `bw`x`bh` box.
fn fit(aspect: f32, bw: f32, bh: f32) -> (f32, f32) {
    if aspect >= bw / bh {
        (bw, bw / aspect)
    } else {
        (bh * aspect, bh)
    }
}

/// Draws a logo centered at the current origin, with an optional glow.
fn draw_logo(cv: &mut Cv, e: &Entry, w: f32, h: f32, alpha: f32, glow: f32, fonts: &Fonts) {
    match &e.logo {
        Some(l) => {
            if glow > 0.01 {
                let g = w.max(h) * GLOW_SIZE as f32 / GLOW_INNER as f32;
                let mut p = Path::new();
                p.rect(-g / 2.0, -g / 2.0, g, g);
                cv.fill_path(&p, &Paint::image(l.glow, -g / 2.0, -g / 2.0, g, g, 0.0, 0.85 * glow * alpha));
            }
            let mut p = Path::new();
            p.rect(-w / 2.0, -h / 2.0, w, h);
            cv.fill_path(&p, &Paint::image(l.img, -w / 2.0, -h / 2.0, w, h, 0.0, alpha));
        }
        None => placeholder(cv, e, w, h, alpha, fonts),
    }
}

/// Stand-in while the logo loads, or when a game has no wheel image.
fn placeholder(cv: &mut Cv, e: &Entry, w: f32, h: f32, alpha: f32, fonts: &Fonts) {
    let (pw, ph) = (w, h * 0.6);
    let mut p = Path::new();
    p.rounded_rect(-pw / 2.0, -ph / 2.0, pw, ph, ph * 0.18);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.07, 0.09, 0.13, 0.85 * alpha)));
    cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.2 * alpha)).with_line_width(2.0));
    let size = (pw / (e.title.chars().count().max(6) as f32) * 1.7).min(ph * 0.42);
    let paint = Paint::color(Color::rgbaf(0.92, 0.94, 0.98, alpha))
        .with_font(&[fonts.display])
        .with_font_size(size)
        .with_text_align(Align::Center)
        .with_text_baseline(Baseline::Middle);
    let _ = cv.fill_text(0.0, 0.0, &e.title, &paint);
}

fn visible_items(pos: f32, n: usize, range: f32) -> Vec<(usize, f32)> {
    let base = pos.round() as i64;
    let mut items: Vec<(usize, f32)> = (-7..=7)
        .map(|k| base + k)
        .map(|v| (modi(v, n as i64) as usize, v as f32 - pos))
        .filter(|(_, d)| d.abs() <= range)
        .collect();
    // With fewer games than visible slots, show each game once (nearest copy).
    items.sort_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
    let mut seen = std::collections::HashSet::new();
    items.retain(|(i, _)| seen.insert(*i));
    // Draw far items first so the selection ends up on top.
    items.reverse();
    items
}

fn draw_arc(cv: &mut Cv, vw: f32, dh: f32, f: &Frame) {
    let (r, step, cx, sy) = (1500.0f32, 0.205f32, vw / 2.0, dh - 240.0);
    let cy = sy + r;
    let sel = &f.entries[f.wheel.selected()];

    for (rad, a0, alpha, lw) in [(r, 0.95f32, 0.10f32, 3.0f32), (r + 190.0, 0.8, 0.05, 2.0)] {
        let mut p = Path::new();
        p.arc(cx, cy, rad, -PI / 2.0 - a0, -PI / 2.0 + a0, femtovg::Solidity::Hole);
        cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, alpha)).with_line_width(lw));
    }
    let pos = f.wheel.pos;
    let near = 1.0 - clamp((pos - pos.round()).abs() * 2.0, 0.0, 1.0);
    slot_glow(cv, cx, sy, 360.0, 200.0, entry_hue(sel), 0.6 + 0.4 * near);

    for (i, d) in visible_items(pos, f.entries.len(), 4.6) {
        let e = &f.entries[i];
        let ang = d * step;
        let ad = d.abs();
        let (x, y) = (cx + r * ang.sin(), cy - r * ang.cos());
        let sc = 1.0 + 0.62 * (1.0 - ad).max(0.0);
        let aspect = e.logo.as_ref().map_or(1.6, |l| l.aspect);
        let (w, h) = fit(aspect, 240.0 * sc, 190.0 * sc);
        let alpha = clamp(1.25 - ad * 0.22, 0.0, 1.0) * if ad < 0.5 { 1.0 } else { 0.82 };
        cv.save();
        cv.translate(x, y);
        cv.rotate(ang * 0.85);
        draw_logo(cv, e, w, h, alpha, (1.0 - ad * 1.6).max(0.0), f.fonts);
        cv.restore();
    }
}

fn draw_flow(cv: &mut Cv, vw: f32, dh: f32, f: &Frame) {
    let (cx, y0, focal) = (vw / 2.0, dh - 320.0, 1600.0f32);
    const STRIPS: usize = 22;
    let sel = &f.entries[f.wheel.selected()];
    fill_rect(cv, 0.0, y0 + 182.0, vw, 2.0, Color::rgbaf(1.0, 1.0, 1.0, 0.06));
    slot_glow(cv, cx, y0, 420.0, 230.0, entry_hue(sel), 0.9);

    for (i, d) in visible_items(f.wheel.pos, f.entries.len(), 5.5) {
        let e = &f.entries[i];
        let a = d.abs();
        let sgn = d.signum();
        let a1 = a.min(1.0);
        let squeeze = (vw / DESIGN_W).clamp(0.6, 1.0);
        let off = (a1 * 400.0 + (a - 1.0).max(0.0) * 135.0) * sgn * squeeze;
        let theta = -sgn * a1 * 1.2;
        let aspect = e.logo.as_ref().map_or(1.6, |l| l.aspect);
        let (w, hgt) = fit(aspect, 520.0, 330.0);
        let alpha = clamp(1.3 - a * 0.2, 0.0, 1.0);

        let Some(logo) = (if a < 0.04 { None } else { e.logo.as_ref() }) else {
            // Centered (or not yet loaded): flat logo plus reflection.
            cv.save();
            cv.translate(cx + off, y0);
            draw_logo(cv, e, w, hgt, alpha, if a < 0.04 { 1.0 } else { 0.0 }, f.fonts);
            cv.restore();
            if let Some(l) = &e.logo {
                reflection(cv, l.img, cx + off - w / 2.0, y0 + hgt / 2.0 + 4.0, w, hgt, 0.16 * alpha, None);
            }
            continue;
        };

        let dim = 1.0 - 0.4 * a1 - 0.06 * (a - 1.0).max(0.0);
        let strip_alpha = alpha * dim;
        for k in 0..STRIPS {
            let u0 = k as f32 / STRIPS as f32 - 0.5;
            let u1 = (k + 1) as f32 / STRIPS as f32 - 0.5;
            let p0 = focal / (focal + u0 * w * theta.sin());
            let p1 = focal / (focal + u1 * w * theta.sin());
            let x0 = cx + off + u0 * w * theta.cos() * p0;
            let x1 = cx + off + u1 * w * theta.cos() * p1;
            let hh = hgt * ((p0 + p1) / 2.0) * (1.0 - 0.08 * a1);
            let (dx, dw) = (x0.min(x1), (x1 - x0).abs());
            if dw < 0.05 {
                continue;
            }
            // Place the whole image so that slice k lands exactly on [dx, dx+dw].
            // (|theta| <= 1.2 rad, so x1 > x0 and slices stay in order.)
            let img_w = dw * STRIPS as f32;
            let img_x = dx - k as f32 * dw;
            let mut paint = Paint::image(logo.img, img_x, y0 - hh / 2.0, img_w, hh, 0.0, strip_alpha);
            paint.set_anti_alias(false);
            let mut p = Path::new();
            p.rect(dx, y0 - hh / 2.0, dw, hh);
            cv.fill_path(&p, &paint);
            reflection(cv, logo.img, img_x, y0 + hh / 2.0 + 4.0, img_w, hh, 0.13 * strip_alpha, Some((dx, dw)));
        }
    }
}

/// Mirrored bottom half of an image under a logo whose bottom edge is at `top`.
/// `clip_x` limits it to one strip of a turned cover.
fn reflection(cv: &mut Cv, img: ImageId, x: f32, top: f32, w: f32, h: f32, alpha: f32, clip_x: Option<(f32, f32)>) {
    cv.save();
    cv.translate(0.0, top);
    cv.scale(1.0, -1.0);
    let (rx, rw) = clip_x.unwrap_or((x, w));
    let mut paint = Paint::image(img, x, -h / 2.0, w, h, 0.0, alpha);
    paint.set_anti_alias(clip_x.is_none());
    let mut p = Path::new();
    p.rect(rx, -h / 2.0, rw, h / 2.0);
    cv.fill_path(&p, &paint);
    cv.restore();
}

fn draw_info(cv: &mut Cv, vw: f32, f: &Frame) {
    let idx = f.wheel.selected();
    let e = &f.entries[idx];
    let hue = entry_hue(e);
    let a = ease_out(f.wheel.info_t / INFO_FADE);
    let off = (1.0 - a) * 28.0;
    let (x, y) = (96.0f32, 120.0f32);

    let text = |font: FontId, size: f32, c: Color, align: Align| {
        Paint::color(c).with_font(&[font]).with_font_size(size).with_text_align(align).with_text_baseline(Baseline::Alphabetic)
    };
    if !e.subtitle.is_empty() {
        let _ = cv.fill_text(x + off, y, &e.subtitle, &text(f.fonts.mono, 26.0, hue_color(hue, 0.9, 0.72, a), Align::Left));
    }
    // Title with a soft dark shadow, shrunk to fit long names.
    let max_w = vw - 2.0 * x;
    let probe = text(f.fonts.display, 92.0, Color::rgbaf(1.0, 1.0, 1.0, 1.0), Align::Left);
    let tw = cv.measure_text(0.0, 0.0, &e.title, &probe).map(|m| m.width()).unwrap_or(0.0);
    let size = if tw > max_w { (92.0 * max_w / tw).max(40.0) } else { 92.0 };
    let title_shadow = text(f.fonts.display, size, Color::rgbaf(0.0, 0.0, 0.0, 0.45 * a), Align::Left);
    let _ = cv.fill_text(x + off * 1.6 + 3.0, y + 95.0, &e.title, &title_shadow);
    let _ = cv.fill_text(x + off * 1.6, y + 92.0, &e.title, &text(f.fonts.display, size, Color::rgbaf(0.957, 0.965, 0.984, a), Align::Left));

    let mut chip_x = x + off * 2.0;
    for chip in &e.chips {
        let paint = text(f.fonts.body, 24.0, Color::rgbaf(0.875, 0.894, 0.933, a), Align::Left);
        let tw = cv.measure_text(0.0, 0.0, chip, &paint).map(|m| m.width()).unwrap_or(100.0);
        let w = tw + 32.0;
        let mut p = Path::new();
        p.rounded_rect(chip_x, y + 120.0, w, 42.0, 21.0);
        cv.fill_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.08 * a)));
        cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.18 * a)).with_line_width(1.5));
        let _ = cv.fill_text(chip_x + 16.0, y + 149.0, chip, &paint);
        chip_x += w + 12.0;
    }
    let counter = format!("{:02} / {}", idx + 1, f.entries.len());
    let _ = cv.fill_text(vw - 96.0, y, &counter, &text(f.fonts.mono, 28.0, Color::rgbaf(0.9, 0.92, 0.96, 0.7), Align::Right));
}
