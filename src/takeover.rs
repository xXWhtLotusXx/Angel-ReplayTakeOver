//! Takeover state machine and hotkeys.
//!
//! Hotkeys (F6/F7 take over P1/P2, F8 Reset Battle, F9 end) only queue a request. Requests
//! are carried out at the next frame boundary — our `XInputGetState` export, which the game
//! calls from its input-polling task on the main thread, outside the battle simulation — and
//! only when that call really is on Angel's recorded main thread.
//!
//! Lifecycle: a takeover captures the game state (via Angel) and gives the chosen side to the
//! live controller. At the KO the live control pauses but the snapshot is kept, so F8 can
//! retry the exchange. Everything is released on F9, when the next round starts, or when
//! replay playback ends.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::{angel, config, game, log};

const NO_SIDE: i32 = -1;
const HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(8);
/// How long to measure the frame-boundary rate during the first Replay Theater fight.
const RATE_SAMPLE: Duration = Duration::from_secs(5);

/// Side whose inputs currently come from the live controller (read by the hook); -1 = none.
static LIVE_SIDE: AtomicI32 = AtomicI32::new(NO_SIDE);

/// True while a takeover session exists (live or paused at the KO). While set, the replay
/// viewer's own button controls are suppressed and Select resets.
static IN_TAKEOVER: AtomicBool = AtomicBool::new(false);

/// XInput Back ("Select") button.
const XINPUT_GAMEPAD_BACK: u16 = 0x0020;
static SELECT_WAS_DOWN: AtomicBool = AtomicBool::new(false);

pub fn in_takeover() -> bool {
    IN_TAKEOVER.load(Ordering::Acquire)
}

/// Called with each XInput state the game reads for `user`: pressing Select (Back) on the
/// takeover controller during a takeover queues Reset Battle.
pub fn on_xinput_buttons(user: u32, buttons: u16) {
    if user != config::get().controller as u32 {
        return;
    }
    let down = buttons & XINPUT_GAMEPAD_BACK != 0;
    let was_down = SELECT_WAS_DOWN.swap(down, Ordering::AcqRel);
    if down && !was_down && in_takeover() {
        queue(Request::Reset);
    }
}

/// Live input word cached per replay frame, so every button query within one frame sees the
/// same controller state. Only touched from the game thread (inside the hook).
static FRAME_WORD: Mutex<Option<(u32, u16)>> = Mutex::new(None);

#[derive(Clone, Copy, Debug)]
pub enum Request {
    TakeOver(i32),
    Reset,
    End,
}

static PENDING: Mutex<Option<Request>> = Mutex::new(None);

struct Session {
    side: i32,
    /// None if Angel's capture failed; then Reset Battle is unavailable for this takeover.
    snapshot: Option<angel::Snapshot>,
    cursor_at_capture: u32,
    angel_last_frame_at_capture: u32,
    round_index: u32,
}

/// Only accessed on the game thread (frame boundary).
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

// Every takeover and reset stops playback and resumes it after `countdown_seconds`.
/// (side, when playback resumes); set while the countdown runs.
static COUNTDOWN_END: Mutex<Option<(i32, Instant)>> = Mutex::new(None);

/// The running takeover countdown: (side, whole seconds left, rounded up).
pub fn countdown() -> Option<(i32, u32)> {
    let countdown = *COUNTDOWN_END.lock().unwrap_or_else(|e| e.into_inner());
    countdown.map(|(side, end)| {
        let left = end.saturating_duration_since(Instant::now());
        (side, (left.as_millis() as u32).div_ceil(1000).max(1))
    })
}

/// Holds playback for a countdown so you can get ready (every takeover and every reset).
fn start_countdown(side: i32) {
    let seconds = config::get().countdown_seconds;
    if seconds == 0 {
        // No countdown: play straight away, even if playback was stopped.
        cancel_countdown();
        game::set_playback_stopped(false);
        return;
    }
    game::set_playback_stopped(true);
    *COUNTDOWN_END.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((side, Instant::now() + Duration::from_secs(seconds as u64)));
    log::write(&format!("countdown: playback resumes in {seconds} seconds"));
}

fn cancel_countdown() {
    COUNTDOWN_END.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// Called every frame on the game thread: resumes playback when the countdown is over, and
/// drops a countdown whose takeover is gone.
pub fn tick_countdown() {
    let mut countdown = COUNTDOWN_END.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, end)) = *countdown else { return };
    if !in_takeover() || !game::is_replay_theater_playback() {
        *countdown = None;
        return;
    }
    if Instant::now() >= end {
        *countdown = None;
        game::set_playback_stopped(false);
        log::write("countdown over: playback resumed");
    }
}

