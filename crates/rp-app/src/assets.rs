//! Background loading of wheel logos. Each logo is decoded, shrunk to at most
//! `LOGO_MAX` px, and paired with a pre-blurred glow sprite in the logo's
//! dominant color, so the render thread only uploads textures.

use image::{imageops, imageops::FilterType, GrayImage, RgbaImage};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

const LOGO_MAX: u32 = 512;
/// The glow sprite is the logo silhouette at GLOW_INNER px inside a
/// GLOW_SIZE px canvas, blurred so the halo can spread past the edges.
pub const GLOW_SIZE: u32 = 160;
pub const GLOW_INNER: u32 = 112;

pub struct Logo {
    pub index: usize,
    pub rgba: RgbaImage,
    pub glow: RgbaImage,
    /// Dominant hue in degrees, used for the selection glow and info text.
    pub hue: f32,
}

/// Loads `paths` on a worker thread in `order` (closest to the selection first).
pub fn spawn_loader(paths: Vec<Option<PathBuf>>, order: Vec<usize>) -> Receiver<Logo> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("logo-loader".into())
        .spawn(move || {
            for i in order {
                let Some(path) = &paths[i] else { continue };
                match load(i, path) {
                    Ok(logo) => {
                        if tx.send(logo).is_err() {
                            return;
                        }
                    }
                    Err(e) => eprintln!("wheel {}: {e}", path.display()),
                }
            }
        })
        .expect("spawn logo loader");
    rx
}

fn load(index: usize, path: &PathBuf) -> anyhow::Result<Logo> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    let scale = (LOGO_MAX as f32 / w.max(h) as f32).min(1.0);
    let rgba = if scale < 1.0 {
        imageops::resize(&img, (w as f32 * scale) as u32, (h as f32 * scale) as u32, FilterType::Triangle)
    } else {
        img
    };
    let hue = dominant_hue(&rgba).unwrap_or(195.0);
    let glow = make_glow(&rgba, hue);
    Ok(Logo { index, rgba, glow, hue })
}

fn make_glow(rgba: &RgbaImage, hue: f32) -> RgbaImage {
    let (w, h) = rgba.dimensions();
    let s = GLOW_INNER as f32 / w.max(h) as f32;
    let (iw, ih) = (((w as f32) * s).max(1.0) as u32, ((h as f32) * s).max(1.0) as u32);
    let small = imageops::resize(rgba, iw, ih, FilterType::Triangle);
    let mut alpha = GrayImage::new(GLOW_SIZE, GLOW_SIZE);
    let (ox, oy) = ((GLOW_SIZE - iw) / 2, (GLOW_SIZE - ih) / 2);
    for (x, y, p) in small.enumerate_pixels() {
        alpha.put_pixel(x + ox, y + oy, image::Luma([p[3]]));
    }
    let blurred = imageops::blur(&alpha, 9.0);
    let (r, g, b) = hsl_to_rgb(hue, 1.0, 0.6);
    let mut out = RgbaImage::new(GLOW_SIZE, GLOW_SIZE);
    for (x, y, p) in blurred.enumerate_pixels() {
        let a = (p[0] as f32 * 1.6).min(255.0) as u8;
        out.put_pixel(x, y, image::Rgba([r, g, b, a]));
    }
    out
}

/// Saturation-weighted circular mean of the opaque pixels' hues.
fn dominant_hue(img: &RgbaImage) -> Option<f32> {
    let small = imageops::resize(img, 48, 48, FilterType::Nearest);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for p in small.pixels() {
        if p[3] < 128 {
            continue;
        }
        let (r, g, b) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        if d < 0.15 || max < 0.2 {
            continue;
        }
        let h = if max == r {
            60.0 * (((g - b) / d) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        let wgt = d * max;
        let rad = h.to_radians();
        sx += rad.cos() * wgt;
        sy += rad.sin() * wgt;
    }
    (sx.abs() + sy.abs() > 0.5).then(|| (sy.atan2(sx).to_degrees() + 360.0) % 360.0)
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (f(r), f(g), f(b))
}
