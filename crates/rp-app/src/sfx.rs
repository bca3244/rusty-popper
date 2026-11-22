//! Menu sound effects, using Popper's own menu sounds
//! (PUPVideos\PinUpMenu\PuPSounds: fx_flipperup, fx_bumper, ...).
//!
//! The WAVs are small, so each one is decoded once and rebuilt as a stereo
//! 16-bit WAV in memory with the volume (and left/right pan) baked in, then
//! played with PlaySound from memory: instant, no extra player. A new sound
//! cuts off the previous one, which suits fast scrolling.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum Sfx {
    /// Browsing one game; the sign gives the side (Popper pans left/right).
    Move(i64),
    /// Letter / page jump.
    Jump,
    /// Starting a table.
    Launch,
    /// Moving the highlight in a menu.
    MenuMove,
    /// Opening a menu or picking an entry (also switching playlist).
    Select,
}

impl Sfx {
    /// The Popper sound file and pan (-1 left .. 1 right).
    fn file_and_pan(self) -> (&'static str, i8) {
        match self {
            Sfx::Move(d) if d < 0 => ("fx_flipperup", -1),
            Sfx::Move(_) => ("fx_flipperup", 1),
            Sfx::Jump => ("fx_bumper", 0),
            Sfx::Launch => ("fx_knocker", 0),
            Sfx::MenuMove => ("fx_rubber_flipper", 0),
            Sfx::Select => ("fx_kicker", 0),
        }
    }
}

const FILES: &[&str] = &["fx_flipperup", "fx_bumper", "fx_knocker", "fx_rubber_flipper", "fx_kicker"];

/// Decoded PCM: interleaved 16-bit samples.
struct Pcm {
    channels: u16,
    rate: u32,
    samples: Vec<i16>,
}

pub struct Sounds {
    pcm: HashMap<&'static str, Pcm>,
    /// Ready-to-play WAV images for (file, pan).
    built: HashMap<(&'static str, i8), Vec<u8>>,
    pub enabled: bool,
    /// 0..=100.
    pub volume: i32,
}

impl Sounds {
    pub fn load(dir: &Path, enabled: bool, volume: i32) -> Sounds {
        let mut pcm = HashMap::new();
        for &name in FILES {
            let path = dir.join(format!("{name}.wav"));
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| parse_wav(&b)) {
                Ok(p) => {
                    pcm.insert(name, p);
                }
                Err(e) => log::warn!("Sound {}: {e}", path.display()),
            }
        }
        if pcm.is_empty() {
            log::info!("No menu sounds found in {}; sound effects off", dir.display());
        }
        let mut s = Sounds { pcm, built: HashMap::new(), enabled, volume: volume.clamp(0, 100) };
        s.rebuild();
        s
    }

    /// Re-bakes the volume into the WAV images (after a volume change).
    pub fn rebuild(&mut self) {
        stop();
        self.built.clear();
        let gain = self.volume as f32 / 100.0;
        for (&name, pcm) in &self.pcm {
            for pan in [-1i8, 0, 1] {
                // Equal-ish pan: the far side drops to 35%.
                let (l, r) = match pan {
                    -1 => (1.0, 0.35),
                    1 => (0.35, 1.0),
                    _ => (1.0, 1.0),
                };
                self.built.insert((name, pan), build_wav(pcm, gain * l, gain * r));
            }
        }
    }

    pub fn set_volume(&mut self, volume: i32) {
        self.volume = volume.clamp(0, 100);
        self.rebuild();
    }

    pub fn play(&self, sfx: Sfx) {
        if !self.enabled || self.volume == 0 {
            return;
        }
        let key = sfx.file_and_pan();
        if let Some(wav) = self.built.get(&key) {
            use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
            // The buffer lives in `self.built` for as long as it can be
            // playing: rebuild() stops playback before replacing buffers.
            unsafe {
                PlaySoundW(wav.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
            }
        }
    }
}

impl Drop for Sounds {
    fn drop(&mut self) {
        stop();
    }
}

fn stop() {
    use windows_sys::Win32::Media::Audio::PlaySoundW;
    unsafe {
        PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
    }
}

/// Reads a PCM 16-bit WAV (mono or stereo).
fn parse_wav(b: &[u8]) -> Result<Pcm, String> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut pos = 12;
    let (mut fmt, mut data) = (None, None);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let fmt = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    if fmt.len() < 16 {
        return Err("short fmt chunk".into());
    }
    let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
    if tag != 1 || bits != 16 || !(1..=2).contains(&channels) || !(1..=192_000).contains(&rate) {
        return Err(format!("unsupported WAV (format {tag}, {bits}-bit, {channels} channels); need 16-bit PCM up to 192 kHz"));
    }
    let samples = data.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes(*s)).collect();
    Ok(Pcm { channels, rate, samples })
}

/// A stereo 16-bit WAV image of `pcm` with per-side gains.
fn build_wav(pcm: &Pcm, gl: f32, gr: f32) -> Vec<u8> {
    let frames = pcm.samples.len() / pcm.channels as usize;
    let data_len = frames * 4;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&pcm.rate.to_le_bytes());
    out.extend_from_slice(&(pcm.rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    let scale = |s: i16, g: f32| ((s as f32 * g).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes();
    for f in 0..frames {
        let (l, r) = if pcm.channels == 1 {
            let s = pcm.samples[f];
            (s, s)
        } else {
            (pcm.samples[2 * f], pcm.samples[2 * f + 1])
        };
        out.extend_from_slice(&scale(l, gl));
        out.extend_from_slice(&scale(r, gr));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono_wav(samples: &[i16]) -> Vec<u8> {
        let pcm = Pcm { channels: 1, rate: 22050, samples: samples.to_vec() };
        // Build a mono file by hand via the stereo builder's header shape.
        let st = build_wav(&pcm, 1.0, 1.0);
        let mut b = st[..44].to_vec();
        b[22] = 1; // channels
        b[32] = 2; // block align
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        b[40..44].copy_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend(data);
        b
    }

    #[test]
    fn mono_round_trips_to_panned_stereo() {
        let p = parse_wav(&mono_wav(&[1000, -2000, 3000])).unwrap();
        assert_eq!((p.channels, p.rate, p.samples.len()), (1, 22050, 3));
        let w = build_wav(&p, 1.0, 0.5);
        let s: Vec<i16> = w[44..].as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect();
        assert_eq!(s, [1000, 500, -2000, -1000, 3000, 1500]);
        assert_eq!(parse_wav(&w).unwrap().channels, 2);
    }

    #[test]
    fn rejects_non_wav() {
        assert!(parse_wav(b"hello").is_err());
    }
}
