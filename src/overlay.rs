//! On-screen text drawn on top of the battle HUD during Replay Theater.
//!
//! - The takeover countdown ("3", "2", "1") shown when a takeover starts (see `takeover`).
//! - A controls bar at the bottom of the screen for rewind / fast-forward / frame stepping.
//!   It follows the Replay Theater controls window: X hides both, and both are hidden during
//!   a takeover and while the pause menu is open.
//!
//! Drawn from a hook on the battle HUD draw (0x4D6BC0, reached every frame from the battle
//! draw task, not hooked by Angel) with the game's own text routine. Our strings use private
//! text ids that the text hook answers through `text_override`.

use crate::game::{self, runtime_address};
use crate::text::Text;
use crate::{config, takeover};

/// `thiscall DrawText(id, 0, style*)` on the text manager (0x500820, ret 0xC).
const FN_DRAW_TEXT: usize = 0x50_0820;
/// Pointer to the text manager singleton.
const TEXT_MANAGER: usize = 0xAA9_8440;

/// Style flags: font 6, centred (the menu rows use the same value).
const STYLE_CENTRED: u32 = 0x0101_0006;
const WHITE: u32 = 0xFFFF_FFFF;
const SHADOW: u32 = 0xFF00_0000;
const YELLOW: u32 = 0xFFFF_E040;

const SCREEN_CENTRE_X: i32 = 640;
const COUNTDOWN_Y: i32 = 300;
const COUNTDOWN_SCALE: f32 = 4.0;
const COUNTDOWN_LABEL_Y: i32 = 390;
const CONTROLS_BAR_Y: i32 = 688;
const CONTROLS_BAR_SCALE: f32 = 0.9;

/// Countdown digits: id TEXT_COUNTDOWN_DIGITS + n shows `COUNTDOWN_DIGITS[n]`.
const TEXT_COUNTDOWN_DIGITS: u32 = 0x7E40;
const COUNTDOWN_DIGITS: [&str; config::MAX_COUNTDOWN_SECONDS as usize + 1] =
    ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
const TEXT_COUNTDOWN_P1: u32 = 0x7E23;
const TEXT_COUNTDOWN_P2: u32 = 0x7E24;
const TEXT_CONTROLS_PLAYING: u32 = 0x7E30;
const TEXT_CONTROLS_STOPPED: u32 = 0x7E31;

/// Text hook: our private ids.
pub fn text_override(id: u32) -> Option<Text> {
    if let Some(digits) = id.checked_sub(TEXT_COUNTDOWN_DIGITS) {
        if let Some(text) = COUNTDOWN_DIGITS.get(digits as usize) {
            return Some(Text::Custom(text));
        }
    }
    Some(Text::Custom(match id {
        TEXT_COUNTDOWN_P1 => "Take Over - Player 1",
        TEXT_COUNTDOWN_P2 => "Take Over - Player 2",
        TEXT_CONTROLS_PLAYING => "LB / LT  Rewind        RB / RT  Fast-forward        A  Stop",
        TEXT_CONTROLS_STOPPED => "LB  Previous frame        RB  Next frame        A  Play",
        _ => return None,
    }))
}

unsafe fn draw_text(id: u32, x: i32, y: i32, scale: f32, colour: u32) {
    let text_manager = std::ptr::read_volatile(runtime_address(TEXT_MANAGER) as *const usize);
    if text_manager == 0 {
        return;
    }
    let draw: unsafe extern "thiscall" fn(usize, u32, u32, *const u32) =
        std::mem::transmute(runtime_address(FN_DRAW_TEXT));
    // Text style block, laid out as the game builds it (see `menu::draw_extra_rows`).
    let style: [u32; 12] = [
        STYLE_CENTRED,
        0,
        x as u32,
        y as u32,
        scale.to_bits(),
        scale.to_bits(),
        0,
        colour,
        SHADOW,
        3,
        0,
        0,
    ];
    draw(text_manager, id, 0, style.as_ptr());
}

/// Called after the game's battle HUD draw.
pub unsafe fn draw() {
    if !game::is_replay_theater_playback() {
        return;
    }
    if let Some((side, seconds_left)) = takeover::countdown() {
        let digit = TEXT_COUNTDOWN_DIGITS + seconds_left.min(config::MAX_COUNTDOWN_SECONDS);
        draw_text(digit, SCREEN_CENTRE_X, COUNTDOWN_Y, COUNTDOWN_SCALE, YELLOW);
        let label = if side == 0 { TEXT_COUNTDOWN_P1 } else { TEXT_COUNTDOWN_P2 };
        draw_text(label, SCREEN_CENTRE_X, COUNTDOWN_LABEL_Y, 1.0, WHITE);
        return;
    }
    let controls_shown = game::read_u8(game::CONTROLS_WINDOW_SHOWN) != 0;
    if takeover::in_takeover() || game::pause_menu_open() || !controls_shown {
        return;
    }
    let id = if game::playback_stopped() { TEXT_CONTROLS_STOPPED } else { TEXT_CONTROLS_PLAYING };
    draw_text(id, SCREEN_CENTRE_X, CONTROLS_BAR_Y, CONTROLS_BAR_SCALE, WHITE);
}
