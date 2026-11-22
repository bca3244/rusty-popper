//! Logging to `rusty-popper.log` next to the exe (and to the console when
//! there is one). The exe is a GUI-subsystem program, so on the cabinet the
//! file is the only record of what happened.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// The log is moved to `rusty-popper.old.log` once it passes this size.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

struct Logger {
    file: Mutex<Option<File>>,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {:5} {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), record.level(), record.args());
        // Write errors are ignored on purpose: with no console attached,
        // println! would panic, and logging must never take the menu down.
        if let Ok(mut f) = self.file.lock() {
            if let Some(f) = f.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        let console = format!("{}\n", record.args());
        if record.level() <= log::Level::Warn {
            let _ = std::io::stderr().write_all(console.as_bytes());
        } else {
            let _ = std::io::stdout().write_all(console.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            if let Some(f) = f.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// Starts logging to `path` (appending).
pub fn init(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("old.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    let logger: &'static Logger = Box::leak(Box::new(Logger { file: Mutex::new(file) }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
