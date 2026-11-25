//! Phone remote: a small web page on the home network that shows what's
//! playing, changes the Options settings, installs updates, and can exit
//! Rusty Popper, restart or shut down the cabinet.
//!
//! Off by default (Options → Phone remote, or `[remote] enabled`). One
//! `tiny_http` server on its own thread, no async runtime: it idles waiting
//! for a connection. The menu thread publishes a small snapshot of what's on
//! screen whenever it changes, and picks up the page's commands each frame.
//! A QR code in the corner of the wheel links to the page.
//!
//! Only machines on the local network are answered (private, link-local and
//! loopback addresses). A PIN is optional (`[remote] pin`). Commands need a
//! POST with a custom header, which a web page on another site can't send
//! without the browser asking first, so a link or form elsewhere can't press
//! the buttons.

use anyhow::{Context, Result};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

/// Default port (fixed, so the link and QR code never change).
pub const DEFAULT_PORT: u16 = 8642;
/// How often the server thread wakes to look for a stop or a new address.
const TICK: Duration = Duration::from_secs(1);
/// How often to look whether the cabinet's address has changed (the network
/// can come up after Rusty Popper at boot).
const ADDRESS_EVERY: Duration = Duration::from_secs(20);
/// How long a command waits for the menu thread to take it.
const REPLY_WAIT: Duration = Duration::from_secs(3);
/// Wheel images bigger than this aren't sent.
const MAX_IMAGE: u64 = 8 << 20;
/// How often the menu thread refreshes the settings the page shows.
const SETTINGS_EVERY: Duration = Duration::from_millis(500);
/// The header every command needs.
const COMMAND_HEADER: &str = "X-Rusty-Popper";

static PAGE: &str = include_str!("remote.html");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Exit,
    Restart,
    Shutdown,
    /// Change a setting (`key`, `value`), as Options would.
    Set(String, String),
}

/// The Options settings, for the page's Settings card.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SettingsView {
    pub style: &'static str,
    pub attract: bool,
    pub attract_idle: u64,
    pub attract_interval: u64,
    pub idle_choices: &'static [u64],
    pub interval_choices: &'static [u64],
    pub volume: i32,
    pub sfx: bool,
    pub sfx_volume: i32,
    pub game_audio: bool,
    pub game_audio_volume: i32,
    pub dof: bool,
    /// Where updates stand, as Options words it.
    pub update: String,
    /// What the update button does now: "install", "check" or "" (busy).
    pub update_action: &'static str,
}

/// A command from the page, and where to say whether it was taken.
pub type Request = (Command, SyncSender<Result<(), String>>);

/// A game's details for the page.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct GameInfo {
    pub title: String,
    pub emulator: String,
    pub manufacturer: Option<String>,
    pub year: Option<i64>,
    pub players: Option<i64>,
    pub rom: Option<String>,
    pub file: String,
    pub tags: Vec<String>,
}

/// What's on the cabinet, as the menu thread last published it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    /// "menu", "launching", "playing", "exiting" or "other" (a table another
    /// program started).
    pub state: &'static str,
    /// The game being played, or the one on the wheel.
    pub game: Option<GameInfo>,
    /// A playlist entry on the wheel, or the program of an "other" table.
    pub note: Option<String>,
    /// Seconds since the game started (filled in per request).
    pub secs: Option<u64>,
    pub version: &'static str,
    /// Bumped whenever the wheel image changes, so the page reloads it.
    pub image: u64,
    #[serde(skip)]
    pub since: Option<SystemTime>,
    #[serde(skip)]
    pub wheel_png: Option<PathBuf>,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    settings: Mutex<SettingsView>,
    url: Mutex<String>,
    /// Bumped when `url` changes.
    url_version: AtomicU64,
    stop: AtomicBool,
}

/// The running web server.
pub struct Remote {
    shared: Arc<Shared>,
    rx: Receiver<Request>,
    server: Option<Arc<tiny_http::Server>>,
    thread: Option<JoinHandle<()>>,
    port: u16,
    /// The last thing published, to skip unchanged snapshots.
    last_key: Option<(u8, usize)>,
    image_version: u64,
    /// When the settings were last published.
    settings_at: Option<std::time::Instant>,
}

