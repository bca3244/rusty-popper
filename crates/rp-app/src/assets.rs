//! Background loading of wheel logos. Each logo is decoded, shrunk to at most
//! `LOGO_MAX` px, and paired with a pre-blurred glow sprite in the logo's
//! dominant color, so the render thread only uploads textures. Only the
//! logos near the wheel's selection are loaded, and each is kept in a disk
//! cache once made, so loading it again is a quick read.

use image::{imageops, imageops::FilterType, GrayImage, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

/// Logos are loaded this many places either side of the selection...
pub const LOAD_AROUND: usize = 60;
/// ...and freed once they are further than this (the wheel draws at most
/// seven each side, and lags the selection by at most six), so a big
/// library doesn't keep every logo in video memory, while scrolling back and
/// forth doesn't reload them.
pub const KEEP_AROUND: usize = 120;

/// The view positions within `radius` places of `sel` on a wheel of `n`
/// (which wraps), nearest first, each once.
pub fn around(n: usize, sel: usize, radius: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let sel = sel % n;
    let mut out = Vec::with_capacity((2 * radius + 1).min(n));
    out.push(sel);
    let mut seen = if 2 * radius + 1 >= n { vec![false; n] } else { Vec::new() };
    if let Some(s) = seen.get_mut(sel) {
        *s = true;
    }
    for d in 1..=radius.min(n) {
        for p in [(sel + d) % n, (sel + n - d % n) % n] {
            match seen.get_mut(p) {
                Some(s) if *s => {}
                Some(s) => {
                    *s = true;
                    out.push(p);
                }
                None => out.push(p),
            }
        }
    }
    out
}

/// Which of `items` items are within `radius` places of view position
/// `sel` (the logos to keep).
pub fn near_set(view: &[usize], sel: usize, radius: usize, items: usize) -> Vec<bool> {
    let mut keep = vec![false; items];
    for p in around(view.len(), sel, radius) {
        if let Some(k) = keep.get_mut(view[p]) {
            *k = true;
        }
    }
    keep
}

struct Queue {
    /// Items to load, the nearest last (popped first).
    todo: Mutex<Vec<usize>>,
    ready: Condvar,
    /// Set when the loader is dropped, so its threads stop.
    stop: AtomicBool,
}

/// Logos being loaded on helper threads, the ones the render thread asks for
/// (those near the wheel's selection) nearest first.
pub struct Loader {
    rx: Receiver<Logo>,
    queue: Arc<Queue>,
    /// Every item's logo file (None: no logo); empty for an idle loader.
    paths: Arc<Vec<Option<PathBuf>>>,
    /// Items asked for and not back yet (render thread only). A logo that
    /// can't be loaded stays here, so it isn't tried again.
    pending: Vec<bool>,
}

impl Loader {
    /// The next finished logo, if any.
    pub fn try_recv(&mut self) -> Option<Logo> {
        let logo = self.rx.try_recv().ok()?;
        if let Some(p) = self.pending.get_mut(logo.index) {
            *p = false;
        }
        Some(logo)
    }

    /// Asks for the logos of `view` within LOAD_AROUND places of position
    /// `sel`, nearest first, except those already `loaded`. Anything asked
    /// for before and not yet started is dropped first, so after a jump or
    /// fast scrolling the logos on screen come next.
    pub fn want(&mut self, view: &[usize], sel: usize, loaded: impl Fn(usize) -> bool) {
        if self.paths.is_empty() {
            return;
        }
        let Ok(mut todo) = self.queue.todo.lock() else { return };
        for i in todo.drain(..) {
            self.pending[i] = false;
        }
        for p in around(view.len(), sel, LOAD_AROUND) {
            let i = view[p];
            if self.paths.get(i).is_some_and(Option::is_some) && !self.pending[i] && !loaded(i) {
                self.pending[i] = true;
                todo.push(i);
            }
        }
        todo.reverse();
        self.queue.ready.notify_all();
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // Under the lock, so a thread about to wait can't miss it and wait for ever.
        let _todo = self.queue.todo.lock();
        self.queue.stop.store(true, Ordering::Relaxed);
        self.queue.ready.notify_all();
    }
}

/// A loader with nothing to load.
pub fn idle() -> Loader {
    let queue = Arc::new(Queue { todo: Mutex::new(Vec::new()), ready: Condvar::new(), stop: AtomicBool::new(true) });
    Loader { rx: channel().1, queue, paths: Arc::new(Vec::new()), pending: Vec::new() }
}

/// Starts a few helper threads that load the logos asked for with
/// `Loader::want`, from the disk cache in `cache` when it has them.
/// `paths` has every item's logo file.
pub fn spawn_loader(paths: Vec<Option<PathBuf>>, cache: Option<PathBuf>) -> Loader {
    let (tx, rx) = channel();
    let queue = Arc::new(Queue { todo: Mutex::new(Vec::new()), ready: Condvar::new(), stop: AtomicBool::new(false) });
    let pending = vec![false; paths.len()];
    let paths = Arc::new(paths);
    if let Some(dir) = &cache {
        clean_cache_once(dir, &paths);
    }
    // Few threads, at low priority, so the decoders and the menu come first.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() / 2).clamp(1, 2);
    for t in 0..threads {
        let (tx, queue, paths, cache) = (tx.clone(), queue.clone(), paths.clone(), cache.clone());
        let spawned = std::thread::Builder::new().name(format!("logo-loader-{t}")).spawn(move || {
            lower_priority();
            let mut busy_since: Option<Instant> = None;
            loop {
                let i = {
                    let Ok(mut todo) = queue.todo.lock() else { return };
                    loop {
                        if queue.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(i) = todo.pop() {
                            break i;
                        }
                        // Caught up: say how long that took (from the first
                        // logo of this batch), then wait for more.
                        if let Some(at) = busy_since.take() {
                            crate::perf::note("Loading wheel logos", at, 2000);
                        }
                        let Ok(t) = queue.ready.wait(todo) else { return };
                        todo = t;
                    }
                };
                busy_since.get_or_insert_with(Instant::now);
                let Some(path) = &paths[i] else { continue };
                // A bad image must not stop the rest from loading, even if the
                // decoder panics on it.
                match std::panic::catch_unwind(|| load_cached(i, path, cache.as_deref())) {
                    Ok(Ok(logo)) => {
                        if tx.send(logo).is_err() {
                            return;
                        }
                    }
                    Ok(Err(e)) => log::warn!("wheel {}: {e}", path.display()),
                    Err(_) => log::warn!("wheel {}: decoder panicked", path.display()),
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("Couldn't start a logo loader: {e}");
        }
    }
    Loader { rx, queue, paths, pending }
}

#[cfg(windows)]
fn lower_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

#[cfg(not(windows))]
fn lower_priority() {}

fn load(index: usize, path: &Path) -> anyhow::Result<(Logo, Vec<u8>)> {
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
    let glow_alpha = glow_alpha(&rgba);
    let glow = tint(&glow_alpha, hue);
    Ok((Logo { index, rgba, glow, hue }, glow_alpha))
}

/// The glow's opacity: the logo's silhouette, small, blurred and boosted.
fn glow_alpha(rgba: &RgbaImage) -> Vec<u8> {
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
    blurred.into_raw().into_iter().map(|a| (a as f32 * 1.6).min(255.0) as u8).collect()
}

/// The glow sprite: its opacity in the logo's hue.
fn tint(alpha: &[u8], hue: f32) -> RgbaImage {
    let (r, g, b) = hsl_to_rgb(hue, 1.0, 0.6);
    let raw = alpha.iter().flat_map(|&a| [r, g, b, a]).collect();
    RgbaImage::from_raw(GLOW_SIZE, GLOW_SIZE, raw).unwrap_or_else(|| RgbaImage::new(GLOW_SIZE, GLOW_SIZE))
}

// The disk cache: each logo as it comes out of `load` (shrunk, with its glow
// and hue), so the next start, or the return from a game, reads a small PNG
// instead of decoding and shrinking the full-size wheel image again. It sits
// in `cache\logos` next to the exe. Anything wrong with it just means the
// logo is loaded from its file as before.

/// Bump when what's stored (or how it's made) changes: old files then miss.
const CACHE_VERSION: u8 = 1;
const MAGIC: &[u8; 7] = b"RPLOGO\0";
const HEADER: usize = 8 + 8 + 4 * 5;
/// The cache is trimmed (oldest first) to this size once a run.
const CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// The cache file's key for a logo file of `len` bytes changed at `mtime`
/// (ns since 1970): FNV-1a, which (unlike std's hasher) stays the same
/// from one build to the next.
fn cache_key(path: &Path, len: u64, mtime: u128) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(path.to_string_lossy().as_bytes());
    eat(&len.to_le_bytes());
    eat(&mtime.to_le_bytes());
    eat(&[CACHE_VERSION]);
    eat(&LOGO_MAX.to_le_bytes());
    eat(&GLOW_SIZE.to_le_bytes());
    eat(&GLOW_INNER.to_le_bytes());
    h
}

/// The cache file for `path` as it is now, and its key.
fn cache_file(dir: &Path, path: &Path) -> Option<(PathBuf, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let key = cache_key(path, meta.len(), mtime);
    Some((dir.join(format!("{key:016x}.logo")), key))
}

/// A cache file's contents: header, glow opacity, then the logo as PNG.
fn encode_cached(key: u64, hue: f32, (w, h): (u32, u32), glow_alpha: &[u8], png: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + glow_alpha.len() + png.len());
    out.extend_from_slice(MAGIC);
    out.push(CACHE_VERSION);
    out.extend_from_slice(&key.to_le_bytes());
    for v in [w, h, hue.to_bits(), glow_alpha.len() as u32, png.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(glow_alpha);
    out.extend_from_slice(png);
    out
}

struct Cached<'a> {
    w: u32,
    h: u32,
    hue: f32,
    glow_alpha: &'a [u8],
    png: &'a [u8],
}

/// Reads a cache file's contents, if they are whole and for `key`.
fn parse_cached(bytes: &[u8], key: u64) -> Option<Cached<'_>> {
    if bytes.len() < HEADER || &bytes[..7] != MAGIC || bytes[7] != CACHE_VERSION {
        return None;
    }
    let u64_at = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap_or_default());
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap_or_default());
    if u64_at(8) != key {
        return None;
    }
    let (w, h, hue, glow_len, png_len) = (u32_at(16), u32_at(20), f32::from_bits(u32_at(24)), u32_at(28) as usize, u32_at(32) as usize);
    let ok = w > 0 && h > 0 && w.max(h) <= LOGO_MAX && hue.is_finite() && glow_len == (GLOW_SIZE * GLOW_SIZE) as usize;
    if !ok || bytes.len() != HEADER + glow_len + png_len {
        return None;
    }
    let glow_alpha = &bytes[HEADER..HEADER + glow_len];
    Some(Cached { w, h, hue, glow_alpha, png: &bytes[HEADER + glow_len..] })
}

