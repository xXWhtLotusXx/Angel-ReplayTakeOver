//! Read-only access to AALib.exe globals. All addresses are the game's preferred virtual
//! addresses (image base 0x400000) and are rebased to the actual load address at runtime.
//! Only valid for the game build verified in `version`.

use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const PREFERRED_BASE: usize = 0x40_0000;

/// Scene id. 8 / 14 are online scenes (see `is_online`).
/// UNVERIFIED: read 0 throughout menus and Replay Theater in testing.
pub const SCENE: usize = 0xE3D_C9A4;
/// Set while an online scene is actually connected.
pub const ONLINE_FLAG: usize = 0xE3D_D3D8;
/// Game mode; 6 = replay playback (checked by the game at 0x501B00), 7 = another mode checked at 0x501D10.
pub const GAME_MODE: usize = 0xE12_3514;
/// Per-player input source type (structs are 0x278 bytes apart); 6 = replay, 4 = network.
pub const P1_INPUT_SOURCE: usize = 0xE12_37D8;
pub const P2_INPUT_SOURCE: usize = 0xE12_3A50;
/// Replay object holding per-frame inputs; `REPLAY_INPUTS` = object + 0x3E0.
pub const REPLAY_INPUTS: usize = 0xDEF_F7F8;
/// Current replay frame, and the round/slot index used to pick the input row.
pub const REPLAY_CURSOR: usize = 0xE0B_6CD8;
pub const REPLAY_SLOT: usize = 0xE0B_6CDC;
/// Set when battle-key queries read from the replay instead of the controllers (0x5FFEA6).
pub const REPLAY_READ_FLAG: usize = 0xE3D_ABF4;
/// Round-state candidates found by diffing memory across a round transition in Replay
/// Theater (round 1 fighting / KO / round 2 fighting). Still being confirmed via the log.
/// Battle phase (two copies): 3 while fighting, 5 during the KO sequence.
pub const BATTLE_PHASE_A: usize = 0xE0B_9AA4;
pub const BATTLE_PHASE_B: usize = 0xE0B_9AA8;
/// 0 while fighting, 1 during the KO sequence.
pub const ROUND_OVER_FLAG: usize = 0xE0B_99FC;
/// 0-based round index; incremented when the next round starts.
pub const ROUND_INDEX: usize = 0xE0B_9A9C;
/// Rounds won so far, per side (which address is which side is unconfirmed).
pub const WINS_A: usize = 0xE0B_9A84;
pub const WINS_B: usize = 0xE0B_9AD0;

/// Game function: `bool __cdecl GetBattleKey(int player, int button)`. Answers "is `player`
/// pressing battle button `button` (0-8, same bit order as the replay word) this frame"; in
/// replay playback it reads the replay buffer. Not hooked by Angel.
pub const FN_GET_BATTLE_KEY: usize = 0x5F_FE70;
/// Game function: `u16 __cdecl BuildInputWord(int player)`. Builds the 9-bit replay input word
/// from `player`'s live controller using their key config (what the game records into replays).
pub const FN_BUILD_INPUT_WORD: usize = 0x5F_F500;

/// Game function: Replay Theater viewer input (pause menu open/confirm/close; A = Play/Stop).
pub const FN_REPLAY_VIEWER_INPUT: usize = 0x49_A890;
/// Game function: Replay Theater viewer buttons (Y = HUD on/off, X = info window on/off).
pub const FN_REPLAY_VIEWER_BUTTONS: usize = 0x59_82A0;
/// Game function: `bool __cdecl MenuKeyPressed(int player, int key, bool trigger)`.
pub const FN_MENU_KEY_PRESSED: usize = 0x5F_F4A0;
/// Non-zero while the Replay Theater pause menu is open.
pub const PAUSE_MENU_OPEN: usize = 0xDEF_E724;
/// u32: Replay Theater Play/Stop state toggled by A in the viewer (0x49A9C3); non-zero =
/// stopped. While it (or the pause menu flag) is set, the battle simulation is skipped: the
/// scene update gates on 0x49A740 and battle code asks 0x4A6C40 ("paused?").
pub const PLAYBACK_STOPPED: usize = 0xDEF_E7D0;
/// u8: Replay Theater controls window (bottom right: Play/Stop, HUD, Window, Pause Menu);
/// 1 = shown, 0 = hidden. Toggled by X in the viewer button handler (0x5982A0).
pub const CONTROLS_WINDOW_SHOWN: usize = 0x286_F167;

pub const ANY_PLAYER: i32 = -1;
/// Menu key ids: 0x17 opens the pause menu, 0x19 confirm (A), 0x1A cancel (B).
pub const MENU_KEY_PAUSE_MENU: i32 = 0x17;
pub const MENU_KEY_CONFIRM: i32 = 0x19;

pub const PHASE_FIGHTING: u32 = 3;

pub const MAX_REPLAY_FRAMES: u32 = 360_000;
const MODE_REPLAY: u32 = 6;
const MODE_REPLAY_THEATER: u32 = 7;
const INPUT_SOURCE_REPLAY: u32 = 6;

fn load_base() -> usize {
    static BASE: OnceLock<usize> = OnceLock::new();
    *BASE.get_or_init(|| unsafe { GetModuleHandleW(std::ptr::null()) } as usize)
}

/// Runtime address of a game function or global given its preferred virtual address.
pub fn runtime_address(preferred_va: usize) -> usize {
    load_base() + (preferred_va - PREFERRED_BASE)
}

