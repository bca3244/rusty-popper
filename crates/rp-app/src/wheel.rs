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
    /// Every game's entry; `order` picks and orders the ones in the current playlist.
    pub entries: &'a [Entry],
    pub order: &'a [usize],
    /// Current playlist name, shown above the counter.
    pub playlist: &'a str,
    pub fonts: &'a Fonts,
    /// 0 = black, 1 = video fully visible (fade after a media swap).
    pub video_fade: f32,
    /// Letter jump indicator: the letter and seconds since the jump.
    pub letter: Option<(char, f32)>,
}

impl Frame<'_> {
    /// Entry at wheel position `pos` (index into the playlist).
    fn entry(&self, pos: usize) -> &Entry {
        &self.entries[self.order[pos]]
    }
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

    if !f.order.is_empty() {
        match f.style {
            WheelStyle::Arc => draw_arc(cv, vw, dh, f),
            WheelStyle::Flow => draw_flow(cv, vw, dh, f),
        }
        draw_info(cv, vw, f);
    } else {
        let paint = Paint::color(Color::rgbaf(0.9, 0.92, 0.96, 0.8))
            .with_font(&[f.fonts.display])
            .with_font_size(56.0)
            .with_text_align(Align::Center)
            .with_text_baseline(Baseline::Middle);
        let _ = cv.fill_text(vw / 2.0, dh / 2.0, format!("{} is empty", f.playlist), &paint);
    }
    if let Some((letter, t)) = f.letter {
        draw_letter(cv, vw, dh, letter, t, f.fonts);
    }
    cv.restore();
}

