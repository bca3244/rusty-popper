//! Update check and one-press install from GitHub releases.
//!
//! - At startup a background thread asks GitHub for the latest release. If
//!   it's newer, the wheel shows a small note and Options offers to install.
//! - Installing downloads the release zip, checks it against the SHA-256
//!   GitHub publishes for the asset, unpacks it next to the exe, moves the
//!   files it replaces into `rollback\`, and restarts into the new version.
//!   rusty-popper.toml and other local files are never touched.
//! - Until the new version has run for a while it is "on probation"
//!   (`update-pending.json`): if it fails to start, or keeps dying, the old
//!   files are put back and the old version is started again. Only failed
//!   starts count: one that ends well (Exit, a shutdown) is taken back.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const USER_AGENT: &str = concat!("rusty-popper/", env!("CARGO_PKG_VERSION"));
const MARKER: &str = "update-pending.json";
/// Files in the release that are user data or not ours to replace.
const KEEP: &[&str] = &["rusty-popper.toml"];
/// A new version that has failed to get going this many times is rolled back.
const MAX_ATTEMPTS: u32 = 3;
/// What the update row says when GitHub couldn't be reached.
const CHECK_FAILED: &str = "couldn't reach GitHub";
/// When the startup check can't reach GitHub (the network may come up after
/// Rusty Popper at boot), it tries again after these waits.
const CHECK_RETRIES: &[std::time::Duration] = &[
    std::time::Duration::from_secs(60),
    std::time::Duration::from_secs(120),
    std::time::Duration::from_secs(300),
    std::time::Duration::from_secs(600),
];

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub zip_url: String,
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub enum Status {
    Off,
    Checking,
    UpToDate,
    Available(Release),
    Downloading { version: String, percent: u32 },
    /// Unpacked into this folder, ready to swap in.
    Staged { version: String, dir: PathBuf },
    /// Swapped in; the new version is starting and this one is exiting.
    Restarting(String),
    Failed(String),
}

/// Parses "v0.2.13" / "0.2.13" into numbers.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

/// Is `candidate` a newer version than `current`?
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            b > a
        }
        _ => false,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into()
}

/// For the zip (`size` bytes, ~50 MB): give up on a dead connection, but
/// not on a slow one.
fn download_agent(size: u64) -> ureq::Agent {
    use std::time::Duration;
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // A stalled transfer fails (and can be retried) instead of hanging.
        .timeout_recv_body(Some(body_timeout(size)))
        .build()
        .into()
}

/// How long the zip's body may take: ureq only has a limit for the whole
/// body, so it is sized to allow even a slow line (16 KB/s), plus ten minutes.
fn body_timeout(size: u64) -> std::time::Duration {
    std::time::Duration::from_secs(600 + size / (16 * 1024))
}

/// Whether a release's version is safe to use in file names: it names the
/// download and the folder it is unpacked into (and cleared out of).
fn safe_version(v: &str) -> bool {
    v.len() <= 64 && v.starts_with(|c: char| c.is_ascii_alphanumeric()) && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// Asks GitHub for the latest release of `repo` ("owner/name").
pub fn latest_release(repo: &str) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let text = agent()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("asking GitHub for {repo}'s latest release"))?
        .body_mut()
        .read_to_string()?;
    parse_release(&text)
}

fn parse_release(json: &str) -> Result<Release> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let tag = v["tag_name"].as_str().ok_or_else(|| anyhow!("release has no tag"))?;
    let asset = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.ends_with("-windows-x64.zip"))))
        .ok_or_else(|| anyhow!("release {tag} has no Windows zip"))?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    if !safe_version(&version) {
        bail!("release tag {tag:?} isn't a plain version");
    }
    Ok(Release {
        version,
        zip_url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
        sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
        size: asset["size"].as_u64().unwrap_or(0),
    })
}

/// Shared state between the menu and the background update work.
#[derive(Clone)]
pub struct Updater {
    status: Arc<Mutex<Status>>,
    repo: String,
    exe_dir: PathBuf,
}

