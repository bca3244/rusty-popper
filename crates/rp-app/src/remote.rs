//! Phone remote: a small web page on the home network that shows what's
//! playing, changes the Options settings, installs updates, and can exit
//! Rusty Popper, restart or shut down the cabinet.
//!
//! Off by default (Options → Phone remote, or `[remote] enabled`). A tiny
//! HTTP/1.1 server on std's TCP sockets, no async runtime: one thread waits
//! for connections and each request is answered on its own short-lived
//! thread. The page only sends GETs and body-less POSTs, so request bodies
//! are refused unread, and every socket has read and write timeouts. The
//! menu thread publishes a small snapshot of what's on screen whenever it
//! changes, and picks up the page's commands each frame.
//! A QR code in the corner of the wheel links to the page.
//!
//! Only machines on the local network are answered (private, link-local and
//! loopback addresses). A PIN is optional (`[remote] pin`). Commands need a
//! POST with a custom header, which a web page on another site can't send
//! without the browser asking first, so a link or form elsewhere can't press
//! the buttons.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

/// Default port (fixed, so the link and QR code never change).
pub const DEFAULT_PORT: u16 = 8642;
/// How long the server thread waits after a failed accept.
const TICK: Duration = Duration::from_secs(1);
/// How often the server thread looks for a new connection or a stop.
const ACCEPT_POLL: Duration = Duration::from_millis(50);
/// A phone that stops reading or writing for this long is dropped.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// A request's head (request line and headers) may be at most this big.
const MAX_HEAD: usize = 8 << 10;
/// Requests answered at once; more connections are closed unanswered.
const MAX_CONNECTIONS: usize = 16;
/// Wrong PINs before requests are refused for `PIN_LOCKOUT`.
const PIN_TRIES: u32 = 5;
const PIN_LOCKOUT: Duration = Duration::from_secs(30);
/// The longest lockout (after several in a row).
const PIN_LOCKOUT_MAX: Duration = Duration::from_secs(30 * 60);
/// Wrong PINs are forgotten after this long without one.
const PIN_FORGET: Duration = Duration::from_secs(60 * 60);
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

/// Processes the page lists and can end (a hung table): any whose name
/// starts with one of these (lowercase). Not "pinup" as a whole: that would
/// take in PinUpDOF.exe, the DOF bridge Rusty Popper itself talks to.
const KILLABLE: &[&str] = &[
    // Game engines
    "vpinball",
    "future pinball",
    "pinmame",
    "vpinmame",
    // PinUP Popper / PuP (PinUpMenu also covers PinUpMenuSetup)
    "pinupmenu",
    "pinupplayer",
    "pinupdisplayer",
    "pinuppackeditor",
    "pupserver",
    // DMD and backglass
    "dmdext",
    "flexdmd",
    "b2sbackglassserverexe",
];

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
    pub video_sound: bool,
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
    /// Where updates stand: "idle", "checking", "none" (no new update),
    /// "available", "busy" (downloading or installing) or "failed".
    pub update_state: &'static str,
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
    /// Requests being answered right now.
    connections: AtomicUsize,
    pin_guard: Mutex<PinGuard>,
    pin: Option<String>,
    /// `[remote] address`, also accepted as a Host name.
    address: Option<String>,
}