pub fn start() {
    std::thread::spawn(run_hotkeys);
}

fn fighting_in_replay_theater() -> bool {
    game::is_replay_theater_playback() && game::battle_phase() == game::PHASE_FIGHTING
}

/// Called from the hook for every button query. Returns the live input word if `player` is
/// currently taken over, otherwise `None` (the caller then uses the original function).
pub fn live_input_word_for(player: i32) -> Option<u16> {
    if LIVE_SIDE.load(Ordering::Acquire) != player || !fighting_in_replay_theater() {
        return None;
    }
    let frame = game::read_u32(game::REPLAY_CURSOR);
    let mut cached = FRAME_WORD.lock().unwrap_or_else(|e| e.into_inner());
    match *cached {
        Some((cached_frame, word)) if cached_frame == frame => Some(word),
        _ => {
            let word = game::build_live_input_word(config::get().controller);
            *cached = Some((frame, word));
            Some(word)
        }
    }
}

fn set_live(side: i32) {
    *FRAME_WORD.lock().unwrap_or_else(|e| e.into_inner()) = None;
    LIVE_SIDE.store(side, Ordering::Release);
}

// ---------------------------------------------------------------------------------------
// Frame boundary (game thread)
// ---------------------------------------------------------------------------------------

static BOUNDARY_CALLS: AtomicU32 = AtomicU32::new(0);
static REPORTED_WRONG_THREAD: AtomicBool = AtomicBool::new(false);

/// Called at the start of every `XInputGetState` call.
pub fn on_frame_boundary() {
    BOUNDARY_CALLS.fetch_add(1, Ordering::Relaxed);
    let main_thread = angel::read_global(angel::GLOBAL_MAIN_THREAD_ID);
    let on_main_thread = main_thread != 0 && main_thread == unsafe { GetCurrentThreadId() };
    if !on_main_thread {
        if PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_some()
            && !REPORTED_WRONG_THREAD.swap(true, Ordering::Relaxed)
        {
            log::write(&format!(
                "frame boundary: XInputGetState is not on the game's main thread (this {}, main {main_thread}); requests are not processed",
                unsafe { GetCurrentThreadId() }
            ));
        }
        return;
    }

    let request = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(request) = request {
        handle_request(request, &mut session);
    }
    supervise(&mut session);
    IN_TAKEOVER.store(session.is_some(), Ordering::Release);
    hide_replay_controls_window(session.is_some());
}

/// Value of the Replay Theater controls window flag before the takeover hid it.
static CONTROLS_WINDOW_BEFORE: Mutex<Option<u8>> = Mutex::new(None);

/// Keeps the Replay Theater controls window (bottom right) hidden during a takeover and puts
/// it back as it was afterwards. Enforced every frame, so a Reset Battle that restores the
/// old value can't bring it back.
fn hide_replay_controls_window(in_takeover: bool) {
    let mut before = CONTROLS_WINDOW_BEFORE.lock().unwrap_or_else(|e| e.into_inner());
    if in_takeover {
        if before.is_none() {
            *before = Some(game::read_u8(game::CONTROLS_WINDOW_SHOWN));
        }
        game::write_u8(game::CONTROLS_WINDOW_SHOWN, 0);
    } else if let Some(value) = before.take() {
        game::write_u8(game::CONTROLS_WINDOW_SHOWN, value);
    }
}

fn handle_request(request: Request, session: &mut Option<Session>) {
    match request {
        Request::TakeOver(side) => {
            if !fighting_in_replay_theater() {
                log::write(&format!(
                    "takeover P{} ignored: only available while a Replay Theater round is being fought (phase {})",
                    side + 1,
                    game::battle_phase()
                ));
                return;
            }
            // A new takeover always starts from a fresh snapshot of this moment.
            if let Some(old) = session.take() {
                drop(old.snapshot);
            }
            let frame = game::read_u32(game::REPLAY_CURSOR);
            let snapshot = match angel::capture() {
                Ok(snapshot) => Some(snapshot),
                Err(e) => {
                    log::write(&format!("snapshot failed, Reset Battle unavailable for this takeover: {e}"));
                    None
                }
            };
            log::write(&format!(
                "TAKEOVER: P{} now follows your controller (frame {frame}{})",
                side + 1,
                if snapshot.is_some() { ", state saved: F8 resets here" } else { "" }
            ));
            *session = Some(Session {
                side,
                snapshot,
                cursor_at_capture: frame,
                angel_last_frame_at_capture: angel::read_global(angel::GLOBAL_LAST_REPLAY_FRAME),
                round_index: game::read_u32(game::ROUND_INDEX),
            });
            set_live(side);
            start_countdown(side);
        }
        Request::Reset => {
            let Some(active) = session.as_ref() else {
                log::write("reset ignored: no takeover in progress");
                return;
            };
            let Some(snapshot) = active.snapshot.as_ref() else {
                log::write("reset ignored: this takeover has no saved state");
                return;
            };
            let from = game::read_u32(game::REPLAY_CURSOR);
            match angel::restore(snapshot) {
                Ok(()) => {
                    game::write_u32(game::REPLAY_CURSOR, active.cursor_at_capture);
                    angel::write_global(angel::GLOBAL_LAST_REPLAY_FRAME, active.angel_last_frame_at_capture);
                    log::write(&format!(
                        "RESET: frame {from} -> {} (phase now {})",
                        active.cursor_at_capture,
                        game::battle_phase()
                    ));
                    set_live(active.side);
                    start_countdown(active.side);
                }
                Err(e) => log::write(&format!("reset failed, state left as is: {e}")),
            }
        }
        Request::End => {
            if session.take().is_some() {
                cancel_countdown();
                set_live(NO_SIDE);
                log::write("TAKEOVER ENDED: end requested (menu or hotkey)");
            }
        }
    }
}

