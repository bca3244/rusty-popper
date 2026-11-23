# Rusty Popper

A fast, sleek alternate front end for [PinUP Popper](https://www.nailbuster.com/wikipinup/) on virtual pinball cabinets, written in Rust.

Rusty Popper replaces **PinUpMenu.exe** only. Everything else stays as Popper set it up: your tables, media, playlists, button setup, emulator launch scripts, PinUP Player, B2S and DOF. You can switch back to Popper at any time.

https://github.com/user-attachments/assets/669fd271-41e6-48a0-bba9-ca5fbae91a3a

## Features

- **Two wheel styles:** an arc wheel and a cover flow with sliding videos. Switch in Options.
- **Every cabinet screen:** playfield, backglass, DMD, FullDMD and topper all follow the selected game, using Popper's own screen layout and playfield rotation.
- **Launches through Popper's scripts:** VPX, Future Pinball and others start exactly as they do in Popper. "Launching" and "Exiting" screens mean the desktop never shows.
- **Playlists as wheels:** Popper's playlists sit on the wheel where it wraps from the last game back to the first.
- **Jump by letter** on the magna-saves, **Game Info / Help cards**, **attract mode**, and **menu sounds** from Popper's own sound set.
- **Play statistics** written back to Popper, so "Most Played" keeps working.
- **Updates itself:** it tells you when a new release is out and installs it from the Options menu, and goes back to the previous version if the new one won't start.

## Requirements

- Windows 10 or 11, 64-bit
- PinUP Popper installed and working (Rusty Popper reads its database and media)
- A graphics card with OpenGL 3.3 or newer

## Installation

1. **Download** the latest `rusty-popper-vX-windows-x64.zip` from the [Releases page](https://github.com/bca3244/rusty-popper/releases/latest).
2. **Unzip** the `rusty-popper` folder somewhere permanent, for example `C:\vPinball\RustyPopper`. It contains:
   - `rusty-popper.exe`, the front end
   - `libmpv-2.dll`, the video player library it needs (keep it next to the exe)
   - `rusty-popper.toml`, your settings, and `rusty-popper.example.toml`, which lists every option
   - `Enable-RustyPopperStartup.ps1` and `Disable-RustyPopperStartup.ps1`, for starting at boot
3. **Close Popper** if it's running, then **run `rusty-popper.exe`** to try it.
   - It finds Popper by itself: the default `C:\vPinball\PinUPSystem` first, then where Windows has PinUpPlayer registered, then the usual install folders on every drive. If it had to search, the folder it found is saved to `rusty-popper.toml` so the next start goes straight there.
   - If it can't start (Popper not found, `libmpv-2.dll` missing), it shows an error screen on the playfield saying why (any button closes it). More detail is in `rusty-popper.log` next to the exe.
4. **Start it at boot (optional).** Right-click `Enable-RustyPopperStartup.ps1` and choose *Run with PowerShell*. It asks for administrator rights. It creates a "Run Rusty Popper" logon task and disables Popper's own "Run Pinup Popper" task, so only one front end starts.

### Going back to Popper

Run `Disable-RustyPopperStartup.ps1` the same way. It turns Rusty Popper's logon task off and Popper's back on. Nothing is deleted, so you can switch again whenever you like. Add `-WhatIf` to either script to see what it would do without changing anything.

## Using it

Rusty Popper uses the buttons you set up in **Popper's controller setup**, so your cabinet works straight away. Keyboard keys work too.

| Popper function | Usually | Keyboard | What it does |
|---|---|---|---|
| Game Prior / Game Next | Flippers | ← / → or Left / Right Shift | Previous / next game |
| Page Prior / Page Next | Magna-saves | ↑ / ↓ or Left / Right Ctrl | Jump to the previous / next letter |
| Game Start | Start | Enter | Launch the game, or open a playlist wheel |
| Game Info/Flyer | Info | I | Show the game's info card (any button closes it) |
| Game Help | | H | Show the game's help card |
| System Menu / Exit Emulators | Exit | Esc | Open the exit menu |
| List Next / List Prior | | P | Jump to the playlist wheels |
| | | W | Switch between arc and cover flow |
| | | Q | Quit at once (can be turned off) |

**While a game runs**, Popper's *Exit Emulators* button closes it with the emulator's own close script, just like Popper.

**Playlists** appear as wheels after the last game (step back from the first game to reach them). Press Start on one to switch to it. Inside a playlist, the **All games** wheel takes you back; it sits one step back from the first game. Rusty Popper remembers the last playlist you used.

**The exit menu** has *Back to games*, *Options*, *Exit Rusty Popper*, *Restart computer* and *Shut down computer*. It always opens on *Back to games*, so a stray press can't power anything off.

**Options** (Exit → Options) change things on the spot and save them:

- Wheel style (arc or cover flow)
- Attract mode on or off, how long until it starts, and how long it stays on each game
- Video volume
- Sound effects on or off, and their volume
- Install update / check for updates

## Updating

When Rusty Popper starts, it checks GitHub for a newer release. If there is one, a small **UPDATE vX AVAILABLE** note appears in the corner of the wheel.

To install it, go to **Exit → Options → Install update vX**. Rusty Popper then:

1. downloads the release
2. checks it against the SHA-256 checksum GitHub publishes for it, and refuses it if they don't match
3. replaces its files all at once, keeping the old ones in `rollback\` (your `rusty-popper.toml` is never touched). If any file can't be replaced, nothing changes.
4. restarts into the new version

If the new version stops with an error during startup, or fails to stay up through three starts in a row, the old files are put back and the old version starts again. Once the new version has run for a minute it counts as good, and the download is cleaned up. `rollback\` keeps the previous version until the next update, so you can also go back by hand by copying its files over. Updates never install during a game.

To turn the startup check off, set `check = false` under `[update]` in `rusty-popper.toml`. To update by hand instead, download the new zip and copy `rusty-popper.exe` (and `libmpv-2.dll`, if it changed) over the old ones, keeping your own `rusty-popper.toml`.

## Settings

`rusty-popper.toml` next to the exe holds your settings. Every setting is optional, and most can be changed in Options instead. `rusty-popper.example.toml` documents them all:

| Section | What's in it |
|---|---|
| `[wheel]` | `style`: `"arc"` or `"flow"` |
| `[playfield]` | `ui_rotation`: turn the menu to face the player (default: Popper's own setting) |
| `[input]` | Extra joystick buttons, whether buttons need focus, the Q quit key |
| `[launch]` | Extra helper programs to ignore when deciding whether a game is still running |
| `[paths]` | `popper_dir` (found automatically) and `mpv_dll` |
| `[attract]` | `enabled`, `idle_secs`, `interval_secs` |
| `[audio]` | `volume` for the playfield video |
| `[sound]` | Menu sound effects: `enabled`, `volume`, `dir` |
| `[update]` | `check` for updates at startup, and the `repo` they come from |

## What it changes

- **Popper's database:** only play statistics (last played, play count, time played), after plays of 20 seconds or more. The database is backed up to `backups\` next to the exe before the first write of each session, and the newest 10 backups are kept.
- **Next to the exe:**
  - `rusty-popper.log`, for troubleshooting
  - `rusty-popper.toml`, when you change Options
  - `last-playlist.txt`, and `last-screen.txt` (where to show an error screen)
  - `update\` while an update downloads (removed once the new version is confirmed), and `rollback\`, which holds the previous version

Nothing else of Popper's is changed. Rusty Popper notices when the database changes (tables added in Popper's setup or VPin Studio, playlists edited) and picks the changes up without a restart.

## Troubleshooting

- **Nothing happens, or an error screen says it couldn't start:** read `rusty-popper.log` next to the exe. The usual causes are:
  - Popper isn't installed where it looked: set `popper_dir` under `[paths]`.
  - `libmpv-2.dll` isn't next to the exe.
- **A game launches but Rusty Popper doesn't come back:** a helper program from the emulator's folder may still be running. Add it to `ignore_processes` under `[launch]`.
- **Buttons do the wrong thing:** they come from Popper's controller setup, so fix them there (PinUP Popper Setup → Controller Setup).

Still stuck? [Report a bug](https://github.com/bca3244/rusty-popper/issues/new?template=bug_report.yml) with your `rusty-popper.log`. What changed in each version is in the [changelog](CHANGELOG.md).

## Building from source

```
cargo build --release
cargo test
```

The app is Windows-only and needs Rust stable with the MSVC toolchain. At run time it needs `libmpv-2.dll` from the `mpv-dev-x86_64` build of zhongfly/mpv-winbuild. The DLL is loaded at run time, so building needs no import library. You can point to it with `[paths] mpv_dll`, the `RUSTY_POPPER_MPV` environment variable, or `--mpv PATH`.

Useful command-line options:

```
rusty-popper --list                                  # list the games it found
rusty-popper --dry-run --game "addams family"        # check setup without opening windows
rusty-popper --print-launch --game "addams family"   # show the filled-in launch scripts
rusty-popper --game "addams family" --seconds 60     # start on a game, quit after a minute
rusty-popper --popper D:\PinUPSystem                 # use a specific Popper folder
```

The exe is a windowed program, so a terminal doesn't wait for it. Use `start /wait rusty-popper --list` (cmd) or `rusty-popper --list | Out-Host` (PowerShell), or read `rusty-popper.log`.

### How it's built

- **Windowing / GL:** `winit` and `glutin`, with one borderless window per cabinet screen sharing one GL context
- **Video:** the `libmpv` render API (hardware decode, seamless looping), drawn into textures so videos can slide
- **2D and text:** `femtovg`
- **Database:** `rusqlite`, reading a consistent snapshot of `PUPDatabase.db`
- **Input:** Popper's own key and joystick setup (`PinUPFunctions`), read through winit and winmm
- **Updates:** `ureq` (rustls) against the GitHub releases API, `sha2` to verify, `zip` to unpack

```
crates/
  rp-app/     binary: rusty-popper
    main.rs      startup, arguments, the event loop
    render.rs    windows, GL, players, drawing a frame of the menu
    menu.rs      what the buttons do: browsing, playlists, cards, exit/options, attract mode
    session.rs   a game session: launch, the Launching/Exiting screens, the exit button
    reload.rs    following database changes, keeping the keyboard focus
    library.rs   games and playlists as read from Popper
    wheel.rs     the arc wheel and cover flow
    discover.rs  finding Popper
    overlay.rs   Game Info / Help cards
    sfx.rs       menu sounds
    updater.rs   update check, install and rollback
  rp-db/      PUPDatabase.db access
  rp-config/  PinUpPlayer.ini and POPMedia path resolution
  rp-launch/  emulator launch/post scripts, process tracking
```

Rusty Popper is not affiliated with or endorsed by nailbuster / PinUP Popper.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
