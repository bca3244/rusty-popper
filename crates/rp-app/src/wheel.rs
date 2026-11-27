//! Game wheel state and drawing (arc, cover flow, minimal and vertical), drawn with
//! femtovg over the playfield video. Layout numbers are in a 1080-px-tall
//! design space (matching the Wheel Style Lab mockup) and scaled to the screen.

use crate::assets::{GLOW_INNER, GLOW_SIZE};
use crate::settings::{Layout, WheelPosition, WheelStyle};
use crate::textcache::{Group, TextCache};
use femtovg::{renderer::OpenGl, Align, Baseline, Canvas, Color, FontId, ImageId, Paint, Path};
use std::cell::RefCell;
use std::f32::consts::PI;

pub const DESIGN_H: f32 = 1080.0;
pub const DESIGN_W: f32 = 1920.0;
/// How far cover flow's side covers are turned (radians).
const FLOW_TURN: f32 = 0.85;
/// Delay after the selection settles before media on the screens is swapped.
pub const MEDIA_DEBOUNCE: f32 = 0.16;
/// How quickly the drawn wheel catches up with the selection (per second).
/// It stiffens further the further behind it is (see `update`), so fast
/// held scrolling stays within about a game of the selection.
const SPRING: f32 = 18.0;
const INFO_FADE: f32 = 0.35;

pub struct Fonts {
    pub display: FontId,
    pub body: FontId,
    pub mono: FontId,
    /// Pictures of the text drawn with these fonts (see `textcache`). Kept
    /// here because everything that draws text is handed the fonts.
    pub text: RefCell<TextCache>,
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
    /// Seconds since start (f64: an f32 loses the fractions after days of uptime).
    pub changed_at: f64,
    /// Seconds between the last selection change and the one before it.
    pub gap: f64,
    /// Direction of the last step (+1 next, -1 previous), for slide transitions.
    pub last_dir: i64,
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
        Wheel { target: start as i64, pos: start as f32, n: n.max(1) as i64, info_t: 1.0, last_sel: start, changed_at: 0.0, gap: f64::INFINITY, last_dir: 1 }
    }

    pub fn selected(&self) -> usize {
        modi(self.target, self.n) as usize
    }

    pub fn step(&mut self, d: i64) {
        if d != 0 {
            self.last_dir = d.signum();
        }
        self.target += d;
        // Keep the spring from replaying a long spin if many steps queue up.
        self.pos = self.pos.clamp(self.target as f32 - 6.0, self.target as f32 + 6.0);
    }

    /// Whether the wheel has stopped moving and the info has faded in.
    pub fn at_rest(&self) -> bool {
        self.pos == self.target as f32 && self.info_t >= INFO_FADE
    }

    /// Advances the animation. Returns true when the selection changed.
    pub fn update(&mut self, dt: f32, now: f64) -> bool {
        // Keep the numbers near zero (the same place on the wheel, whole
        // turns back): far out, an f32 step rounds to nothing and the wheel
        // would never come to rest (and the playfield never go still).
        let turns = self.target.div_euclid(self.n) * self.n;
        if turns != 0 {
            self.target -= turns;
            self.pos -= turns as f32;
        }
        let behind = ((self.target as f32 - self.pos).abs() - 1.0).clamp(0.0, 3.0);
        let k = 1.0 - (-dt * SPRING * (1.0 + 0.5 * behind)).exp();
        let before = self.pos;
        self.pos += (self.target as f32 - self.pos) * k;
        // Close enough, or so close that the step rounded to nothing.
        let left = (self.target as f32 - self.pos).abs();
        if left < 0.0005 || (dt > 0.0 && self.pos == before && left < 0.01) {
            self.pos = self.target as f32;
        }
        self.info_t += dt;
        let s = self.selected();
        if s != self.last_sel {
            self.last_sel = s;
            self.info_t = 0.0;
            self.gap = now - self.changed_at;
            self.changed_at = now;
            return true;
        }
        false
    }
}

/// Everything the wheel needs to draw one frame.
pub struct Frame<'a> {
    /// The layout template (Cover flow draws turned covers, Minimal stays plain).
    pub style: WheelStyle,
    pub layout: Layout,
    pub wheel: &'a Wheel,
    /// Every game's entry; `order` picks and orders the ones in the current playlist.
    pub entries: &'a [Entry],
    pub order: &'a [usize],
    /// Current playlist name, shown above the counter.
    pub playlist: &'a str,
    pub fonts: &'a Fonts,
    /// Letter jump indicator: the letter and seconds since the jump.
    pub letter: Option<(char, f32)>,
    /// "Update v… available" note, shown small in the corner.
    pub update_note: Option<String>,
    /// The phone remote's QR code (one pixel per module, and its size in
    /// pixels), shown in the top-right corner beside the counter.
    pub qr: Option<(ImageId, usize)>,
}

