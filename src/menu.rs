//! Extended Replay Theater pause menu (SF6-style takeover options).
//!
//! The game's pause menu (menu type 1 on the battle UI object at 0xE1BB180) has two rows,
//! "Replay Theater" and "Close". During a Replay Theater fight, or while a takeover is active,
//! we open it with four rows instead:
//!
//! | row | not in takeover | in takeover    |
//! |-----|-----------------|----------------|
//! | 0   | Take Over P1    | Reset Battle   |
//! | 1   | Take Over P2    | End Takeover   |
//! | 2   | Replay Theater  | Replay Theater |
//! | 3   | Close           | Close          |
//!
//! The game still draws rows 0/1, the cursor bar and the help line; while the extended menu is
//! open the text hook relabels them (`text_override`) and we draw rows 2/3 with the game's own
//! text routine. Confirming rows 2/3 shifts the cursor back to 0/1 and lets the original
//! handler run, so "Replay Theater" and "Close" behave exactly as before.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::game::{self, runtime_address};
use crate::takeover::{self, Request};
use crate::text::Text;

/// Battle UI object hosting the pause menu: +0x06 centre x, +0x08 centre y (i16),
/// +0x0E cursor row (i16), +0x38 open-animation counter (i32).
const MENU_OBJECT: usize = 0xE1B_B180;
const MENU_TYPE_PAUSE: i32 = 1;
/// Row count per menu type (i16 table at 0xE12D8C4); type 1's entry. The game sets it to 2.
const ROW_COUNT_TYPE_PAUSE: usize = 0xE12_D8C6;
const ORIGINAL_ROWS: i16 = 2;
const EXTENDED_ROWS: i16 = 4;
/// Set (u8) once "Replay Theater" was chosen and the game is leaving playback.
const LEAVING_TO_THEATER: usize = 0xE12_4073;

/// `thiscall OpenMenu(type, width, height, centre_x, centre_y, arg)` (0x503A80, ret 0x18).
const FN_OPEN_MENU: usize = 0x50_3A80;
/// `thiscall CloseMenu()` (0x505B80).
const FN_CLOSE_MENU: usize = 0x50_5B80;
/// `cdecl PlaySound(id, 0)`: 0x5F0800 is used for the open sound, 0x5F0230 for close.
const FN_SOUND_OPEN: usize = 0x5F_0800;
const FN_SOUND_CLOSE: usize = 0x5F_0230;
const SOUND_MENU_OPEN: u32 = 0x1280;
const SOUND_MENU_CLOSE: u32 = 0x1279;
/// `thiscall DrawText(id, 0, style*)` on the text manager (0x500820, ret 0xC).
const FN_DRAW_TEXT: usize = 0x50_0820;
/// Pointer to the text manager singleton.
const TEXT_MANAGER: usize = 0xAA9_8440;
/// Menu open-animation length (i16) and the scale turning progress into alpha (f64).
const ANIMATION_LENGTH: usize = 0x520_BA90;
const ALPHA_SCALE: usize = 0x6DB_E48;

/// Original geometry: OpenMenu(1, 500, 200 (raised to the 256 minimum), 640, 330, 0).
/// Rows are 0x28 apart starting 0x14 above the centre; two more rows need 80 px more height,
/// and moving the centre up 40 px keeps the list centred.
const MENU_WIDTH: i32 = 500;
const MENU_HEIGHT: i32 = 256 + 80;
const MENU_CENTRE_X: i32 = 640;
const MENU_CENTRE_Y: i32 = 330 - 40;
const ROW_SPACING: i32 = 0x28;
const FIRST_ROW_OFFSET: i32 = -0x14;

/// Game text ids used by the original menu for its row labels.
const TEXT_ROW0_LABEL: u32 = 0x2866; // "Replay Theater"
const TEXT_ROW1_LABEL: u32 = 0x2867; // "Close"
/// The draw code sets the help line to 0x286C + cursor, but the help bar remaps that id
/// before looking it up (row 2 was observed requesting 0x286F), so help lines are answered
/// by the current cursor row for any id in this range. The button legend (0x2868-0x286B) is
/// outside it.
const TEXT_HELP_IDS: std::ops::RangeInclusive<u32> = 0x286C..=0x2870;
/// Private ids for the labels we draw on rows 2/3 (redirected to the original labels).
const TEXT_ROW2_LABEL: u32 = 0x7E10;
const TEXT_ROW3_LABEL: u32 = 0x7E11;

