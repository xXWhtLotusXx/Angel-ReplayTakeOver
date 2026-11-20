//! Settings from `replay_takeover.ini` in the game folder. Created with defaults if missing;
//! unknown or invalid lines are reported in the log and fall back to defaults.

use std::path::Path;
use std::sync::OnceLock;

use crate::log;

pub struct Config {
    pub key_take_over_p1: u16,
    pub key_take_over_p2: u16,
    pub key_reset: u16,
    pub key_end: u16,
    /// Game controller index (0 = the game's player-1 controller) used during a takeover.
    pub controller: i32,
    /// Log replay state changes and every recorded input (large logs; for troubleshooting).
    pub debug_log: bool,
    /// Countdown before control starts after a takeover or reset; 0 = none.
    pub countdown_seconds: u32,
}

pub const MAX_COUNTDOWN_SECONDS: u32 = 10;
const DEFAULT_COUNTDOWN_SECONDS: u32 = 3;
/// Added to files written by older versions that don't have the setting yet.
const COUNTDOWN_BLOCK: &str = "
; Seconds of countdown before you get control after taking over or resetting: 0-10
; (0 = no countdown).
countdown_seconds = 3
";

const DEFAULT_FILE: &str = "\
; Replay Takeover settings. Delete this file to restore the defaults.
;
; Keys: F1-F12, A-Z, 0-9, NUMPAD0-NUMPAD9, or a Windows virtual-key code like 0x75.
; Angel already uses F1-F5.
take_over_p1 = F6
take_over_p2 = F7
reset_battle = F8
end_takeover = F9

; Which of the game's controllers you play with during a takeover: 1 or 2.
controller = 1

; Seconds of countdown before you get control after taking over or resetting: 0-10
; (0 = no countdown).
countdown_seconds = 3

; true = log replay state and every recorded input (only needed for troubleshooting).
debug_log = false
";

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Current settings; the defaults if `load` hasn't run (never panics inside the game).
pub fn get() -> &'static Config {
    CONFIG.get_or_init(|| parse(DEFAULT_FILE))
}

pub fn load(game_dir: &Path) {
    let path = game_dir.join("replay_takeover.ini");
    let text = match std::fs::read_to_string(&path) {
        Ok(mut text) => {
            if !text.lines().any(|line| line.trim_start().to_ascii_lowercase().starts_with("countdown_seconds")) {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(COUNTDOWN_BLOCK);
                if std::fs::write(&path, &text).is_ok() {
                    log::write("config: added the new countdown_seconds setting to replay_takeover.ini");
                }
            }
            text
        }
        Err(_) => {
            if std::fs::write(&path, DEFAULT_FILE).is_ok() {
                log::write(&format!("config: created {} with defaults", path.display()));
            }
            DEFAULT_FILE.to_owned()
        }
    };
    let config = parse(&text);
    log::write(&format!(
        "config: take over P1 = {}, take over P2 = {}, reset = {}, end = {}, controller = {}, countdown = {} s, debug_log = {}",
        key_name(config.key_take_over_p1),
        key_name(config.key_take_over_p2),
        key_name(config.key_reset),
        key_name(config.key_end),
        config.controller + 1,
        config.countdown_seconds,
        config.debug_log
    ));
    let _ = CONFIG.set(config);
}