/// About how big the phone remote's QR code is, in design units (it is
/// rounded to a whole number of pixels per module, so it stays crisp).
const QR_SIZE: f32 = 104.0;

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
    // the wheel); horizontal spacing is set per row in Track.
    let s = (h / DESIGN_H).min(w / DESIGN_H);
    let (vw, dh) = (w / s, h / s);
    cv.save();
    cv.scale(s, s);

    // Darken the top for the info text, and behind the row of wheels.
    let mut p = Path::new();
    p.rect(0.0, 0.0, vw, 450.0);
    cv.fill_path(&p, &Paint::linear_gradient(0.0, 0.0, 0.0, 450.0, Color::rgbaf(0.012, 0.016, 0.03, 0.7), Color::rgbaf(0.012, 0.016, 0.03, 0.0)));
    let track = Track::new(vw, dh, f.layout, f.style);
    draw_shade(cv, vw, dh, &track);

    if !f.order.is_empty() {
        draw_track(cv, &track, f);
        draw_info(cv, vw, f, &track);
    } else {
        let mut g = Group::new(vw / 2.0, dh / 2.0);
        g.text(vw / 2.0, dh / 2.0, format!("{} is empty", f.playlist), f.fonts.display, 56.0, Color::rgbaf(0.9, 0.92, 0.96, 0.8), Align::Center, Baseline::Middle);
        f.fonts.text.borrow_mut().draw(cv, ("empty", 0), &g, 0.0, 0.0, 1.0, 1.0);
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

/// Width over height of a game's box while it has no logo (a placeholder).
const PLACEHOLDER_ASPECT: f32 = 1.6;

/// Stand-in while the logo loads, or when a game has no wheel image.
fn placeholder(cv: &mut Cv, e: &Entry, w: f32, h: f32, alpha: f32, fonts: &Fonts) {
    let (pw, ph) = (w, h * 0.6);
    let mut p = Path::new();
    p.rounded_rect(-pw / 2.0, -ph / 2.0, pw, ph, ph * 0.18);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.07, 0.09, 0.13, 0.85 * alpha)));
    cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.2 * alpha)).with_line_width(2.0));
    // The text is sized by the box, and the box changes size as the wheel
    // turns: while it does, its picture is made at the next of a few fixed
    // sizes up and drawn a little smaller, so a turning wheel doesn't make
    // a new one every frame. Once the box keeps its size, the picture is
    // made at the box's own size and put down pixel for pixel. The sizes
    // are worked out from the scale and the placeholder's fixed aspect, not
    // through `w`, so the same step gives the same text exactly.
    let k = cv.transform().average_scale();
    let px = w * k;
    if px.is_nan() || px < 4.0 {
        return;
    }
    // One place per game (its entry's address: a rebuilt list just makes new pictures).
    let slot = ("placeholder", e as *const Entry as usize);
    let mut cache = fonts.text.borrow_mut();
    let (ref_w, zoom) = if cache.steady(slot, px) { (w, 1.0) } else { (placeholder_ref_px(px) / k, w * k / placeholder_ref_px(px)) };
    let (pw, ph) = (ref_w, ref_w / PLACEHOLDER_ASPECT * 0.6);
    let size = (pw / (e.title.chars().count().max(6) as f32) * 1.7).min(ph * 0.42);
    let mut g = Group::new(0.0, 0.0);
    g.text(0.0, 0.0, &e.title, fonts.display, size, Color::rgbaf(0.92, 0.94, 0.98, 1.0), Align::Center, Baseline::Middle);
    cache.draw(cv, slot, &g, 0.0, 0.0, alpha, zoom);
}

