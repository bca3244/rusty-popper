//! Menu sound effects, using Popper's own menu sounds
//! (PUPVideos\PinUpMenu\PuPSounds: fx_flipperup, fx_bumper, ...).
//!
//! The WAVs are small, so each one is decoded once and baked into stereo
//! 44.1 kHz samples with the volume (and left/right pan) applied. A mixer
//! thread keeps one waveOut stream open while sounds play and mixes up to
//! [`MAX_VOICES`] at once, so fast scrolling never waits on the audio device
//! and sounds overlap instead of cutting each other off. With nothing
//! playing, the thread sleeps and the device sits idle. The device is
//! opened on the first sound and reopened if it fails or goes away.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

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

/// The mixer's output rate.
const RATE: u32 = 44_100;
/// 10 ms per buffer, 6 buffers: at most 60 ms queued ahead.
const BUF_FRAMES: usize = 441;
const BUFS: usize = 6;
/// Sounds playing at once; a new one past this cuts the oldest.
const MAX_VOICES: usize = 4;

/// Decoded PCM: interleaved 16-bit samples.
struct Pcm {
    channels: u16,
    rate: u32,
    samples: Vec<i16>,
}

enum Cmd {
    Play(Arc<[i16]>),
    Stop,
}

pub struct Sounds {
    pcm: HashMap<&'static str, Pcm>,
    /// Ready-to-mix stereo samples for (file, pan).
    built: HashMap<(&'static str, i8), Arc<[i16]>>,
    /// The mixer thread (None without sounds).
    mixer: Option<mpsc::Sender<Cmd>>,
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
        let mixer = if pcm.is_empty() {
            log::info!("No menu sounds found in {}; sound effects off", dir.display());
            None
        } else {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new().name("sfx".into()).spawn(move || mixer(rx)) {
                Ok(_) => Some(tx),
                Err(e) => {
                    log::warn!("Couldn't start the sound mixer: {e}");
                    None
                }
            }
        };
        let mut s = Sounds { pcm, built: HashMap::new(), mixer, enabled, volume: volume.clamp(0, 100) };
        s.rebuild();
        s
    }

    /// Re-bakes the volume into the samples (after a volume change).
    pub fn rebuild(&mut self) {
        if let Some(tx) = &self.mixer {
            let _ = tx.send(Cmd::Stop);
        }
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
                self.built.insert((name, pan), build_stereo(pcm, gain * l, gain * r).into());
            }
        }
    }

    pub fn set_volume(&mut self, volume: i32) {
        self.volume = volume.clamp(0, 100);
        self.rebuild();
    }

    /// Queues a sound; never blocks.
    pub fn play(&self, sfx: Sfx) {
        if !self.enabled || self.volume == 0 {
            return;
        }
        if let (Some(tx), Some(s)) = (&self.mixer, self.built.get(&sfx.file_and_pan())) {
            let _ = tx.send(Cmd::Play(s.clone()));
        }
    }
}

/// One sound in the mix.
struct Voice {
    data: Arc<[i16]>,
    pos: usize,
}

fn handle(cmd: Cmd, voices: &mut Vec<Voice>) {
    match cmd {
        Cmd::Play(data) => {
            if voices.len() >= MAX_VOICES {
                voices.remove(0);
            }
            voices.push(Voice { data, pos: 0 });
        }
        Cmd::Stop => voices.clear(),
    }
}

/// Mixes the next stretch of every voice into `out` (stereo samples) and
/// drops the voices that have finished.
fn mix(out: &mut [i16], voices: &mut Vec<Voice>) {
    let mut acc = vec![0i32; out.len()];
    for v in voices.iter_mut() {
        let n = (v.data.len() - v.pos).min(out.len());
        for (a, s) in acc.iter_mut().zip(&v.data[v.pos..v.pos + n]) {
            *a += *s as i32;
        }
        v.pos += n;
    }
    voices.retain(|v| v.pos < v.data.len());
    for (o, a) in out.iter_mut().zip(acc) {
        *o = a.clamp(-32768, 32767) as i16;
    }
}

/// A buffer handed to the device and not back after this long means the
/// device has gone (unplugged, or the audio service restarted).
const STUCK_AFTER: Duration = Duration::from_secs(1);
/// After the device couldn't be opened, wait this long before trying again
/// (on the next sound).
const REOPEN_AFTER: Duration = Duration::from_secs(5);

