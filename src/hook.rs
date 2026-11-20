//! Game hooks. None of these functions are hooked by Angel.
//!
//! - `GetBattleKey(player, button)` (0x5FFE70): while a takeover is live, the taken-over
//!   player's buttons come from the live controller; all other calls go to the original, so
//!   the opponent keeps reading the recorded replay inputs.
//! - Replay viewer input (0x49A890): opens/handles the pause menu and toggles Play/Stop with
//!   A. `menu` opens the extended takeover menu here and, during a takeover, keeps A from
//!   toggling Play/Stop.
//! - Replay viewer buttons (0x5982A0): Y toggles the HUD, X the info window. Skipped during
//!   a takeover, so those buttons are free for fighting.
//! - Pause menu draw (0x508900): `menu` adds the extended menu's extra rows.
//! - Scene update (0x476E10): `seek` runs fast-forward / rewind / frame steps before it.
//! - Battle HUD draw (0x4D6BC0): `overlay` draws the countdown and controls bar after it.

use std::sync::OnceLock;

use retour::RawDetour;

use crate::{game, log, menu, takeover};

type GetBattleKey = unsafe extern "C" fn(player: i32, button: i32) -> u8;
type VoidFn = unsafe extern "C" fn();
type DrawMenu = unsafe extern "thiscall" fn(menu: *mut u8);

const FN_PAUSE_MENU_DRAW: usize = 0x50_8900;
/// `CAHSceneAH30Game::Update` (thiscall, no arguments): the game's per-frame scene update.
const FN_SCENE_UPDATE: usize = 0x47_6E10;
/// Battle HUD draw (cdecl, no arguments), called every frame from the battle draw task.
const FN_BATTLE_HUD_DRAW: usize = 0x4D_6BC0;
static ORIGINAL_BATTLE_HUD_DRAW: OnceLock<VoidFn> = OnceLock::new();

type SceneUpdate = unsafe extern "thiscall" fn(scene: *mut u8) -> u32;
static ORIGINAL_SCENE_UPDATE: OnceLock<SceneUpdate> = OnceLock::new();

static ORIGINAL_GET_BATTLE_KEY: OnceLock<GetBattleKey> = OnceLock::new();
static ORIGINAL_VIEWER_INPUT: OnceLock<VoidFn> = OnceLock::new();
static ORIGINAL_VIEWER_BUTTONS: OnceLock<VoidFn> = OnceLock::new();
static ORIGINAL_PAUSE_MENU_DRAW: OnceLock<DrawMenu> = OnceLock::new();

/// Detours `game_va` to `detour` and returns the trampoline to the original function.
/// Installed once at startup, from the title screen, before any battle code runs.
unsafe fn install_one(name: &str, game_va: usize, detour: *const ()) -> Result<*const (), String> {
    let target = game::runtime_address(game_va) as *const ();
    let hook = RawDetour::new(target, detour).map_err(|e| format!("could not hook {name}: {e}"))?;
    let trampoline = hook.trampoline() as *const ();
    hook.enable().map_err(|e| format!("could not enable {name} hook: {e}"))?;
    // Hooks live for the rest of the process.
    std::mem::forget(hook);
    log::write(&format!("hook: {name} ({:#x}) hooked", target as usize));
    Ok(trampoline)
}

pub fn install() -> Result<(), String> {
    unsafe {
        let t = install_one("GetBattleKey", game::FN_GET_BATTLE_KEY, get_battle_key_detour as *const ())?;
        let _ = ORIGINAL_GET_BATTLE_KEY.set(std::mem::transmute(t));
        let t = install_one("ReplayViewerInput", game::FN_REPLAY_VIEWER_INPUT, viewer_input_detour as *const ())?;
        let _ = ORIGINAL_VIEWER_INPUT.set(std::mem::transmute(t));
        let t = install_one("ReplayViewerButtons", game::FN_REPLAY_VIEWER_BUTTONS, viewer_buttons_detour as *const ())?;
        let _ = ORIGINAL_VIEWER_BUTTONS.set(std::mem::transmute(t));
        let t = install_one("PauseMenuDraw", FN_PAUSE_MENU_DRAW, pause_menu_draw_detour as *const ())?;
        let _ = ORIGINAL_PAUSE_MENU_DRAW.set(std::mem::transmute(t));
        let t = install_one("SceneUpdate", FN_SCENE_UPDATE, scene_update_detour as *const ())?;
        let _ = ORIGINAL_SCENE_UPDATE.set(std::mem::transmute(t));
        let t = install_one("BattleHudDraw", FN_BATTLE_HUD_DRAW, battle_hud_draw_detour as *const ())?;
        let _ = ORIGINAL_BATTLE_HUD_DRAW.set(std::mem::transmute(t));
    }
    Ok(())
}

unsafe extern "thiscall" fn scene_update_detour(scene: *mut u8) -> u32 {
    crate::seek::before_scene_update(scene);
    match ORIGINAL_SCENE_UPDATE.get() {
        Some(original) => original(scene),
        None => 0,
    }
}

unsafe extern "C" fn battle_hud_draw_detour() {
    if let Some(original) = ORIGINAL_BATTLE_HUD_DRAW.get() {
        original();
    }
    crate::overlay::draw();
}

unsafe extern "thiscall" fn pause_menu_draw_detour(this: *mut u8) {
    if let Some(original) = ORIGINAL_PAUSE_MENU_DRAW.get() {
        original(this);
    }
    menu::draw_extra_rows(this);
}

unsafe extern "C" fn get_battle_key_detour(player: i32, button: i32) -> u8 {
    if let Some(word) = takeover::live_input_word_for(player) {
        return match button {
            0..=8 => ((word >> button) & 1) as u8,
            _ => 0,
        };
    }
    match ORIGINAL_GET_BATTLE_KEY.get() {
        Some(original) => original(player, button),
        None => 0,
    }
}

unsafe extern "C" fn viewer_input_detour() {
    if menu::on_viewer_input() {
        return;
    }
    if let Some(original) = ORIGINAL_VIEWER_INPUT.get() {
        original();
    }
}

unsafe extern "C" fn viewer_buttons_detour() {
    if takeover::in_takeover() {
        return;
    }
    if let Some(original) = ORIGINAL_VIEWER_BUTTONS.get() {
        original();
    }
}