/// The width in pixels a placeholder's text picture is made at for a box
/// `px` pixels wide: the next of a ladder of sizes about 9% apart.
fn placeholder_ref_px(px: f32) -> f32 {
    let step = (px.log2() * 8.0 - 1e-4).ceil();
    2f32.powf(step / 8.0).max(px)
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

/// The main wheel's box at 100% size (design units); a logo is fitted into it.
const BOX_W: f32 = 400.0;
const BOX_H: f32 = 250.0;
/// How big the side wheels are next to the main one (flat logos).
const SIDE_SCALE: f32 = 0.6;
/// Radius of the big wheel the row curves round when Arc is on.
const ARC_R: f32 = 1500.0;
/// Cover flow's perspective distance.
const FOCAL: f32 = 1600.0;

/// Where the row of wheels sits on this screen, worked out from the layout
/// once a frame. The row runs along the bottom, or down the left or right
/// side; "along" is the row's direction and "inward" is towards the screen
/// edge it hugs (where Arc bends the far wheels).
struct Track {
    pos: WheelPosition,
    /// Cover flow's turned covers rather than flat logos.
    covers: bool,
    /// Minimal: no glow and a lighter shade, so more video shows.
    plain: bool,
    arc: bool,
    /// The main wheel's box.
    bw: f32,
    bh: f32,
    /// The main wheel's centre.
    cx: f32,
    cy: f32,
    /// Distance along the row to the first side wheel, then between the rest.
    first: f32,
    next: f32,
    /// Wheels shown each side.
    side: f32,
    /// Half the row's length (from the main wheel to either end).
    half: f32,
    vw: f32,
    /// The size along the row that the spacing is worked out for (tests check it).
    #[cfg_attr(not(test), allow(dead_code))]
    e: f32,
}

impl Track {
    fn new(vw: f32, dh: f32, layout: Layout, style: WheelStyle) -> Track {
        let covers = style == WheelStyle::Flow;
        let k = layout.size as f32 / 100.0;
        let (bw, bh) = (BOX_W * k, BOX_H * k);
        let side_row = layout.position != WheelPosition::Bottom;
        // A side row runs from below the title block (portrait) or the
        // counter (landscape) down to near the bottom.
        let (lo, hi) = if side_row { (vertical_col_top(vw), dh - 50.0) } else { (0.0, vw) };
        let half = (hi - lo) / 2.0;
        // The wheels' size along the row. Along the bottom most wheel images
        // are round or a little wide, narrower than the box: space them for
        // that (a very wide one may overlap its neighbour a little). Covers
        // and Minimal (neighbours at the screen edges) keep the box width.
        let plain = style == WheelStyle::Minimal;
        let e = if side_row { bh } else if covers || plain { bw } else { (bw + bh) / 2.0 };
        // Exactly as many wheels as asked for: more than fit overlap or run
        // off the screen, which is the owner's choice.
        let (first, next) = if covers {
            // Covers overlap like a shelf of records.
            (0.75 * e, 0.52 * e)
        } else {
            let gap = 0.06 * e;
            (0.5 * e * (1.0 + SIDE_SCALE) + gap, e * SIDE_SCALE + gap)
        };
        // Wheel Spacing stretches or tightens the whole row.
        let k_space = layout.spacing as f32 / 100.0;
        let (first, next) = (first * k_space, next * k_space);
        let side = layout.side_wheels as f32;
        let last = if side >= 1.0 { first + (side - 1.0) * next } else { 0.0 };
        // How far Arc bends the last wheel on screen towards the edge.
        let bulge = if layout.arc { ARC_R * (1.0 - (last.min(half) / ARC_R).min(PI / 2.0).cos()) } else { 0.0 };
        // A side row hugs its edge: a round logo sits 40 units from it.
        let inset = 40.0 + bw.min(bh) / 2.0 + bulge;
        let (cx, cy) = match layout.position {
            WheelPosition::Bottom => {
                // Arc's side wheels drop lower; covers need room for their reflection.
                let lift = if layout.arc { 60.0 } else { 0.0 } + if covers { 0.35 * bh } else { 0.0 };
                (vw / 2.0, dh - 50.0 - bh / 2.0 - lift)
            }
            WheelPosition::Right => (vw - inset, (lo + hi) / 2.0),
            WheelPosition::Left => (inset, (lo + hi) / 2.0),
        };
        Track { pos: layout.position, covers, plain, arc: layout.arc, bw, bh, cx, cy, first, next, side, half, vw, e }
    }

    fn side_row(&self) -> bool {
        self.pos != WheelPosition::Bottom
    }

    /// Centre and turn of the wheel `d` places from the selection.
    fn place(&self, d: f32) -> (f32, f32, f32) {
        let a = d.abs();
        let s = d.signum() * (a.min(1.0) * self.first + (a - 1.0).max(0.0) * self.next);
        let (along, inward, th) = if self.arc {
            let th = (s / ARC_R).clamp(-PI / 2.0, PI / 2.0);
            (ARC_R * th.sin(), ARC_R * (1.0 - th.cos()), th)
        } else {
            (s, 0.0, 0.0)
        };
        // Arc tilts each wheel as if it sat on the big wheel's rim.
        match self.pos {
            WheelPosition::Bottom => (self.cx + along, self.cy + inward, th * 0.85),
            WheelPosition::Right => (self.cx + inward, self.cy + along, -th * 0.85),
            WheelPosition::Left => (self.cx - inward, self.cy + along, th * 0.85),
        }
    }

    /// The widest a wheel centred at `x` may be, so a side row's wheels stay
    /// 30 units clear of the screen edge.
    fn max_w(&self, x: f32) -> f32 {
        match self.pos {
            WheelPosition::Bottom => f32::INFINITY,
            WheelPosition::Right => 2.0 * (self.vw - 30.0 - x),
            WheelPosition::Left => 2.0 * (x - 30.0),
        }
        .max(1.0)
    }

    /// A logo's drawn size `a` places out (0 = the main wheel), fitted and capped.
    fn logo_size(&self, aspect: f32, a: f32, x: f32) -> (f32, f32) {
        let sc = if self.covers { 1.0 } else { SIDE_SCALE + (1.0 - SIDE_SCALE) * (1.0 - a).max(0.0) };
        let (w, h) = fit(aspect, self.bw * sc, self.bh * sc);
        let cap = self.max_w(x);
        if w > cap { (cap, h * cap / w) } else { (w, h) }
    }

    /// How far from its screen edge a side row reaches (the main wheel and
    /// the selection bar beside it), so the title can keep clear of it.
    fn reach(&self) -> f32 {
        let w = self.bw.min(self.max_w(self.cx));
        let edge_dist = if self.pos == WheelPosition::Right { self.vw - self.cx } else { self.cx };
        edge_dist + w / 2.0 + 40.0
    }
}

/// The darkening behind the row of wheels.
fn draw_shade(cv: &mut Cv, vw: f32, dh: f32, t: &Track) {
    let c = |a: f32| Color::rgbaf(0.016, 0.024, 0.04, a);
    let a = if t.plain { 0.45 } else { 0.6 };
    let mut p = Path::new();
    match t.pos {
        WheelPosition::Bottom => {
            let h = (dh - t.cy + t.bh * 0.6).min(dh);
            p.rect(0.0, dh - h, vw, h);
            cv.fill_path(&p, &Paint::linear_gradient(0.0, dh - h, 0.0, dh, c(0.0), c(a)));
        }
        WheelPosition::Right => {
            let gw = (t.reach() + 260.0).min(vw);
            p.rect(vw - gw, 0.0, gw, dh);
            cv.fill_path(&p, &Paint::linear_gradient(vw - gw, 0.0, vw, 0.0, c(0.0), c(a)));
        }
        WheelPosition::Left => {
            let gw = (t.reach() + 260.0).min(vw);
            p.rect(0.0, 0.0, gw, dh);
            cv.fill_path(&p, &Paint::linear_gradient(gw, 0.0, 0.0, 0.0, c(0.0), c(a)));
        }
    }
}

/// The row of wheels: the selection big in its slot, `side` smaller ones
/// (or turned covers) each side. The outermost fades in and out as the row
/// moves instead of popping.
fn draw_track(cv: &mut Cv, t: &Track, f: &Frame) {
    let pos = f.wheel.pos;
    let sel = f.entry(f.wheel.selected());
    let hue = entry_hue(sel);
    let near = 1.0 - clamp((pos - pos.round()).abs() * 2.0, 0.0, 1.0);

    // Arc: faint rims of the big wheel.
    if t.arc && !t.covers {
        let reach = if t.side_row() { ((t.half + 80.0) / ARC_R).min(1.0).asin().min(0.95) } else { 0.95 };
        for (extra, a0, alpha, lw) in [(0.0f32, reach, 0.10f32, 3.0f32), (190.0, reach * 0.85, 0.05, 2.0)] {
            let (ccx, ccy, mid) = match t.pos {
                WheelPosition::Bottom => (t.cx, t.cy + ARC_R, -PI / 2.0),
                WheelPosition::Right => (t.cx + ARC_R, t.cy, PI),
                WheelPosition::Left => (t.cx - ARC_R, t.cy, 0.0),
            };
            let mut p = Path::new();
            p.arc(ccx, ccy, ARC_R + extra, mid - a0, mid + a0, femtovg::Solidity::Hole);
            cv.stroke_path(&p, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, alpha)).with_line_width(lw));
        }
    }
    // Cover flow along the bottom: a faint floor line under the covers.
    if t.covers && t.pos == WheelPosition::Bottom && !t.arc {
        fill_rect(cv, 0.0, t.cy + t.bh * 0.55, t.vw, 2.0, Color::rgbaf(1.0, 1.0, 1.0, 0.06));
    }
    let sel_aspect = sel.logo.as_ref().map_or(PLACEHOLDER_ASPECT, |l| l.aspect);
    let (sel_w, sel_h) = t.logo_size(sel_aspect, 0.0, t.cx);
    if !t.plain {
        slot_glow(cv, t.cx, t.cy, sel_w.max(t.bh) * 0.85, t.bh * 0.75, hue, 0.6 + 0.4 * near);
    }
    // A side row marks the selection with a thin accent bar on its inner side.
    if t.side_row() {
        let bx = if t.pos == WheelPosition::Right { t.cx - sel_w / 2.0 - 30.0 } else { t.cx + sel_w / 2.0 + 24.0 };
        let mut p = Path::new();
        p.rounded_rect(bx, t.cy - sel_h * 0.4, 6.0, sel_h * 0.8, 3.0);
        cv.fill_path(&p, &Paint::color(hue_color(hue, 0.9, 0.65, 0.5 + 0.4 * near)));
    }

    // Covers along the bottom show their reflection on the floor.
    let reflect = t.covers && t.pos == WheelPosition::Bottom;
    for (i, d) in visible_items(pos, f.order.len(), t.side + 1.0) {
        let e = f.entry(i);
        let a = d.abs();
        let fade = clamp(t.side + 1.0 - a, 0.0, 1.0);
        if fade <= 0.0 {
            continue;
        }
        let (x, y, rot) = t.place(d);
        let aspect = e.logo.as_ref().map_or(PLACEHOLDER_ASPECT, |l| l.aspect);
        let (w, h) = t.logo_size(aspect, a, x);
        cv.save();
        cv.translate(x, y);
        cv.rotate(rot);
        if t.covers {
            let alpha = clamp(1.3 - a * 0.2, 0.0, 1.0) * fade;
            draw_cover(cv, e, w, h, d, alpha, t.side_row(), reflect, f.fonts);
        } else {
            // Side wheels are dimmer, Minimal's more so (it keeps the video clear).
            let dimmer = if t.plain { 0.3 } else { 0.8 };
            let alpha = clamp(1.2 - a * 0.2, 0.0, 1.0) * (1.0 - (1.0 - dimmer) * a.min(1.0)) * fade;
            draw_logo(cv, e, w, h, alpha, (1.0 - a * 1.6).max(0.0), f.fonts);
        }
        cv.restore();
    }
}

