//! Game Info / Help cards: Popper's GameInfo and GameHelp images, shown on
//! the screen Popper's PinUpPlayer.ini assigns them (the playfield if that
//! screen isn't one Rusty Popper drives).

use crate::textcache::Group;
use crate::wheel::Fonts;
use femtovg::{renderer::OpenGl, Align, Baseline, Canvas, Color, ImageId, Paint, Path};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Info,
    Help,
}

pub struct Card {
    pub kind: CardKind,
    /// The card image and its size, or None when the game has no card.
    pub image: Option<(ImageId, f32, f32)>,
    pub opened: Instant,
    /// The image while it's still being decoded off the UI thread.
    pub pending: Option<std::sync::mpsc::Receiver<Option<image::RgbaImage>>>,
    /// Which screen (index into the outputs) shows it.
    pub out: usize,
}

/// How long the "no card" notice stays up.
pub const MISSING_SECS: f32 = 2.0;

impl Card {
    pub fn expired(&self) -> bool {
        self.image.is_none() && self.pending.is_none() && self.opened.elapsed().as_secs_f32() > MISSING_SECS
    }
}

/// Draws the card over a `w`x`h` frame (the canvas transform is already set up).
pub fn draw(cv: &mut Canvas<OpenGl>, w: f32, h: f32, card: &Card, fonts: &Fonts) {
    let t = card.opened.elapsed().as_secs_f32();
    let a = (t / 0.2).min(1.0);
    let mut p = Path::new();
    p.rect(0.0, 0.0, w, h);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.0, 0.0, 0.0, 0.78 * a)));
    match card.image {
        Some((img, iw, ih)) => {
            let s = (w * 0.94 / iw).min(h * 0.94 / ih);
            let (dw, dh) = (iw * s, ih * s);
            let (x, y) = ((w - dw) / 2.0, (h - dh) / 2.0 + (1.0 - a) * 0.03 * h);
            let mut p = Path::new();
            p.rect(x, y, dw, dh);
            cv.fill_path(&p, &Paint::image(img, x, y, dw, dh, 0.0, a));
        }
        None if card.pending.is_some() => {}
        None => {
            let what = if card.kind == CardKind::Info { "info" } else { "help" };
            let size = (w.min(h) * 0.05).max(24.0);
            // From a picture (see `textcache`) that fades in.
            let mut g = Group::new(w / 2.0, h / 2.0);
            g.text(w / 2.0, h / 2.0, format!("No {what} card for this game"), fonts.display, size, Color::rgbaf(0.92, 0.94, 0.98, 1.0), Align::Center, Baseline::Middle);
            fonts.text.borrow_mut().draw(cv, ("no card", 0), &g, 0.0, 0.0, a, 1.0);
        }
    }
}

/// How long a notice ("Couldn't start ...") stays up.
pub const NOTICE_SECS: f32 = 4.0;

/// Draws a one-line notice on a dark band across the middle of a `w`x`h`
/// frame, fading in and out. `t` is seconds since it appeared.
pub fn draw_notice(cv: &mut Canvas<OpenGl>, w: f32, h: f32, text: &str, t: f32, fonts: &Fonts) {
    let a = (t / 0.2).min(1.0).min(((NOTICE_SECS - t) / 0.4).clamp(0.0, 1.0));
    let mut size = (w.min(h) * 0.045).max(22.0);
    // A long game title shrinks to fit the width.
    let probe = Paint::color(Color::white()).with_font(&[fonts.display]).with_font_size(size);
    let tw = cv.measure_text(0.0, 0.0, text, &probe).map(|m| m.width()).unwrap_or(0.0);
    if tw > w * 0.92 {
        size *= w * 0.92 / tw;
    }
    let band = size * 2.6;
    let mut p = Path::new();
    p.rect(0.0, (h - band) / 2.0, w, band);
    cv.fill_path(&p, &Paint::color(Color::rgbaf(0.0, 0.0, 0.0, 0.8 * a)));
    // From a picture (see `textcache`) that fades in and out.
    let mut g = Group::new(w / 2.0, h / 2.0);
    g.text(w / 2.0, h / 2.0, text, fonts.display, size, Color::rgbaf(1.0, 0.72, 0.62, 1.0), Align::Center, Baseline::Middle);
    fonts.text.borrow_mut().draw(cv, ("notice", 0), &g, 0.0, 0.0, a, 1.0);
}
