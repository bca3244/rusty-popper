//! Game Info / Help cards: Popper's GameInfo and GameHelp images, shown on
//! the screen Popper's PinUpPlayer.ini assigns them (the playfield if that
//! screen isn't one Rusty Popper drives).

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
    /// Which screen (index into the outputs) shows it.
    pub out: usize,
}

/// How long the "no card" notice stays up.
pub const MISSING_SECS: f32 = 2.0;

impl Card {
    pub fn expired(&self) -> bool {
        self.image.is_none() && self.opened.elapsed().as_secs_f32() > MISSING_SECS
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
        None => {
            let what = if card.kind == CardKind::Info { "info" } else { "help" };
            let size = (w.min(h) * 0.05).max(24.0);
            let paint = Paint::color(Color::rgbaf(0.92, 0.94, 0.98, a))
                .with_font(&[fonts.display])
                .with_font_size(size)
                .with_text_align(Align::Center)
                .with_text_baseline(Baseline::Middle);
            let _ = cv.fill_text(w / 2.0, h / 2.0, format!("No {what} card for this game"), &paint);
        }
    }
}