/// One cover-flow cover centred at the origin, turned towards the
/// selection `d` places away. A side row turns covers about the other axis.
#[allow(clippy::too_many_arguments)]
fn draw_cover(cv: &mut Cv, e: &Entry, w: f32, hgt: f32, d: f32, alpha: f32, side_row: bool, reflect: bool, fonts: &Fonts) {
    const STRIPS: usize = 22;
    const FAR_STRIPS: usize = 8;
    let a = d.abs();
    let a1 = a.min(1.0);
    let Some(logo) = (if a < 0.04 { None } else { e.logo.as_ref() }) else {
        // Centred (or not yet loaded): flat logo plus reflection.
        draw_logo(cv, e, w, hgt, alpha, if a < 0.04 { 1.0 } else { 0.0 }, fonts);
        if let (true, Some(l)) = (reflect, &e.logo) {
            reflection(cv, l.img, -w / 2.0, hgt / 2.0 + 4.0, w, hgt, 0.16 * alpha, None);
        }
        return;
    };
    let theta = -d.signum() * a1 * FLOW_TURN;
    let dim = 1.0 - 0.4 * a1 - 0.06 * (a - 1.0).max(0.0);
    let strip_alpha = alpha * dim;
    // Covers further out are smaller and dimmer: fewer strips do.
    let strips = if a > 1.5 { FAR_STRIPS } else { STRIPS };
    // A reflection this faint can't be seen.
    let reflect = reflect && strip_alpha >= 0.05;
    // The cover is sliced across the row; each slice is scaled for its depth.
    let (len, across) = if side_row { (hgt, w) } else { (w, hgt) };
    for k in 0..strips {
        let u0 = k as f32 / strips as f32 - 0.5;
        let u1 = (k + 1) as f32 / strips as f32 - 0.5;
        let p0 = FOCAL / (FOCAL + u0 * len * theta.sin());
        let p1 = FOCAL / (FOCAL + u1 * len * theta.sin());
        let t0 = u0 * len * theta.cos() * p0;
        let t1 = u1 * len * theta.cos() * p1;
        let cross = across * ((p0 + p1) / 2.0) * (1.0 - 0.08 * a1);
        let (dt, dl) = (t0.min(t1), (t1 - t0).abs());
        if dl < 0.05 {
            continue;
        }
        // Place the whole image so that slice k lands exactly on [dt, dt+dl].
        // (|theta| <= 1.2 rad, so t1 > t0 and slices stay in order.)
        let img_len = dl * strips as f32;
        let img_t = dt - k as f32 * dl;
        let mut p = Path::new();
        let mut paint = if side_row {
            p.rect(-cross / 2.0, dt, cross, dl);
            Paint::image(logo.img, -cross / 2.0, img_t, cross, img_len, 0.0, strip_alpha)
        } else {
            p.rect(dt, -cross / 2.0, dl, cross);
            Paint::image(logo.img, img_t, -cross / 2.0, img_len, cross, 0.0, strip_alpha)
        };
        paint.set_anti_alias(false);
        cv.fill_path(&p, &paint);
        if reflect {
            reflection(cv, logo.img, img_t, cross / 2.0 + 4.0, img_len, cross, 0.13 * strip_alpha, Some((dt, dl)));
        }
    }
}