impl Updater {
    pub fn new(enabled: bool, repo: &str, exe_dir: &Path) -> Updater {
        let u = Updater {
            status: Arc::new(Mutex::new(if enabled { Status::Checking } else { Status::Off })),
            repo: repo.to_string(),
            exe_dir: exe_dir.to_path_buf(),
        };
        if enabled {
            u.check_with_retries(CHECK_RETRIES);
        }
        u
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or(Status::Off)
    }

    pub fn fail(&self, why: String) {
        self.set(Status::Failed(why));
    }

    /// Marks the swap as done so it is never applied twice.
    pub fn restarting(&self, version: String) {
        self.set(Status::Restarting(version));
    }

    fn set(&self, s: Status) {
        if let Ok(mut g) = self.status.lock() {
            *g = s;
        }
    }

    /// Checks for a newer release in the background.
    pub fn check(&self) {
        self.check_with_retries(&[]);
    }

    /// Checks in the background; if GitHub can't be reached, tries again
    /// after each of `retries`, as long as nothing else (a check from
    /// Options, a download) has happened meanwhile.
    fn check_with_retries(&self, retries: &'static [std::time::Duration]) {
        self.set(Status::Checking);
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let mut waits = retries.iter();
            loop {
                let s = match latest_release(&me.repo) {
                    Ok(r) if is_newer(env!("CARGO_PKG_VERSION"), &r.version) => {
                        log::info!("Update available: {} (running {})", r.version, env!("CARGO_PKG_VERSION"));
                        Status::Available(r)
                    }
                    Ok(_) => Status::UpToDate,
                    Err(e) => {
                        log::warn!("Update check failed: {e:#}");
                        Status::Failed(CHECK_FAILED.into())
                    }
                };
                let failed = matches!(&s, Status::Failed(_));
                me.set(s);
                let Some(wait) = waits.next().filter(|_| failed) else { return };
                std::thread::sleep(*wait);
                if !matches!(me.status(), Status::Failed(why) if why == CHECK_FAILED) {
                    return;
                }
                log::info!("Checking for an update again");
            }
        });
    }

    /// Downloads, verifies and unpacks `rel` in the background.
    pub fn download(&self, rel: Release) {
        self.set(Status::Downloading { version: rel.version.clone(), percent: 0 });
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-download".into()).spawn(move || {
            let s = match me.fetch_and_stage(&rel) {
                Ok(dir) => Status::Staged { version: rel.version.clone(), dir },
                Err(e) => {
                    log::error!("Update to {} failed: {e:#}", rel.version);
                    Status::Failed(format!("{e:#}"))
                }
            };
            me.set(s);
        });
    }

    fn fetch_and_stage(&self, rel: &Release) -> Result<PathBuf> {
        let Some(want) = rel.sha256.clone() else {
            bail!("the release has no published checksum");
        };
        let work = self.exe_dir.join("update");
        std::fs::create_dir_all(&work)?;
        let zip_path = work.join(format!("rusty-popper-{}.zip", rel.version));
        let mut resp = download_agent(rel.size).get(&rel.zip_url).header("User-Agent", USER_AGENT).call().context("downloading the update")?;
        let mut reader = resp.body_mut().with_config().limit(500 * 1024 * 1024).reader();
        let mut file = std::fs::File::create(&zip_path)?;
        let mut hasher = Sha256::new();
        let (mut got, mut buf) = (0u64, vec![0u8; 256 * 1024]);
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            got += n as u64;
            if let Some(p) = (got * 100).checked_div(rel.size) {
                self.set(Status::Downloading { version: rel.version.clone(), percent: p.min(99) as u32 });
            }
        }
        file.flush()?;
        drop(file);
        let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if hex != want {
            let _ = std::fs::remove_file(&zip_path);
            bail!("the download didn't match GitHub's checksum");
        }
        log::info!("Downloaded and verified {}", zip_path.display());
        let dir = work.join(&rel.version);
        let _ = std::fs::remove_dir_all(&dir);
        unpack(&zip_path, &dir)?;
        if !dir.join("rusty-popper.exe").is_file() {
            bail!("the update doesn't contain rusty-popper.exe");
        }
        Ok(dir)
    }
}