fn parse(text: &str) -> Config {
    let mut config = Config {
        key_take_over_p1: 0x75, // F6
        key_take_over_p2: 0x76, // F7
        key_reset: 0x77,        // F8
        key_end: 0x78,          // F9
        controller: 0,
        debug_log: false,
        countdown_seconds: DEFAULT_COUNTDOWN_SECONDS,
    };
    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            log::write(&format!("config: line {}: expected `name = value`, ignored", number + 1));
            continue;
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        let key_slot = match name.as_str() {
            "take_over_p1" => Some(&mut config.key_take_over_p1),
            "take_over_p2" => Some(&mut config.key_take_over_p2),
            "reset_battle" => Some(&mut config.key_reset),
            "end_takeover" => Some(&mut config.key_end),
            _ => None,
        };
        if let Some(slot) = key_slot {
            match parse_key(value) {
                Some(vk) => {
                    if (0x70..=0x74).contains(&vk) {
                        log::write(&format!("config: {name} = {value} is also an Angel hotkey (F1-F5); both will react"));
                    }
                    *slot = vk;
                }
                None => log::write(&format!("config: {name} = {value}: unknown key, keeping default")),
            }
            continue;
        }
        match name.as_str() {
            "controller" => match value {
                "1" => config.controller = 0,
                "2" => config.controller = 1,
                _ => log::write(&format!("config: controller = {value}: must be 1 or 2, keeping 1")),
            },
            "countdown_seconds" => match value.parse::<u32>() {
                Ok(seconds) if seconds <= MAX_COUNTDOWN_SECONDS => config.countdown_seconds = seconds,
                _ => log::write(&format!(
                    "config: countdown_seconds = {value}: must be 0-{MAX_COUNTDOWN_SECONDS}, keeping {DEFAULT_COUNTDOWN_SECONDS}"
                )),
            },
            "debug_log" => match value.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => config.debug_log = true,
                "false" | "0" | "no" => config.debug_log = false,
                _ => log::write(&format!("config: debug_log = {value}: must be true or false, keeping false")),
            },
            _ => log::write(&format!("config: line {}: unknown setting `{name}`, ignored", number + 1)),
        }
    }
    config
}

/// Parses F1-F12, A-Z, 0-9, NUMPAD0-NUMPAD9 or a hex virtual-key code (0x..).
fn parse_key(value: &str) -> Option<u16> {
    let upper = value.to_ascii_uppercase();
    if let Some(hex) = upper.strip_prefix("0X") {
        return u16::from_str_radix(hex, 16).ok().filter(|&vk| (1..=0xFE).contains(&vk));
    }
    if let Some(n) = upper.strip_prefix("NUMPAD") {
        return n.parse::<u16>().ok().filter(|&n| n <= 9).map(|n| 0x60 + n);
    }
    if let Some(n) = upper.strip_prefix('F') {
        if let Ok(n) = n.parse::<u16>() {
            return (1..=12).contains(&n).then_some(0x6F + n);
        }
    }
    match upper.as_bytes() {
        [c @ b'A'..=b'Z'] | [c @ b'0'..=b'9'] => Some(*c as u16),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_names() {
        assert_eq!(parse_key("F6"), Some(0x75));
        assert_eq!(parse_key("f12"), Some(0x7B));
        assert_eq!(parse_key("F13"), None);
        assert_eq!(parse_key("K"), Some(b'K' as u16));
        assert_eq!(parse_key("7"), Some(b'7' as u16));
        assert_eq!(parse_key("numpad4"), Some(0x64));
        assert_eq!(parse_key("0x75"), Some(0x75));
        assert_eq!(parse_key("0x00"), None);
        assert_eq!(parse_key("Space"), None);
    }

    #[test]
    fn key_names_round_trip() {
        for name in ["F6", "F12", "K", "7", "NUMPAD4"] {
            assert_eq!(key_name(parse_key(name).unwrap()), name);
        }
    }

    #[test]
    fn defaults_and_overrides() {
        let defaults = parse(DEFAULT_FILE);
        assert_eq!((defaults.key_take_over_p1, defaults.key_reset, defaults.controller, defaults.debug_log), (0x75, 0x77, 0, false));

        let custom = parse("reset_battle = R\ncontroller = 2\ndebug_log = yes\nnonsense\nend_takeover = Space\n");
        assert_eq!(custom.key_reset, b'R' as u16);
        assert_eq!(custom.controller, 1);
        assert!(custom.debug_log);
        assert_eq!(custom.key_end, 0x78, "invalid key keeps the default");
        assert_eq!(defaults.countdown_seconds, 3);
        assert_eq!(parse("countdown_seconds = 0").countdown_seconds, 0);
        assert_eq!(parse("countdown_seconds = 7").countdown_seconds, 7);
        assert_eq!(parse("countdown_seconds = 11").countdown_seconds, 3, "out of range keeps the default");
    }
}

pub fn key_name(vk: u16) -> String {
    match vk {
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("NUMPAD{}", vk - 0x60),
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        _ => format!("{vk:#04x}"),
    }
}
