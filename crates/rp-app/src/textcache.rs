//! Text drawn once into a picture and then reused, frame after frame.
//!
//! On a cabinet the playfield is mounted turned, so the menu is drawn
//! rotated, and femtovg only uses its glyph atlas for an unrotated, evenly
//! scaled transform (and glyphs up to 92 px): every other glyph is filled as
//! a vector path, every frame. The info block alone is about 150 glyphs. So
//! each block of text is drawn unrotated, at the screen's own resolution,
//! into an image when it changes, and each frame only that image is drawn,
//! turned, faded and slid the way the text would be.

use femtovg::{renderer::OpenGl, Align, Baseline, Canvas, Color, FontId, ImageFlags, ImageId, Paint, Path, PixelFormat, RenderTarget, Transform2D};
use std::hash::{Hash, Hasher};

type Cv = Canvas<OpenGl>;

/// Frames a picture is kept after it was last drawn (a menu row, a wheel's
/// placeholder that scrolled away).
const KEEP_FRAMES: u64 = 120;
/// Pictures are at most this big a side; bigger text is drawn as it is.
const MAX_SIDE: i32 = 4096;

#[derive(Clone, Debug, PartialEq)]
enum Op {
    Text { x: f32, y: f32, text: String, font: FontId, size: f32, color: Color, align: Align, baseline: Baseline },
    /// A rounded rectangle, filled, then outlined `line` wide.
    Pill { x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color, stroke: Color, line: f32 },
}

/// Text (and the odd shape behind it) that moves and fades as one. Its parts
/// are kept relative to an anchor, so the same text drawn somewhere else
/// (a menu sliding in) reuses its picture.
#[derive(Clone, Debug, Default)]
pub struct Group {
    anchor: (f32, f32),
    ops: Vec<Op>,
}

impl Group {
    pub fn new(x: f32, y: f32) -> Self {
        Group { anchor: (x, y), ops: Vec::new() }
    }

    /// One line of text, as `fill_text` would draw it at `x`, `y`.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, x: f32, y: f32, text: impl Into<String>, font: FontId, size: f32, color: Color, align: Align, baseline: Baseline) -> &mut Self {
        let (x, y) = (x - self.anchor.0, y - self.anchor.1);
        self.ops.push(Op::Text { x, y, text: text.into(), font, size, color, align, baseline });
        self
    }

    /// A filled and outlined rounded rectangle (a tag chip).
    #[allow(clippy::too_many_arguments)]
    pub fn pill(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color, stroke: Color, line: f32) -> &mut Self {
        let (x, y) = (x - self.anchor.0, y - self.anchor.1);
        self.ops.push(Op::Pill { x, y, w, h, r, fill, stroke, line });
        self
    }

    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let f = |h: &mut std::collections::hash_map::DefaultHasher, v: f32| v.to_bits().hash(h);
        let c = |h: &mut std::collections::hash_map::DefaultHasher, c: Color| [c.r, c.g, c.b, c.a].iter().for_each(|v| v.to_bits().hash(h));
        for op in &self.ops {
            match op {
                Op::Text { x, y, text, font, size, color, align, baseline } => {
                    0u8.hash(&mut h);
                    f(&mut h, *x);
                    f(&mut h, *y);
                    text.hash(&mut h);
                    font.hash(&mut h);
                    f(&mut h, *size);
                    c(&mut h, *color);
                    align.hash(&mut h);
                    baseline.hash(&mut h);
                }
                Op::Pill { x, y, w, h: ph, r, fill, stroke, line } => {
                    1u8.hash(&mut h);
                    for v in [*x, *y, *w, *ph, *r, *line] {
                        f(&mut h, v);
                    }
                    c(&mut h, *fill);
                    c(&mut h, *stroke);
                }
            }
        }
        h.finish()
    }
}

