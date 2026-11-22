//! Finding PinUP Popper's folder (the one holding PUPDatabase.db).
//!
//! Order: `--popper`, then `[paths] popper_dir` in rusty-popper.toml, then
//! the default C:\vPinball\PinUPSystem, then where Windows says PinUpPlayer
//! is registered (Popper's installer registers it as a COM server), then the
//! usual install folders on every drive.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub const DEFAULT_POPPER_DIR: &str = r"C:\vPinball\PinUPSystem";

/// Where Popper was found, and whether it took a search to find it (worth
/// saving to rusty-popper.toml so the next start doesn't search again).
pub struct Found {
    pub dir: PathBuf,
    pub searched: bool,
}

fn has_db(dir: &Path) -> bool {
    dir.join("PUPDatabase.db").is_file()
}

pub fn find_popper(explicit: Option<PathBuf>, configured: Option<PathBuf>) -> Result<Found> {
    if let Some(dir) = explicit {
        if has_db(&dir) {
            return Ok(Found { dir, searched: false });
        }
        bail!("no PUPDatabase.db in {} (from --popper)", dir.display());
    }
    if let Some(dir) = &configured {
        if has_db(dir) {
            return Ok(Found { dir: dir.clone(), searched: false });
        }
        log::warn!("popper_dir {} in rusty-popper.toml has no PUPDatabase.db; looking elsewhere", dir.display());
    }
    let default = PathBuf::from(DEFAULT_POPPER_DIR);
    if has_db(&default) {
        // Only worth saving if the toml pointed somewhere wrong.
        return Ok(Found { dir: default, searched: configured.is_some() });
    }
    let mut checked: Vec<PathBuf> = configured.into_iter().chain([default]).collect();
    if let Some(dir) = registered_popper_dir() {
        if has_db(&dir) {
            log::info!("Found Popper through its PinUpPlayer registration: {}", dir.display());
            return Ok(Found { dir, searched: true });
        }
        checked.push(dir);
    }
    for dir in drive_candidates() {
        if has_db(&dir) {
            log::info!("Found Popper at {}", dir.display());
            return Ok(Found { dir, searched: true });
        }
        checked.push(dir);
    }
    let shown: Vec<String> = checked.iter().take(8).map(|p| p.display().to_string()).collect();
    bail!(
        "couldn't find PinUP Popper (no PUPDatabase.db in {}{}). Set popper_dir under [paths] in rusty-popper.toml to Popper's PinUPSystem folder",
        shown.join(", "),
        if checked.len() > shown.len() { ", ..." } else { "" }
    )
}

/// The folder of PinUpPlayer.exe as registered with Windows, if any.
fn registered_popper_dir() -> Option<PathBuf> {
    let clsid = reg_string(r"PinUpPlayer.PinDisplay\Clsid")?;
    let server = [format!(r"WOW6432Node\CLSID\{clsid}\LocalServer32"), format!(r"CLSID\{clsid}\LocalServer32")]
        .iter()
        .find_map(|k| reg_string(k))?;
    // The value can be quoted and can carry arguments: keep just the exe path.
    let exe = if let Some(rest) = server.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest).to_string()
    } else {
        let lower = server.to_ascii_lowercase();
        match lower.find(".exe") {
            Some(i) => server[..i + 4].to_string(),
            None => server.clone(),
        }
    };
    Path::new(exe.trim()).parent().map(Path::to_path_buf)
}

/// Default value of a key under HKEY_CLASSES_ROOT.
fn reg_string(subkey: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ};
    let key: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(HKEY_CLASSES_ROOT, key.as_ptr(), std::ptr::null(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if rc != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string())
}

/// The usual PinUPSystem locations on every drive, plus any top-level folder
/// that has a PinUPSystem inside it.
fn drive_candidates() -> Vec<PathBuf> {
    const USUAL: &[&str] = &[
        r"vPinball\PinUPSystem",
        r"PinUPSystem",
        r"Pinball\PinUPSystem",
        r"Visual Pinball\PinUPSystem",
        r"PinUP\PinUPSystem",
        r"Games\PinUPSystem",
        r"Games\vPinball\PinUPSystem",
    ];
    let mut out = Vec::new();
    for letter in b'C'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", letter as char));
        if !root.is_dir() {
            continue;
        }
        for rel in USUAL {
            out.push(root.join(rel));
        }
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten().take(400) {
                let p = e.path().join("PinUPSystem");
                if e.path().is_dir() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a machine with Popper installed this prints its folder; elsewhere None.
    #[test]
    fn registry_lookup_does_not_panic() {
        println!("registered Popper dir: {:?}", registered_popper_dir());
    }

    #[test]
    fn bad_explicit_path_is_an_error() {
        let e = find_popper(Some(PathBuf::from(r"Z:\no\such\dir")), None).err().unwrap();
        assert!(e.to_string().contains("--popper"));
    }

    #[test]
    fn drive_candidates_include_the_default_layout() {
        assert!(drive_candidates().iter().any(|p| p.ends_with(r"vPinball\PinUPSystem")));
    }
}