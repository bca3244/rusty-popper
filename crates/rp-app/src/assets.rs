//! Background loading of wheel logos. Each logo is decoded, shrunk to at most
//! `LOGO_MAX` px, and paired with a pre-blurred glow sprite in the logo's
//! dominant color, so the render thread only uploads textures.

use image::{imageops, imageops::FilterType, GrayImage, RgbaImage};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

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

/// Logos being loaded on helper threads, nearest the wheel's selection first.
pub struct Loader {
    rx: Receiver<Logo>,
    near: Arc<AtomicUsize>,
    /// Set when this loader is dropped, so its threads stop early.
    stop: Arc<AtomicBool>,
}

impl Loader {
    /// The next finished logo, if any.
    pub fn try_recv(&self) -> Option<Logo> {
        self.rx.try_recv().ok()
    }

    /// Steers loading towards item `index` (e.g. after a letter jump).
    pub fn set_near(&self, index: usize) {
        self.near.store(index, Ordering::Relaxed);
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A loader with nothing to load.
pub fn idle() -> Loader {
    Loader { rx: channel().1, near: Arc::new(AtomicUsize::new(0)), stop: Arc::new(AtomicBool::new(true)) }
}

/// Loads `paths` on a few helper threads, always picking the unloaded logo
/// closest to the selection (which `Loader::set_near` keeps up to date), so
/// the logos on screen arrive first even after a jump.
pub fn spawn_loader(paths: Vec<Option<PathBuf>>, near: usize) -> Loader {
    let (tx, rx) = channel();
    let near = Arc::new(AtomicUsize::new(near));
    let stop = Arc::new(AtomicBool::new(false));
    let claimed: Arc<Vec<AtomicBool>> = Arc::new(paths.iter().map(|p| AtomicBool::new(p.is_none())).collect());
    let paths = Arc::new(paths);
    // Few threads, at low priority, so the decoders and the menu come first.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() / 2).clamp(1, 2);
    let started = Instant::now();
    let remaining = Arc::new(AtomicUsize::new(threads));
    for t in 0..threads {
        let (tx, near, stop, claimed, paths, remaining) = (tx.clone(), near.clone(), stop.clone(), claimed.clone(), paths.clone(), remaining.clone());
        let spawned = std::thread::Builder::new().name(format!("logo-loader-{t}")).spawn(move || {
            lower_priority();
            while let Some(i) = claim_nearest(&claimed, near.load(Ordering::Relaxed)) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let Some(path) = &paths[i] else { continue };
                // A bad image must not stop the rest from loading, even if the
                // decoder panics on it.
                match std::panic::catch_unwind(|| load(i, path)) {
                    Ok(Ok(logo)) => {
                        if tx.send(logo).is_err() {
                            return;
                        }
                    }
                    Ok(Err(e)) => log::warn!("wheel {}: {e}", path.display()),
                    Err(_) => log::warn!("wheel {}: decoder panicked", path.display()),
                }
            }
            if remaining.fetch_sub(1, Ordering::Relaxed) == 1 {
                crate::perf::note("Loading wheel logos", started, 2000);
            }
        });
        if let Err(e) = spawned {
            log::warn!("Couldn't start a logo loader: {e}");
        }
    }
    Loader { rx, near, stop }
}

#[cfg(windows)]
fn lower_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

#[cfg(not(windows))]
fn lower_priority() {}

/// Claims the unclaimed index closest to `near` (around the wheel's wrap).
fn claim_nearest(claimed: &[AtomicBool], near: usize) -> Option<usize> {
    let n = claimed.len();
    if n == 0 {
        return None;
    }
    let near = near.min(n - 1);
    for d in 0..=n / 2 {
        for i in [(near + d) % n, (near + n - d) % n] {
            if !claimed[i].load(Ordering::Relaxed) && !claimed[i].swap(true, Ordering::Relaxed) {
                return Some(i);
            }
        }
    }
    None
}

fn load(index: usize, path: &PathBuf) -> anyhow::Result<Logo> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    anyhow::ensure!(w > 0 && h > 0, "empty image");
    let scale = (LOGO_MAX as f32 / w.max(h) as f32).min(1.0);
    let rgba = if scale < 1.0 {
        imageops::resize(&img, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1), FilterType::Triangle)
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
    let (iw, ih) = ((((w as f32) * s) as u32).clamp(1, GLOW_INNER), (((h as f32) * s) as u32).clamp(1, GLOW_INNER));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_nearest_first_and_each_once() {
        let claimed: Vec<AtomicBool> = (0..6).map(|i| AtomicBool::new(i == 4)).collect();
        let mut got = Vec::new();
        while let Some(i) = claim_nearest(&claimed, 0) {
            got.push(i);
        }
        assert_eq!(got, [0, 1, 5, 2, 3]);
        assert!(claim_nearest(&[], 0).is_none());
    }
}