fn vertical_portrait(vw: f32) -> bool {
    vw < DESIGN_W * 0.75
}

/// Where a side row starts: below the title block on a portrait screen
/// (so the title keeps the full width), else below the playlist name and
/// counter in the top corner.
fn vertical_col_top(vw: f32) -> f32 {
    if vertical_portrait(vw) { 540.0 } else { 190.0 }
}

/// Mirrored bottom half of an image under a logo whose bottom edge is at `top`.
/// `clip_x` limits it to one strip of a turned cover.
#[allow(clippy::too_many_arguments)]
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

/// A stable hue for a tag name, picked from a palette of clearly different
/// colours by a case-insensitive FNV-1a hash, so each tag keeps its colour.
fn tag_hue(tag: &str) -> f32 {
    const HUES: [f32; 12] = [0.0, 22.0, 42.0, 95.0, 135.0, 165.0, 188.0, 208.0, 232.0, 262.0, 290.0, 330.0];
    let mut h: u32 = 0x811c_9dc5;
    for b in tag.to_ascii_lowercase().bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h ^= h >> 16;
    HUES[(h % HUES.len() as u32) as usize]
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
fn draw_info(cv: &mut Cv, vw: f32, f: &Frame, track: &Track) {
    let idx = f.wheel.selected();
    let e = f.entry(idx);
    let hue = entry_hue(e);
    let a = ease_out(f.wheel.info_t / INFO_FADE);
    // Slide in from the side the wheel is moving from: from the right on Next, the left on Prior.
    let off = (1.0 - a) * 28.0 * f.wheel.last_dir as f32;
    let y = 120.0f32;
    // On a landscape screen a side row runs alongside the title: keep clear of it.
    let beside = if track.side_row() && !vertical_portrait(vw) { track.reach() + 40.0 } else { 0.0 };
    let (x, max_w) = match track.pos {
        WheelPosition::Left if beside > 0.0 => (beside.max(96.0), vw - 96.0 - beside.max(96.0)),
        _ => (96.0, vw - 96.0 - beside.max(96.0)),
    };
    let max_w = max_w.max(200.0);

    let text = |font: FontId, size: f32, c: Color, align: Align| {
        Paint::color(c).with_font(&[font]).with_font_size(size).with_text_align(align).with_text_baseline(Baseline::Alphabetic)
    };
    // The text is drawn from pictures (see `textcache`), made at full
    // opacity where it rests; each part then fades and slides in as before.
    let mut cache = f.fonts.text.borrow_mut();
    let al = Align::Left;
    let ab = Baseline::Alphabetic;
    // The small maker/year line only when the title doesn't already end with it.
    if !e.subtitle.is_empty() && split_suffix(&e.title).1.is_none() {
        let mut g = Group::new(x, y);
        g.text(x, y, &e.subtitle, f.fonts.mono, 26.0, hue_color(hue, 0.9, 0.72, 1.0), al, ab);
        cache.draw(cv, ("info subtitle", 0), &g, off, 0.0, a, 1.0);
    }
    // Title: the game name, then its "(Maker Year)" on a smaller second line.
    // A long name shrinks to fit, then wraps onto two lines itself.
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
    let tx = x;
    let mut title = Group::new(x, y);
    for line in &name_lines {
        title.text(tx + 3.0, ty + 3.0, line, f.fonts.display, size, Color::rgbaf(0.0, 0.0, 0.0, 0.45), al, ab);
        title.text(tx, ty, line, f.fonts.display, size, Color::rgbaf(0.957, 0.965, 0.984, 1.0), al, ab);
        ty += line_h;
    }
    let mut last_baseline = ty - line_h;
    if let Some(suffix) = &suffix {
        let small = (size * 0.6).max(40.0);
        let sw = width_at_92(cv, suffix) * small / 92.0;
        let small = if sw > max_w { small * max_w / sw } else { small };
        last_baseline += small * 1.2;
        title.text(tx + 2.0, last_baseline + 2.0, suffix, f.fonts.display, small, Color::rgbaf(0.0, 0.0, 0.0, 0.45), al, ab);
        title.text(tx, last_baseline, suffix, f.fonts.display, small, Color::rgbaf(0.86, 0.88, 0.93, 0.85), al, ab);
    }
    cache.draw(cv, ("info title", 0), &title, off * 1.6, 0.0, a, 1.0);

    // Tag chips below the title, wrapping onto more rows as needed.
    let mut chip_top = last_baseline + 28.0;
    let left = x;
    let mut chip_x = left;
    let mut chips = Group::new(x, y);
    for chip in &e.chips {
        let paint = text(f.fonts.body, 24.0, Color::rgbaf(1.0, 1.0, 1.0, 1.0), Align::Left);
        let tw = cv.measure_text(0.0, 0.0, chip, &paint).map(|m| m.width()).unwrap_or(100.0);
        let w = tw + 32.0;
        if chip_x > left && chip_x + w > x + max_w {
            chip_x = left;
            chip_top += 54.0;
        }
        // A solid colour per tag (the same tag always gets the same colour)
        // so the text stays readable over busy playfield video.
        let hue = tag_hue(chip);
        chips.pill(chip_x, chip_top, w, 42.0, 21.0, hue_color(hue, 0.65, 0.34, 0.92), hue_color(hue, 0.8, 0.62, 0.9), 1.5);
        chips.text(chip_x + 17.0, chip_top + 30.5, chip, f.fonts.body, 24.0, Color::rgbaf(0.0, 0.0, 0.0, 0.5), al, ab);
        chips.text(chip_x + 16.0, chip_top + 29.0, chip, f.fonts.body, 24.0, Color::rgbaf(1.0, 1.0, 1.0, 1.0), al, ab);
        chip_x += w + 12.0;
    }
    cache.draw(cv, ("info chips", 0), &chips, off * 2.0, 0.0, a, 1.0);
    // The phone remote's QR code in the corner, the text to its left.
    let mut right = vw - 96.0;
    if let Some((img, modules)) = f.qr {
        let s = cv.transform().average_scale().max(0.01);
        let per_module = (QR_SIZE * s / modules.max(1) as f32).round().max(2.0);
        let size = per_module * modules as f32 / s;
        // Bottom on the counter's baseline, clear of the title below.
        let (qx, qy) = (right - size, y - size);
        let mut p = Path::new();
        p.rect(qx, qy, size, size);
        cv.fill_path(&p, &Paint::image(img, qx, qy, size, size, 0.0, 0.95));
        right = qx - 24.0;
    }
    let ar = Align::Right;
    let mut corner = Group::new(right, y);
    if let Some(note) = &f.update_note {
        corner.text(right, y - 76.0, note, f.fonts.mono, 22.0, Color::rgbaf(1.0, 0.75, 0.33, 0.95), ar, ab);
    }
    corner.text(right, y - 40.0, f.playlist.to_uppercase(), f.fonts.mono, 24.0, hue_color(195.0, 0.9, 0.72, 0.9), ar, ab);
    let counter = format!("{:02} / {}", idx + 1, f.order.len());
    corner.text(right, y, &counter, f.fonts.mono, 28.0, Color::rgbaf(0.9, 0.92, 0.96, 0.7), ar, ab);
    cache.draw(cv, ("info corner", 0), &corner, 0.0, 0.0, 1.0, 1.0);
}

/// The exit menu as drawn: options, which is selected, and seconds since it opened.
pub struct MenuView<'a> {
    pub title: &'a str,
    pub options: &'a [&'a str],
    pub selected: usize,
    pub t: f32,
}

