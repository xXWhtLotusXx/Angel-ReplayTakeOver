//! Fast-forward, rewind and frame stepping for Replay Theater playback.
//!
//! The game's per-frame scene update, `CAHSceneAH30Game::Update` (0x476E10, not hooked by
//! Angel), runs one simulation block while playback is running:
//!   input cache (0x5D7FA0) -> battle pause input (0x610D50) -> replay cursor advance
//!   (0x49AFA0, only when reading the replay) -> CMLMoe update (via 0x56A150 / 0x56A1A0)
//! and then menus/UI/drawing. We run that same block ourselves (`simulate_one_frame`):
//! - Fast-forward runs it extra times before the normal frame.
//! - Rewind restores the nearest earlier snapshot from `history` and silently re-simulates up
//!   to the target frame; the normal frame then draws it.
//! - While playback is stopped, a tap steps one frame forward (simulate once) or back (rewind
//!   by one); holding repeats.
//!
//! Controls on the takeover controller, only in Replay Theater outside takeover mode:
//!   playing: hold RB = 2x, RT = 4x forward; hold LB = 2x, LT = 4x rewind.
//!   stopped: tap RB/RT = next frame, LB/LT = previous frame (hold to repeat).
//! Rewind reaches back to the start of the current round.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use crate::{angel, config, game, history, log, takeover};

const FN_INPUT_CACHE: usize = 0x5D_7FA0;
const FN_BATTLE_PAUSE_INPUT: usize = 0x61_0D50;
const FN_REPLAY_OBJECT: usize = 0x49_AA50;
const FN_REPLAY_ADVANCE_CURSOR: usize = 0x49_AFA0;
const FN_MAIN_LOOP_UPDATE: usize = 0x56_A150;
const FN_MAIN_LOOP_POST_UPDATE: usize = 0x56_A1A0;
/// Returns 0 while Replay Theater playback is running (not stopped, pause menu closed).
const FN_PLAYBACK_STOPPED: usize = 0x49_A740;
/// Offset of the CMLMoe main-loop member inside the game scene object.
const SCENE_MAIN_LOOP: usize = 0x28;

const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const TRIGGER_THRESHOLD: u8 = 30;

/// Frame stepping while stopped: held this many frames before repeating, then every N frames.
const STEP_REPEAT_DELAY: u32 = 20;
const STEP_REPEAT_EVERY: u32 = 3;

/// Requested speed from the controller: negative = rewind, positive = forward, 0 = none.
/// Magnitude 2 for the shoulder button, 4 for the trigger.
static REQUEST: AtomicU32 = AtomicU32::new(0);

/// Called with each XInput state the game reads.
pub fn on_xinput(user: u32, buttons: u16, left_trigger: u8, right_trigger: u8) {
    if user != config::get().controller as u32 {
        return;
    }
    let back = if left_trigger > TRIGGER_THRESHOLD {
        4
    } else if buttons & XINPUT_GAMEPAD_LEFT_SHOULDER != 0 {
        2
    } else {
        0
    };
    let forward = if right_trigger > TRIGGER_THRESHOLD {
        4
    } else if buttons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0 {
        2
    } else {
        0
    };
    // Both directions at once cancel out.
    let request: i32 = if back > 0 && forward > 0 { 0 } else { forward - back };
    REQUEST.store(request as u32, Ordering::Relaxed);
}

fn request() -> i32 {
    REQUEST.load(Ordering::Relaxed) as i32
}

fn playback_running() -> bool {
    let stopped: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(game::runtime_address(FN_PLAYBACK_STOPPED)) };
    unsafe { stopped() == 0 }
}

fn cursor() -> u32 {
    game::read_u32(game::REPLAY_CURSOR)
}

/// While stopped, the battle code skips its simulation (it asks "paused?" via 0x4A6C40), so
/// frames we simulate then must run with Play/Stop briefly on Play. Restored on drop.
struct PlayingWhileSimulating {
    was_stopped: bool,
}

impl PlayingWhileSimulating {
    fn begin() -> Self {
        let was_stopped = game::playback_stopped();
        if was_stopped {
            game::set_playback_stopped(false);
        }
        PlayingWhileSimulating { was_stopped }
    }
}

impl Drop for PlayingWhileSimulating {
    fn drop(&mut self) {
        if self.was_stopped {
            game::set_playback_stopped(true);
        }
    }
}

/// One simulation block, exactly as `CAHSceneAH30Game::Update` runs it.
unsafe fn simulate_one_frame(scene: *mut u8) {
    let addr = game::runtime_address;
    let input_cache: unsafe extern "C" fn() = std::mem::transmute(addr(FN_INPUT_CACHE));
    let battle_pause_input: unsafe extern "C" fn() = std::mem::transmute(addr(FN_BATTLE_PAUSE_INPUT));
    let replay_object: unsafe extern "C" fn() -> *mut u8 = std::mem::transmute(addr(FN_REPLAY_OBJECT));
    let advance_cursor: unsafe extern "thiscall" fn(*mut u8) -> u32 = std::mem::transmute(addr(FN_REPLAY_ADVANCE_CURSOR));
    let update: unsafe extern "thiscall" fn(*mut u8) -> u32 = std::mem::transmute(addr(FN_MAIN_LOOP_UPDATE));
    let post_update: unsafe extern "thiscall" fn(*mut u8, u32) -> u32 = std::mem::transmute(addr(FN_MAIN_LOOP_POST_UPDATE));

    input_cache();
    battle_pause_input();
    if game::read_u32(game::REPLAY_READ_FLAG) != 0 {
        advance_cursor(replay_object());
    }
    let main_loop = scene.add(SCENE_MAIN_LOOP);
    update(main_loop);
    // The original passes a debug-key byte here (0 unless special keys are held).
    post_update(main_loop, 0);
}

