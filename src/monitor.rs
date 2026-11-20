//! Troubleshooting log (enabled by `debug_log = true`): polls replay/round state on a
//! background thread and logs every change plus each player's recorded inputs. Read-only.

use std::time::Duration;

use crate::game;
use crate::log;

/// Faster than one game frame (~16.7 ms) so most single-frame changes are seen.
const POLL_INTERVAL: Duration = Duration::from_millis(4);
/// While a replay plays, also log the cursor every this many frames (~10 s).
const CURSOR_LOG_INTERVAL: u32 = 600;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Globals {
    scene: u32,
    game_mode: u32,
    p1_source: u32,
    p2_source: u32,
    replay_read_flag: u32,
    replay_slot: u32,
    phase_a: u32,
    phase_b: u32,
    round_over: u32,
    round_index: u32,
    wins_a: u32,
    wins_b: u32,
}

impl Globals {
    fn read() -> Self {
        Globals {
            scene: game::read_u32(game::SCENE),
            game_mode: game::read_u32(game::GAME_MODE),
            p1_source: game::read_u32(game::P1_INPUT_SOURCE),
            p2_source: game::read_u32(game::P2_INPUT_SOURCE),
            replay_read_flag: game::read_u32(game::REPLAY_READ_FLAG),
            replay_slot: game::read_u32(game::REPLAY_SLOT),
            phase_a: game::read_u32(game::BATTLE_PHASE_A),
            phase_b: game::read_u32(game::BATTLE_PHASE_B),
            round_over: game::read_u32(game::ROUND_OVER_FLAG),
            round_index: game::read_u32(game::ROUND_INDEX),
            wins_a: game::read_u32(game::WINS_A),
            wins_b: game::read_u32(game::WINS_B),
        }
    }

    fn describe(&self) -> String {
        format!(
            "scene={:#x} mode={} p1src={} p2src={} replayRead={} slot={} phase={}/{} roundOver={} roundIndex={} wins={}/{}",
            self.scene,
            self.game_mode,
            self.p1_source,
            self.p2_source,
            self.replay_read_flag,
            self.replay_slot,
            self.phase_a,
            self.phase_b,
            self.round_over,
            self.round_index,
            self.wins_a,
            self.wins_b
        )
    }
}

pub fn start() {
    std::thread::spawn(run);
}

fn run() {
    log::write("monitor: started (read-only)");
    let mut last_globals: Option<Globals> = None;
    let mut was_replay = false;
    let mut last_cursor: Option<u32> = None;
    let mut last_inputs: [Option<u16>; 2] = [None, None];

    loop {
        let globals = Globals::read();
        if last_globals != Some(globals) {
            let frame = game::read_u32(game::REPLAY_CURSOR);
            log::write(&format!("state @frame {frame}: {}", globals.describe()));
            last_globals = Some(globals);
        }

        let replay = game::is_replay_playback();
        if replay != was_replay {
            log::write(match (replay, game::is_replay_theater_playback()) {
                (true, true) => "replay playback STARTED (Replay Theater)",
                (true, false) => "replay playback STARTED (replay-driven battle, mode 6)",
                (false, _) => "replay playback ENDED",
            });
            was_replay = replay;
            last_cursor = None;
            last_inputs = [None, None];
        }

        if replay {
            let cursor = game::read_u32(game::REPLAY_CURSOR);
            match last_cursor {
                Some(previous) if cursor < previous => {
                    log::write(&format!("cursor went BACK {previous} -> {cursor} (new round or reset?)"))
                }
                Some(previous) if cursor / CURSOR_LOG_INTERVAL != previous / CURSOR_LOG_INTERVAL => {
                    log::write(&format!("cursor at frame {cursor}"))
                }
                None => log::write(&format!("cursor at frame {cursor}")),
                _ => {}
            }
            last_cursor = Some(cursor);

            for player in 0..2u32 {
                let input = game::replay_input(player, cursor);
                if input != last_inputs[player as usize] {
                    if let Some(word) = input {
                        log::write(&format!(
                            "frame {cursor}: P{} {} ({word:#06x})",
                            player + 1,
                            game::describe_input(word)
                        ));
                    }
                    last_inputs[player as usize] = input;
                }
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}