/// Height of one menu row: 96 when the panel fits, smaller when the screen is
/// too short for that many rows (title 150, padding 40, a 40 margin each side).
fn menu_row_height(rows: usize, screen_h: f32) -> f32 {
    let room = (screen_h - 80.0 - 190.0) / rows.max(1) as f32;
    room.clamp(48.0, 96.0)
}

/// Draws the exit menu centered over the playfield (same frame as `draw`).
pub fn draw_menu(cv: &mut Cv, w: f32, h: f32, m: &MenuView, fonts: &Fonts) {
    let s = (h / DESIGN_H).min(w / DESIGN_H);
    let (vw, dh) = (w / s, h / s);
    let a = ease_out(m.t / 0.2);
    cv.save();
    cv.scale(s, s);
    fill_rect(cv, 0.0, 0.0, vw, dh, Color::rgbaf(0.02, 0.025, 0.04, 0.72 * a));

    // Rows shrink to fit a short (landscape) screen; a portrait playfield has room.
    let row_h = menu_row_height(m.options.len(), dh);
    let pw = (vw - 160.0).min(820.0);
    let ph = 150.0 + m.options.len() as f32 * row_h + 40.0;
    let (px, py) = ((vw - pw) / 2.0, (dh - ph) / 2.0 + (1.0 - a) * 40.0);
    let mut p = Path::new();
    p.rounded_rect(px, py, pw, ph, 28.0);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.07, 0.086, 0.13, 0.96 * a)));
    cv.stroke_path(&p, &Paint::color(Color::rgbaf(0.13, 0.16, 0.23, a)).with_line_width(2.0));

    // The text comes from pictures (see `textcache`) that slide and fade with the panel.
    let mut cache = fonts.text.borrow_mut();
    let mut title = Group::new(vw / 2.0, py);
    title.text(vw / 2.0, py + 104.0, m.title, fonts.display, 64.0, Color::rgbaf(0.9, 0.92, 0.96, 1.0), Align::Center, Baseline::Alphabetic);
    cache.draw(cv, ("menu title", 0), &title, 0.0, 0.0, a, 1.0);

    let accent = Color::rgbaf(0.325, 0.827, 1.0, a);
    for (i, opt) in m.options.iter().enumerate() {
        let y = py + 150.0 + i as f32 * row_h;
        let sel = i == m.selected;
        if sel {
            let mut p = Path::new();
            p.rounded_rect(px + 32.0, y + 8.0, pw - 64.0, row_h - 16.0, (row_h - 16.0) / 2.0);
            cv.fill_path(&p, &Paint::color(accent));
        }
        let c = if sel { Color::rgbaf(0.04, 0.047, 0.07, 1.0) } else { Color::rgbaf(0.88, 0.9, 0.95, 0.85) };
        let mut row = Group::new(vw / 2.0, y);
        row.text(vw / 2.0, y + row_h / 2.0, *opt, fonts.body, 42.0 * row_h / 96.0, c, Align::Center, Baseline::Middle);
        cache.draw(cv, ("menu row", i), &row, 0.0, 0.0, a, 1.0);
    }
    cv.restore();
}

