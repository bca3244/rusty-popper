# Rusty Popper

A fast, sleek alternate front end for [PinUP Popper](https://www.nailbuster.com/wikipinup/) on virtual pinball cabinets, written in Rust.

Rusty Popper replaces **PinUpMenu.exe** only. Everything else stays as Popper set it up: your tables, media, playlists, button setup, emulator launch scripts, PinUP Player, B2S and DOF. You can switch back to Popper at any time.

https://github.com/user-attachments/assets/669fd271-41e6-48a0-bba9-ca5fbae91a3a

## Features

- **Adjustable wheel layout:** start from one of four templates (an arc wheel, a cover flow with sliding videos, minimal with one big logo and its neighbours peeking in at the edges, and vertical with a list of logos down the right edge), then change the wheel size, its position (bottom, left or right), how many wheels show each side and whether the row curves. All in Options → Layout.
- **Every cabinet screen:** playfield, backglass, DMD, FullDMD and topper all follow the selected game, using Popper's own screen layout and playfield rotation.
- **Launches through Popper's scripts:** VPX, Future Pinball and others start exactly as they do in Popper. "Launching" and "Exiting" screens mean the desktop never shows.
- **Playlists as wheels:** Popper's playlists sit on the wheel where it wraps from the last game back to the first.
- **Jump by letter** on the magna-saves, **Game Info / Help cards**, **attract mode**, **menu sounds** from Popper's own sound set, and each game's **Audio media** once the wheel rests on it.
- **DOF (optional):** cabinet lights, flashers and shakers react to the menu, using your existing DOF config for Popper's menu.
- **Works alongside VPin Studio:** hides and pauses while a table started elsewhere is on screen, so media recordings stay clean.
- **Phone remote (optional):** scan the QR code in the corner of the wheel to see what's playing, change settings, install updates, watch CPU, GPU and disk load, kill a hung table, and exit Rusty Popper, restart or shut down the cabinet from your phone.
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
   - `Allow-RustyPopperRemote.ps1`, to let the phone remote through the Windows firewall
3. **Close PinUp Popper** if it's running, then **run `rusty-popper.exe`** to try it.
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
| | | W | Switch layout template (arc, cover flow, minimal, vertical) |
| | | Q | Quit at once (can be turned off) |

**While a game runs**, Popper's *Exit Emulators* button closes it with the emulator's own close script, just like Popper. If the table is stuck and doesn't close, press Exit again to end it at once instead of waiting 20 seconds. If a game doesn't start at all, the wheel says so for a few seconds.

**Tables started elsewhere** (VPin Studio's launcher or its media recorder, for example): while the table is on screen, Rusty Popper hides its windows and pauses its videos, as if the menu had been closed, so recordings show the table rather than the menu. It comes back when the table closes. In VPin Studio's recorder, use the *Emulator* launch mode; the *Frontend* mode starts Popper's own menu.

**Playlists** appear as wheels after the last game (step back from the first game to reach them). Press Start on one to switch to it. Inside a playlist, the **All games** wheel takes you back; it sits one step back from the first game. Rusty Popper always starts in All games, on the first game alphabetically.

**The exit menu** has *Back to Games*, *Options*, *Exit Rusty Popper*, *Restart Cabinet* and *Shut Down Cabinet*. It always opens on *Back to Games*, so a stray press can't power anything off.

**Options** (Exit → Options) change things on the spot and save them. A fresh install starts with the arc wheel, attract mode after 3 minutes idle (12 seconds per game), video volume 50, sound effects 30 and game audio 50.

- **Layout ›**, a submenu for how the wheel looks. *Template* picks Arc, Cover Flow, Minimal or Vertical and resets the rest to that template's values (the row says *(Changed)* once you've changed any of them). *Wheel Size* (60% to 150%), *Position* (Bottom, Left or Right), *Wheels Each Side* (0 to 5), *Wheel Spacing* (50% to 200%) and *Arc* (curve the row round a big wheel off the screen edge) can then be changed one by one. *Reset to Template* puts them back. You always get exactly the number of wheels you set; more than fit will overlap or run off the screen edge. W on a keyboard cycles the templates, which resets the layout too.
- **Attract Mode ›**, a submenu with attract mode on or off, how long until it starts, and how long it stays on each game
- **Sound ›**, a submenu with video sound, sound effects and game audio, each on or off with its own volume (a volume only shows while its sound is on). Game audio is the game's Popper Audio media (`POPMedia\<emulator>\Audio`), looped once the wheel rests on it for about a second. *Back* in either submenu returns to Options.
- **DOF ›**, a submenu with DOF on or off (off by default; see below) and, while it's on, its **Rate Cap**: the fewest ms between wheel/page pulses, Off or 50 to 500 ms (default 100)
- **Phone Remote** on or off (off by default; see below)
- Install Update / Check for Updates

**DOF (cabinet lights, flashers, shakers and other toys)** is optional and off until you turn it on in Options. Rusty Popper drives DOF the way Popper's menu does, using the **PinUP Menu** table (ROM `pinupmenu`) from the DOF Config Tool, so your existing config works as it is: flashes for wheel and page moves (`E800`, `E801`, `E804`, `E805`), a random effect (`E901` to `E990`) for each game the wheel stops on, DOF handed to the table while a game runs, and everything off when the menu exits. It uses Popper's DOF bridge (`PinUPDOF.DOFLink`) if it's there, otherwise DOF's own COM object (`DirectOutput.ComObject`, registered for 64-bit). If neither is installed, nothing happens. There's no lighting setup in Rusty Popper itself; change effects in the DOF Config Tool.

**Phone remote** is optional and off until you turn it on in Options. It's a small web page on your home network that shows the game being played (or the one on the wheel) with its details and play time, has a **Settings** card with everything in Options (except the phone remote's own switch) plus checking for and installing updates, and has *Exit Rusty Popper*, *Restart Cabinet* and *Shut Down Cabinet* buttons. Each button needs a second tap to confirm. A *Pinball Processes* page lists running VPinball, Future Pinball, PinMAME, PinUP Popper/PuP, dmdext, FlexDMD and B2S backglass processes, with a *Kill* button for each (and *Kill All*) for a table that hangs; Rusty Popper then goes back to the wheel. *Real-time Performance*, below the Settings button, shows how busy each CPU core, the GPU (with its 3D and video-decode engines) and each disk are, refreshed every second, the same numbers as Task Manager. The cabinet only reads these while that page is open on a phone. *Exit* is greyed out while a game is running; *Restart* and *Shut down* close the game properly first (its close script and play statistics), then power off. A small QR code in the top-right corner of the wheel, beside the game counter, opens the page. The address is also shown in Options, on the *Phone Remote* row.