impl Remote {
    /// Starts listening on `port` on every interface. `address` overrides the
    /// host the QR code links to (otherwise this machine's LAN address).
    /// `wait` is how long to keep trying while the port is taken (at start
    /// the copy being replaced by a restart or update may still hold it).
    pub fn start(port: u16, pin: Option<String>, address: Option<String>, wait: Duration) -> Result<Remote> {
        let tried_since = std::time::Instant::now();
        let server = loop {
            match tiny_http::Server::http(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) {
                Ok(s) => break Arc::new(s),
                Err(_) if tried_since.elapsed() < wait => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => return Err(anyhow::anyhow!("{e}")).with_context(|| format!("listening on port {port}")),
            }
        };
        let pin = pin.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
        if pin.as_ref().is_some_and(|p| !p.chars().all(|c| c.is_ascii_alphanumeric())) {
            anyhow::bail!("the PIN may only have letters and digits");
        }
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot { state: "menu", version: env!("CARGO_PKG_VERSION"), ..Default::default() }),
            settings: Mutex::new(SettingsView::default()),
            url: Mutex::new(make_url(&host_for(address.as_deref()), port, pin.as_deref())),
            url_version: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let (worker, worker_server) = (shared.clone(), server.clone());
        let thread = std::thread::Builder::new()
            .name("remote".into())
            .spawn(move || serve(&worker_server, worker, tx, port, pin, address))
            .context("starting the web server thread")?;
        let remote = Remote { shared, rx, server: Some(server), thread: Some(thread), port, last_key: None, image_version: first_image_version(), settings_at: None };
        log::info!("Phone remote on {}", short_url(&remote.url().0));
        Ok(remote)
    }

    /// The page's address (with the PIN, if any) and its version number.
    pub fn url(&self) -> (String, u64) {
        let url = self.shared.url.lock().map(|u| u.clone()).unwrap_or_default();
        (url, self.shared.url_version.load(Ordering::Relaxed))
    }

    /// Publishes what's on screen. `key` identifies it (state and item):
    /// `build` only runs when that changed since the last call.
    pub fn publish(&mut self, key: (u8, usize), build: impl FnOnce() -> Snapshot) {
        if self.last_key == Some(key) {
            return;
        }
        let same_item = self.last_key.is_some_and(|k| k.1 == key.1);
        self.last_key = Some(key);
        let mut snap = build();
        if !same_item {
            self.image_version += 1;
        }
        snap.image = self.image_version;
        snap.version = env!("CARGO_PKG_VERSION");
        if let Ok(mut s) = self.shared.snapshot.lock() {
            *s = snap;
        }
    }

    /// Whether it's time to publish the settings again.
    pub fn settings_due(&mut self) -> bool {
        let due = self.settings_at.is_none_or(|t| t.elapsed() >= SETTINGS_EVERY);
        if due {
            self.settings_at = Some(std::time::Instant::now());
        }
        due
    }

    /// Publishes the settings, if they changed.
    pub fn publish_settings(&mut self, view: SettingsView) {
        if let Ok(mut s) = self.shared.settings.lock() {
            if *s != view {
                *s = view;
            }
        }
    }

    /// Makes the next `publish` rebuild the snapshot even for the same key
    /// (the database was re-read, so an index may now be another game).
    pub fn invalidate(&mut self) {
        self.last_key = None;
    }

    /// The next command from the page, if any.
    pub fn try_recv(&self) -> Option<Request> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        // Wakes our thread at once.
        if let Some(s) = &self.server {
            s.unblock();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // Dropping the server closes the listener only once tiny_http's
        // accept thread wakes. It wakes itself by connecting to the address
        // it listens on, 0.0.0.0, which Windows refuses: wake it through
        // localhost instead, so the port is free when this returns.
        drop(self.server.take());
        let _ = std::net::TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_millis(300));
    }
}