fn read_cached(index: usize, file: &Path, key: u64) -> Option<Logo> {
    let bytes = std::fs::read(file).ok()?;
    let c = parse_cached(&bytes, key)?;
    let rgba = image::load_from_memory_with_format(c.png, image::ImageFormat::Png).ok()?.to_rgba8();
    if rgba.dimensions() != (c.w, c.h) {
        return None;
    }
    Some(Logo { index, rgba, glow: tint(c.glow_alpha, c.hue), hue: c.hue })
}

fn write_cached(file: &Path, key: u64, logo: &Logo, glow_alpha: &[u8]) -> anyhow::Result<()> {
    use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
    use image::ImageEncoder;
    let mut png = Vec::new();
    // Fast compression: quick to write, and it reads back quickly too.
    let (w, h) = logo.rgba.dimensions();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive).write_image(logo.rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)?;
    let bytes = encode_cached(key, logo.hue, (w, h), glow_alpha, &png);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written aside and renamed, so a file is either whole or not there.
    let tmp = file.with_extension(format!("{:?}.tmp", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', ""));
    std::fs::write(&tmp, &bytes)?;
    if let Err(e) = std::fs::rename(&tmp, file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Loads a logo from the cache, or from its file (then caches it).
fn load_cached(index: usize, path: &Path, cache: Option<&Path>) -> anyhow::Result<Logo> {
    let file = cache.and_then(|dir| cache_file(dir, path));
    if let Some(logo) = file.as_ref().and_then(|(f, key)| read_cached(index, f, *key)) {
        return Ok(logo);
    }
    let (logo, glow_alpha) = load(index, path)?;
    if let Some((f, key)) = &file {
        if let Err(e) = write_cached(f, *key, &logo, &glow_alpha) {
            log::debug!("wheel cache {}: {e}", f.display());
        }
    }
    Ok(logo)
}

/// Once a run, on a helper thread: removes cache files for logos that are
/// no longer in the library (or have changed), and the oldest ones beyond
/// CACHE_MAX_BYTES.
fn clean_cache_once(dir: &Path, paths: &Arc<Vec<Option<PathBuf>>>) {
    static CLEANED: AtomicBool = AtomicBool::new(false);
    if CLEANED.swap(true, Ordering::Relaxed) {
        return;
    }
    let (dir, paths) = (dir.to_path_buf(), paths.clone());
    let _ = std::thread::Builder::new().name("logo-cache-clean".into()).spawn(move || {
        lower_priority();
        let Ok(list) = std::fs::read_dir(&dir) else { return };
        let wanted: std::collections::HashSet<std::ffi::OsString> =
            paths.iter().flatten().filter_map(|p| cache_file(&dir, p)).filter_map(|(f, _)| f.file_name().map(|n| n.to_os_string())).collect();
        let now = SystemTime::now();
        let mut kept = Vec::new();
        for e in list.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            let changed = meta.modified().unwrap_or(now);
            let name = e.file_name();
            let tmp = name.to_string_lossy().ends_with(".tmp");
            // A half-written file from a crash (not one being written now).
            let stale_tmp = tmp && now.duration_since(changed).is_ok_and(|d| d.as_secs() > 600);
            if stale_tmp || (!tmp && !wanted.contains(&name)) {
                let _ = std::fs::remove_file(e.path());
            } else if !tmp {
                kept.push((changed, meta.len(), e.path()));
            }
        }
        for f in over_budget(kept, CACHE_MAX_BYTES) {
            let _ = std::fs::remove_file(f);
        }
    });
}

/// The files to remove, oldest first, to bring `files` (changed, size,
/// path) within `max` bytes.
fn over_budget(mut files: Vec<(SystemTime, u64, PathBuf)>, max: u64) -> Vec<PathBuf> {
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    let mut out = Vec::new();
    for (_, len, path) in files {
        if total <= max {
            break;
        }
        total -= len;
        out.push(path);
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
    fn around_goes_nearest_first_and_wraps() {
        assert_eq!(around(10, 0, 2), [0, 1, 9, 2, 8]);
        assert_eq!(around(10, 9, 1), [9, 0, 8]);
        // A small wheel: each position once.
        let mut all = around(5, 3, 60);
        assert_eq!(all[0], 3);
        all.sort();
        assert_eq!(all, [0, 1, 2, 3, 4]);
        assert_eq!(around(4, 1, 2).len(), 4);
        assert_eq!(around(1, 0, 60), [0]);
        assert!(around(0, 0, 60).is_empty());
    }

    #[test]
    fn keeps_the_logos_the_wheel_can_draw() {
        // A playlist (view) of every other item, out of order.
        let view: Vec<usize> = (0..1000).rev().filter(|i| i % 2 == 0).collect();
        let keep = near_set(&view, 0, KEEP_AROUND, 1000);
        // The wheel draws up to 7 each side, and may lag 6 behind the selection.
        for d in 0..=13 {
            assert!(keep[view[d]] && keep[view[view.len() - 1 - d]]);
        }
        assert_eq!(keep.iter().filter(|&&k| k).count(), 2 * KEEP_AROUND + 1);
        assert!(!keep[1] && !keep[view[250]]);
        // Everything asked for loading is kept.
        let load = near_set(&view, 0, LOAD_AROUND, 1000);
        assert!(load.iter().zip(&keep).all(|(l, k)| !l || *k));
    }

    #[test]
    fn cache_files_read_back_only_when_whole_and_matching() {
        let glow = vec![7u8; (GLOW_SIZE * GLOW_SIZE) as usize];
        let bytes = encode_cached(42, 123.5, (300, 200), &glow, b"pngdata");
        let c = parse_cached(&bytes, 42).unwrap();
        assert_eq!((c.w, c.h, c.hue, c.png), (300, 200, 123.5, &b"pngdata"[..]));
        assert_eq!(c.glow_alpha, &glow[..]);
        // Another logo's file, a cut-off one, an older format, junk.
        assert!(parse_cached(&bytes, 43).is_none());
        assert!(parse_cached(&bytes[..bytes.len() - 1], 42).is_none());
        let mut old = bytes.clone();
        old[7] = CACHE_VERSION.wrapping_add(1);
        assert!(parse_cached(&old, 42).is_none());
        assert!(parse_cached(b"RPLOGO", 42).is_none());
        assert!(parse_cached(&[], 42).is_none());
    }

    #[test]
    fn cache_key_changes_with_the_file() {
        let p = Path::new(r"C:\vPinball\PinUPSystem\POPMedia\Visual Pinball X\Wheel\Attack.png");
        let k = cache_key(p, 1000, 5);
        assert_eq!(k, cache_key(p, 1000, 5));
        assert_ne!(k, cache_key(p, 1001, 5));
        assert_ne!(k, cache_key(p, 1000, 6));
        assert_ne!(k, cache_key(Path::new(r"C:\other.png"), 1000, 5));
    }

    #[test]
    fn a_cached_logo_is_the_same_as_a_fresh_one() {
        let dir = std::env::temp_dir().join(format!("rp-logo-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("wheel.png");
        let img = RgbaImage::from_fn(900, 400, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 40, if (x / 50 + y / 50) % 2 == 0 { 255 } else { 0 }]));
        img.save(&src).unwrap();
        let cache = dir.join("cache");
        let fresh = load_cached(3, &src, Some(&cache)).unwrap();
        let (file, _) = cache_file(&cache, &src).unwrap();
        assert!(file.is_file(), "nothing cached");
        let again = load_cached(3, &src, Some(&cache)).unwrap();
        assert_eq!(again.index, 3);
        assert_eq!(again.rgba, fresh.rgba);
        assert_eq!(again.glow, fresh.glow);
        assert_eq!(again.hue, fresh.hue);
        // A damaged cache file: loaded from the source (and cached again).
        std::fs::write(&file, b"junk").unwrap();
        assert_eq!(load_cached(3, &src, Some(&cache)).unwrap().rgba, fresh.rgba);
        assert!(parse_cached(&std::fs::read(&file).unwrap(), cache_file(&cache, &src).unwrap().1).is_some());
        // No cache folder to be had: still loads.
        let blocked = dir.join("wheel.png").join("cache");
        assert_eq!(load_cached(3, &src, Some(&blocked)).unwrap().rgba, fresh.rgba);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trims_the_oldest_files_to_the_budget() {
        let t = |s: u64| UNIX_EPOCH + std::time::Duration::from_secs(s);
        let files = vec![(t(3), 10, PathBuf::from("c")), (t(1), 10, PathBuf::from("a")), (t(2), 10, PathBuf::from("b"))];
        assert_eq!(over_budget(files.clone(), 30), Vec::<PathBuf>::new());
        assert_eq!(over_budget(files.clone(), 15), [PathBuf::from("a"), PathBuf::from("b")]);
        assert_eq!(over_budget(files, 0).len(), 3);
    }
}