/// Big letter that pops up and fades after a jump-by-letter.
fn draw_letter(cv: &mut Cv, vw: f32, dh: f32, letter: char, t: f32, fonts: &Fonts) {
    const SHOW: f32 = 0.9;
    if t > SHOW {
        return;
    }
    let a = (1.0 - t / SHOW).powf(0.6);
    let sc = 0.85 + 0.15 * ease_out(t / 0.15);
    let (cx, cy) = (vw / 2.0, dh * 0.45);
    let mut p = Path::new();
    p.rounded_rect(cx - 150.0 * sc, cy - 150.0 * sc, 300.0 * sc, 300.0 * sc, 48.0);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.03, 0.04, 0.07, 0.75 * a)));
    cv.stroke_path(&p, &Paint::color(Color::rgbaf(0.325, 0.827, 1.0, 0.8 * a)).with_line_width(4.0));
    let paint = Paint::color(Color::rgbaf(0.96, 0.97, 0.99, a))
        .with_font(&[fonts.display])
        .with_font_size(220.0 * sc)
        .with_text_align(Align::Center)
        .with_text_baseline(Baseline::Middle);
    let _ = cv.fill_text(cx, cy + 8.0, letter.to_string(), &paint);
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
    // A narrow (portrait) screen packs the arc tighter and the logos smaller
    // so two neighbors fit on each side of the selection.
    let narrow = vw < DESIGN_W * 0.75;
    let (step, base_w, base_h) = if narrow { (0.145f32, 180.0f32, 150.0f32) } else { (0.205, 240.0, 190.0) };
    let (r, cx, sy) = (1500.0f32, vw / 2.0, dh - 240.0);
    let cy = sy + r;
    let sel = f.entry(f.wheel.selected());

    for (rad, a0, alpha, lw) in [(r, 0.95f32, 0.10f32, 3.0f32), (r + 190.0, 0.8, 0.05, 2.0)] {
        let mut p = Path::new();
        p.arc(cx, cy, rad, -PI / 2.0 - a0, -PI / 2.0 + a0, femtovg::Solidity::Hole);
        cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, alpha)).with_line_width(lw));
    }
    let pos = f.wheel.pos;
    let near = 1.0 - clamp((pos - pos.round()).abs() * 2.0, 0.0, 1.0);
    slot_glow(cv, cx, sy, 360.0, 200.0, entry_hue(sel), 0.6 + 0.4 * near);

    for (i, d) in visible_items(pos, f.order.len(), 4.6) {
        let e = f.entry(i);
        let ang = d * step;
        let ad = d.abs();
        let (x, y) = (cx + r * ang.sin(), cy - r * ang.cos());
        let sc = 1.0 + 0.62 * (1.0 - ad).max(0.0);
        let aspect = e.logo.as_ref().map_or(1.6, |l| l.aspect);
        let (w, h) = fit(aspect, base_w * sc, base_h * sc);
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
    let sel = f.entry(f.wheel.selected());
    fill_rect(cv, 0.0, y0 + 182.0, vw, 2.0, Color::rgbaf(1.0, 1.0, 1.0, 0.06));
    slot_glow(cv, cx, y0, 420.0, 230.0, entry_hue(sel), 0.9);

    // Portrait shows three covers each side, landscape five.
    // The outermost cover fades in/out as the shelf moves instead of popping.
    let shown = if vw < DESIGN_W * 0.75 { 3.0 } else { 5.0 };
    for (i, d) in visible_items(f.wheel.pos, f.order.len(), shown + 1.0) {
        let e = f.entry(i);
        let a = d.abs();
        let sgn = d.signum();
        let a1 = a.min(1.0);
        let squeeze = (vw / DESIGN_W).clamp(0.6, 1.0);
        let off = (a1 * 400.0 + (a - 1.0).max(0.0) * 135.0) * sgn * squeeze;
        let theta = -sgn * a1 * 1.2;
        let aspect = e.logo.as_ref().map_or(1.6, |l| l.aspect);
        let (w, hgt) = fit(aspect, 520.0, 330.0);
        let alpha = clamp(1.3 - a * 0.2, 0.0, 1.0) * clamp(shown + 1.0 - a, 0.0, 1.0);
        if alpha <= 0.0 {
            continue;
        }

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

/// Splits "Name (Maker Year)" into the name and its parenthesised suffix.
fn split_suffix(t: &str) -> (String, Option<String>) {
    let t = t.trim();
    if t.ends_with(')') {
        if let Some(i) = t.rfind(" (") {
            if i > 0 {
                return (t[..i].trim_end().to_string(), Some(t[i + 1..].to_string()));
            }
        }
    }
    (t.to_string(), None)
}

/// Splits text into two lines at the space nearest the middle.
fn split_middle(t: &str) -> (String, String) {
    let mid = t.len() / 2;
    let best = t.match_indices(' ').map(|(i, _)| i).min_by_key(|i| (*i as i64 - mid as i64).abs());
    match best {
        Some(i) => (t[..i].to_string(), t[i + 1..].to_string()),
        None => (t.to_string(), String::new()),
    }
}
fn draw_info(cv: &mut Cv, vw: f32, f: &Frame) {
    let idx = f.wheel.selected();
    let e = f.entry(idx);
    let hue = entry_hue(e);
    let a = ease_out(f.wheel.info_t / INFO_FADE);
    let off = (1.0 - a) * 28.0;
    let (x, y) = (96.0f32, 120.0f32);

    let text = |font: FontId, size: f32, c: Color, align: Align| {
        Paint::color(c).with_font(&[font]).with_font_size(size).with_text_align(align).with_text_baseline(Baseline::Alphabetic)
    };
    // The small maker/year line only when the title doesn't already end with it.
    if !e.subtitle.is_empty() && split_suffix(&e.title).1.is_none() {
        let _ = cv.fill_text(x + off, y, &e.subtitle, &text(f.fonts.mono, 26.0, hue_color(hue, 0.9, 0.72, a), Align::Left));
    }
    // Title: the game name, then its "(Maker Year)" on a smaller second line.
    // A long name shrinks to fit, then wraps onto two lines itself.
    let max_w = vw - 2.0 * x;
    let probe = text(f.fonts.display, 92.0, Color::rgbaf(1.0, 1.0, 1.0, 1.0), Align::Left);
    let width_at_92 = |cv: &Cv, s: &str| cv.measure_text(0.0, 0.0, s, &probe).map(|m| m.width()).unwrap_or(0.0);
    let (name, suffix) = split_suffix(&e.title);
    let tw = width_at_92(cv, &name).max(1.0);
    let (name_lines, size): (Vec<String>, f32) = if tw <= max_w {
        (vec![name.clone()], 92.0)
    } else if 92.0 * max_w / tw >= 64.0 {
        (vec![name.clone()], 92.0 * max_w / tw)
    } else {
        let (l1, l2) = split_middle(&name);
        let widest = width_at_92(cv, &l1).max(width_at_92(cv, &l2)).max(1.0);
        (vec![l1, l2], (92.0 * max_w / widest).min(80.0))
    };
    let line_h = size * 1.05;
    let mut ty = y + size; // first baseline sits one title line below the subtitle
    let tx = x + off * 1.6;
    for line in &name_lines {
        let shadow = text(f.fonts.display, size, Color::rgbaf(0.0, 0.0, 0.0, 0.45 * a), Align::Left);
        let _ = cv.fill_text(tx + 3.0, ty + 3.0, line, &shadow);
        let _ = cv.fill_text(tx, ty, line, &text(f.fonts.display, size, Color::rgbaf(0.957, 0.965, 0.984, a), Align::Left));
        ty += line_h;
    }
    let mut last_baseline = ty - line_h;
    if let Some(suffix) = &suffix {
        let small = (size * 0.6).max(40.0);
        let sw = width_at_92(cv, suffix) * small / 92.0;
        let small = if sw > max_w { small * max_w / sw } else { small };
        last_baseline += small * 1.2;
        let shadow = text(f.fonts.display, small, Color::rgbaf(0.0, 0.0, 0.0, 0.45 * a), Align::Left);
        let _ = cv.fill_text(tx + 2.0, last_baseline + 2.0, suffix, &shadow);
        let _ = cv.fill_text(tx, last_baseline, suffix, &text(f.fonts.display, small, Color::rgbaf(0.86, 0.88, 0.93, 0.85 * a), Align::Left));
    }

    // Tag chips below the title, wrapping onto more rows as needed.
    let mut chip_top = last_baseline + 28.0;
    let left = x + off * 2.0;
    let mut chip_x = left;
    for chip in &e.chips {
        let paint = text(f.fonts.body, 24.0, Color::rgbaf(0.875, 0.894, 0.933, a), Align::Left);
        let tw = cv.measure_text(0.0, 0.0, chip, &paint).map(|m| m.width()).unwrap_or(100.0);
        let w = tw + 32.0;
        if chip_x > left && chip_x + w > x + max_w {
            chip_x = left;
            chip_top += 54.0;
        }
        let mut p = Path::new();
        p.rounded_rect(chip_x, chip_top, w, 42.0, 21.0);
        cv.fill_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.08 * a)));
        cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.18 * a)).with_line_width(1.5));
        let _ = cv.fill_text(chip_x + 16.0, chip_top + 29.0, chip, &paint);
        chip_x += w + 12.0;
    }    let _ = cv.fill_text(vw - 96.0, y - 40.0, f.playlist.to_uppercase(), &text(f.fonts.mono, 24.0, hue_color(195.0, 0.9, 0.72, 0.9), Align::Right));
    let counter = format!("{:02} / {}", idx + 1, f.order.len());
    let _ = cv.fill_text(vw - 96.0, y, &counter, &text(f.fonts.mono, 28.0, Color::rgbaf(0.9, 0.92, 0.96, 0.7), Align::Right));
}

