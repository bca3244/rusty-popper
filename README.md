# Rusty Popper

A fast, sleek alternate frontend for [PinUP Popper](https://www.nailbuster.com/wikipinup/) on virtual pinball cabinets, written in Rust.

Rusty Popper aims to replace **PinUpMenu.exe** only. It reuses everything Popper already manages, so your existing setup keeps working and you can switch back at any time.

## What it reuses from Popper

| Popper piece | How Rusty Popper uses it |
|---|---|
| `PUPDatabase.db` | Read via SQLite (games, emulators, playlists, button setup); play statistics written back |
| `POPMedia\` tree | Playfield / backglass / DMD / topper / wheel media per game |
| `PinUpPlayer.ini` | Screen layout: which monitor and rectangle each display uses |
| Emulator `LaunchScript` / `PostScript` | Run as-is to start and close tables (VPX, FP, FX, etc.) |
| PinUP Player, B2S, DOF | Untouched, keep doing their jobs |

## How it's built

- **Windowing / GL:** `winit` + `glutin`, one borderless window per cabinet screen, sharing one GL context
- **Video:** `libmpv` render API (hardware decode, seamless looping), drawn into textures so videos can slide
- **2D / text:** `femtovg`
- **Database:** `rusqlite`, reading a consistent snapshot of `PUPDatabase.db`
- **Input:** Popper's own key and joystick button setup (`PinUPFunctions`), read through winit and winmm

## Workspace layout

```
crates/
  rp-app/     binary: rusty-popper
    main.rs     startup, arguments, the event loop
    render.rs   windows, GL, players, drawing a frame of the menu
    menu.rs     what the buttons do: browsing, playlists, exit/options, attract mode
    session.rs  a game session: launch, the Launching/Exiting screens, the exit button
    reload.rs   following database changes, keeping the keyboard focus
    library.rs  games and playlists as read from Popper
    wheel.rs    the arc wheel and cover flow
  rp-db/      PUPDatabase.db access
  rp-config/  PinUpPlayer.ini + POPMedia path resolution
  rp-launch/  emulator launch/post scripts, process tracking
```

## Building

```
cargo build --release
cargo test
```

The app is Windows-only. It needs `libmpv-2.dll` from the `mpv-dev-x86_64` build of zhongfly/mpv-winbuild at run time: put it next to the exe, set `[paths] mpv_dll` in `rusty-popper.toml`, set the `RUSTY_POPPER_MPV` environment variable, or pass `--mpv PATH`. The DLL is loaded at run time, so building needs no import library.

## Running

```
rusty-popper                     # the menu
rusty-popper --list              # list the games it found
rusty-popper --dry-run --game "addams family"
rusty-popper --print-launch --game "addams family"   # show the filled-in launch scripts
rusty-popper --game "addams family" --seconds 60
```

The exe is a windowed program, so a terminal doesn't wait for it and its output can arrive after the next prompt. Use `start /wait rusty-popper --list` (cmd) or `rusty-popper --list | Out-Host` (PowerShell) to wait, or read `rusty-popper.log` next to the exe, which gets everything that is printed.

Popper's folder defaults to `C:\vPinball\PinUPSystem`; change it with `--popper DIR` or `[paths] popper_dir`. Copy `rusty-popper.example.toml` next to the exe as `rusty-popper.toml` for the other settings; most can also be changed in the menu under Exit, Options.

Browse with the buttons Popper has bound (or the arrow keys), Start launches, and the exit button opens the exit menu. While a game runs, Popper's **Exit Emulators** button closes it by running the emulator's close script, like Popper does.

## What it writes

- **Popper's database:** only play statistics (`GamesStats`: last played, play count, time played), after plays of 20 seconds or more. The database is backed up to `backups\` next to the exe before the first write of a session (the newest 10 are kept).
- **Next to the exe:** `rusty-popper.log` (what happened, for troubleshooting), `rusty-popper.toml` (when you change Options) and `last-playlist.txt`.

Nothing else of Popper's is changed. Rusty Popper notices when the database changes (tables added in Popper's setup or VPin Studio, playlists edited) and picks the changes up without a restart.

Windows is the target platform. Rusty Popper is not affiliated with or endorsed by nailbuster / PinUP Popper.

## Starting at boot

Two PowerShell scripts in `scripts\` (also included next to the exe in the release zip) switch which front end starts at logon:

```powershell
.\Enable-RustyPopperStartup.ps1    # start Rusty Popper at logon, disable "Run Pinup Popper"
.\Disable-RustyPopperStartup.ps1   # go back to Pinup Popper
```

Both scripts ask for administrator rights, which changing Popper's task needs. Nothing is deleted, and `-WhatIf` shows what a script would do without changing anything.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
