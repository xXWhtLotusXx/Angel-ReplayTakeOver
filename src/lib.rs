//! Replay Takeover companion DLL for Arcana Heart 3 LMSS XTEND with Angel v9.
//!
//! Loaded by the game as `xinput1_3.dll` (see `proxy`). Verifies the game and Angel builds
//! (`version`), lets you take over one player's inputs during Replay Theater (`hook`,
//! `takeover`), adds takeover options to the game's own pause menu (`menu`, `text`), and
//! resets to the takeover moment using Angel's own state capture (`angel`). Settings come from
//! `replay_takeover.ini` (`config`); `monitor` logs replay state when `debug_log` is on.

mod angel;
mod config;
mod history;
mod game;
mod hook;
mod log;
mod menu;
mod monitor;
mod overlay;
mod proxy;
mod seek;
mod takeover;
mod text;
mod version;

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{BOOL, HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};

/// Set once the game and Angel builds are verified; later steps only act when this is true.
pub static ENABLED: AtomicBool = AtomicBool::new(false);

#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { DisableThreadLibraryCalls(module) };
        // Do the real work outside the loader lock.
        std::thread::spawn(initialize);
    }
    TRUE
}

fn initialize() {
    let Some(game_exe) = module_path(std::ptr::null_mut()) else { return };
    let game_dir = game_exe.parent().map(PathBuf::from).unwrap_or_default();
    log::init(&game_dir);
    log::write(&format!("Replay Takeover {} starting", env!("CARGO_PKG_VERSION")));

    let angel = unsafe { GetModuleHandleW(wide("dinput8.dll").as_ptr()) };
    let angel_path = if angel.is_null() { None } else { module_path(angel) };

    let report = version::check(&game_exe, angel_path);
    for line in &report.details {
        log::write(line);
    }

    if report.supported() {
        config::load(&game_dir);
        angel::set_module_base(angel as usize);
        ENABLED.store(true, Ordering::SeqCst);
        log::write("game and Angel v9 verified; Replay Takeover enabled");
        if config::get().debug_log {
            monitor::start();
        }
        match hook::install().and_then(|()| text::install()) {
            Ok(()) => takeover::start(),
            Err(e) => log::write(&format!("DISABLED: {e}")),
        }
    } else {
        let reason = match (report.game_ok, report.angel_ok) {
            (false, false) => "the game and Angel versions are not the ones it was built for",
            (false, true) => "the game version is not the one it was built for",
            _ => "Angel is missing or is not version 9",
        };
        log::write(&format!("DISABLED: {reason}"));
        let text = wide(&format!(
            "Replay Takeover is disabled because {reason}.\n\nThe game and Angel will run normally. See replay_takeover.log in the game folder for details."
        ));
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), wide("Replay Takeover").as_ptr(), MB_OK | MB_ICONWARNING) };
    }
}

fn module_path(module: HMODULE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (len > 0 && len < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
