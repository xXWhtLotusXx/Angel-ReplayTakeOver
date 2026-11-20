//! Text overrides for the extended Replay Theater pause menu and our on-screen overlay.
//!
//! The game looks text up by id through two functions on its text database (neither is
//! hooked by Angel): `u32 Length(id, sub)` (0x5F7560) and `void Copy(id, sub, buffer, max)`
//! (0x5F7630, writes UTF-16 plus a terminating zero). While the extended pause menu is open,
//! some ids are answered with our own strings or redirected to other game strings; every other
//! lookup goes to the original unchanged.

use std::sync::OnceLock;

use retour::RawDetour;

use crate::{game, log, menu, overlay};

const FN_TEXT_LENGTH: usize = 0x5F_7560;
const FN_TEXT_COPY: usize = 0x5F_7630;

type TextLength = unsafe extern "thiscall" fn(db: *mut u8, id: u32, sub: u32) -> u32;
/// Returns non-zero when the text was copied (callers such as 0x50AEF0 test it).
type TextCopy = unsafe extern "thiscall" fn(db: *mut u8, id: u32, sub: u32, buffer: *mut u16, max: u32) -> u32;

static ORIGINAL_LENGTH: OnceLock<TextLength> = OnceLock::new();
static ORIGINAL_COPY: OnceLock<TextCopy> = OnceLock::new();

/// What an id resolves to while the extended menu is open.
pub enum Text {
    Custom(&'static str),
    /// Another game text id (used to move the original rows' labels to new positions).
    Game(u32),
}

pub fn install() -> Result<(), String> {
    unsafe {
        let length = hook("TextLength", FN_TEXT_LENGTH, text_length_detour as *const ())?;
        let _ = ORIGINAL_LENGTH.set(std::mem::transmute(length));
        let copy = hook("TextCopy", FN_TEXT_COPY, text_copy_detour as *const ())?;
        let _ = ORIGINAL_COPY.set(std::mem::transmute(copy));
    }
    Ok(())
}

unsafe fn hook(name: &str, game_va: usize, detour: *const ()) -> Result<*const (), String> {
    let target = game::runtime_address(game_va) as *const ();
    let hook = RawDetour::new(target, detour).map_err(|e| format!("could not hook {name}: {e}"))?;
    let trampoline = hook.trampoline() as *const ();
    hook.enable().map_err(|e| format!("could not enable {name} hook: {e}"))?;
    std::mem::forget(hook);
    log::write(&format!("hook: {name} ({:#x}) hooked", target as usize));
    Ok(trampoline)
}

fn override_for(id: u32) -> Option<Text> {
    menu::text_override(id).or_else(|| overlay::text_override(id))
}

unsafe extern "thiscall" fn text_length_detour(db: *mut u8, id: u32, sub: u32) -> u32 {
    let original = ORIGINAL_LENGTH.get().copied();
    match override_for(id) {
        Some(Text::Custom(s)) => s.encode_utf16().count() as u32,
        Some(Text::Game(other)) => original.map_or(0, |f| f(db, other, sub)),
        None => original.map_or(0, |f| f(db, id, sub)),
    }
}

unsafe extern "thiscall" fn text_copy_detour(db: *mut u8, id: u32, sub: u32, buffer: *mut u16, max: u32) -> u32 {
    let original = ORIGINAL_COPY.get().copied();
    match override_for(id) {
        Some(Text::Custom(s)) => {
            // Same contract as the game: copy only if it fits including the terminator.
            let units: Vec<u16> = s.encode_utf16().collect();
            if buffer.is_null() || units.len() as u32 >= max {
                return 0;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), buffer, units.len());
            *buffer.add(units.len()) = 0;
            1
        }
        Some(Text::Game(other)) => original.map_or(0, |f| f(db, other, sub, buffer, max)),
        None => original.map_or(0, |f| f(db, id, sub, buffer, max)),
    }
}