/// The running web server.
pub struct Remote {
    shared: Arc<Shared>,
    rx: Receiver<Request>,
    thread: Option<JoinHandle<()>>,
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
        let pin = pin.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
        if pin.as_ref().is_some_and(|p| !p.chars().all(|c| c.is_ascii_alphanumeric())) {
            anyhow::bail!("the PIN may only have letters and digits");
        }
        let tried_since = std::time::Instant::now();
        let listener = loop {
            match TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) {
                Ok(l) => break l,
                Err(_) if tried_since.elapsed() < wait => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => return Err(e).with_context(|| format!("listening on port {port}")),
            }
        };
        // Polled, so the thread notices a stop without a wake-up connection.
        listener.set_nonblocking(true).context("setting up the listener")?;
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot { state: "menu", version: env!("CARGO_PKG_VERSION"), ..Default::default() }),
            settings: Mutex::new(SettingsView::default()),
            url: Mutex::new(make_url(&host_for(address.as_deref()), port, pin.as_deref())),
            url_version: AtomicU64::new(1),
            stop: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
            pin_guard: Mutex::new(PinGuard::default()),
            pin,
            address,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = shared.clone();
        let thread = std::thread::Builder::new().name("remote".into()).spawn(move || serve(listener, worker, tx, port)).context("starting the web server thread")?;
        let remote = Remote { shared, rx, thread: Some(thread), last_key: None, image_version: first_image_version(), settings_at: None };
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
        // The accept loop sees this within one poll and closes the listener,
        // so the port is free when this returns. Requests still being
        // answered finish on their own threads (each has socket timeouts).
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One request's place in `Shared::connections`, given back when dropped.
struct Slot(Arc<Shared>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Wrong-PIN throttle: after `PIN_TRIES` wrong PINs, every request is
/// refused for `PIN_LOCKOUT`, doubling with each lockout in a row. One
/// counter for everyone (a phone that scanned the QR code never gets it
/// wrong), and a right PIN doesn't reset it, so guesses can't hide among a
/// phone's polls. It's forgotten after `PIN_FORGET` with no wrong PIN.
#[derive(Default)]
struct PinGuard {
    wrong: u32,
    lockouts: u32,
    last_wrong: Option<std::time::Instant>,
    locked_until: Option<std::time::Instant>,
}

/// The server thread: accepts connections until told to stop, answering
/// each on a short-lived thread so a slow or stalled phone holds up nobody.
fn serve(listener: TcpListener, shared: Arc<Shared>, tx: Sender<Request>, port: u16) {
    let mut address_checked = std::time::Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        if address_checked.elapsed() >= ADDRESS_EVERY {
            address_checked = std::time::Instant::now();
            let url = make_url(&host_for(shared.address.as_deref()), port, shared.pin.as_deref());
            if let Ok(mut u) = shared.url.lock() {
                if *u != url {
                    log::info!("Phone remote moved to {}", short_url(&url));
                    *u = url;
                    shared.url_version.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                // Nothing is read from anyone off the local network.
                if !is_local(peer.ip()) {
                    continue;
                }
                if shared.connections.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
                    shared.connections.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                // Gives the slot back however the request ends (a panic too),
                // or at once if its thread can't start.
                let slot = Slot(shared.clone());
                let tx = tx.clone();
                let spawned = std::thread::Builder::new().name("remote-request".into()).spawn(move || {
                    connection(stream, &slot.0, &tx);
                    drop(slot);
                });
                if spawned.is_err() {
                    log::warn!("Phone remote: couldn't start a request thread");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(ACCEPT_POLL),
            // A client that gave up before we got to it: nothing wrong here.
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => {}
            Err(e) => {
                log::warn!("Phone remote: {e}");
                std::thread::sleep(TICK);
            }
        }
    }
    log::info!("Phone remote stopped");
}

/// A parsed request: only what the page uses.
#[derive(Debug, Default)]
struct Req {
    method: String,
    path: String,
    query: String,
    host: String,
    has_command_header: bool,
}

/// Why a request couldn't be read.
#[derive(Debug, PartialEq)]
enum BadRequest {
    /// Closed or timed out before a full request arrived: say nothing.
    Gone,
    /// Answer with this status and close.
    Status(u16, &'static str),
}

/// Reads one request's head (at most `MAX_HEAD` bytes). The page never sends
/// a body, so any request announcing one is refused unread.
fn read_request(stream: &mut impl Read) -> Result<Req, BadRequest> {
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    let end = loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => return Err(BadRequest::Gone),
            Ok(n) => n,
        };
        // Search only the new bytes (and the three before them).
        let from = head.len().saturating_sub(3);
        head.extend_from_slice(&buf[..n]);
        let found = head[from..].windows(4).position(|w| w == b"\r\n\r\n").map(|i| from + i);
        // Too big whether or not this read finished it.
        if found.unwrap_or(head.len()) > MAX_HEAD {
            return Err(BadRequest::Status(431, "Request too large"));
        }
        if let Some(end) = found {
            break end;
        }
    };
    let head = std::str::from_utf8(&head[..end]).map_err(|_| BadRequest::Status(400, "Bad request"))?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target), Some(version), None) = (first.next(), first.next(), first.next(), first.next()) else {
        return Err(BadRequest::Status(400, "Bad request"));
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(BadRequest::Status(400, "Bad request"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut req = Req { method: method.to_string(), path: path.to_string(), query: query.to_string(), ..Default::default() };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(BadRequest::Status(400, "Bad request"));
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("Host") {
            req.host = value.to_string();
        } else if name.eq_ignore_ascii_case(COMMAND_HEADER) {
            req.has_command_header = true;
        } else if name.eq_ignore_ascii_case("Transfer-Encoding") || (name.eq_ignore_ascii_case("Content-Length") && value != "0") {
            return Err(BadRequest::Status(413, "No request body accepted"));
        }
    }
    Ok(req)
}

/// Answers one connection: one request, then close.
fn connection(mut stream: TcpStream, shared: &Shared, tx: &Sender<Request>) {
    // Accepted sockets may inherit the listener's non-blocking mode.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let response = match read_request(&mut stream) {
        Ok(req) => handle(&req, shared, tx),
        Err(BadRequest::Gone) => return,
        Err(BadRequest::Status(code, why)) => text(code, why),
    };
    let _ = response.write_to(&mut stream);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn handle(req: &Req, shared: &Shared, tx: &Sender<Request>) -> Response {
    let path = req.path.as_str();
    let (get, post) = (req.method == "GET", req.method == "POST");
    if !host_ok(&req.host, shared.address.as_deref()) {
        return text(403, "Forbidden");
    }
    if get && (path == "/icon.png" || path == "/favicon.ico" || path == "/apple-touch-icon.png") {
        // The icon needs no PIN: browsers fetch it without the page's query.
        return respond(200, "image/png", crate::ICON_PNG.to_vec()).with_header("Cache-Control", "max-age=86400");
    }
    if let Some(refusal) = check_pin(shared, query_value(&req.query, "pin")) {
        return refusal;
    }
    let has_header = req.has_command_header;
    let query = req.query.as_str();
    match path {
        "/" if get => page(),
        "/api/status" if get => status(shared),
        "/api/wheel.png" if get => wheel_image(shared),
        "/api/procs" if get => procs(),
        // Ended here, not on the menu thread (taskkill takes a moment). A
        // game Rusty Popper started then ends as if it had exited.
        "/api/kill" if post && !has_header => text(403, "Missing header"),
        "/api/kill" if post => match kill_target(query) {
            Some(target) => kill(target),
            None => text(400, "Bad process"),
        },
        p if post && p.starts_with("/api/") => match command_of(p, query) {
            None => text(404, "Not found"),
            Some(_) if !has_header => text(403, "Missing header"),
            Some(cmd) => run_command(cmd, tx),
        },
        _ => text(404, "Not found"),
    }
}

/// None if the PIN (if one is set) is right; otherwise the refusal. Wrong
/// PINs count towards the lockout; a missing one (an old bookmark) doesn't.
fn check_pin(shared: &Shared, given: Option<&str>) -> Option<Response> {
    let pin = shared.pin.as_deref()?;
    let mut guard = shared.pin_guard.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if guard.locked_until.is_some_and(|t| now < t) {
        return Some(text(429, "Too many wrong PINs: try again later"));
    }
    if guard.last_wrong.is_some_and(|t| now.duration_since(t) >= PIN_FORGET) {
        *guard = PinGuard::default();
    }
    match given {
        Some(g) if g == pin => None,
        Some(_) => {
            guard.wrong += 1;
            guard.last_wrong = Some(now);
            if guard.wrong >= PIN_TRIES {
                let lockout = PIN_LOCKOUT.saturating_mul(1 << guard.lockouts.min(6)).min(PIN_LOCKOUT_MAX);
                log::warn!("Phone remote: {} wrong PINs; refusing requests for {} s", guard.wrong, lockout.as_secs());
                guard.wrong = 0;
                guard.lockouts += 1;
                guard.locked_until = Some(now + lockout);
            }
            Some(text(403, "Wrong PIN: scan the QR code on the cabinet again"))
        }
        None => Some(text(403, "Missing PIN: scan the QR code on the cabinet again")),
    }
}

/// An answer: status, body and headers. Every answer closes the connection.
struct Response {
    code: u16,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
}

impl Response {
    fn with_header(mut self, name: &'static str, value: &str) -> Response {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name, value.to_string()));
        self
    }

    fn write_to(&self, out: &mut impl Write) -> std::io::Result<()> {
        let mut head = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n", self.code, reason(self.code), self.body.len());
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.write_all(&self.body)
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

fn respond(code: u16, content_type: &str, body: Vec<u8>) -> Response {
    Response { code, body, headers: Vec::new() }
        .with_header("Content-Type", content_type)
        .with_header("Cache-Control", "no-store")
        .with_header("X-Content-Type-Options", "nosniff")
        // The PIN is in the page's address: never pass it on, and never let
        // another site's page show this one inside a frame.
        .with_header("Referrer-Policy", "no-referrer")
        .with_header("X-Frame-Options", "DENY")
}

fn text(code: u16, body: &str) -> Response {
    respond(code, "text/plain; charset=utf-8", body.as_bytes().to_vec())
}

fn page() -> Response {
    respond(200, "text/html; charset=utf-8", PAGE.as_bytes().to_vec())
        .with_header("Content-Security-Policy", "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'")
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

/// The processes a hung table may have left, as JSON.
fn procs() -> Response {
    let list: Vec<_> = rp_launch::processes_named(KILLABLE).into_iter().map(|p| serde_json::json!({ "pid": p.pid, "name": p.name, "secs": p.secs })).collect();
    respond(200, "application/json", serde_json::to_vec(&list).unwrap_or_default())
}

/// `pid=123` (one process) or `pid=all` (`Some(None)`).
fn kill_target(query: &str) -> Option<Option<u32>> {
    match query_value(query, "pid")? {
        "all" => Some(None),
        n => n.parse().ok().map(Some),
    }
}

fn kill(target: Option<u32>) -> Response {
    let ended = rp_launch::end_processes(target, KILLABLE);
    if ended.is_empty() {
        return text(409, "Not running any more");
    }
    let names: Vec<&str> = ended.iter().map(|p| p.name.as_str()).collect();
    log::warn!("Phone remote: ending {}", names.join(", "));
    text(200, &names.join(", "))
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
                    // A submenu open on the cabinet may have lost rows.
                    self.clamp_menu();
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
                    match self.playing.as_mut() {
                        // Close the game properly first (its close script, the
                        // play statistics); return_from_game then powers off.
                        Some(p) => {
                            log::info!("Phone remote: closing the game before {flag}");
                            p.ctl.request_stop();
                            p.stop_pressed = Some(std::time::Instant::now());
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
        assert_eq!(kill_target("pin=1&pid=4242"), Some(Some(4242)));
        assert_eq!(kill_target("pid=all"), Some(None));
        assert_eq!(kill_target("pid=-1"), None);
        assert_eq!(kill_target(""), None);
    }

    #[test]
    fn killable_names() {
        let killable = |n: &str| KILLABLE.iter().any(|p| n.to_lowercase().starts_with(p));
        for ok in ["VPinballX64.exe", "Future Pinball.exe", "PinUpMenu.exe", "PinUpMenuSetup.exe", "PinUpPlayer.exe", "PinUpDisplayer.exe", "PUPServer.exe", "PinUpPackEditor.exe", "dmdext.exe", "FlexDMDUI.exe", "B2SBackglassServerEXE.exe", "PinMAME32.exe"] {
            assert!(killable(ok), "{ok}");
        }
        for never in ["rusty-popper.exe", "PinUpDOF.exe", "explorer.exe", "B2SServer.exe"] {
            assert!(!killable(never), "{never}");
        }
    }

    #[test]
    fn qr_has_a_border() {
        let (rgba, n) = qr_rgba("http://192.168.1.20:8642/").unwrap();
        assert_eq!(rgba.len(), n * n * 4);
        assert!(rgba[..2 * n * 4].iter().all(|&b| b == 255), "top border is white");
        assert!(rgba.chunks(4).any(|p| p[0] == 0), "has dark modules");
    }

    #[test]
    fn parses_requests_and_refuses_bodies() {
        let parse = |raw: &str| read_request(&mut raw.as_bytes());
        let r = parse("POST /api/set?pin=1&key=volume&value=40 HTTP/1.1\r\nHost: 10.0.0.2:8642\r\nx-rusty-popper: 1\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str(), r.query.as_str(), r.host.as_str(), r.has_command_header), ("POST", "/api/set", "pin=1&key=volume&value=40", "10.0.0.2:8642", true));
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: 5000000000\r\n\r\n").unwrap_err(), BadRequest::Status(413, "No request body accepted"));
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: 18446744073709551615\r\n\r\n").unwrap_err(), BadRequest::Status(413, "No request body accepted"));
        assert_eq!(parse("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap_err(), BadRequest::Status(413, "No request body accepted"));
        assert_eq!(parse("GET / HTTP/1.1\r\nHost: x").unwrap_err(), BadRequest::Gone, "cut short");
        assert_eq!(parse("GET /\r\n\r\n").unwrap_err(), BadRequest::Status(400, "Bad request"));
        assert_eq!(parse("GET http://evil/ HTTP/1.1\r\n\r\n").unwrap_err(), BadRequest::Status(400, "Bad request"));
        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert_eq!(parse(&huge).unwrap_err(), BadRequest::Status(431, "Request too large"));
    }

    #[test]
    fn bad_pin_is_refused_before_binding() {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        let err = Remote::start(port, Some("12 34".into()), None, Duration::ZERO).err().expect("refused");
        assert!(err.to_string().contains("PIN"), "{err:#}");
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
        assert!(get("POST /api/kill?pin=9&pid=all HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 403"), "a kill needs the header");
        let list = get("GET /api/procs?pin=9 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert!(list.starts_with("HTTP/1.1 200") && list.ends_with(']'), "{list}");
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

    #[test]
    fn hostile_clients_cant_hang_or_crash_it() {
        use std::io::{Read, Write};
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let remote = Remote::start(port, Some("42".into()), Some("127.0.0.1".into()), Duration::ZERO).unwrap();
        let get = |req: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        // A huge announced body is refused at once, unread.
        let big = get("POST /api/exit?pin=42 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2000000000\r\n\r\n");
        assert!(big.starts_with("HTTP/1.1 413"), "{big}");
        // A client that connects and sends nothing doesn't hold up others.
        let _stalled = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let ok = get("GET /api/status?pin=42 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        // Wrong PINs lock everyone out for a while, the right one too.
        for _ in 0..PIN_TRIES {
            assert!(get("GET /api/status?pin=0 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").starts_with("HTTP/1.1 403"));
        }
        let locked = get("GET /api/status?pin=42 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(locked.starts_with("HTTP/1.1 429"), "{locked}");
        // Stopping is quick even with the stalled client, and frees the port.
        let t = std::time::Instant::now();
        drop(remote);
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        std::net::TcpListener::bind(("0.0.0.0", port)).expect("port free after stop");
    }

    #[test]
    fn page_cant_be_framed() {
        let mut out = Vec::new();
        page().write_to(&mut out).unwrap();
        let head = String::from_utf8_lossy(&out);
        assert!(head.contains("frame-ancestors 'none'") && head.contains("X-Frame-Options: DENY") && head.contains("Referrer-Policy: no-referrer"), "{head}");
        assert!(head.contains(&format!("Content-Length: {}", PAGE.len())));
    }
}