/// The server thread: answers requests until told to stop.
fn serve(server: &tiny_http::Server, shared: Arc<Shared>, tx: Sender<Request>, port: u16, pin: Option<String>, address: Option<String>) {
    let mut address_checked = std::time::Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        if address_checked.elapsed() >= ADDRESS_EVERY {
            address_checked = std::time::Instant::now();
            let url = make_url(&host_for(address.as_deref()), port, pin.as_deref());
            if let Ok(mut u) = shared.url.lock() {
                if *u != url {
                    log::info!("Phone remote moved to {}", short_url(&url));
                    *u = url;
                    shared.url_version.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        match server.recv_timeout(TICK) {
            Ok(Some(req)) => handle(req, &shared, &tx, pin.as_deref(), address.as_deref()),
            Ok(None) => {}
            Err(_) if shared.stop.load(Ordering::Relaxed) => break,
            Err(e) => {
                log::warn!("Phone remote: {e}");
                std::thread::sleep(TICK);
            }
        }
    }
    log::info!("Phone remote stopped");
}

fn handle(req: tiny_http::Request, shared: &Shared, tx: &Sender<Request>, pin: Option<&str>, address: Option<&str>) {
    let from_lan = req.remote_addr().is_some_and(|a| is_local(a.ip()));
    let host = req.headers().iter().find(|h| h.field.equiv("Host")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
    let (path, query) = req.url().split_once('?').unwrap_or((req.url(), ""));
    let path = path.to_string();
    let pin_ok = pin.is_none_or(|p| query_value(query, "pin").is_some_and(|v| v == p));
    let response = if !from_lan || !host_ok(&host, address) {
        text(403, "Forbidden")
    } else if !pin_ok {
        text(403, "Wrong or missing PIN: scan the QR code on the cabinet again")
    } else {
        let has_header = req.headers().iter().any(|h| h.field.equiv(COMMAND_HEADER));
        match (req.method(), path.as_str()) {
            (tiny_http::Method::Get, "/") => page(),
            (tiny_http::Method::Get, "/api/status") => status(shared),
            (tiny_http::Method::Get, "/api/wheel.png") => wheel_image(shared),
            (tiny_http::Method::Post, p) if p.starts_with("/api/") => match command_of(p, query) {
                None => text(404, "Not found"),
                Some(_) if !has_header => text(403, "Missing header"),
                Some(cmd) => run_command(cmd, tx),
            },
            _ => text(404, "Not found"),
        }
    };
    let _ = req.respond(response);
}

type Response = tiny_http::Response<std::io::Cursor<Vec<u8>>>;

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn respond(code: u16, content_type: &str, body: Vec<u8>) -> Response {
    tiny_http::Response::from_data(body)
        .with_status_code(code)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
}

fn text(code: u16, body: &str) -> Response {
    respond(code, "text/plain; charset=utf-8", body.as_bytes().to_vec())
}

fn page() -> Response {
    respond(200, "text/html; charset=utf-8", PAGE.as_bytes().to_vec())
        .with_header(header("Content-Security-Policy", "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'"))
}

fn status(shared: &Shared) -> Response {
    let mut snap = shared.snapshot.lock().map(|s| s.clone()).unwrap_or_default();
    snap.secs = snap.since.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
    let settings = shared.settings.lock().map(|s| s.clone()).unwrap_or_default();
    let mut json = serde_json::to_value(&snap).unwrap_or_default();
    json["settings"] = serde_json::to_value(&settings).unwrap_or_default();
    respond(200, "application/json", serde_json::to_vec(&json).unwrap_or_default())
}

fn wheel_image(shared: &Shared) -> Response {
    let path = shared.snapshot.lock().ok().and_then(|s| s.wheel_png.clone());
    let data = path.filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_IMAGE)).and_then(|p| std::fs::read(p).ok());
    match data {
        Some(bytes) => respond(200, "image/png", bytes),
        None => text(404, "No wheel image"),
    }
}

fn command_of(path: &str, query: &str) -> Option<Command> {
    // Setting names and values are short words and numbers.
    let word = |s: &str| !s.is_empty() && s.len() <= 24 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    match path {
        "/api/exit" => Some(Command::Exit),
        "/api/restart" => Some(Command::Restart),
        "/api/shutdown" => Some(Command::Shutdown),
        "/api/set" => {
            let (key, value) = (query_value(query, "key")?, query_value(query, "value")?);
            (word(key) && word(value)).then(|| Command::Set(key.to_string(), value.to_string()))
        }
        _ => None,
    }
}

/// Hands a command to the menu thread and waits for its answer.
fn run_command(cmd: Command, tx: &Sender<Request>) -> Response {
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    if tx.send((cmd, reply_tx)).is_err() {
        return text(503, "Rusty Popper is closing");
    }
    match reply_rx.recv_timeout(REPLY_WAIT) {
        Ok(Ok(())) => text(200, "OK"),
        Ok(Err(why)) => text(409, &why),
        Err(_) => text(503, "Rusty Popper didn't answer"),
    }
}

/// The value of `key` in a query string (no percent-decoding: PINs are plain).
fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

/// Whether a request comes from this network (or this machine).
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

/// Whether the Host header names this machine: an IP address, localhost, the
/// configured `address`, or the computer's name (bare, or in a home-network
/// domain). Any other name pointing here (DNS rebinding) is turned away.
fn host_ok(host: &str, address: Option<&str>) -> bool {
    host_ok_for(host, &std::env::var("COMPUTERNAME").unwrap_or_default(), address)
}

fn host_ok_for(host: &str, computer: &str, address: Option<&str>) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        // [v6]:port
        return rest.split(']').next().is_some_and(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok());
    } else {
        host.rsplit_once(':').map_or(host, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { host })
    };
    if name.parse::<Ipv4Addr>().is_ok() || name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if address.is_some_and(|a| a.trim().eq_ignore_ascii_case(name)) {
        return true;
    }
    let (first, domain) = name.split_once('.').unwrap_or((name, ""));
    let home_domain = ["", "local", "lan", "home", "home.arpa", "localdomain"].iter().any(|d| domain.eq_ignore_ascii_case(d));
    !computer.is_empty() && first.eq_ignore_ascii_case(computer) && home_domain
}