/// True while the open pause menu is our extended version.
static EXTENDED: AtomicBool = AtomicBool::new(false);

fn object() -> *mut u8 {
    runtime_address(MENU_OBJECT) as *mut u8
}

fn cursor() -> i16 {
    unsafe { std::ptr::read_volatile(object().add(0xE) as *const i16) }
}

fn set_cursor(row: i16) {
    unsafe { std::ptr::write_volatile(object().add(0xE) as *mut i16, row) }
}

fn set_row_count(rows: i16) {
    unsafe { std::ptr::write_volatile(runtime_address(ROW_COUNT_TYPE_PAUSE) as *mut i16, rows) }
}

pub fn is_extended() -> bool {
    EXTENDED.load(Ordering::Acquire) && game::pause_menu_open()
}

/// The extended menu is offered during a Replay Theater fight, or any time a takeover is active.
fn extended_menu_applies() -> bool {
    game::is_replay_theater_playback()
        && (takeover::in_takeover() || game::battle_phase() == game::PHASE_FIGHTING)
}

/// Replay viewer input hook body. Returns true when it fully handled the call (the original
/// must not run).
pub fn on_viewer_input() -> bool {
    if unsafe { std::ptr::read_volatile(runtime_address(LEAVING_TO_THEATER) as *const u8) } != 0 {
        return false;
    }
    if !game::pause_menu_open() {
        if EXTENDED.swap(false, Ordering::AcqRel) {
            // Closed by the game (B / menu button / "Close"): put the original row count back.
            set_row_count(ORIGINAL_ROWS);
        }
        if game::menu_key_pressed(game::ANY_PLAYER, game::MENU_KEY_PAUSE_MENU) {
            if extended_menu_applies() {
                open_extended();
                return true;
            }
            return false;
        }
        // During a takeover, A must not toggle Play/Stop.
        return takeover::in_takeover();
    }
    if !EXTENDED.load(Ordering::Acquire) || !game::menu_key_pressed(game::ANY_PLAYER, game::MENU_KEY_CONFIRM) {
        return false;
    }
    match cursor() {
        row @ (0 | 1) => {
            close_extended();
            let taking_over = takeover::in_takeover();
            takeover::queue(match (row, taking_over) {
                (0, false) => Request::TakeOver(0),
                (1, false) => Request::TakeOver(1),
                (0, true) => Request::Reset,
                _ => Request::End,
            });
            true
        }
        row => {
            // "Replay Theater" / "Close": the original handler expects them on rows 0 / 1.
            set_cursor(row - 2);
            false
        }
    }
}

fn open_extended() {
    set_row_count(EXTENDED_ROWS);
    unsafe {
        let open: unsafe extern "thiscall" fn(*mut u8, i32, i32, i32, i32, i32, i32) =
            std::mem::transmute(runtime_address(FN_OPEN_MENU));
        open(object(), MENU_TYPE_PAUSE, MENU_WIDTH, MENU_HEIGHT, MENU_CENTRE_X, MENU_CENTRE_Y, 0);
        let sound: unsafe extern "C" fn(u32, u32) = std::mem::transmute(runtime_address(FN_SOUND_OPEN));
        sound(SOUND_MENU_OPEN, 0);
        std::ptr::write_volatile(runtime_address(game::PAUSE_MENU_OPEN) as *mut u32, 1);
    }
    EXTENDED.store(true, Ordering::Release);
}

