//! Refuse to run unless the game and Angel are exactly the builds this DLL was written against.
//! Every game/Angel address used by later steps is only valid for these builds.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// SHA-256 of AALib.exe's `.text` section. Only the code is hashed so that header-only
/// patches (e.g. the recommended 4GB / Large Address Aware patch) still match.
const GAME_TEXT_SHA256: &str = "67621ec8c07ab86777be1392d64169fec09d50f71ca4923efb635b0b504a751e";

/// SHA-256 of Angel version 9's dinput8.dll (whole file).
const ANGEL_V9_SHA256: &str = "9f4487c277b6f55b458bf5a82700377fb27851f1940ca173667313d92043f0e8";

pub struct Report {
    pub game_ok: bool,
    pub angel_ok: bool,
    pub details: Vec<String>,
}

impl Report {
    pub fn supported(&self) -> bool {
        self.game_ok && self.angel_ok
    }
}

pub fn check(game_exe: &Path, angel_dll: Option<PathBuf>) -> Report {
    let mut details = Vec::new();

    let game_ok = match std::fs::read(game_exe).map_err(|e| e.to_string()).and_then(|b| text_section_sha256(&b)) {
        Ok(hash) => {
            details.push(format!("game .text sha256 {hash} ({})", game_exe.display()));
            hash == GAME_TEXT_SHA256
        }
        Err(e) => {
            details.push(format!("could not fingerprint {}: {e}", game_exe.display()));
            false
        }
    };

    let angel_ok = match angel_dll {
        None => {
            details.push("Angel (dinput8.dll) is not loaded in this process".to_owned());
            false
        }
        Some(path) => match std::fs::read(&path) {
            Ok(bytes) => {
                let hash = hex(&Sha256::digest(&bytes));
                details.push(format!("Angel sha256 {hash} ({})", path.display()));
                hash == ANGEL_V9_SHA256
            }
            Err(e) => {
                details.push(format!("could not read {}: {e}", path.display()));
                false
            }
        },
    };

    Report { game_ok, angel_ok, details }
}

/// Hashes the raw bytes of the PE section named `.text`.
fn text_section_sha256(pe: &[u8]) -> Result<String, String> {
    let u16_at = |o: usize| pe.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| pe.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let malformed = || "not a valid PE file".to_owned();

    let nt = u32_at(0x3c).ok_or_else(malformed)? as usize;
    let section_count = u16_at(nt + 6).ok_or_else(malformed)? as usize;
    let optional_header_size = u16_at(nt + 20).ok_or_else(malformed)? as usize;
    let first_section = nt + 24 + optional_header_size;

    for i in 0..section_count {
        let header = first_section + i * 40;
        let name = pe.get(header..header + 8).ok_or_else(malformed)?;
        if name.starts_with(b".text\0") {
            let raw_size = u32_at(header + 16).ok_or_else(malformed)? as usize;
            let raw_offset = u32_at(header + 20).ok_or_else(malformed)? as usize;
            let code = pe.get(raw_offset..raw_offset + raw_size).ok_or_else(malformed)?;
            return Ok(hex(&Sha256::digest(code)));
        }
    }
    Err("no .text section".to_owned())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