- The address is `http://<cabinet's IP>:8642/`. The port stays the same (change it with `port` under `[remote]`). To keep the IP the same too, give the cabinet a fixed address (a DHCP reservation) in your router. Rusty Popper finds its own address, and you can set the one the QR code shows with `address`.
- Run `Allow-RustyPopperRemote.ps1` once (right-click, *Run with PowerShell*) so Windows Firewall lets phones in. Otherwise Windows may ask the first time, and the question can be hidden behind the cabinet's screens.
- Only devices on your local network get an answer. For a PIN as well, set `pin` (letters and digits) under `[remote]`; the QR code includes it.
- If Options shows `localhost` as the address, Rusty Popper couldn't work out its address (a network with no internet route, say): set `address` under `[remote]`.

## Updating

When Rusty Popper starts, it checks GitHub for a newer release. If there is one, a small **UPDATE vX AVAILABLE** note appears in the corner of the wheel.

To install it, go to **Exit → Options → Install Update vX**. Rusty Popper then:

1. downloads the release
2. checks it against the SHA-256 checksum GitHub publishes for it, and refuses it if they don't match
3. replaces its files all at once, keeping the old ones in `rollback\` (your `rusty-popper.toml` is never touched). If any file can't be replaced, nothing changes.
4. restarts into the new version

If the new version stops with an error during startup, or crashes three starts in a row, the old files are put back and the old version starts again (a start you end yourself, with Exit or a shutdown, doesn't count). Once the new version has run for a minute it counts as good, and the download is cleaned up. `rollback\` keeps the previous version until the next update, so you can also go back by hand by copying its files over. Updates never install during a game.

To turn the startup check off, set `check = false` under `[update]` in `rusty-popper.toml`. To update by hand instead, download the new zip and copy `rusty-popper.exe` (and `libmpv-2.dll`, if it changed) over the old ones, keeping your own `rusty-popper.toml`.

## Settings

`rusty-popper.toml` next to the exe holds your settings. Every setting is optional, and most can be changed in Options instead. A value that can't be read (a typo, a wrong type) is left at its default, and a line that isn't valid TOML costs only its own section; the wheel says what was ignored for a few seconds. If `[remote]` can't be read, the phone remote stays off. `rusty-popper.example.toml` documents them all:

| Section | What's in it |
|---|---|
| `[wheel]` | `style` (the layout template): `"arc"`, `"flow"`, `"minimal"` or `"vertical"`; `size` (60 to 150), `position` (`"bottom"`, `"left"`, `"right"`), `side_wheels` (0 to 5), `spacing` (50 to 200), `arc` (true/false), each defaulting to the template's value |
| `[playfield]` | `ui_rotation`: turn the menu to face the player (default: Popper's own setting) |
| `[input]` | Extra joystick buttons, whether buttons need focus, the Q quit key |
| `[launch]` | Extra helper programs to ignore when deciding whether a game is still running |
| `[paths]` | `popper_dir` (found automatically) and `mpv_dll` |
| `[attract]` | `enabled`, `idle_secs`, `interval_secs` |
| `[audio]` | `volume` for the playfield video |
| `[sound]` | Menu sound effects: `enabled`, `volume`, `dir`; game audio: `game_audio`, `game_audio_volume` |
| `[dof]` | `enabled`: DOF for the menu, driving lights, flashers, shakers and other toys (default off); `rate_cap_ms`: Rate Cap, the fewest ms between wheel/page pulses (0 = off, up to 500; default 100) |
| `[remote]` | The phone remote: `enabled` (default off), `port` (8642), and optional `pin` and `address` |
| `[update]` | `check` for updates at startup, and the `repo` they come from |

## What it changes

- **Popper's database:** only play statistics (last played, play count, time played), after plays of 20 seconds or more. The database is backed up to `backups\` next to the exe before the first write of each session, and the newest 10 backups are kept. If that folder can't be written, the backup goes to `RustyPopper-backups` in the Windows temp folder instead.
- **Next to the exe:**
  - `rusty-popper.log`, for troubleshooting
  - `rusty-popper.toml`, when you change Options
  - `last-screen.txt` (where to show an error screen)
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
- **Sound:** menu effects mixed on their own thread into a `waveOut` stream; game audio through libmpv
- **DOF:** COM late binding (`IDispatch`) via the `windows` crate
- **Updates:** `ureq` (rustls) against the GitHub releases API, `sha2` to verify, `zip` to unpack
- **Phone remote:** a small built-in HTTP server on its own thread, one embedded page, `qrcodegen` for the QR code, Windows PDH counters for Real-time Performance

```
crates/
  rp-app/     binary: rusty-popper
    main.rs       startup, arguments, the event loop
    render.rs     windows, GL, players, drawing a frame of the menu
    menu.rs       what the buttons do: browsing, playlists, cards, exit/options, attract mode
    input.rs      keyboard and joystick, mapped from Popper's controller setup
    session.rs    a game session: launch, the Launching/Exiting screens, the exit button
    reload.rs     following database changes, keeping the keyboard focus
    library.rs    games and playlists as read from Popper
    wheel.rs      the wheel layout (row of wheels, templates' looks), info text and menus
    video.rs      offscreen video targets and the cover-flow slide
    mpv.rs        minimal libmpv binding, loaded at run time
    assets.rs     background loading of wheel logos
    discover.rs   finding Popper
    settings.rs   rusty-popper.toml
    overlay.rs    Game Info / Help cards
    sfx.rs        menu sounds
    gameaudio.rs  game audio on the wheel
    dof.rs        optional DOF output
    errscreen.rs  the full-screen "couldn't start" notice
    updater.rs    update check, install and rollback
    remote.rs     the optional phone remote (web page, QR code)
    sysload.rs    CPU, GPU and disk load for the phone remote (Windows counters)
    win.rs, logging.rs  Win32 helpers, rusty-popper.log
  rp-db/      PUPDatabase.db access
  rp-config/  PinUpPlayer.ini and POPMedia path resolution
  rp-launch/  emulator launch/post scripts, process tracking
```

Rusty Popper is not affiliated with or endorsed by nailbuster / PinUP Popper.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