/// The exit menu as drawn: options, which is selected, and seconds since it opened.
pub struct MenuView<'a> {
    pub title: &'a str,
    pub options: &'a [&'a str],
    pub selected: usize,
    pub t: f32,
}

/// Draws the exit menu centered over the playfield (same frame as `draw`).
pub fn draw_menu(cv: &mut Cv, w: f32, h: f32, m: &MenuView, fonts: &Fonts) {
    let s = (h / DESIGN_H).min(w / DESIGN_H);
    let (vw, dh) = (w / s, h / s);
    let a = ease_out(m.t / 0.2);
    cv.save();
    cv.scale(s, s);
    fill_rect(cv, 0.0, 0.0, vw, dh, Color::rgbaf(0.02, 0.025, 0.04, 0.72 * a));

    let row_h = 96.0;
    let pw = (vw - 160.0).min(820.0);
    let ph = 150.0 + m.options.len() as f32 * row_h + 40.0;
    let (px, py) = ((vw - pw) / 2.0, (dh - ph) / 2.0 + (1.0 - a) * 40.0);
    let mut p = Path::new();
    p.rounded_rect(px, py, pw, ph, 28.0);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.07, 0.086, 0.13, 0.96 * a)));
    cv.stroke_path(&p, &Paint::color(Color::rgbaf(0.13, 0.16, 0.23, a)).with_line_width(2.0));

    let title = Paint::color(Color::rgbaf(0.9, 0.92, 0.96, a))
        .with_font(&[fonts.display])
        .with_font_size(64.0)
        .with_text_align(Align::Center)
        .with_text_baseline(Baseline::Alphabetic);
    let _ = cv.fill_text(vw / 2.0, py + 104.0, m.title, &title);

    let accent = Color::rgbaf(0.325, 0.827, 1.0, a);
    for (i, opt) in m.options.iter().enumerate() {
        let y = py + 150.0 + i as f32 * row_h;
        let sel = i == m.selected;
        if sel {
            let mut p = Path::new();
            p.rounded_rect(px + 32.0, y + 8.0, pw - 64.0, row_h - 16.0, (row_h - 16.0) / 2.0);
            cv.fill_path(&p, &Paint::color(accent));
        }
        let c = if sel { Color::rgbaf(0.04, 0.047, 0.07, a) } else { Color::rgbaf(0.88, 0.9, 0.95, 0.85 * a) };
        let paint = Paint::color(c)
            .with_font(&[fonts.body])
            .with_font_size(42.0)
            .with_text_align(Align::Center)
            .with_text_baseline(Baseline::Middle);
        let _ = cv.fill_text(vw / 2.0, y + row_h / 2.0, opt, &paint);
    }
    cv.restore();
}

