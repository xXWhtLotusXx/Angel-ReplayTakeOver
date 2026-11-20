# Replay Takeover

Take control of either player while watching a replay in **Arcana Heart 3 LOVEMAX SIXSTARS!!!!!! XTEND**, then reset to that moment and try again, the way Street Fighter 6's Replay Takeover works. Rewind, fast-forward and frame-step through the replay to find the exact moment to take over.

This is a companion to the [Angel](https://github.com/super-continent/Angel) mod. It requires **Angel version 9** and does not modify Angel's files.

## Install

1. Install Angel v9 as usual (`dinput8.dll` in the game folder).
2. Apply a Large Address Awareness patch to `AALib.exe`, as Angel recommends to prevent rare crashes when loading into a match. For example, run [4GB Patch](https://ntcore.com/4gb-patch/) on `AALib.exe` (it keeps a backup, `AALib.exe.Backup`). Replay Takeover works with both the patched and the unpatched game.
3. Download `xinput1_3.dll` from the [latest release](https://github.com/americoperez49/Angel-ReplayTakeOver/releases/latest) and copy it into the game folder:
   `...\steamapps\common\ArcanaHeart3LMSS\`
4. Start the game. On first launch `replay_takeover.ini` is created next to the game.

If the game or Angel isn't the exact supported version, a message says so and the mod stays inactive; the game and Angel run normally.

**Uninstall:** delete `xinput1_3.dll` (and optionally `replay_takeover.ini` and `replay_takeover.log`) from the game folder.

## Use

### Finding the moment: rewind, fast-forward, frame step

In **Replay Theater** (outside takeover mode), use the shoulder buttons of your controller:

| | While playing | While stopped (A) |
|---|---|---|
| **LB** | hold: rewind 2x | tap: previous frame (hold to repeat) |
| **LT** | hold: rewind 4x | tap: previous frame (hold to repeat) |
| **RB** | hold: fast-forward 2x | tap: next frame (hold to repeat) |
| **RT** | hold: fast-forward 4x | tap: next frame (hold to repeat) |

- Rewind reaches back to the start of the current round, through the part you have already watched.
- A controls bar at the bottom of the screen lists these buttons. X hides it together with the game's controls window.

### From the controller (pause menu)

In **Replay Theater**, while a round is being fought, open the pause menu (Start, or X on the keyboard). It has two extra options:

| Not in takeover | In takeover mode |
|---|---|
| **Take Over P1**: control Player 1 from this moment | **Reset Battle**: return to the moment you took over |
| **Take Over P2**: control Player 2 from this moment | **End Takeover**: stop controlling and keep watching |
| Replay Theater | Replay Theater |
| Close | Close |

In takeover mode:

- The taken-over character follows your controller (your normal key config, including combo buttons). The other character keeps doing exactly what it did in the recorded match.
- **Select** (Back) resets to the takeover moment.
- The replay viewer's own button controls (A = Play/Stop, Y = HUD, X = window) are turned off so those buttons only fight, and the controls window in the bottom-right corner is hidden. The pause menu still opens.
- At the KO, control pauses but the saved moment is kept, so Select / Reset Battle still retries. Takeover mode ends with End Takeover, when the next round starts, or when playback ends; the controls window then comes back as it was.
- Taking over again starts a new takeover from the current moment.
- Every takeover and every reset holds the replay for a countdown (3 seconds by default, see `countdown_seconds`) before you get control, whether playback was playing or stopped.

### From the keyboard

| Key | Action |
|---|---|
| F6 | Take over P1 (saves this moment) |
| F7 | Take over P2 (saves this moment) |
| F8 | Reset Battle |
| F9 | End the takeover |

## Settings (`replay_takeover.ini`)

```ini
take_over_p1 = F6      ; keys: F1-F12, A-Z, 0-9, NUMPAD0-NUMPAD9, or a code like 0x75
take_over_p2 = F7      ; Angel already uses F1-F5
reset_battle = F8
end_takeover = F9
controller = 1         ; which of the game's controllers you use during a takeover: 1 or 2
countdown_seconds = 3  ; countdown before you get control after a takeover or reset: 0-10 (0 = none)
debug_log = false      ; true = detailed replay log for troubleshooting
```

Changes take effect the next time the game starts. Settings added by newer versions are appended to an existing file automatically.

## Troubleshooting

`replay_takeover.log` in the game folder describes each session: version checks, settings, and every takeover, reset and refusal.

- **"reset failed … validator refused"**: Angel's own safety check found that the game state can no longer be restored to the saved moment, so nothing was changed. Start a new takeover (F6/F7).
- **Nothing happens on F6/F7**: takeover only works in Replay Theater while a round is being fought, and the game window must have focus.

## How it works

- Loaded by the game as `xinput1_3.dll`; all XInput calls are forwarded to the real system DLL.
- Hooks only game functions that Angel doesn't hook:
  - `GetBattleKey` (0x5FFE70): for the taken-over player it answers from the live controller via the game's own replay-recording conversion (0x5FF500); everything else goes to the original, so the recorded inputs are never modified.
  - Replay viewer input (0x49A890) and buttons (0x5982A0): opens the extended pause menu, runs its takeover options, and turns off A/Y/X viewer controls during a takeover.
  - Pause menu draw (0x508900) and text lookup (0x5F7560 / 0x5F7630): draws the two extra rows with the game's own text routine and supplies the takeover labels, help lines and overlay text.
  - Scene update `CAHSceneAH30Game::Update` (0x476E10): fast-forward runs the game's own per-frame simulation block extra times; rewind restores the nearest earlier snapshot and silently re-simulates to the target frame; frame steps do the same one frame at a time. While playback is stopped the battle code skips simulation, so stepped frames run with Play/Stop briefly set to Play.
  - Battle HUD draw (0x4D6BC0): draws the takeover countdown and the controls bar.
- Rewind history: during a Replay Theater round a snapshot (about 2.4 MB, about 1 ms) is taken every 10 frames with Angel's capture function. Past 384 MB every second snapshot is dropped and the interval doubles. The history is cleared when the round changes, and no snapshots are taken while the game is off the recorded timeline (during a takeover) until a rewind restores a recorded moment.
- Reset Battle calls Angel's state capture (0x100E32E0) and validator (0x100E53F0) and reproduces Angel's restore loop, plus the replay cursor. Capture/restore run at the game's XInput poll on its main thread, between battle frames. Select is read from the same XInput poll.
- Every game and Angel address is only valid for the verified builds: the game's code section (SHA-256 `67621ec8…`, unaffected by the 4GB patch) and Angel v9's `dinput8.dll` (SHA-256 `9f4487c2…`).

## Build

Requires Rust with the `i686-pc-windows-msvc` target and the MSVC build tools.

```
rustup target add i686-pc-windows-msvc
cargo build --release
```

Output: `target\i686-pc-windows-msvc\release\replay_takeover.dll`, installed as `xinput1_3.dll`.