/// The open waveOut stream and its buffers.
struct Device {
    dev: windows_sys::Win32::Media::Audio::HWAVEOUT,
    // The sample buffers and their headers stay put (on the heap, never
    // resized) while the device may be reading them, and are only freed
    // after waveOutReset (in Drop).
    _bufs: Vec<Box<[i16]>>,
    hdrs: Vec<windows_sys::Win32::Media::Audio::WAVEHDR>,
    /// When each buffer was handed to the device, while it has it.
    queued: [Option<Instant>; BUFS],
}

const HDR_SIZE: u32 = std::mem::size_of::<windows_sys::Win32::Media::Audio::WAVEHDR>() as u32;

impl Device {
    fn open() -> Result<Device, u32> {
        use windows_sys::Win32::Media::Audio::{waveOutOpen, waveOutPrepareHeader, CALLBACK_NULL, HWAVEOUT, WAVEFORMATEX, WAVEHDR, WAVE_FORMAT_PCM, WAVE_MAPPER};
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut dev: HWAVEOUT = std::ptr::null_mut();
        let r = unsafe { waveOutOpen(&mut dev, WAVE_MAPPER, &fmt, 0, 0, CALLBACK_NULL) };
        if r != 0 {
            return Err(r);
        }
        let mut bufs: Vec<Box<[i16]>> = (0..BUFS).map(|_| vec![0i16; BUF_FRAMES * 2].into_boxed_slice()).collect();
        let mut hdrs: Vec<WAVEHDR> = bufs
            .iter_mut()
            .map(|buf| {
                let mut h: WAVEHDR = unsafe { std::mem::zeroed() };
                h.lpData = buf.as_mut_ptr().cast();
                h.dwBufferLength = (BUF_FRAMES * 4) as u32;
                h
            })
            .collect();
        // Prepared where they'll stay.
        for h in hdrs.iter_mut() {
            unsafe { waveOutPrepareHeader(dev, h, HDR_SIZE) };
        }
        Ok(Device { dev, _bufs: bufs, hdrs, queued: [None; BUFS] })
    }

    /// Takes back the buffers the device has finished with. Returns false if
    /// one has been out far too long (the device is gone).
    fn reap(&mut self) -> bool {
        use windows_sys::Win32::Media::Audio::WHDR_DONE;
        for (q, h) in self.queued.iter_mut().zip(&self.hdrs) {
            // WAVEHDR is packed, so read the flag unaligned; the fence stops the
            // compiler from reusing an earlier read (the driver sets it).
            std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
            if q.is_some() && unsafe { std::ptr::addr_of!(h.dwFlags).read_unaligned() } & WHDR_DONE != 0 {
                *q = None;
            }
        }
        !self.queued.iter().flatten().any(|at| at.elapsed() >= STUCK_AFTER)
    }

    fn busy(&self) -> bool {
        self.queued.iter().any(Option::is_some)
    }

