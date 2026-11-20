//! Snapshot history of the current Replay Theater round, used by rewind.
//!
//! Every `interval` replay frames we capture the game state with Angel's capture function,
//! keyed by the replay cursor at the top of the frame (state before that frame is simulated).
//! When the history grows past `MEMORY_BUDGET`, every second snapshot is dropped and the
//! interval doubles, so a long round still fits. The history is cleared when the round changes
//! or playback ends.
//!
//! Snapshots are only taken while the game is on the recorded replay's timeline: a takeover
//! changes what happens, so capturing stops until a rewind restores a recorded state again.
//!
//! Only used on the game's main thread (from the scene-update hook).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::{angel, game, log};

/// Frames between snapshots at the start of a round.
const START_INTERVAL: u32 = 10;
/// Total snapshot data kept for one round before thinning.
const MEMORY_BUDGET: usize = 384 * 1024 * 1024;

pub struct Entry {
    pub snapshot: angel::Snapshot,
    pub angel_last_frame: u32,
}

struct History {
    round: u32,
    entries: BTreeMap<u32, Entry>,
    interval: u32,
    total_bytes: usize,
    /// False once the game left the recorded timeline (a takeover); then nothing is captured.
    on_replay_timeline: bool,
    capture_failed_logged: bool,
}

impl History {
    fn new(round: u32) -> Self {
        History {
            round,
            entries: BTreeMap::new(),
            interval: START_INTERVAL,
            total_bytes: 0,
            on_replay_timeline: true,
            capture_failed_logged: false,
        }
    }

    fn thin_out(&mut self) {
        while self.total_bytes > MEMORY_BUDGET && self.entries.len() > 2 {
            let frames: Vec<u32> = self.entries.keys().copied().collect();
            for frame in frames.iter().skip(1).step_by(2) {
                if let Some(entry) = self.entries.remove(frame) {
                    self.total_bytes -= entry.snapshot.data_len();
                }
            }
            self.interval *= 2;
            log::write(&format!(
                "rewind: history over {} MB, kept {} snapshots, now one every {} frames",
                MEMORY_BUDGET / (1024 * 1024),
                self.entries.len(),
                self.interval
            ));
        }
    }
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);
static FIRST_CAPTURE_LOGGED: Mutex<bool> = Mutex::new(false);

fn with_history<R>(f: impl FnOnce(&mut Option<History>) -> R) -> R {
    f(&mut HISTORY.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Clears the history when Replay Theater playback is over or the round changed.
pub fn maintain() {
    let round = game::read_u32(game::ROUND_INDEX);
    with_history(|history| {
        let stale = match history.as_ref() {
            Some(h) => !game::is_replay_theater_playback() || h.round != round,
            None => false,
        };
        if stale {
            if let Some(h) = history.take() {
                log::write(&format!("rewind: history of round {} cleared ({} snapshots)", h.round + 1, h.entries.len()));
            }
        }
        if history.is_none() && game::is_replay_theater_playback() {
            *history = Some(History::new(round));
        }
    });
}

/// The game is leaving the recorded timeline (takeover): stop capturing until a rewind.
pub fn mark_diverged() {
    with_history(|history| {
        if let Some(h) = history.as_mut() {
            h.on_replay_timeline = false;
        }
    });
}

/// Captures a snapshot at the current replay frame if the last one is `interval` frames old.
pub fn maybe_capture() {
    let frame = game::read_u32(game::REPLAY_CURSOR);
    with_history(|history| {
        let Some(h) = history.as_mut() else { return };
        if !h.on_replay_timeline {
            return;
        }
        let recent = h.entries.range(frame.saturating_sub(h.interval - 1)..=frame).next().is_some();
        if recent {
            return;
        }
        let started = Instant::now();
        match angel::capture() {
            Ok(snapshot) => {
                let bytes = snapshot.data_len();
                let mut first = FIRST_CAPTURE_LOGGED.lock().unwrap_or_else(|e| e.into_inner());
                if !*first {
                    *first = true;
                    log::write(&format!(
                        "rewind: first snapshot at frame {frame}: {:.1} MB, captured in {:.2} ms",
                        bytes as f64 / (1024.0 * 1024.0),
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                }
                h.total_bytes += bytes;
                h.entries.insert(frame, Entry { snapshot, angel_last_frame: angel::read_global(angel::GLOBAL_LAST_REPLAY_FRAME) });
                h.thin_out();
            }
            Err(e) => {
                if !h.capture_failed_logged {
                    h.capture_failed_logged = true;
                    log::write(&format!("rewind: snapshot at frame {frame} failed (phase {}): {e}", game::battle_phase()));
                }
            }
        }
    });
}

/// Frame of the earliest snapshot, if any.
pub fn earliest_frame() -> Option<u32> {
    with_history(|history| history.as_ref().and_then(|h| h.entries.keys().next().copied()))
}

/// Restores the latest snapshot at or before `target` (cursor and Angel's frame counter
/// included) and returns its frame. A snapshot Angel's validator refuses is discarded.
pub fn restore_at_or_before(target: u32) -> Result<u32, String> {
    with_history(|history| {
        let h = history.as_mut().ok_or("no rewind history")?;
        let (&frame, entry) = h.entries.range(..=target).next_back().ok_or("no snapshot that early")?;
        match angel::restore(&entry.snapshot) {
            Ok(()) => {
                game::write_u32(game::REPLAY_CURSOR, frame);
                angel::write_global(angel::GLOBAL_LAST_REPLAY_FRAME, entry.angel_last_frame);
                h.on_replay_timeline = true;
                Ok(frame)
            }
            Err(e) => {
                if let Some(entry) = h.entries.remove(&frame) {
                    h.total_bytes -= entry.snapshot.data_len();
                }
                Err(format!("snapshot at frame {frame} discarded: {e}"))
            }
        }
    })
}
