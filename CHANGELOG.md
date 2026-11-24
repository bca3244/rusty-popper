# Changelog

All notable changes to Rusty Popper. Versions follow the GitHub [releases](https://github.com/bca3244/rusty-popper/releases), and each one can be installed from **Exit → Options → Install update** (from v0.2.14 on).

## [0.2.26] - 2026-11-24

### Changed
- New defaults for a fresh install: video volume at 50 (was 100, set in the starter `rusty-popper.toml` in the download), menu sound effects at 30 (was 70) and game audio at 50 (was 80). A `rusty-popper.toml` that already sets these volumes keeps its own values.

## [0.2.25] - 2026-11-23

### Fixed
- No more white screens at startup. The windows used to appear before anything was drawn in them, so the backglass and FullDMD flashed white and a white box showed on the playfield while the players loaded. Now every window stays hidden until its first frame is drawn.

## [0.2.24] - 2026-11-23

### Fixed
- The playfield's **Launching** screen no longer switches to **Exiting** while a table is still loading. It used to count the table as up as soon as any of the table's windows (a B2S backglass or a DMD window, say) filled one of the screens; now only a window covering the playfield does. If none is seen, it waits up to two minutes instead of one. The log names the window it took for the table.

## [0.2.23] - 2026-11-23

### Changed
- The attract mode settings (on/off, starts after, time per game) moved into an **Options → Attract mode** submenu, like Sound. Options is now five rows: Wheel style, Attract mode, Sound, Update, Back.

## [0.2.22] - 2026-11-23

### Changed
- All volume and sound settings (video volume, sound effects, effects volume, game audio, game audio volume) moved into an **Options → Sound** submenu, so Options is shorter. *Back* in Sound returns to Options.

## [0.2.21] - 2026-11-23

### Fixed
- The Options menu shrinks its rows to fit on a short (landscape, unrotated) playfield instead of running off the screen. On a portrait playfield it's unchanged.

## [0.2.20] - 2026-11-23

### Added
- **Game audio:** when the wheel rests on a game for about a second, its Popper Audio media (`POPMedia\<emulator>\Audio\<game>.mp3`, or .ogg, .wav, .m4a and the like) plays on a loop. It stops on the next move, when a game launches and during attract mode, so fast scrolling stays quiet. It can be turned off, and its volume set, in Options (`game_audio` and `game_audio_volume` under `[sound]`).

## [0.2.19] - 2026-11-23

### Changed
- When a table started by another program (VPin Studio's launcher or its media recorder) is on screen, Rusty Popper hides its windows and pauses its videos, so recordings of the backglass, FullDMD and playfield show the table and not the menu. It comes back when the table closes.

## [0.2.18] - 2026-11-23

### Changed
- If Rusty Popper can't start (or stops with an error), it now shows a full-screen error screen on the playfield, turned to face the player like the menu, instead of a sideways Windows message box. Any button, key or click closes it. Where to draw it is remembered in `last-screen.txt` after each good start.

### Added
- This changelog, and a bug report form on GitHub's Issues page.

## [0.2.17] - 2026-11-23

### Changed
- Inside a playlist, the **All games** (go back) wheel now sits after the other playlists, right before the first game, so one step back from the games reaches it.

## [0.2.16] - 2026-11-23

### Fixed
- Menu sounds no longer drop out or make the menu stutter when scrolling fast. Sounds are mixed on their own thread and can overlap (up to 4 at once), so the menu never waits on the audio device.

## [0.2.15] - 2026-11-22

### Fixed
- Updates install all at once or not at all: if any file can't be replaced, everything is put back as it was.
- A second update is refused until the previous one is confirmed, so the backup of the old version is never lost.
- A failed rollback no longer stops every later start; the current version keeps running.
- Only real menu starts count as tries of a new version (not `--list`, `--dry-run` or `--print-launch`).
- After the menu restarts itself, the old copy no longer rolls back or shows an error over the new one.
- A stalled update download now times out instead of hanging.
- Game Info / Help card images load in the background, so videos keep playing smoothly.
- Attract mode waits while a card is open.
- Saving the Popper folder copes with an inline `[paths]` table and never overwrites a settings file it couldn't read.
- Corrected README wording about discovery, rollback and the update folders.

## [0.2.14] - 2026-11-22

### Added
- Self-update: a startup check against GitHub releases shows **UPDATE vX AVAILABLE** on the wheel, and **Options → Install update** downloads it, verifies its SHA-256 checksum, installs it and restarts. A new version that fails to start is rolled back automatically.

## [0.2.13] - 2026-11-22

### Changed
- Cover flow: side covers are spaced more evenly and tilted further, so they look wider.

## [0.2.12] - 2026-11-22

### Added
- **Game Info / Help cards** from Popper's GameInfo and GameHelp images, on Popper's own buttons (or I / H), shown on the screen Popper assigns them.
- **Menu sounds** from Popper's own sound set, with on/off and volume in Options and a `[sound]` section in the settings.

### Fixed
- Clippy warnings that failed the CI build.

## [0.2.11] - 2026-11-22

### Added
- Popper is found automatically: the default folder, then where Windows has PinUpPlayer registered, then the usual install folders on every drive. The folder found is saved to `rusty-popper.toml`.
- Startup errors are shown on screen instead of leaving a bare desktop.

## [0.2.10] - 2026-11-22

### Added
- App icon: a copper-ring RP wheel.

## [0.2.9] - 2026-11-22

### Fixed
- Code review fixes for input handling, game launch tracking, keeping the keyboard focus, and database handling.

## [0.2.8] - 2026-11-22

### Added
- Cover flow: videos slide in and out when the game changes.

## [0.2.7] - 2026-11-22

### Changed
- Cover flow shows one fewer cover per side.

## [0.2.6] - 2026-11-22

### Fixed
- Popper's root "Home" playlist no longer shows as a playlist wheel.
- When a table closes, the "Exiting" screen shows instead of a stale "Launching".

## [0.2.5] - 2026-11-22

### Fixed
- One Start press could both switch playlist and launch a game.

## [0.2.4] - 2026-11-22

### Changed
- Playlists are wheels where the game wheel wraps from the last game back to the first.

## [0.2.3] - 2026-11-22

### Added
- Coloured backgrounds behind the tag chips.

## [0.2.2] - 2026-11-22

### Added
- The game title on two lines (name, then maker and year), with tags as chips.
- `Enable-RustyPopperStartup.ps1` and `Disable-RustyPopperStartup.ps1` to switch the logon task between Rusty Popper and Popper.

## [0.2.1] - 2026-11-22

### Fixed
- Joystick buttons follow Popper's controller setup exactly (Popper's codes are 0-based).

## [0.2.0] - 2026-11-22

### Added
- Playlists, jump by letter on the magna-saves, and attract mode.
- An Options screen in the exit menu.

## [0.1.0] - 2026-11-22

First release.

### Added
- Game media looping on every cabinet screen, using Popper's screen layout.
- Arc wheel and cover flow over the playfield video.
- Launching games through Popper's emulator scripts.
- Play statistics written back to Popper's database.

[0.2.23]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.23
[0.2.22]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.22
[0.2.21]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.21
[0.2.20]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.20
[0.2.19]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.19
[0.2.18]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.18
[0.2.17]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.17
[0.2.16]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.16
[0.2.15]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.15
[0.2.14]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.14
[0.2.13]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.13
[0.2.12]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.12
[0.2.11]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.11
[0.2.10]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.10
[0.2.9]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.9
[0.2.8]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.8
[0.2.7]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.7
[0.2.6]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.6
[0.2.5]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.5
[0.2.4]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.4
[0.2.3]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.3
[0.2.2]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.2
[0.2.1]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.1
[0.2.0]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.0
[0.1.0]: https://github.com/bca3244/rusty-popper/releases/tag/v0.1.0