/// Same sequence the game uses to close the pause menu (0x49A890).
fn close_extended() {
    unsafe {
        let close: unsafe extern "thiscall" fn(*mut u8) = std::mem::transmute(runtime_address(FN_CLOSE_MENU));
        close(object());
        let sound: unsafe extern "C" fn(u32, u32) = std::mem::transmute(runtime_address(FN_SOUND_CLOSE));
        sound(SOUND_MENU_CLOSE, 0);
        std::ptr::write_volatile(runtime_address(game::PAUSE_MENU_OPEN) as *mut u32, 0);
    }
    set_row_count(ORIGINAL_ROWS);
    EXTENDED.store(false, Ordering::Release);
}

/// Text hook: labels and help lines of the extended menu.
pub fn text_override(id: u32) -> Option<Text> {
    if !is_extended() {
        return None;
    }
    let taking_over = takeover::in_takeover();
    Some(match id {
        TEXT_ROW0_LABEL => Text::Custom(if taking_over { "Reset Battle" } else { "Take Over P1" }),
        TEXT_ROW1_LABEL => Text::Custom(if taking_over { "End Takeover" } else { "Take Over P2" }),
        TEXT_ROW2_LABEL => Text::Game(TEXT_ROW0_LABEL),
        TEXT_ROW3_LABEL => Text::Game(TEXT_ROW1_LABEL),
        id if TEXT_HELP_IDS.contains(&id) => Text::Custom(help_line(cursor(), taking_over)),
        _ => return None,
    })
}

/// Help line for the highlighted row (the original rows' lines are the game's own wording).
fn help_line(row: i16, taking_over: bool) -> &'static str {
    match (row, taking_over) {
        (0, false) => "Control Player 1 from this moment.",
        (1, false) => "Control Player 2 from this moment.",
        (0, true) => "Return to the moment you took over. Select also resets.",
        (1, true) => "Stop controlling and keep watching the replay.",
        (2, _) => "Return to the Replay Theater.",
        _ => "Close the menu.",
    }
}

/// Called after the game's pause-menu draw (0x508900) with its `this`: draws rows 2/3 the
/// same way the original draws rows 0/1.
pub unsafe fn draw_extra_rows(menu: *const u8) {
    if !is_extended() {
        return;
    }
    let counter = std::ptr::read_volatile(menu.add(0x38) as *const i32);
    if counter <= 0xF {
        return; // the original waits for the open animation in the same way
    }
    let text_manager = std::ptr::read_volatile(runtime_address(TEXT_MANAGER) as *const usize);
    if text_manager == 0 {
        return;
    }
    let length = std::ptr::read_volatile(runtime_address(ANIMATION_LENGTH) as *const i16) as i32;
    let scale = std::ptr::read_volatile(runtime_address(ALPHA_SCALE) as *const f64);
    let alpha: u32 = if counter < length && length > 0 {
        ((counter as f64 / length as f64) * scale).round() as u8 as u32
    } else {
        0xFF
    };
    let centre_x = std::ptr::read_volatile(menu.add(6) as *const i16) as i32;
    let centre_y = std::ptr::read_volatile(menu.add(8) as *const i16) as i32;
    let selected = std::ptr::read_volatile(menu.add(0xE) as *const i16) as i32;
    let draw: unsafe extern "thiscall" fn(usize, u32, u32, *const u32) =
        std::mem::transmute(runtime_address(FN_DRAW_TEXT));
    for (row, id) in [(2, TEXT_ROW2_LABEL), (3, TEXT_ROW3_LABEL)] {
        // Highlighted row: the original computes (alpha<<24 | 0xFFFFFF) * 0x1000000 + 0xFFFF.
        let colour = if selected == row {
            ((alpha << 24) | 0x00FF_FFFF).wrapping_mul(0x0100_0000).wrapping_add(0xFFFF)
        } else {
            0xFFFF_FFFF
        };
        // Text style block, laid out exactly as the original builds it on its stack.
        let style: [u32; 12] = [
            0x0101_0006, // font 6, flags
            0,
            centre_x as u32,
            (centre_y + FIRST_ROW_OFFSET + row * ROW_SPACING) as u32,
            1.0f32.to_bits(), // scale x
            1.0f32.to_bits(), // scale y
            0,
            colour,
            alpha << 24, // shadow colour
            3,
            0,
            0,
        ];
        draw(text_manager, id, 0, style.as_ptr());
    }
}
