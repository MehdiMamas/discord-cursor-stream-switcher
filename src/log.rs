//! A small file logger. The log stays on this PC; it is capped at 1 MB with one old copy.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static SINK: Mutex<Option<File>> = Mutex::new(None);
const MAX_BYTES: u64 = 1024 * 1024;

pub fn init(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("old.log"));
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = file;
}

pub fn write(level: &str, args: std::fmt::Arguments) {
    // SAFETY: GetLocalTime only fills in the returned struct.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {level:5} {args}\n",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    );
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
    if let Some(f) = SINK.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::log::write("INFO", format_args!($($t)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::log::write("WARN", format_args!($($t)*)) };
}

#[macro_export]
macro_rules! error {
    ($($t:tt)*) => { $crate::log::write("ERROR", format_args!($($t)*)) };
}