/// "Launching <game>" / "Exiting <game>" screen shown on the playfield while the emulator starts or closes
/// (and left behind the game window, so the desktop never shows).
pub fn draw_launch(cv: &mut Cv, w: f32, h: f32, e: &Entry, verb: &str, emulator: &str, t: f32, fonts: &Fonts) {
    let s = (h / DESIGN_H).min(w / DESIGN_H);
    let (vw, dh) = (w / s, h / s);
    cv.save();
    cv.scale(s, s);
    fill_rect(cv, 0.0, 0.0, vw, dh, Color::rgbf(0.0, 0.0, 0.0));
    let a = ease_out(t / 0.3);
    let hue = entry_hue(e);
    let cy = dh * 0.42;
    slot_glow(cv, vw / 2.0, cy, 520.0, 300.0, hue, a);
    let sc = 0.92 + 0.08 * ease_out(t / 0.5);
    let aspect = e.logo.as_ref().map_or(1.6, |l| l.aspect);
    let (lw, lh) = fit(aspect, 760.0 * sc, 520.0 * sc);
    cv.save();
    cv.translate(vw / 2.0, cy);
    draw_logo(cv, e, lw, lh, a, a, fonts);
    cv.restore();

    let spin_y = cy + 520.0 * 0.5 + 120.0;
    let rot = t * 5.5;
    let mut p = Path::new();
    p.arc(vw / 2.0, spin_y, 34.0, rot, rot + PI * 1.4, femtovg::Solidity::Hole);
    let mut paint = Paint::color(hue_color(hue, 1.0, 0.66, a)).with_line_width(6.0);
    paint.set_line_cap(femtovg::LineCap::Round);
    cv.stroke_path(&p, &paint);

    let title_size = 46.0f32.min((vw - 120.0) / (e.title.chars().count().max(1) as f32 + 10.0) * 1.9);
    let text = |font: FontId, size: f32, c: Color| {
        Paint::color(c).with_font(&[font]).with_font_size(size).with_text_align(Align::Center).with_text_baseline(Baseline::Alphabetic)
    };
    let _ = cv.fill_text(vw / 2.0, spin_y + 110.0, format!("{verb} {}", e.title), &text(fonts.display, title_size, Color::rgbaf(0.93, 0.945, 0.97, a)));
    let _ = cv.fill_text(vw / 2.0, spin_y + 156.0, emulator.to_uppercase(), &text(fonts.mono, 24.0, Color::rgbaf(0.9, 0.92, 0.96, 0.6 * a)));
    cv.restore();
}

/// Clears whatever surface is current to black (secondary screens during a game).
pub fn clear_black(cv: &mut Cv, w: u32, h: u32) {
    cv.set_size(w, h, 1.0);
    cv.clear_rect(0, 0, w, h, Color::rgbf(0.0, 0.0, 0.0));
    cv.flush();
}