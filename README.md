# Rusty Popper

A fast, sleek alternate frontend for [PinUP Popper](https://www.nailbuster.com/wikipinup/) on virtual pinball cabinets, written in Rust.

Rusty Popper aims to replace **PinUpMenu.exe** only. It reuses everything Popper already manages, so your existing setup keeps working and you can switch back at any time.

## What it reuses from Popper

| Popper piece | How Rusty Popper uses it |
|---|---|
| `PUPDatabase.db` | Read via SQLite (games, emulators, playlists, stats) |
| `POPMedia\` tree | Playfield / backglass / DMD / topper / wheel media per game |
| `PinUpPlayer.ini` | Screen layout: which monitor and rectangle each display uses |
| Emulator `LaunchScript` / `PostScript` | Run as-is to start and close tables (VPX, FP, FX, etc.) |
| PinUP Player, B2S, DOF | Untouched, keep doing their jobs |

## Planned stack

- **Windowing / GL:** `winit` + `glutin`, one borderless window per cab screen
- **Video:** `libmpv` render API (hardware decode, seamless looping)
- **2D / text:** `skia-safe` (or `femtovg` as a lighter option)
- **Database:** `rusqlite`
- **Input:** `gilrs` for controllers, Windows Raw Input for keyboard encoders

## Workspace layout

```
crates/
  rp-app/     binary: rusty-popper (UI, main loop)
  rp-db/      PUPDatabase.db access
  rp-config/  PinUpPlayer.ini + POPMedia path resolution
  rp-launch/  emulator launch/post scripts, process tracking
```

## Roadmap

1. **Spike:** open the DB, list games, play a looping playfield video on the right monitor.
2. **MVP:** wheel navigation, multi-screen media, launch and return via emulator scripts.
3. **Polish:** playlists, search, attract mode, themes.

## Building

```
cargo build --release
```

## Phase 1 spike

`rusty-popper` reads `PUPDatabase.db` from a temp copy, opened read-only. It also reads the screen layout from `PinUpPlayer.ini`. It then loops one game's media on each live screen (Topper, DMD, Backglass, Playfield and FullDMD), each in a borderless Win32 window at Popper's coordinates and rotation. libmpv renders into each window via `wid`.

It needs `libmpv-2.dll` from the `mpv-dev-x86_64` build of zhongfly/mpv-winbuild. The spike looks for it next to the exe, then in `D:\Claude\deps\mpv\`, and you can also pass `--mpv PATH`. The DLL is loaded at runtime, so building needs no import library.

```
rusty-popper --list
rusty-popper --dry-run --game "addams family"
rusty-popper --game "addams family" --seconds 60
```

Press Esc (or q in a video window) to quit. The spike never writes to Popper's files.

Windows is the target platform. Rusty Popper is not affiliated with or endorsed by nailbuster / PinUP Popper.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