pub fn battle_phase() -> u32 {
    read_u32(BATTLE_PHASE_A)
}

pub fn read_u8(preferred_va: usize) -> u8 {
    unsafe { std::ptr::read_volatile(runtime_address(preferred_va) as *const u8) }
}

/// Writes a u8 global. Only used on the game thread at a frame boundary.
pub fn write_u8(preferred_va: usize, value: u8) {
    unsafe { std::ptr::write_volatile(runtime_address(preferred_va) as *mut u8, value) }
}

pub fn pause_menu_open() -> bool {
    read_u32(PAUSE_MENU_OPEN) != 0
}

/// True while the viewer's Play/Stop is on Stop (independent of the pause menu).
pub fn playback_stopped() -> bool {
    read_u32(PLAYBACK_STOPPED) != 0
}

pub fn set_playback_stopped(stopped: bool) {
    write_u32(PLAYBACK_STOPPED, stopped as u32)
}

/// True on the frame `key` was pressed (trigger) by `player` (or any player with -1).
/// Call on the game thread.
pub fn menu_key_pressed(player: i32, key: i32) -> bool {
    let pressed: unsafe extern "C" fn(i32, i32, u8) -> u8 =
        unsafe { std::mem::transmute(runtime_address(FN_MENU_KEY_PRESSED)) };
    unsafe { pressed(player, key, 1) != 0 }
}

/// Calls the game's own controller-to-replay-word conversion for `player`'s controller.
/// Must be called on the game thread (it reads the key controllers' current-frame state).
pub fn build_live_input_word(player: i32) -> u16 {
    let build: unsafe extern "C" fn(i32) -> u32 =
        unsafe { std::mem::transmute(runtime_address(FN_BUILD_INPUT_WORD)) };
    (unsafe { build(player) } & 0x1FF) as u16
}

/// Reads a u32 global. These globals live in the game's static data, which is mapped for the
/// whole process lifetime, so reading them never faults.
pub fn read_u32(preferred_va: usize) -> u32 {
    unsafe { std::ptr::read_volatile(runtime_address(preferred_va) as *const u32) }
}

/// Writes a u32 global. Only used on the game thread at a frame boundary.
pub fn write_u32(preferred_va: usize, value: u32) {
    unsafe { std::ptr::write_volatile(runtime_address(preferred_va) as *mut u32, value) }
}

fn read_u16(preferred_va: usize) -> u16 {
    unsafe { std::ptr::read_volatile(runtime_address(preferred_va) as *const u16) }
}

pub fn is_online() -> bool {
    matches!(read_u32(SCENE), 8 | 14) && read_u32(ONLINE_FLAG) != 0
}

/// True while battle inputs are being read from the replay buffer.
///
/// The game's reader (0x49A5A0) accepts two modes: mode 6 with a replay input source
/// (0x501B00, used for Angel's battles) and mode 7 (0x501D10, observed in Replay Theater).
/// The deciding signal is the game's own "read inputs from replay" flag checked at 0x5FFEA6.
pub fn is_replay_playback() -> bool {
    if read_u32(REPLAY_READ_FLAG) == 0 {
        return false;
    }
    match read_u32(GAME_MODE) {
        MODE_REPLAY_THEATER => true,
        MODE_REPLAY => {
            !is_online()
                && (read_u32(P1_INPUT_SOURCE) == INPUT_SOURCE_REPLAY
                    || read_u32(P2_INPUT_SOURCE) == INPUT_SOURCE_REPLAY)
        }
        _ => false,
    }
}

/// True only for Replay Theater playback (mode 7), which is where takeover is offered.
pub fn is_replay_theater_playback() -> bool {
    read_u32(REPLAY_READ_FLAG) != 0 && read_u32(GAME_MODE) == MODE_REPLAY_THEATER
}

/// Recorded input word for `player` (0 or 1) at `frame` of the current replay slot,
/// using the same indexing as the game's reader at 0x49A5A0.
pub fn replay_input(player: u32, frame: u32) -> Option<u16> {
    if player > 1 || frame >= MAX_REPLAY_FRAMES {
        return None;
    }
    let slot = read_u32(REPLAY_SLOT);
    if slot > 1 {
        return None;
    }
    let index = ((player + slot * 2) * MAX_REPLAY_FRAMES + frame) as usize;
    Some(read_u16(REPLAY_INPUTS + index * 2))
}

/// Human-readable form of a replay input word: bits 0-3 Up/Down/Left/Right,
/// bits 4-8 the five attack buttons (labelled A-E; labels inferred from slot order).
pub fn describe_input(word: u16) -> String {
    const DIRECTIONS: [&str; 4] = ["Up", "Down", "Left", "Right"];
    const BUTTONS: [&str; 5] = ["A", "B", "C", "D", "E"];
    let mut parts: Vec<&str> = Vec::new();
    for (bit, name) in DIRECTIONS.iter().enumerate() {
        if word & (1 << bit) != 0 {
            parts.push(name);
        }
    }
    for (i, name) in BUTTONS.iter().enumerate() {
        if word & (1 << (4 + i)) != 0 {
            parts.push(name);
        }
    }
    let mut text = if parts.is_empty() { "neutral".to_owned() } else { parts.join("+") };
    if word & 0xFE00 != 0 {
        text.push_str(&format!(" (+other bits {:#06x})", word & 0xFE00));
    }
    text
}
