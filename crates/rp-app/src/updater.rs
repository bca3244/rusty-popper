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
//!   files are put back and the old version is started again.

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

/// For the zip: no overall limit (it's ~50 MB), but give up on a dead connection.
fn download_agent() -> ureq::Agent {
    use std::time::Duration;
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into()
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
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
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
            u.check();
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
        self.set(Status::Checking);
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let s = match latest_release(&me.repo) {
                Ok(r) if is_newer(env!("CARGO_PKG_VERSION"), &r.version) => {
                    log::info!("Update available: {} (running {})", r.version, env!("CARGO_PKG_VERSION"));
                    Status::Available(r)
                }
                Ok(_) => Status::UpToDate,
                Err(e) => {
                    log::warn!("Update check failed: {e:#}");
                    Status::Failed("couldn't reach GitHub".into())
                }
            };
            me.set(s);
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
        let mut resp = download_agent().get(&rel.zip_url).header("User-Agent", USER_AGENT).call().context("downloading the update")?;
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

/// Swaps the staged files in (the replaced ones go to `rollback\`), marks the
/// new version as on probation and starts it. The caller then exits.
pub fn apply_and_restart(staged: &Path, exe_dir: &Path, to_version: &str) -> Result<()> {
    let rollback = exe_dir.join("rollback");
    let _ = std::fs::remove_dir_all(&rollback);
    std::fs::create_dir_all(&rollback)?;
    let mut moved: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(staged)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if KEEP.iter().any(|k| k.eq_ignore_ascii_case(&name)) || !entry.path().is_file() {
            continue;
        }
        let target = exe_dir.join(&name);
        if target.exists() {
            // A running exe or loaded DLL can't be overwritten but can be renamed.
            std::fs::rename(&target, rollback.join(&name)).with_context(|| format!("moving {name} aside"))?;
            moved.push(name.clone());
        }
        std::fs::copy(entry.path(), &target).with_context(|| format!("installing {name}"))?;
    }
    let marker = serde_json::json!({ "from": env!("CARGO_PKG_VERSION"), "to": to_version, "attempts": 0, "files": moved });
    std::fs::write(exe_dir.join(MARKER), marker.to_string())?;
    log::info!("Installed {to_version}; restarting (old files in {})", rollback.display());
    std::process::Command::new(exe_dir.join("rusty-popper.exe")).current_dir(exe_dir).spawn().context("starting the new version")?;
    Ok(())
}

/// Puts the files from `rollback\` back and starts the old version.
pub fn rollback(exe_dir: &Path, why: &str) -> Result<()> {
    log::error!("Rolling back the update: {why}");
    let rollback = exe_dir.join("rollback");
    for entry in std::fs::read_dir(&rollback)? {
        let entry = entry?;
        let target = exe_dir.join(entry.file_name());
        if target.exists() {
            let failed = exe_dir.join("update").join("failed");
            std::fs::create_dir_all(&failed)?;
            let _ = std::fs::remove_file(failed.join(entry.file_name()));
            std::fs::rename(&target, failed.join(entry.file_name()))?;
        }
        std::fs::rename(entry.path(), &target)?;
    }
    let _ = std::fs::remove_file(exe_dir.join(MARKER));
    std::process::Command::new(exe_dir.join("rusty-popper.exe")).current_dir(exe_dir).spawn()?;
    Ok(())
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
    if v["to"].as_str() != Some(env!("CARGO_PKG_VERSION")) {
        // We're not the version being tried (e.g. already rolled back).
        let _ = std::fs::remove_file(&path);
        return Probation::None;
    }
    let attempts = v["attempts"].as_u64().unwrap_or(0) + 1;
    v["attempts"] = attempts.into();
    let _ = std::fs::write(&path, v.to_string());
    if attempts as u32 > MAX_ATTEMPTS { Probation::GiveUp } else { Probation::OnProbation }
}

/// Is a freshly installed version still on probation?
pub fn on_probation(exe_dir: &Path) -> bool {
    exe_dir.join(MARKER).is_file()
}

/// The new version has proved itself: keep it.
pub fn confirm(exe_dir: &Path) {
    if std::fs::remove_file(exe_dir.join(MARKER)).is_ok() {
        log::info!("Update confirmed: {} is running fine", env!("CARGO_PKG_VERSION"));
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
        // A marker for some other version is just cleared.
        std::fs::write(dir.join(MARKER), r#"{"to":"9.9.9"}"#).unwrap();
        assert_eq!(start_probation(&dir), Probation::None);
        assert!(!on_probation(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