/// The host the QR code links to: the configured one, else this machine's
/// LAN address.
fn host_for(address: Option<&str>) -> String {
    match address.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => a.to_string(),
        None => lan_ip().map_or_else(|| "localhost".into(), |ip| ip.to_string()),
    }
}

/// This machine's address on the network the default route uses. Connecting
/// a UDP socket sends nothing; it only picks the outgoing interface.
fn lan_ip() -> Option<IpAddr> {
    ["8.8.8.8:80", "192.168.1.1:80", "192.168.0.1:80", "10.0.0.1:80", "172.16.0.1:80"].iter().find_map(|target| {
        let s = UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect(target).ok()?;
        let ip = s.local_addr().ok()?.ip();
        (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
    })
}

fn make_url(host: &str, port: u16, pin: Option<&str>) -> String {
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    match pin {
        Some(p) => format!("http://{host}:{port}/?pin={p}"),
        None => format!("http://{host}:{port}/"),
    }
}

/// The QR code for `text` as an RGBA image, one pixel per module with a
/// thin white border (two modules: phones read that fine, and it keeps the
/// code small). Drawn scaled up without smoothing.
pub fn qr_rgba(text: &str) -> Option<(Vec<u8>, usize)> {
    use qrcodegen::{QrCode, QrCodeEcc};
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let border = 2;
    let n = qr.size() as usize + 2 * border;
    let mut rgba = Vec::with_capacity(n * n * 4);
    for y in 0..n as i32 {
        for x in 0..n as i32 {
            let dark = qr.get_module(x - border as i32, y - border as i32);
            let v = if dark { 0 } else { 255 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Some((rgba, n))
}

/// Where the wheel-image counter starts: different for every start, so a
/// page left open across a restart can't mistake the new game's number for
/// the old one's.
fn first_image_version() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(1, |d| d.as_secs()) * 1000
}

/// The part of a URL to print under the QR code: host and port.
pub fn short_url(url: &str) -> &str {
    let s = url.strip_prefix("http://").unwrap_or(url);
    s.split('/').next().unwrap_or(s)
}

/// What the menu publishes, by state number (the first half of a publish key).
pub const MENU: u8 = 0;
pub const LAUNCHING: u8 = 1;
pub const PLAYING: u8 = 2;
pub const EXITING: u8 = 3;
pub const OTHER: u8 = 4;

impl Remote {
    /// Bumped when the page's address changes (the QR code must be redrawn).
    pub fn url_version(&self) -> u64 {
        self.shared.url_version.load(Ordering::Relaxed)
    }
}

impl crate::App {
    /// Turns the phone remote on or off (from Options, or at start).
    pub(crate) fn set_remote(&mut self, on: bool, wait: Duration) {
        self.remote_cfg.enabled = on;
        self.remote_error = None;
        if !on {
            self.remote = None;
            if let Some(g) = self.gfx.as_mut() {
                if let Some(q) = g.qr.take() {
                    g.canvas.delete_image(q.img);
                }
            }
            return;
        }
        if self.remote.is_some() {
            return;
        }
        let c = &self.remote_cfg;
        match Remote::start(c.port, c.pin.clone(), c.address.clone(), wait) {
            Ok(r) => self.remote = Some(r),
            Err(e) => {
                log::warn!("Phone remote: {e:#}");
                let pin_bad = e.chain().any(|c| c.to_string().contains("PIN"));
                self.remote_error = Some(if pin_bad { "PIN: letters and digits only".into() } else { format!("port {} busy", c.port) });
            }
        }
    }

    /// Carries out the page's commands. Runs every tick, also during a game.
    pub(crate) fn poll_remote(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        let Some(r) = self.remote.as_mut() else { return };
        let requests: Vec<Request> = std::iter::from_fn(|| r.try_recv()).collect();
        // The page's Settings card: a couple of times a second is plenty,
        // and at once after a change (published after the commands below).
        let changed = requests.iter().any(|(c, _)| matches!(c, Command::Set(..)));
        let due = r.settings_due() || changed;
        for (cmd, reply) in requests {
            log::info!("Phone remote: {cmd:?}");
            match cmd {
                Command::Set(key, value) => {
                    let _ = reply.send(self.set_from_remote(&key, &value));
                }
                // Leaving mid-game would skip the emulator's close-down script.
                Command::Exit if self.playing.is_some() => {
                    let _ = reply.send(Err("A game is running. Close it first.".into()));
                }
                Command::Exit => {
                    let _ = reply.send(Ok(()));
                    el.exit();
                }
                Command::Restart | Command::Shutdown => {
                    let flag = if cmd == Command::Restart { "/r" } else { "/s" };
                    let _ = reply.send(Ok(()));
                    match &self.playing {
                        // Close the game properly first (its close script, the
                        // play statistics); return_from_game then powers off.
                        Some(p) => {
                            log::info!("Phone remote: closing the game before {flag}");
                            p.ctl.request_stop();
                            self.pending_power = Some((flag, std::time::Instant::now()));
                        }
                        None => crate::power(el, flag),
                    }
                }
            }
        }
        if due {
            let view = self.settings_view();
            if let Some(r) = self.remote.as_mut() {
                r.publish_settings(view);
            }
        }
    }

    /// Tells the page what's on screen: `state` (MENU, PLAYING, ...) and the
    /// item it's about. Cheap when nothing changed.
    pub(crate) fn publish_remote(&mut self, state: u8, index: Option<usize>, since: Option<std::time::Instant>, note: Option<&str>) {
        let Some(r) = self.remote.as_mut() else { return };
        let lib = &self.lib;
        // Without an item, the note tells snapshots apart.
        let key = index.unwrap_or_else(|| note.map_or(usize::MAX, |n| n.bytes().fold(5381usize, |h, b| h.wrapping_mul(33) ^ b as usize)));
        r.publish((state, key), || {
            let item = index.and_then(|i| lib.items.get(i));
            let (game, note) = match item.map(|it| (it, &it.kind)) {
                Some((it, crate::Kind::Game { game, emu })) => (
                    Some(GameInfo {
                        title: it.title.clone(),
                        emulator: emu.name.clone(),
                        manufacturer: game.manufacturer.clone(),
                        year: game.year,
                        players: game.players,
                        rom: game.rom.clone(),
                        file: game.file_name.clone(),
                        tags: game.tags.clone(),
                    }),
                    None,
                ),
                Some((it, _)) => (None, Some(format!("Playlist: {}", it.title))),
                None => (None, note.map(str::to_string)),
            };
            let state = match state {
                LAUNCHING => "launching",
                PLAYING => "playing",
                EXITING => "exiting",
                OTHER => "other",
                _ => "menu",
            };
            Snapshot {
                state,
                wheel_png: game.as_ref().and(item).and_then(|it| it.wheel_png.clone()),
                game,
                note,
                since: since.map(|t| SystemTime::now() - t.elapsed()),
                ..Default::default()
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lan_addresses() {
        for ok in ["192.168.1.20", "10.0.0.5", "172.16.3.4", "127.0.0.1", "169.254.1.1", "::1", "fe80::1", "fd00::5", "::ffff:192.168.1.9"] {
            assert!(is_local(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:1.1.1.1"] {
            assert!(!is_local(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn host_must_name_this_machine() {
        assert!(host_ok_for("192.168.1.20:8642", "GAMING", None));
        assert!(host_ok_for("localhost:8642", "GAMING", None));
        assert!(host_ok_for("gaming:8642", "GAMING", None));
        assert!(host_ok_for("gaming.lan", "GAMING", None));
        assert!(host_ok_for("[fe80::1]:8642", "GAMING", None));
        assert!(host_ok_for("pinball.lan:8642", "DESKTOP-XYZ", Some("pinball.lan")));
        assert!(!host_ok_for("gaming.evil.com:8642", "GAMING", None));
        assert!(!host_ok_for("evil.example.com:8642", "GAMING", None));
        assert!(!host_ok_for("", "GAMING", None));
        assert!(!host_ok_for("gaming", "", None));
    }

    #[test]
    fn pin_from_query() {
        assert_eq!(query_value("pin=1234", "pin"), Some("1234"));
        assert_eq!(query_value("a=1&pin=77", "pin"), Some("77"));
        assert_eq!(query_value("pinx=1", "pin"), None);
        assert_eq!(query_value("", "pin"), None);
    }

    #[test]
    fn urls() {
        assert_eq!(make_url("192.168.1.20", 8642, None), "http://192.168.1.20:8642/");
        assert_eq!(make_url("192.168.1.20", 8642, Some("42")), "http://192.168.1.20:8642/?pin=42");
        assert_eq!(make_url("fe80::1", 80, None), "http://[fe80::1]:80/");
        assert_eq!(short_url("http://192.168.1.20:8642/?pin=42"), "192.168.1.20:8642");
        assert_eq!(command_of("/api/shutdown", ""), Some(Command::Shutdown));
        assert_eq!(command_of("/api/format", ""), None);
        assert_eq!(command_of("/api/set", "pin=1&key=volume&value=40"), Some(Command::Set("volume".into(), "40".into())));
        assert_eq!(command_of("/api/set", "key=volume&value=4%200"), None);
        assert_eq!(command_of("/api/set", "key=volume"), None);
    }

    #[test]
    fn qr_has_a_border() {
        let (rgba, n) = qr_rgba("http://192.168.1.20:8642/").unwrap();
        assert_eq!(rgba.len(), n * n * 4);
        assert!(rgba[..2 * n * 4].iter().all(|&b| b == 255), "top border is white");
        assert!(rgba.chunks(4).any(|p| p[0] == 0), "has dark modules");
    }

    #[test]
    fn serves_status_and_takes_commands() {
        use std::io::{Read, Write};
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut remote = Remote::start(port, Some("9".into()), Some("127.0.0.1".into()), Duration::ZERO).unwrap();
        remote.publish((0, 3), || Snapshot { state: "menu", game: Some(GameInfo { title: "Attack from Mars".into(), ..Default::default() }), ..Default::default() });
        let get = |req: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let ok = get("GET /api/status?pin=9 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("Attack from Mars"), "{ok}");
        assert!(get("GET /api/status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 403"));
        assert!(get("GET /?pin=9 HTTP/1.1\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 403"));
        assert!(get("POST /api/exit?pin=9 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 403"), "a command needs the header");
        // A command reaches the menu thread, which answers it.
        let answer = std::thread::spawn(move || {
            for _ in 0..200 {
                if let Some((cmd, reply)) = remote.try_recv() {
                    assert_eq!(cmd, Command::Restart);
                    let _ = reply.send(Err("busy".into()));
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("no command arrived");
        });
        let r = get("POST /api/restart?pin=9 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Rusty-Popper: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        answer.join().unwrap();
        assert!(r.starts_with("HTTP/1.1 409") && r.ends_with("busy"), "{r}");
    }
}