/// Where a cached picture is kept: a fixed place ("info title") or one of
/// many (a placeholder per game). A place holds one picture at a time, so
/// text that changes (scrolling through games) replaces its old picture at
/// once instead of piling up.
pub type Slot = (&'static str, usize);

struct Cached {
    slot: Slot,
    key: u64,
    ops: Vec<Op>,
    /// Device pixels per unit it was drawn at.
    scale: f32,
    img: ImageId,
    /// The anchor's pixel in the picture (whole pixels), and the picture's size.
    at: (i32, i32),
    size: (i32, i32),
    used: u64,
}

#[derive(Default)]
pub struct TextCache {
    cached: Vec<Cached>,
    frame: u64,
}

/// Pixel layout of a picture for content spanning `bounds` (x0, y0, x1, y1,
/// in units relative to the anchor) at `scale` px per unit: the anchor's
/// pixel and the picture's size, with a pixel spare all round. The anchor
/// sits on a whole pixel so the picture can be put down pixel for pixel.
fn layout(bounds: (f32, f32, f32, f32), scale: f32) -> Option<((i32, i32), (i32, i32))> {
    let (x0, y0, x1, y1) = bounds;
    if !(x0 <= x1 && y0 <= y1) || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let (l, t) = ((x0 * scale).floor() as i64 - 1, (y0 * scale).floor() as i64 - 1);
    let (r, b) = ((x1 * scale).ceil() as i64 + 1, (y1 * scale).ceil() as i64 + 1);
    let (w, h) = (r - l, b - t);
    if w > MAX_SIDE as i64 || h > MAX_SIDE as i64 || l.abs() > MAX_SIDE as i64 || t.abs() > MAX_SIDE as i64 {
        return None;
    }
    Some(((-l as i32, -t as i32), (w as i32, h as i32)))
}

/// Whether the transform only scales and turns by a quarter turn (so a
/// picture's pixels can land exactly on the screen's).
fn square_on(t: &Transform2D) -> bool {
    let [a, b, c, d, ..] = t.0;
    let s = t.average_scale().max(1e-6);
    let eps = 1e-4 * s;
    (b.abs() < eps && c.abs() < eps) || (a.abs() < eps && d.abs() < eps)
}

/// The point near (`x`, `y`) that falls on a whole device pixel, if the
/// transform is square on.
fn snap(t: &Transform2D, x: f32, y: f32) -> Option<(f32, f32)> {
    if !square_on(t) {
        return None;
    }
    let (dx, dy) = t.transform_point(x, y);
    Some(t.inverse().transform_point(dx.round(), dy.round()))
}

/// The scale a picture is made at for the current transform, rounded so
/// float noise in the transform doesn't make a new picture.
fn picture_scale(t: &Transform2D) -> f32 {
    (t.average_scale() * 4096.0).round() / 4096.0
}

fn text_paint(font: FontId, size: f32, color: Color, align: Align, baseline: Baseline) -> Paint {
    Paint::color(color).with_font(&[font]).with_font_size(size).with_text_align(align).with_text_baseline(baseline)
}

fn faded(c: Color, alpha: f32) -> Color {
    Color::rgbaf(c.r, c.g, c.b, c.a * alpha)
}

/// Draws the parts at the current origin, `alpha` times as opaque.
fn draw_ops(cv: &mut Cv, ops: &[Op], alpha: f32) {
    for op in ops {
        match op {
            Op::Text { x, y, text, font, size, color, align, baseline } => {
                let _ = cv.fill_text(*x, *y, text, &text_paint(*font, *size, faded(*color, alpha), *align, *baseline));
            }
            Op::Pill { x, y, w, h, r, fill, stroke, line } => {
                let mut p = Path::new();
                p.rounded_rect(*x, *y, *w, *h, *r);
                cv.fill_path(&p, &Paint::color(faded(*fill, alpha)));
                cv.stroke_path(&p, &Paint::color(faded(*stroke, alpha)).with_line_width(*line));
            }
        }
    }
}

/// What the parts cover, relative to the anchor, with room for glyphs'
/// overhangs and the outline.
fn bounds(cv: &Cv, ops: &[Op]) -> (f32, f32, f32, f32) {
    let mut b = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut add = |x0: f32, y0: f32, x1: f32, y1: f32| b = (b.0.min(x0), b.1.min(y0), b.2.max(x1), b.3.max(y1));
    for op in ops {
        match op {
            Op::Text { x, y, text, font, size, align, baseline, .. } => {
                let paint = text_paint(*font, *size, Color::white(), *align, *baseline);
                let w = cv.measure_text(0.0, 0.0, text, &paint).map(|m| m.width()).unwrap_or(0.0);
                let left = match align {
                    Align::Left => *x,
                    Align::Center => x - w / 2.0,
                    Align::Right => x - w,
                };
                // Generous: a glyph can reach past its advance, and accents above the cap height.
                let (up, down) = match baseline {
                    Baseline::Top => (0.3, 1.5),
                    Baseline::Middle => (0.9, 0.9),
                    Baseline::Alphabetic => (1.25, 0.5),
                    Baseline::Bottom => (1.5, 0.3),
                };
                let pad = 0.3 * size;
                add(left - pad, y - up * size, left + w + pad, y + down * size);
            }
            Op::Pill { x, y, w, h, line, .. } => add(x - line, y - line, x + w + line, y + h + line),
        }
    }
    b
}

impl TextCache {
    /// Draws `g` moved by (`dx`, `dy`), `alpha` times as opaque and `zoom`
    /// times as big (about its anchor), from its picture, which is made
    /// first if the text changed. Falls back to drawing the text itself.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, cv: &mut Cv, slot: Slot, g: &Group, dx: f32, dy: f32, alpha: f32, zoom: f32) {
        if g.ops.is_empty() || alpha <= 0.0 || zoom <= 0.0 {
            return;
        }
        let t = cv.transform();
        // The picture is made at the size the group is drawn at with zoom 1.
        let scale = picture_scale(&t);
        let frame = self.frame;
        let key = g.key();
        let found = match self.cached.iter().position(|c| c.slot == slot) {
            Some(i) if self.cached[i].key == key && self.cached[i].scale == scale && self.cached[i].ops == g.ops => Some(i),
            Some(i) => {
                // The text in this place changed: its old picture goes.
                let old = self.cached.swap_remove(i);
                cv.delete_image(old.img);
                self.make(cv, slot, g, key, scale)
            }
            None => self.make(cv, slot, g, key, scale),
        };
        let Some(i) = found else {
            cv.save();
            cv.translate(g.anchor.0 + dx, g.anchor.1 + dy);
            cv.scale(zoom, zoom);
            draw_ops(cv, &g.ops, alpha);
            cv.restore();
            return;
        };
        let c = &mut self.cached[i];
        c.used = frame;
        let (mut ax, mut ay) = (g.anchor.0 + dx, g.anchor.1 + dy);
        // Put down pixel for pixel when drawn at its own size, so it is as
        // crisp as the text drawn directly (at most half a pixel away).
        if zoom == 1.0 {
            if let Some(p) = snap(&t, ax, ay) {
                (ax, ay) = p;
            }
        }
        let k = zoom / c.scale;
        let (x, y) = (ax - c.at.0 as f32 * k, ay - c.at.1 as f32 * k);
        let (w, h) = (c.size.0 as f32 * k, c.size.1 as f32 * k);
        let mut p = Path::new();
        p.rect(x, y, w, h);
        let mut paint = Paint::image(c.img, x, y, w, h, 0.0, alpha);
        paint.set_anti_alias(false);
        cv.fill_path(&p, &paint);
    }

    /// Draws `g` into a new picture for `slot`. None if it can't be made.
    fn make(&mut self, cv: &mut Cv, slot: Slot, g: &Group, key: u64, scale: f32) -> Option<usize> {
        let (at, size) = layout(bounds(cv, &g.ops), scale)?;
        // Render targets come out bottom row first (FLIP_Y), and femtovg
        // draws premultiplied colour.
        let flags = ImageFlags::FLIP_Y | ImageFlags::PREMULTIPLIED;
        let img = cv.create_image_empty(size.0 as usize, size.1 as usize, PixelFormat::Rgba8, flags).ok()?;
        cv.save();
        cv.reset_transform();
        cv.reset_scissor();
        cv.set_render_target(RenderTarget::Image(img));
        cv.clear_rect(0, 0, size.0 as u32, size.1 as u32, Color::rgbaf(0.0, 0.0, 0.0, 0.0));
        cv.translate(at.0 as f32, at.1 as f32);
        cv.scale(scale, scale);
        draw_ops(cv, &g.ops, 1.0);
        cv.set_render_target(RenderTarget::Screen);
        cv.restore();
        self.cached.push(Cached { slot, key, ops: g.ops.clone(), scale, img, at, size, used: self.frame });
        Some(self.cached.len() - 1)
    }

    /// Call once a frame: frees pictures not drawn for a while.
    pub fn end_frame(&mut self, cv: &mut Cv) {
        self.frame += 1;
        let frame = self.frame;
        self.cached.retain(|c| {
            let keep = frame - c.used <= KEEP_FRAMES;
            if !keep {
                cv.delete_image(c.img);
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_puts_the_anchor_on_a_whole_pixel_with_room_round_it() {
        // 10..110 x -20..5 units at 2 px per unit.
        let (at, size) = layout((10.0, -20.0, 110.0, 5.0), 2.0).unwrap();
        // Content starts at pixel 20 right of the anchor: one spare pixel before it.
        assert_eq!(at, (-19, 41));
        assert_eq!(size, (202, 52));
        // Fractional scale: everything still covered.
        let (at, size) = layout((-3.3, -1.0, 7.7, 1.0), 1.37).unwrap();
        assert!(at.0 as f32 >= 3.3 * 1.37 && (size.0 - at.0) as f32 >= 7.7 * 1.37);
        assert!(at.1 as f32 >= 1.37 && (size.1 - at.1) as f32 >= 1.37);
        // Nothing to draw, or far too big.
        assert!(layout((f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY), 1.0).is_none());
        assert!(layout((0.0, 0.0, 5000.0, 10.0), 1.0).is_none());
        assert!(layout((0.0, 0.0, 10.0, 10.0), 0.0).is_none());
    }

    #[test]
    fn snaps_only_when_pixels_line_up() {
        // The cabinet's playfield: 2160x3840 turned 270 degrees, 2x scale.
        // Built as `face_player` then `scale` build it on the canvas.
        let mut t = Transform2D::scaling(2.0, 2.0);
        t *= Transform2D::translation(-1080.0, -1920.0);
        t *= Transform2D::rotation(270f32.to_radians());
        t *= Transform2D::translation(1920.0, 1080.0);
        assert!(square_on(&t));
        let (x, y) = snap(&t, 96.3, 120.2).unwrap();
        let (dx, dy) = t.transform_point(x, y);
        assert!((dx - dx.round()).abs() < 1e-3 && (dy - dy.round()).abs() < 1e-3, "{dx} {dy}");
        assert!((x - 96.3).abs() <= 0.25 + 1e-3 && (y - 120.2).abs() <= 0.25 + 1e-3);
        // A tilted wheel (Arc) isn't snapped.
        let mut tilted = t;
        tilted *= Transform2D::rotation(0.1);
        assert!(snap(&tilted, 1.0, 1.0).is_none());
        assert_eq!(picture_scale(&t), 2.0);
    }

    #[test]
    fn groups_are_relative_to_their_anchor() {
        // What is drawn matters to the key, not where the group is drawn.
        let mk = |x: f32, w: f32| {
            let mut g = Group::new(x, 10.0);
            g.pill(x + 5.0, 20.0, w, 42.0, 21.0, Color::white(), Color::black(), 1.5);
            g
        };
        assert_eq!(mk(0.0, 80.0).ops, mk(300.0, 80.0).ops);
        assert_eq!(mk(0.0, 80.0).key(), mk(300.0, 80.0).key());
        assert_ne!(mk(0.0, 80.0).key(), mk(0.0, 81.0).key());
    }
}