/// Pauses live control at the KO and releases everything when the round or playback ends.
fn supervise(session: &mut Option<Session>) {
    let Some(active) = session.as_ref() else { return };
    let reason = if !game::is_replay_theater_playback() {
        Some("replay playback ended".to_owned())
    } else if game::read_u32(game::ROUND_INDEX) != active.round_index {
        Some("next round started".to_owned())
    } else {
        None
    };
    if let Some(reason) = reason {
        *session = None;
        set_live(NO_SIDE);
        log::write(&format!("TAKEOVER ENDED: {reason}"));
        return;
    }
    if LIVE_SIDE.load(Ordering::Acquire) != NO_SIDE && game::battle_phase() != game::PHASE_FIGHTING {
        set_live(NO_SIDE);
        log::write(&format!(
            "takeover paused: round over (phase {}); F8 to retry, F9 to end",
            game::battle_phase()
        ));
    }
}

// ---------------------------------------------------------------------------------------
// Hotkeys (background thread)
// ---------------------------------------------------------------------------------------

fn game_window_focused() -> bool {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    pid == unsafe { GetCurrentProcessId() }
}

/// Tracks one key and reports a press only on the transition from up to down.
struct HotKey {
    vk: u16,
    was_down: bool,
}

impl HotKey {
    const fn new(vk: u16) -> Self {
        HotKey { vk, was_down: false }
    }

    fn pressed(&mut self, focused: bool) -> bool {
        let down = focused && unsafe { GetAsyncKeyState(self.vk as i32) } as u16 & 0x8000 != 0;
        let pressed = down && !self.was_down;
        self.was_down = down;
        pressed
    }
}

pub fn queue(request: Request) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(request);
}

fn run_hotkeys() {
    let settings = config::get();
    log::write(&format!(
        "takeover: hotkeys active ({} / {} = take over P1 / P2, {} = Reset Battle, {} = end takeover)",
        config::key_name(settings.key_take_over_p1),
        config::key_name(settings.key_take_over_p2),
        config::key_name(settings.key_reset),
        config::key_name(settings.key_end)
    ));
    let mut keys = [
        (HotKey::new(settings.key_take_over_p1), Request::TakeOver(0)),
        (HotKey::new(settings.key_take_over_p2), Request::TakeOver(1)),
        (HotKey::new(settings.key_reset), Request::Reset),
        (HotKey::new(settings.key_end), Request::End),
    ];
    // One-time diagnostic: how often the frame boundary fires during a Replay Theater fight
    // (requests wait for it, so this is the worst-case delay before F6-F9 take effect).
    let mut rate_sample: Option<(Instant, u32)> = None;
    let mut reported_rate = false;
    loop {
        let focused = game_window_focused();
        for (key, request) in keys.iter_mut() {
            if key.pressed(focused) {
                queue(*request);
            }
        }
        if !reported_rate {
            if !fighting_in_replay_theater() {
                rate_sample = None;
            } else {
                let calls = BOUNDARY_CALLS.load(Ordering::Relaxed);
                match rate_sample {
                    None => rate_sample = Some((Instant::now(), calls)),
                    Some((since, start_calls)) if since.elapsed() >= RATE_SAMPLE => {
                        reported_rate = true;
                        let per_second = (calls - start_calls) as f32 / since.elapsed().as_secs_f32();
                        log::write(&format!("frame boundary during a replay fight: {per_second:.0} per second"));
                    }
                    Some(_) => {}
                }
            }
        }
        std::thread::sleep(HOTKEY_POLL_INTERVAL);
    }
}
