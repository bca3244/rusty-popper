# Changelog

All notable changes to Rusty Popper. Versions follow the GitHub [releases](https://github.com/bca3244/rusty-popper/releases), and each one can be installed from **Exit → Options → Install update** (from v0.2.14 on).

## [0.2.34] - 2026-11-25

### Added
- **Phone remote** (optional, off by default; Exit → Options → Phone remote). A small web page on your home network shows the game being played, or the one on the wheel, with its details and play time, and has *Exit Rusty Popper*, *Restart cabinet* and *Shut down cabinet* buttons (each needs a second tap). A small QR code in the corner of the wheel opens it. The address is fixed (port 8642 by default); a PIN is optional. Only devices on the local network are answered. `Allow-RustyPopperRemote.ps1` lets it through Windows Firewall.

## [0.2.33] - 2026-11-25

### Fixed
- The check for a driver that ignores vsync now runs within a quarter second of start (and the compositor check also ignores still frames), so the first scroll after starting no longer runs unpaced for up to a second.

## [0.2.32] - 2026-11-25

### Changed
- **Less CPU while the menu sits still**: once the wheel and any menu have stopped moving (about a second after the last press), the playfield is redrawn only when its video has a new frame, instead of every refresh of the display. On the 144 Hz test cabinet with 60 fps videos that's 60 frames a second instead of 144, and the menu's CPU use dropped from about 72% to about 45-50% of one core (GPU 3D from about 35% to 25-30%). Scrolling and menus still draw every refresh.
- The check for a driver that ignores vsync now only counts frames drawn every refresh, so it no longer misses that on a still menu and lets the first scroll after start run unpaced for a second.

## [0.2.31] - 2026-11-25

### Changed
- The log line written when Popper's database changes now says what changed, for example "playlist contents changed (Most Played)", "2 games added" or "button setup changed", instead of only repeating the game and playlist counts. Usually it's just a playlist's order (Rusty Popper's own play statistics reorder "Most Played").

## [0.2.30] - 2026-11-24

Speed and responsiveness, from a performance review of v0.2.29.

### Changed
- **Faster exits**: the menu comes back about a second sooner after a game closes. The long wait for a launcher hand-off now only applies in a game's first 20 seconds or so.
- **Faster media on a single step**: in every wheel style, one press now starts loading the new game's videos at once. Before, Arc, Minimal and Vertical always waited 160 ms first. Fast scrolling still waits for the wheel to rest.
- **Smoother holding of a flipper**: the repeat keeps an even beat and speeds up the longer the button is held (about 11 games a second, then 18 after 1.5 s, then 28 after 3 s), and the wheel keeps up more closely with the selection, stiffening when it falls behind. Keyboard browsing keys now repeat on the same beat as joystick buttons, whatever the Windows key-repeat setting.
- **Vsync on the playfield actually works now**: the swap interval belongs to the one shared OpenGL context, so turning it off for the other screens also turned it off for the playfield, and the menu drew about 230 frames a second. It is now switched on just for the playfield's swap. If the driver still doesn't wait for vsync (as on the test cabinet), the menu notices within a second and paces its frames itself, waiting for the desktop compositor's next frame (or sleeping to the refresh rate if that doesn't hold them back). On the 144 Hz test cabinet the menu now draws 144 frames a second instead of about 230. This frees a lot of GPU time.
- Switching videos no longer waits on mpv, and mpv no longer looks through each media folder for subtitle, audio or cover-art files.
- mpv no longer pauses the menu while waiting for a video frame's display time.
- Backglass, DMD, FullDMD and topper screens are only redrawn when their video has a new frame or something on them changed, with a redraw every second anyway. This cuts GPU work, mostly for still images and 30 fps videos. The playfield video, too, is only drawn into its texture when it has a new frame, instead of on every menu frame.
- Fewer OpenGL context switches per frame.
- Wheel logos load on up to two low-priority threads, always nearest the current selection first (also after a letter jump), and are turned into textures within a small time budget per frame.
- Media file lookups are remembered per game instead of checking the disk on every step. They are refreshed after each game and after another program's table closes.
- The playfield video stops decoding while the Launching screen covers it, so the table loads with less competition. Its audio keeps playing.
- The joystick rescan runs on a helper thread, so a slow winmm query can't hold up a frame.
- libmpv loads while Popper's database is read, and the window icon is decoded once instead of once per screen.
- Cover flow draws fewer strips for the outer covers and skips reflections too faint to see.
- Release builds use thin LTO and a single codegen unit.

### Added
- Timing notes in `rusty-popper.log`, written only when something is slow: frames over 25 ms (a summary once a minute, naming the slowest part), media switches over 300 ms, a slow database read, libmpv or window setup, and the time to the first frame. Each screen's video decoder (`hwdec=...`) is logged once, which shows whether hardware decoding is in use.

## [0.2.29] - 2026-11-24

### Added
- Two new wheel styles, picked in **Options → Wheel style** (or with W), which now cycles Arc, Cover flow, Minimal, Vertical:
  - **Minimal**: one big logo near the bottom with its neighbours peeking in, half cut off, at the screen edges, and a lighter shade so more of the playfield video shows.
  - **Vertical**: a list of large logos down the right edge, with the selected game biggest and marked by a glow and an accent bar. On a portrait playfield the list starts below the title so long names keep the full width.
  - Both swap the screens' videos the same way as the arc wheel. In `rusty-popper.toml` they are `style = "minimal"` and `style = "vertical"`.

### Changed
- The arc wheel no longer fades the playfield video in from black after a game change: all screens cut to the new game together.
- The game title, tags and maker line slide in from the side the wheel is moving from (from the right on Next, from the left on Prior). They always came from the right before.
- Cover flow: on a single step the screens now slide together with the wheel, instead of waiting for the wheel to settle and the new video's first frame. The incoming screen stays dark until its video is ready. Fast scrolling still waits for the wheel to rest, so it stays smooth.
## [0.2.28] - 2026-11-24

### Changed
- The Options row is now just **DOF** (it was *DOF lights*), since DOF drives flashers, shakers and other toys too.

## [0.2.27] - 2026-11-24

### Added
- Optional **DOF** support, off by default: turn it on in **Options → DOF lights**. It uses the DOF Config Tool's **PinUP Menu** table (ROM `pinupmenu`) exactly as Popper's menu does, so an existing config lights up the same way: `E800`/`E801` on wheel prior/next, `E804`/`E805` on page next/prior, and a random `E901`-`E990` held for each game the wheel stops on. DOF is handed over to the table while a game runs (ours or one started by another program) and turned off when the menu exits. Uses Popper's `PinUPDOF.DOFLink` bridge, else DOF's own `DirectOutput.ComObject`; if neither is installed it does nothing. Options is now six rows.

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

[0.2.33]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.33
[0.2.32]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.32
[0.2.31]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.31
[0.2.30]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.30
[0.2.29]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.29
[0.2.28]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.28
[0.2.27]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.27
[0.2.26]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.26
[0.2.25]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.25
[0.2.24]: https://github.com/bca3244/rusty-popper/releases/tag/v0.2.24
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