/// Unpacks the release zip into `dir`, dropping its top-level folder.
fn unpack(zip_path: &Path, dir: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else { continue };
        let rel: PathBuf = name.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dir.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
    }
    Ok(())
}

/// The arguments this process was started with (minus the restart counter),
/// so the restarted copy keeps `--popper`, `--mpv` and the like.
pub(crate) fn passthrough_args() -> Vec<String> {
    let mut out = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--restarted" {
            args.next();
        } else {
            out.push(a);
        }
    }
    out
}

/// Swaps the staged files in (the replaced ones go to `rollback\`), marks the
/// new version as on probation and starts it. The caller then exits.
///
/// All or nothing: new files are first copied in beside the old ones as
/// `*.new`, then each old file is renamed aside and its `.new` renamed into
/// place. If anything fails, every step already taken is undone, so the
/// install is left exactly as it was.
pub fn apply_and_restart(staged: &Path, exe_dir: &Path, to_version: &str) -> Result<()> {
    if on_probation(exe_dir) {
        bail!("the previous update hasn't been confirmed yet; restart Rusty Popper first");
    }
    let names: Vec<String> = std::fs::read_dir(staged)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !KEEP.iter().any(|k| k.eq_ignore_ascii_case(n)))
        .collect();
    if !names.iter().any(|n| n.eq_ignore_ascii_case("rusty-popper.exe")) {
        bail!("the update doesn't contain rusty-popper.exe");
    }
    let rollback = exe_dir.join("rollback");
    // The previous update was confirmed (no marker), so its backup can go.
    if rollback.exists() {
        std::fs::remove_dir_all(&rollback).context("clearing the old rollback folder")?;
    }
    std::fs::create_dir_all(&rollback)?;

    // 1. Copy everything in beside the old files.
    let new_name = |n: &str| exe_dir.join(format!("{n}.new"));
    let undo_copies = |names: &[String]| {
        for n in names {
            let _ = std::fs::remove_file(new_name(n));
        }
    };
    for n in &names {
        if let Err(e) = std::fs::copy(staged.join(n), new_name(n)) {
            undo_copies(&names);
            return Err(e).with_context(|| format!("copying {n}"));
        }
    }
    // 2. Mark probation before touching the live files, so an interrupted
    //    swap is rolled back on the next start.
    let marker = serde_json::json!({ "from": env!("CARGO_PKG_VERSION"), "to": to_version, "attempts": 0, "files": names });
    if let Err(e) = std::fs::write(exe_dir.join(MARKER), marker.to_string()) {
        undo_copies(&names);
        return Err(e).context("writing the update marker");
    }
    // 3. Swap. A running exe or loaded DLL can't be overwritten but can be renamed.
    let mut moved: Vec<&str> = Vec::new();
    let mut placed: Vec<&str> = Vec::new();
    let result = (|| -> Result<()> {
        for n in &names {
            let target = exe_dir.join(n);
            if target.exists() {
                std::fs::rename(&target, rollback.join(n)).with_context(|| format!("moving {n} aside"))?;
                moved.push(n);
            }
            std::fs::rename(new_name(n), &target).with_context(|| format!("installing {n}"))?;
            placed.push(n);
        }
        Ok(())
    })();
    if let Err(e) = result {
        for n in &placed {
            let _ = std::fs::remove_file(exe_dir.join(n));
        }
        for n in &moved {
            let _ = std::fs::rename(rollback.join(n), exe_dir.join(n));
        }
        undo_copies(&names);
        let _ = std::fs::remove_file(exe_dir.join(MARKER));
        return Err(e);
    }
    log::info!("Installed {to_version}; restarting (old files in {})", rollback.display());
    if let Err(e) = std::process::Command::new(exe_dir.join("rusty-popper.exe")).current_dir(exe_dir).args(passthrough_args()).spawn() {
        // The files are in place; the next start (e.g. at logon) runs the new version.
        log::error!("Couldn't start the new version: {e}");
    }
    Ok(())
}