/// Called from the `CAHSceneAH30Game::Update` hook before the original runs.
pub unsafe fn before_scene_update(scene: *mut u8) {
    if !angel::on_main_thread() {
        return;
    }
    takeover::tick_countdown();
    history::maintain();
    if !game::is_replay_theater_playback() {
        return;
    }
    if takeover::in_takeover() {
        history::mark_diverged();
        reset_stepper();
        return;
    }
    history::maybe_capture();

    let request = request();
    if playback_running() {
        reset_stepper();
        if request < 0 {
            // The normal frame then advances one, so going back |request| + 1 here shows
            // the replay moving backwards at |request| frames per frame.
            let target = cursor().saturating_sub(request.unsigned_abs() + 1);
            rewind_to(scene, target);
        } else if request > 1 {
            fast_forward(scene, request as u32);
        }
    } else if !game::pause_menu_open() {
        match stepper(request.signum()) {
            1 => {
                let _playing = PlayingWhileSimulating::begin();
                simulate_one_frame(scene);
                history::maybe_capture();
            }
            -1 => {
                if let Some(target) = cursor().checked_sub(1) {
                    rewind_to(scene, target);
                }
            }
            _ => {}
        }
    }
}

unsafe fn fast_forward(scene: *mut u8, speed: u32) {
    let started = Instant::now();
    let mut steps = 0;
    for _ in 1..speed {
        simulate_one_frame(scene);
        steps += 1;
        history::maybe_capture();
        if !game::is_replay_theater_playback() || !playback_running() {
            break; // e.g. playback finished during the extra steps
        }
    }
    STATS.lock().unwrap_or_else(|e| e.into_inner()).add_forward(steps, started);
}

/// Puts the game at the top of replay frame `target` (clamped to the earliest snapshot).
unsafe fn rewind_to(scene: *mut u8, target: u32) {
    let Some(earliest) = history::earliest_frame() else { return };
    let target = target.max(earliest);
    let started = Instant::now();
    match history::restore_at_or_before(target) {
        Ok(frame) => {
            let restored = Instant::now();
            let _playing = PlayingWhileSimulating::begin();
            for _ in frame..target {
                simulate_one_frame(scene);
            }
            let reached = cursor();
            if reached != target {
                log::write(&format!("rewind: re-simulated from {frame} to {reached}, expected {target}"));
            }
            STATS.lock().unwrap_or_else(|e| e.into_inner()).add_rewind(target - frame, started, restored);
        }
        Err(e) => log::write(&format!("rewind failed: {e}")),
    }
}

// ---------------------------------------------------------------------------------------
// Frame stepping while stopped
// ---------------------------------------------------------------------------------------

/// (held direction, frames held)
static STEPPER: Mutex<(i32, u32)> = Mutex::new((0, 0));

fn reset_stepper() {
    *STEPPER.lock().unwrap_or_else(|e| e.into_inner()) = (0, 0);
}

/// Returns the direction to step this frame: once on press, then repeating while held.
fn stepper(direction: i32) -> i32 {
    let mut state = STEPPER.lock().unwrap_or_else(|e| e.into_inner());
    if direction == 0 {
        *state = (0, 0);
        return 0;
    }
    if state.0 != direction {
        *state = (direction, 0);
        return direction;
    }
    state.1 += 1;
    let held = state.1;
    if held >= STEP_REPEAT_DELAY && (held - STEP_REPEAT_DELAY) % STEP_REPEAT_EVERY == 0 {
        direction
    } else {
        0
    }
}

// ---------------------------------------------------------------------------------------
// Diagnostics, logged about once per second while seeking
// ---------------------------------------------------------------------------------------

struct Stats {
    since: Option<Instant>,
    forward_steps: u32,
    forward_secs: f64,
    rewinds: u32,
    resim_frames: u32,
    restore_secs: f64,
    resim_secs: f64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    since: None,
    forward_steps: 0,
    forward_secs: 0.0,
    rewinds: 0,
    resim_frames: 0,
    restore_secs: 0.0,
    resim_secs: 0.0,
});

impl Stats {
    fn add_forward(&mut self, steps: u32, started: Instant) {
        self.forward_steps += steps;
        self.forward_secs += started.elapsed().as_secs_f64();
        self.maybe_report();
    }

    fn add_rewind(&mut self, frames: u32, started: Instant, restored: Instant) {
        self.rewinds += 1;
        self.resim_frames += frames;
        self.restore_secs += (restored - started).as_secs_f64();
        self.resim_secs += restored.elapsed().as_secs_f64();
        self.maybe_report();
    }

    fn maybe_report(&mut self) {
        let since = *self.since.get_or_insert_with(Instant::now);
        if since.elapsed().as_secs_f64() < 1.0 {
            return;
        }
        if self.forward_steps > 0 {
            log::write(&format!(
                "fast-forward: {} extra frames in the last second, {:.2} ms each, replay frame {}",
                self.forward_steps,
                self.forward_secs * 1000.0 / self.forward_steps as f64,
                cursor()
            ));
        }
        if self.rewinds > 0 {
            log::write(&format!(
                "rewind: {} rewinds in the last second, restore {:.2} ms, re-simulating {:.1} frames {:.2} ms (averages), replay frame {}",
                self.rewinds,
                self.restore_secs * 1000.0 / self.rewinds as f64,
                self.resim_frames as f64 / self.rewinds as f64,
                self.resim_secs * 1000.0 / self.rewinds as f64,
                cursor()
            ));
        }
        *self = Stats { since: Some(Instant::now()), forward_steps: 0, forward_secs: 0.0, rewinds: 0, resim_frames: 0, restore_secs: 0.0, resim_secs: 0.0 };
    }
}
