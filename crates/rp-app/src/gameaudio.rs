//! Game audio on the wheel: Popper's per-game Audio media
//! (`POPMedia\<emulator>\Audio\<GameName>.mp3`), played once the selection
//! has rested on a game for a moment, so fast scrolling stays quiet. It
//! stops on the next move and when a game launches. Attract mode plays each
//! game's audio too, as it lands on it.

use crate::mpv;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the wheel must rest on a game before its audio starts.
const DWELL: Duration = Duration::from_millis(900);

const AUDIO_EXTS: &[&str] = &["mp3", "ogg", "wav", "m4a", "flac", "aac", "wma"];

/// `<media dir>\Audio\<name>.<ext>`, if there is one.
pub fn find(media_dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let dir = media_dir.join("Audio");
    AUDIO_EXTS.iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|p| p.is_file())
}

pub struct GameAudio {
    /// An audio-only mpv player (None if it couldn't be created).
    player: Option<mpv::Player>,
    pub enabled: bool,
    /// 0..=100.
    pub volume: i32,
    /// The item the wheel is resting on, and since when.
    want: Option<(usize, Instant)>,
    /// Whether `want` has been dealt with (started, or found to have no audio).
    tried: bool,
    playing: bool,
}

impl GameAudio {
    pub fn new(mpv: &mpv::Mpv, enabled: bool, volume: i32) -> GameAudio {
        let volume = volume.clamp(0, 100);
        let opts = [
            ("vo", "null".to_string()),
            ("vid", "no".to_string()),
            ("audio-display", "no".to_string()),
            ("loop-file", "inf".to_string()),
            ("volume", volume.to_string()),
        ];
        let player = mpv.player(&opts, &[("sub-auto", "no"), ("cover-art-auto", "no")]).map_err(|e| log::warn!("Game audio player: {e:#}; game audio off")).ok();
        GameAudio { player, enabled, volume, want: None, tried: false, playing: false }
    }

    /// Called every frame with the item the wheel is on (None: nothing should
    /// play, e.g. a game is running) and a way to find that item's audio.
    pub fn tick(&mut self, selected: Option<usize>, audio_of: impl FnOnce(usize) -> Option<PathBuf>) {
        // Nothing else reads this player's events. mpv keeps at most 1000 per
        // client and refuses async commands once that queue is full.
        if let Some(p) = &self.player {
            p.poll_events();
        }
        let selected = selected.filter(|_| self.enabled && self.volume > 0);
        if self.want.map(|(i, _)| i) != selected {
            self.stop();
            self.want = selected.map(|i| (i, Instant::now()));
            self.tried = false;
            return;
        }
        if self.playing && !self.tried {
            // An earlier stop failed; nothing should be playing yet.
            self.stop();
        }
        let Some((item, since)) = self.want else { return };
        if self.tried || since.elapsed() < DWELL {
            return;
        }
        self.tried = true;
        if let (Some(p), Some(file)) = (&self.player, audio_of(item)) {
            match p.show(Some(&file)) {
                Ok(_) => self.playing = true,
                Err(e) => log::warn!("Game audio {}: {e:#}", file.display()),
            }
        }
    }

    fn stop(&mut self) {
        if !self.playing {
            return;
        }
        match &self.player {
            // Stay "playing" if the stop didn't go through, so the next call retries it.
            Some(p) => match p.show(None) {
                Ok(_) => self.playing = false,
                Err(e) => log::warn!("Game audio stop: {e:#}"),
            },
            None => self.playing = false,
        }
    }

    pub fn set_volume(&mut self, volume: i32) {
        self.volume = volume.clamp(0, 100);
        if let Some(p) = &self.player {
            let _ = p.command(&["set", "volume", &self.volume.to_string()]);
        }
    }

    /// Turns it on or off; when turned back on, the current game's audio starts.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.stop();
        self.want = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_audio_in_any_supported_format() {
        let dir = std::env::temp_dir().join(format!("rp-audio-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Audio")).unwrap();
        std::fs::write(dir.join("Audio").join("Attack (1980).ogg"), b"x").unwrap();
        assert_eq!(find(&dir, "Attack (1980)"), Some(dir.join("Audio").join("Attack (1980).ogg")));
        assert_eq!(find(&dir, "Missing"), None);
        assert_eq!(find(&dir, ""), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
