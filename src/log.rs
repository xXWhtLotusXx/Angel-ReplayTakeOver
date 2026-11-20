//! Minimal append-only log file next to the game executable (`replay_takeover.log`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use windows_sys::Win32::System::SystemInformation::GetLocalTime;

static LOG: Mutex<Option<File>> = Mutex::new(None);

pub fn init(game_dir: &std::path::Path) {
    let path: PathBuf = game_dir.join("replay_takeover.log");
    // Start a fresh log each launch so it only describes the current session.
    if let Ok(file) = OpenOptions::new().create(true).write(true).truncate(true).open(path) {
        *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    }
}

pub fn write(message: &str) {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = guard.as_mut() {
        let t = unsafe {
            let mut t = std::mem::zeroed();
            GetLocalTime(&mut t);
            t
        };
        let _ = writeln!(
            file,
            "[{:02}:{:02}:{:02}.{:03}] {message}",
            t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        );
        let _ = file.flush();
    }
}