/// "Launching <game>" / "Exiting <game>" screen shown on the playfield while the emulator starts or closes
/// (and left behind the game window, so the desktop never shows).
#[allow(clippy::too_many_arguments)]
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
    let aspect = e.logo.as_ref().map_or(PLACEHOLDER_ASPECT, |l| l.aspect);
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
    // From a picture (see `textcache`) that fades in.
    let (ac, ab) = (Align::Center, Baseline::Alphabetic);
    let mut g = Group::new(vw / 2.0, spin_y);
    g.text(vw / 2.0, spin_y + 110.0, format!("{verb} {}", e.title), fonts.display, title_size, Color::rgbaf(0.93, 0.945, 0.97, 1.0), ac, ab);
    g.text(vw / 2.0, spin_y + 156.0, emulator.to_uppercase(), fonts.mono, 24.0, Color::rgbaf(0.9, 0.92, 0.96, 0.6), ac, ab);
    fonts.text.borrow_mut().draw(cv, ("launch", 0), &g, 0.0, 0.0, a, 1.0);
    cv.restore();
}

/// Clears whatever surface is current to black (secondary screens during a game).
pub fn clear_black(cv: &mut Cv, w: u32, h: u32) {
    cv.set_size(w, h, 1.0);
    cv.clear_rect(0, 0, w, h, Color::rgbf(0.0, 0.0, 0.0));
    cv.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_comes_to_rest_however_far_it_turned() {
        // Thousands of steps one way (attract mode, a held flipper), at 144 Hz.
        for (n, steps) in [(1usize, 5000i64), (7, -3001), (2500, 4000), (2500, -9000)] {
            let mut w = Wheel::new(n, 0);
            for _ in 0..steps.abs() {
                w.step(steps.signum());
                w.update(1.0 / 144.0, 0.0);
            }
            let want = w.selected();
            let mut frames = 0;
            while !w.at_rest() && frames < 2000 {
                w.update(1.0 / 144.0, 0.0);
                frames += 1;
            }
            assert!(w.at_rest(), "{n} games, {steps} steps: never settled at {} -> {}", w.pos, w.target);
            assert_eq!(w.selected(), want);
            assert!((0..n.max(1) as i64).contains(&w.target), "{}", w.target);
        }
    }

    #[test]
    fn every_layout_keeps_the_main_wheel_on_screen() {
        use crate::settings::{SIZE_CHOICES, SPACING_CHOICES, MAX_SIDE_WHEELS};
        for (vw, dh) in [(1080.0, 1920.0), (1920.0, 1080.0)] {
            for style in [WheelStyle::Arc, WheelStyle::Flow, WheelStyle::Minimal, WheelStyle::Vertical] {
                for position in [WheelPosition::Bottom, WheelPosition::Left, WheelPosition::Right] {
                    for &size in SIZE_CHOICES {
                        for side_wheels in 0..=MAX_SIDE_WHEELS {
                            for (spacing, arc) in SPACING_CHOICES.iter().flat_map(|&sp| [(sp, false), (sp, true)]) {
                                let l = Layout { size, position, side_wheels, spacing, arc };
                                let t = Track::new(vw, dh, l, style);
                                for aspect in [0.8, 1.0, 1.6, 3.0] {
                                    let (w, h) = t.logo_size(aspect, 0.0, t.cx);
                                    let what = format!("{vw}x{dh} {style:?} {l:?} aspect {aspect}");
                                    assert!(t.cx - w / 2.0 >= 0.0 && t.cx + w / 2.0 <= vw, "off the side: {what}");
                                    assert!(t.cy + h / 2.0 <= dh, "off the bottom: {what}");
                                    // Clear of the title block (portrait) or the counter (landscape).
                                    assert!(t.cy - h / 2.0 >= if vw < dh { 450.0 } else { 160.0 }, "under the title: {what}");
                                    // Every wheel asked for is drawn, spaced so flat logos don't pile up.
                                    assert_eq!(t.side, side_wheels as f32, "{what}");
                                    assert!(style == WheelStyle::Flow || spacing < 100 || t.next >= t.e * SIDE_SCALE, "wheels pile up: {what}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn arc_fits_two_wheels_each_side_on_the_cabinet() {
        // Portrait playfield, Arc template at its own size and at 100%:
        // the second wheel's centre is on screen.
        for size in [WheelStyle::Arc.template().size, 100] {
            let l = Layout { size, ..WheelStyle::Arc.template() };
            let t = Track::new(1080.0, 1920.0, l, WheelStyle::Arc);
            let (x, _, _) = t.place(2.0);
            assert!(x <= 1080.0 - 40.0, "Arc at {size}%: second wheel at {x}");
        }
    }

    #[test]
    fn placeholder_pictures_come_in_a_few_sizes() {
        for px in [4.0f32, 100.0, 333.3, 512.0, 799.9, 1520.0] {
            let r = placeholder_ref_px(px);
            // Never drawn bigger than made, and at most a step smaller.
            assert!(r >= px && r < px * 1.0906, "{px} -> {r}");
            // A box growing a little (a turning wheel) mostly keeps its picture.
            assert!(placeholder_ref_px(px * 1.0001) <= r * 1.0906);
        }
        assert_eq!(placeholder_ref_px(512.0), 512.0);
        assert_eq!(placeholder_ref_px(470.0), placeholder_ref_px(480.0));
    }

    #[test]
    fn options_menu_fits_portrait_and_landscape() {
        // Portrait playfield (1920 design units tall): full-size rows.
        assert_eq!(menu_row_height(11, 1920.0), 96.0);
        // Landscape (1080 tall): rows shrink so the panel stays on screen.
        let r = menu_row_height(11, 1080.0);
        assert!(r < 96.0 && 190.0 + 11.0 * r <= 1080.0 - 80.0);
    }
}
