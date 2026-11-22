//! Phase 1 spike: read Popper's DB and screen config, then loop one game's
//! media on every configured cabinet screen at the right position and rotation.
//!
//! Usage:
//!   rusty-popper [--popper DIR] [--mpv PATH\libmpv-2.dll] [--game TEXT]
//!                [--seconds N] [--list] [--dry-run]
//!
//! Esc (or q in a video window) quits.

mod mpv;
mod window;

use anyhow::{anyhow, bail, Context, Result};
use rp_config::{self as config, Screen, Slot};
use rp_db as db;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Args {
    popper: PathBuf,
    mpv: Option<PathBuf>,
    game: Option<String>,
    seconds: Option<u64>,
    list: bool,
    dry_run: bool,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        popper: PathBuf::from(r"C:\vPinball\PinUPSystem"),
        mpv: None,
        game: None,
        seconds: None,
        list: false,
        dry_run: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--popper" => a.popper = val()?.into(),
            "--mpv" => a.mpv = Some(val()?.into()),
            "--game" => a.game = Some(val()?),
            "--seconds" => a.seconds = Some(val()?.parse()?),
            "--list" => a.list = true,
            "--dry-run" => a.dry_run = true,
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(a)
}

fn find_mpv_dll(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe.with_file_name("libmpv-2.dll"));
    }
    candidates.push(PathBuf::from(r"D:\Claude\deps\mpv\libmpv-2.dll"));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("libmpv-2.dll not found; pass --mpv"))
}

fn main() -> Result<()> {
    let args = parse_args()?;

    let db = db::PopperDb::open_copy(&args.popper.join("PUPDatabase.db"))?;
    let emus = db.emulators()?;
    let games = db.games()?;
    println!("DB: {} visible emulators, {} visible games", emus.len(), games.len());

    if args.list {
        for g in &games {
            let emu = emus.iter().find(|e| e.id == g.emu_id).map_or("?", |e| e.name.as_str());
            println!("  [{emu}] {} ({})", g.display, g.file_name);
        }
        return Ok(());
    }

    let game = match &args.game {
        Some(q) => {
            let q = q.to_lowercase();
            games
                .iter()
                .find(|g| g.display.to_lowercase().contains(&q) || g.name.to_lowercase().contains(&q))
                .ok_or_else(|| anyhow!("no visible game matches {q:?}"))?
        }
        None => games.first().ok_or_else(|| anyhow!("no visible games"))?,
    };
    let emu = emus
        .iter()
        .find(|e| e.id == game.emu_id)
        .ok_or_else(|| anyhow!("game's emulator {} is hidden or missing", game.emu_id))?;
    let media_dir = emu
        .media_dir
        .clone()
        .unwrap_or_else(|| args.popper.join("POPMedia").join(&emu.name));
    println!("Game: {} [{}] (GameID {})", game.display, emu.name, game.id);

    let screens = config::read_screens(&args.popper.join("PinUpPlayer.ini"))?;
    // Only the always-on menu screens; GameInfo/GameHelp/etc. are on-demand overlays.
    const LIVE: &[Slot] = &[Slot::Topper, Slot::Dmd, Slot::BackGlass, Slot::PlayField, Slot::FullDmd];
    let plan: Vec<(Screen, PathBuf)> = screens
        .into_iter()
        .filter(|s| LIVE.contains(&s.slot))
        .filter_map(|s| {
            let media = config::find_media(&media_dir, s.slot, &game.name);
            match &media {
                Some(m) => println!(
                    "  {:?}: {}x{} at {},{} rot {} -> {}",
                    s.slot, s.width, s.height, s.x, s.y, s.rotation, m.display()
                ),
                None => println!("  {:?}: no media in {}, skipped", s.slot, s.slot.media_folder()),
            }
            media.map(|m| (s, m))
        })
        .collect();

    let dll = find_mpv_dll(args.mpv)?;
    let mpv = mpv::Mpv::load(&dll)?;
    println!("libmpv: {}", dll.display());

    if args.dry_run {
        return Ok(());
    }
    if plan.is_empty() {
        bail!("nothing to play for {}", game.display);
    }

    window::init_dpi();

    // Keep windows alive for as long as their players; players drop first.
    let mut windows = Vec::new();
    let mut players = Vec::new();
    for (s, media) in &plan {
        let win = window::HostWindow::create(&format!("Rusty Popper {:?}", s.slot), s.x, s.y, s.width, s.height)?;
        let audio = s.slot == Slot::PlayField;
        let opts = vec![
            ("config", "no".to_string()),
            ("terminal", "no".to_string()),
            ("wid", win.wid()),
            ("keepaspect", "no".to_string()),
            ("video-rotate", s.rotation.to_string()),
            ("loop-file", "inf".to_string()),
            ("image-display-duration", "inf".to_string()),
            ("hwdec", "auto-safe".to_string()),
            ("osc", "no".to_string()),
            ("osd-level", "0".to_string()),
            ("cursor-autohide", "always".to_string()),
            ("input-default-bindings", "yes".to_string()),
            ("input-vo-keyboard", "yes".to_string()),
            ("aid", if audio { "auto" } else { "no" }.to_string()),
            ("volume", s.volume.to_string()),
        ];
        let p = mpv.player(&opts).with_context(|| format!("{:?}", s.slot))?;
        p.command(&["keybind", "ESC", "quit"])?;
        p.command(&["loadfile", &media.to_string_lossy()])?;
        windows.push(win);
        players.push((s.slot, p));
    }

    let deadline = args.seconds.map(|s| Instant::now() + Duration::from_secs(s));
    'run: loop {
        if !window::pump() {
            break;
        }
        for (slot, p) in &players {
            if p.poll_events().contains(&mpv::EVENT_SHUTDOWN) {
                println!("{slot:?} player quit");
                break 'run;
            }
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    drop(players);
    drop(windows);
    Ok(())
}