/// Puts the files from `rollback\` back and starts the old version. Whatever
/// happens, the probation marker is cleared, so a failed rollback can't stop
/// every later start.
pub fn rollback(exe_dir: &Path, why: &str) -> Result<()> {
    log::error!("Rolling back the update: {why}");
    let result = restore(exe_dir);
    abandon(exe_dir);
    result?;
    std::process::Command::new(exe_dir.join("rusty-popper.exe")).current_dir(exe_dir).args(passthrough_args()).spawn()?;
    Ok(())
}

fn restore(exe_dir: &Path) -> Result<()> {
    let rollback = exe_dir.join("rollback");
    if !rollback.join("rusty-popper.exe").is_file() {
        bail!("there's no previous version in {} to go back to", rollback.display());
    }
    let failed = exe_dir.join("update").join("failed");
    std::fs::create_dir_all(&failed)?;
    for entry in std::fs::read_dir(&rollback)? {
        let entry = entry?;
        let target = exe_dir.join(entry.file_name());
        if target.exists() {
            let _ = std::fs::remove_file(failed.join(entry.file_name()));
            std::fs::rename(&target, failed.join(entry.file_name()))?;
        }
        std::fs::rename(entry.path(), &target)?;
    }
    Ok(())
}

/// Stops treating the current install as on probation.
pub fn abandon(exe_dir: &Path) {
    let _ = std::fs::remove_file(exe_dir.join(MARKER));
}
/// What startup should do about a pending update.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Probation {
    /// Nothing pending.
    None,
    /// This run is the new version, still on probation.
    OnProbation,
    /// The new version has failed too often: roll back now.
    GiveUp,
}

/// Called at startup: counts this attempt of a freshly installed version.
pub fn start_probation(exe_dir: &Path) -> Probation {
    let path = exe_dir.join(MARKER);
    let Ok(text) = std::fs::read_to_string(&path) else { return Probation::None };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
        let _ = std::fs::remove_file(&path);
        return Probation::None;
    };
    let ours = Some(env!("CARGO_PKG_VERSION"));
    if v["to"].as_str() != ours {
        if v["from"].as_str() != ours {
            // The release's exe reports a different version than its tag. It's
            // still the new install, so keep it on probation.
            log::warn!("Installed release {} reports version {}", v["to"], env!("CARGO_PKG_VERSION"));
        } else {
            // We're the version from before the update. A rollback clears
            // the marker itself, so the swap was cut short (a power cut):
            // put back what it had already replaced.
            let files: Vec<String> = v["files"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect()).unwrap_or_default();
            undo_interrupted_swap(exe_dir, &files);
            let _ = std::fs::remove_file(&path);
            return Probation::None;
        }
    }
    let attempts = v["attempts"].as_u64().unwrap_or(0) + 1;
    v["attempts"] = attempts.into();
    let _ = std::fs::write(&path, v.to_string());
    if attempts as u32 > MAX_ATTEMPTS { Probation::GiveUp } else { Probation::OnProbation }
}

/// Undoes a swap that stopped part way, with the old exe still in place:
/// each of `files` already moved to `rollback\` goes back (the new copy
/// that replaced it goes to `update\failed\`), and the `*.new` copies go.
fn undo_interrupted_swap(exe_dir: &Path, files: &[String]) {
    let rollback = exe_dir.join("rollback");
    let failed = exe_dir.join("update").join("failed");
    // Only plain names from our own marker, never a path.
    for n in files.iter().filter(|n| !n.is_empty() && !n.contains(['/', '\\', ':']) && *n != "..") {
        let old = rollback.join(n);
        if old.is_file() {
            let live = exe_dir.join(n);
            if live.exists() {
                let _ = std::fs::create_dir_all(&failed);
                let _ = std::fs::remove_file(failed.join(n));
                if let Err(e) = std::fs::rename(&live, failed.join(n)) {
                    log::error!("Undoing the unfinished update: moving {n} aside: {e}");
                    continue;
                }
            }
            match std::fs::rename(&old, &live) {
                Ok(()) => log::warn!("Undid the unfinished update of {n}"),
                Err(e) => log::error!("Undoing the unfinished update: restoring {n}: {e}"),
            }
        }
        let _ = std::fs::remove_file(exe_dir.join(format!("{n}.new")));
    }
}