    /// Mixes the voices into every free buffer and hands those to the device.
    fn fill(&mut self, voices: &mut Vec<Voice>) -> Result<(), u32> {
        use windows_sys::Win32::Media::Audio::waveOutWrite;
        for (i, h) in self.hdrs.iter_mut().enumerate() {
            if self.queued[i].is_some() || voices.is_empty() {
                continue;
            }
            // SAFETY: this buffer isn't queued, so the device isn't reading it.
            let out = unsafe { std::slice::from_raw_parts_mut(h.lpData.cast::<i16>(), BUF_FRAMES * 2) };
            mix(out, voices);
            let r = unsafe { waveOutWrite(self.dev, h, HDR_SIZE) };
            if r != 0 {
                return Err(r);
            }
            self.queued[i] = Some(Instant::now());
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        use windows_sys::Win32::Media::Audio::{waveOutClose, waveOutReset, waveOutUnprepareHeader};
        unsafe {
            waveOutReset(self.dev);
            for h in self.hdrs.iter_mut() {
                waveOutUnprepareHeader(self.dev, h, HDR_SIZE);
            }
            waveOutClose(self.dev);
        }
    }
}

/// The mixer thread: runs until the [`Sounds`] is dropped. The device is
/// opened on the first sound, and again after it failed or went away (at
/// most every few seconds), so a device that wasn't ready at boot, or came
/// back, still gets the menu's sounds.
fn mixer(rx: mpsc::Receiver<Cmd>) {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let mut device: Option<Device> = None;
    let mut next_open = Instant::now();
    let mut voices: Vec<Voice> = Vec::new();
    // Warn once per kind of trouble, not on every sound.
    let (mut warned_open, mut warned_write) = (false, false);
    'run: loop {
        // Has the device finished with the buffers we gave it?
        if device.as_mut().is_some_and(|d| !d.reap()) {
            log::warn!("The audio device stopped taking menu sounds; reopening it");
            device = None;
            next_open = Instant::now() + REOPEN_AFTER;
        }
        if voices.is_empty() && !device.as_ref().is_some_and(Device::busy) {
            // Nothing playing: sleep until the next sound.
            match rx.recv() {
                Ok(c) => handle(c, &mut voices),
                Err(_) => break 'run,
            }
        }
        loop {
            match rx.try_recv() {
                Ok(c) => handle(c, &mut voices),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'run,
            }
        }
        if device.is_none() && !voices.is_empty() && Instant::now() >= next_open {
            match Device::open() {
                Ok(d) => device = Some(d),
                Err(r) => {
                    if !warned_open {
                        log::warn!("Couldn't open the audio device for menu sounds (error {r}); trying again later");
                        warned_open = true;
                    }
                    next_open = Instant::now() + REOPEN_AFTER;
                }
            }
        }
        match device.as_mut() {
            Some(d) => {
                if let Err(r) = d.fill(&mut voices) {
                    if !warned_write {
                        log::warn!("Menu sound output failed (error {r}); reopening the device");
                        warned_write = true;
                    }
                    device = None;
                    voices.clear();
                    next_open = Instant::now() + REOPEN_AFTER;
                }
            }
            // No device: these sounds are lost rather than played late.
            None => voices.clear(),
        }
        std::thread::sleep(Duration::from_millis(2));
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

/// `pcm` as stereo samples at [`RATE`] with per-side gains (linear resampling).
fn build_stereo(pcm: &Pcm, gl: f32, gr: f32) -> Vec<i16> {
    let ch = pcm.channels as usize;
    let frames = pcm.samples.len() / ch;
    if frames == 0 {
        return Vec::new();
    }
    let frame = |f: usize| -> (f32, f32) {
        if ch == 1 {
            let s = pcm.samples[f] as f32;
            (s, s)
        } else {
            (pcm.samples[2 * f] as f32, pcm.samples[2 * f + 1] as f32)
        }
    };
    let out_frames = ((frames as u64 * RATE as u64) / pcm.rate as u64).max(1) as usize;
    let step = pcm.rate as f64 / RATE as f64;
    let scale = |s: f32, g: f32| (s * g).round().clamp(-32768.0, 32767.0) as i16;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let t = i as f64 * step;
        let f0 = (t.floor() as usize).min(frames - 1);
        let f1 = (f0 + 1).min(frames - 1);
        let k = (t - f0 as f64).clamp(0.0, 1.0) as f32;
        let (a, b) = (frame(f0), frame(f1));
        out.push(scale(a.0 + (b.0 - a.0) * k, gl));
        out.push(scale(a.1 + (b.1 - a.1) * k, gr));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono_wav(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend(data);
        b
    }

    #[test]
    fn mono_becomes_panned_stereo() {
        let p = parse_wav(&mono_wav(44_100, &[1000, -2000, 3000])).unwrap();
        assert_eq!((p.channels, p.rate, p.samples.len()), (1, 44_100, 3));
        assert_eq!(build_stereo(&p, 1.0, 0.5), [1000, 500, -2000, -1000, 3000, 1500]);
    }

    #[test]
    fn resamples_to_the_mixer_rate() {
        let p = parse_wav(&mono_wav(22_050, &[0, 1000, 2000])).unwrap();
        let s = build_stereo(&p, 1.0, 1.0);
        let left: Vec<i16> = s.iter().step_by(2).copied().collect();
        assert_eq!(left, [0, 500, 1000, 1500, 2000, 2000]);
    }

    #[test]
    fn mixes_overlapping_voices_and_caps_them() {
        let mut voices = Vec::new();
        for _ in 0..MAX_VOICES + 2 {
            handle(Cmd::Play(Arc::from(vec![20000i16; 6])), &mut voices);
        }
        assert_eq!(voices.len(), MAX_VOICES);
        let mut out = [0i16; 4];
        mix(&mut out, &mut voices);
        assert_eq!(out, [32767; 4], "clipped, not wrapped");
        mix(&mut out, &mut voices);
        assert_eq!(out[..2], [32767, 32767]);
        assert_eq!(out[2..], [0, 0]);
        assert!(voices.is_empty());
    }

    #[test]
    fn rejects_non_wav() {
        assert!(parse_wav(b"hello").is_err());
    }
}