/// A start that ended well (Exit, a shutdown) before the new version was
/// confirmed: it doesn't count against it.
pub fn forgive_attempt(exe_dir: &Path) {
    let path = exe_dir.join(MARKER);
    let Ok(mut v) = std::fs::read_to_string(&path).map_err(|_| ()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|_| ())) else { return };
    if let Some(n) = v["attempts"].as_u64().filter(|n| *n > 0) {
        v["attempts"] = (n - 1).into();
        let _ = std::fs::write(&path, v.to_string());
    }
}

/// Is a freshly installed version still on probation?
pub fn on_probation(exe_dir: &Path) -> bool {
    exe_dir.join(MARKER).is_file()
}

/// The new version has proved itself: keep it.
pub fn confirm(exe_dir: &Path) {
    if std::fs::remove_file(exe_dir.join(MARKER)).is_ok() {
        log::info!("Update confirmed: {} is running fine", env!("CARGO_PKG_VERSION"));
        // The download and unpacked files aren't needed any more (rollback\ stays
        // until the next update, for going back by hand).
        let _ = std::fs::remove_dir_all(exe_dir.join("update"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.2.13", "v0.2.14"));
        assert!(is_newer("0.2.9", "0.2.10"));
        assert!(is_newer("0.2", "0.2.1"));
        assert!(!is_newer("0.2.13", "0.2.13"));
        assert!(!is_newer("0.3.0", "0.2.99"));
        assert!(!is_newer("0.2.13", "garbage"));
    }

    #[test]
    fn parses_github_release_json() {
        let r = parse_release(
            r#"{"tag_name":"v0.2.14","assets":[{"name":"notes.txt"},{"name":"rusty-popper-v0.2.14-windows-x64.zip","size":123,
            "digest":"sha256:ABCD","browser_download_url":"https://example/x.zip"}]}"#,
        )
        .unwrap();
        assert_eq!((r.version.as_str(), r.sha256.as_deref(), r.size), ("0.2.14", Some("abcd"), 123));
        assert!(parse_release(r#"{"tag_name":"v1","assets":[]}"#).is_err());
    }

    #[test]
    fn apply_then_rollback_round_trip() {
        let root = std::env::temp_dir().join(format!("rp-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (app, staged) = (root.join("app"), root.join("staged"));
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        for (d, v) in [(&app, "old"), (&staged, "new")] {
            std::fs::write(d.join("rusty-popper.exe"), v).unwrap();
            std::fs::write(d.join("libmpv-2.dll"), v).unwrap();
            std::fs::write(d.join("rusty-popper.toml"), v).unwrap();
        }
        let read = |p: &str| std::fs::read_to_string(app.join(p)).unwrap();
        // The dummy exe can't be started; that's only logged.
        apply_and_restart(&staged, &app, "9.9.9").unwrap();
        assert_eq!((read("rusty-popper.exe").as_str(), read("libmpv-2.dll").as_str()), ("new", "new"));
        assert_eq!(read("rusty-popper.toml"), "old", "settings are kept");
        assert_eq!(read("rollback/rusty-popper.exe"), "old");
        assert!(on_probation(&app) && !app.join("rusty-popper.exe.new").exists());
        // A second update before this one is confirmed is refused, keeping the backup.
        assert!(apply_and_restart(&staged, &app, "9.9.10").is_err());
        assert_eq!(read("rollback/rusty-popper.exe"), "old");
        // Rolling back restores the old files and always clears the marker
        // (starting the dummy exe fails, which is reported).
        assert!(rollback(&app, "test").is_err());
        assert_eq!(read("rusty-popper.exe"), "old");
        assert!(!on_probation(&app));
        // With nothing to go back to, the marker still goes.
        std::fs::write(app.join(MARKER), "{}").unwrap();
        assert!(rollback(&app, "test").is_err());
        assert!(!on_probation(&app));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_plain_versions_are_used() {
        for ok in ["0.2.56", "1.0.0-rc.1", "0.3+build.7"] {
            assert!(safe_version(ok), "{ok}");
        }
        for bad in ["", "..", ".", "../x", "0.2/../..", "0.2\\x", "C:x", " 0.2", "-1"] {
            assert!(!safe_version(bad), "{bad}");
        }
        assert!(parse_release(r#"{"tag_name":"v..","assets":[{"name":"rusty-popper-windows-x64.zip"}]}"#).is_err());
        // A slow line gets time: ~50 MB at 16 KB/s is under an hour, plus ten minutes.
        assert!(body_timeout(50 << 20) >= std::time::Duration::from_secs(600 + 3200));
    }

    #[test]
    fn interrupted_swap_is_undone_by_the_old_version() {
        let app = std::env::temp_dir().join(format!("rp-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app);
        std::fs::create_dir_all(app.join("rollback")).unwrap();
        // Power cut after the DLL was swapped, before the exe: the old exe
        // starts with the new DLL in place.
        std::fs::write(app.join("rusty-popper.exe"), "old").unwrap();
        std::fs::write(app.join("rusty-popper.exe.new"), "new").unwrap();
        std::fs::write(app.join("libmpv-2.dll"), "new").unwrap();
        std::fs::write(app.join("rollback").join("libmpv-2.dll"), "old").unwrap();
        let m = serde_json::json!({"from":env!("CARGO_PKG_VERSION"),"to":"9.9.9","attempts":0,"files":["libmpv-2.dll","rusty-popper.exe","../evil"]});
        std::fs::write(app.join(MARKER), m.to_string()).unwrap();
        assert_eq!(start_probation(&app), Probation::None);
        let read = |p: &str| std::fs::read_to_string(app.join(p)).unwrap();
        assert_eq!((read("libmpv-2.dll").as_str(), read("rusty-popper.exe").as_str()), ("old", "old"));
        assert!(!app.join("rusty-popper.exe.new").exists() && !on_probation(&app));
        let _ = std::fs::remove_dir_all(&app);
    }

    #[test]
    fn a_start_that_ended_well_is_forgiven() {
        let dir = std::env::temp_dir().join(format!("rp-forgive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let m = serde_json::json!({"from":"0.0.1","to":env!("CARGO_PKG_VERSION"),"attempts":0});
        std::fs::write(dir.join(MARKER), m.to_string()).unwrap();
        // Many clean starts and exits inside the first minute never roll back.
        for _ in 0..MAX_ATTEMPTS * 3 {
            assert_eq!(start_probation(&dir), Probation::OnProbation);
            forgive_attempt(&dir);
        }
        forgive_attempt(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        forgive_attempt(&dir); // no marker: nothing happens
    }

    #[test]
    fn probation_counts_attempts_and_gives_up() {
        let dir = std::env::temp_dir().join(format!("rp-upd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(start_probation(&dir), Probation::None);
        let m = serde_json::json!({"from":"0.0.1","to":env!("CARGO_PKG_VERSION"),"attempts":0});
        std::fs::write(dir.join(MARKER), m.to_string()).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(start_probation(&dir), Probation::OnProbation);
        }
        assert_eq!(start_probation(&dir), Probation::GiveUp);
        confirm(&dir);
        assert!(!on_probation(&dir));
        // Back on the version the update came from (rolled back): cleared.
        let m = serde_json::json!({"from":env!("CARGO_PKG_VERSION"),"to":"9.9.9"});
        std::fs::write(dir.join(MARKER), m.to_string()).unwrap();
        assert_eq!(start_probation(&dir), Probation::None);
        assert!(!on_probation(&dir));
        // A release whose exe reports another version than its tag stays on probation.
        std::fs::write(dir.join(MARKER), r#"{"from":"0.0.1","to":"9.9.9"}"#).unwrap();
        assert_eq!(start_probation(&dir), Probation::OnProbation);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
