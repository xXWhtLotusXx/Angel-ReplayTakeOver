//! Reuse of Angel v9's internal save-state machinery for Reset Battle.
//!
//! Angel only exposes save/load in training mode and deletes its own slot every tick outside
//! it, so we call its capture function ourselves, keep our own record, and re-implement its
//! inline restore loop exactly (Angel battle tick 0x100B5635..0x100B5BC6). Angel's validator
//! (0x100E53F0) is called before every restore; it refuses restores when the game's object set
//! changed since the capture, so a refused restore leaves memory untouched.
//!
//! Every function here must run on the game's main thread between frames.

use std::sync::OnceLock;

use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapFree};

/// Angel's preferred image base; its code and data addresses below are relative to it.
const ANGEL_PREFERRED_BASE: usize = 0x1000_0000;

const FN_CAPTURE: usize = 0x100E_32E0;
const FN_VALIDATE: usize = 0x100E_53F0;
const FN_DROP_REGION_TABLE: usize = 0x100B_3B10;
const FN_RELEASE_DATA_BUFFER: usize = 0x1001_D350;

/// AALib.exe load base as recorded by Angel; Angel's game offsets are relative to it.
const GLOBAL_GAME_BASE: usize = 0x1027_E9F0;
/// Non-zero means Angel's base-address initialisation failed/hasn't run.
const GLOBAL_BASE_INIT_ERROR: usize = 0x1027_E9EC;
/// Set by Angel after a capture/restore ("state replaced").
const GLOBAL_STATE_DIRTY: usize = 0x1028_5313;
/// Angel's recorded main (game) thread id.
pub const GLOBAL_MAIN_THREAD_ID: usize = 0x1028_52B8;
/// Last replay frame Angel's frame driver has seen.
pub const GLOBAL_LAST_REPLAY_FRAME: usize = 0x1027_D678;

/// Ranges (offsets from the game base) Angel never restores.
const FIXED_EXCLUDES: [(u32, u32); 7] = [
    (0xDCB_9961, 0xDCB_9962),
    (0xDCB_9C88, 0xDCB_9C9C),
    (0xDCB_9CA0, 0xDCB_9CC0),
    (0xDCB_9CF4, 0xDCB_9CF6),
    (0xDDC_3448, 0xDDC_344C),
    (0xDEE_F1C0, 0xDEE_F1C4),
    (0xDEE_F1D0, 0xDEE_F1F0),
];
/// Region whose ranges are captured but always excluded from restore.
const EXCLUDED_REGION_INDEX: usize = 0x12;

const RECORD_DWORDS: usize = 0x84 / 4;
const VALIDATOR_OUT_DWORDS: usize = 13;
const FAILED: u32 = u32::MAX;

/// Angel's runtime base (it is relocated, e.g. loaded at 0x73870000).
static ANGEL_BASE: OnceLock<usize> = OnceLock::new();

pub fn set_module_base(base: usize) {
    let _ = ANGEL_BASE.set(base);
}

fn addr(angel_va: usize) -> usize {
    ANGEL_BASE.get().copied().unwrap_or(ANGEL_PREFERRED_BASE) + (angel_va - ANGEL_PREFERRED_BASE)
}

pub fn read_global(angel_va: usize) -> u32 {
    unsafe { std::ptr::read_volatile(addr(angel_va) as *const u32) }
}

pub fn write_global(angel_va: usize, value: u32) {
    unsafe { std::ptr::write_volatile(addr(angel_va) as *mut u32, value) }
}

/// A capture result we own: Angel's 0x84-byte record. Released on drop.
pub struct Snapshot {
    record: Box<[u32; RECORD_DWORDS]>,
}

// The record only holds pointers into Angel-owned heap memory; it is created, used and dropped
// on the game thread, but stored in a Mutex reachable from the hotkey thread.
unsafe impl Send for Snapshot {}

impl Snapshot {
    /// Size of the captured game-memory buffer in bytes.
    pub fn data_len(&self) -> usize {
        self.record[2] as usize
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let record = self.record.as_mut_ptr();
        unsafe {
            // Same sequence Angel uses when replacing its own slot (0x100B53E8..0x100B540C).
            let region_table_arc = self.record[4] as *const std::sync::atomic::AtomicU32;
            if (*region_table_arc).fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {
                let drop_table: unsafe extern "fastcall" fn(usize, usize) = std::mem::transmute(addr(FN_DROP_REGION_TABLE));
                drop_table(region_table_arc as usize, 0);
            }
            let release_buffer: unsafe extern "fastcall" fn(*mut u32, usize) = std::mem::transmute(addr(FN_RELEASE_DATA_BUFFER));
            release_buffer(record, 0);
        }
    }
}

/// True when called on the thread Angel recorded as the game's main thread.
pub fn on_main_thread() -> bool {
    let main_thread = read_global(GLOBAL_MAIN_THREAD_ID);
    main_thread != 0 && main_thread == unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

pub fn ready() -> bool {
    read_global(GLOBAL_BASE_INIT_ERROR) == 0 && read_global(GLOBAL_GAME_BASE) != 0
}

/// Captures the current game state with Angel's capture function.
pub fn capture() -> Result<Snapshot, String> {
    if !ready() {
        return Err("Angel's game-base initialisation is not complete".to_owned());
    }
    let mut record = Box::new([0u32; RECORD_DWORDS]);
    unsafe {
        let capture: unsafe extern "fastcall" fn(*mut u32, usize) = std::mem::transmute(addr(FN_CAPTURE));
        capture(record.as_mut_ptr(), 0);
    }
    if record[0] == FAILED {
        // A failed record owns nothing, so it must not be released.
        let kind = match record[1] {
            0 => "size overflow",
            1 => "buffer allocation failed",
            2 => "object registry unavailable",
            3 => "player object read failed",
            5 => "region read failed",
            6 => "address not readable",
            _ => "unknown error",
        };
        return Err(format!("Angel capture failed: {kind} (details {:#x} {:#x} {:#x})", record[2], record[3], record[4]));
    }
    Ok(Snapshot { record })
}

/// One (destination, offset into data buffer, length) copy instruction.
#[derive(Clone, Copy)]
struct Triple {
    dest: u32,
    offset: u32,
    len: u32,
}

/// Reads the triples of region-table entry `index` (0x14-byte small-vector entries).
unsafe fn region_triples(table: *const u32, index: usize) -> Vec<Triple> {
    let entry = table.byte_add(index * 0x14);
    let inline_len = *entry.byte_add(0x10);
    let (count, items) = if inline_len < 2 {
        (inline_len as usize, entry.byte_add(4))
    } else {
        (*entry.byte_add(4) as usize, *entry.byte_add(8) as *const u32)
    };
    (0..count)
        .map(|i| {
            let t = items.add(i * 3);
            Triple { dest: *t, offset: *t.add(1), len: *t.add(2) }
        })
        .collect()
}

/// The validator's mismatch report, raw. Its field meanings are not fully known (it checks
/// region layout, 20 watched game values, the object registry and one non-zero flag).
fn describe_validator_failure(out: &[u32; VALIDATOR_OUT_DWORDS]) -> String {
    let words: Vec<String> = out.iter().map(|w| format!("{w:#x}")).collect();
    format!("mismatch report [{}]", words.join(" "))
}

unsafe fn free_validator_output(out: &[u32; VALIDATOR_OUT_DWORDS]) {
    // Angel frees these four (capacity, pointer) vectors with HeapFree on refusal.
    for (cap, ptr) in [(0, 1), (3, 4), (6, 7), (9, 10)] {
        if out[cap] != 0 && out[cap] != FAILED && out[ptr] != 0 {
            HeapFree(GetProcessHeap(), 0, out[ptr] as *const std::ffi::c_void);
        }
    }
}

/// Restores `snapshot` exactly as Angel's training-mode load does. Returns Err (and leaves
/// memory untouched) if Angel's validator says the game state is no longer compatible.
pub fn restore(snapshot: &Snapshot) -> Result<(), String> {
    if !ready() {
        return Err("Angel's game-base initialisation is not complete".to_owned());
    }
    let record = &snapshot.record;
    let game_base = read_global(GLOBAL_GAME_BASE);
    unsafe {
        let region_table_arc = record[4] as *const u32;
        let table = *region_table_arc.byte_add(0xC) as *const u32;
        let region_count = *region_table_arc.byte_add(0x10) as usize;

        let mut excludes: Vec<(u32, u32)> =
            FIXED_EXCLUDES.iter().map(|&(s, e)| (game_base + s, game_base + e)).collect();
        if region_count > EXCLUDED_REGION_INDEX {
            for t in region_triples(table, EXCLUDED_REGION_INDEX) {
                excludes.push((t.dest, t.dest.wrapping_add(t.len)));
            }
        }
        excludes.sort_by_key(|&(start, _)| start);

        let mut out = [0u32; VALIDATOR_OUT_DWORDS];
        let validate: unsafe extern "fastcall" fn(*mut u32, *const u32) = std::mem::transmute(addr(FN_VALIDATE));
        validate(out.as_mut_ptr(), record.as_ptr());
        if out[0] != FAILED {
            let reason = describe_validator_failure(&out);
            free_validator_output(&out);
            return Err(format!("Angel's validator refused the restore: {reason}"));
        }

        let data = record[1] as *const u8;
        let data_len = record[2];
        for index in 0..region_count {
            for t in region_triples(table, index) {
                let end = t.offset.checked_add(t.len).filter(|&end| end <= data_len);
                if end.is_none() {
                    return Err(format!("corrupt snapshot: region {index} triple outside the data buffer"));
                }
                copy_skipping(t, data, &excludes);
            }
        }
        std::ptr::write_volatile(addr(GLOBAL_STATE_DIRTY) as *mut u8, 1);
    }
    Ok(())
}

/// Copies one triple back to game memory, skipping bytes covered by `excludes` (sorted).
unsafe fn copy_skipping(t: Triple, data: *const u8, excludes: &[(u32, u32)]) {
    let dest = t.dest as *mut u8;
    let src = data.add(t.offset as usize);
    let len = t.len;
    let mut pos = 0u32;
    for &(start, end) in excludes {
        let lo = if start < t.dest { 0 } else { (start - t.dest).min(len) };
        let hi = if end < t.dest { 0 } else { (end - t.dest).min(len) };
        if lo > pos {
            std::ptr::copy_nonoverlapping(src.add(pos as usize), dest.add(pos as usize), (lo - pos) as usize);
        }
        pos = pos.max(hi);
    }
    if len > pos {
        std::ptr::copy_nonoverlapping(src.add(pos as usize), dest.add(pos as usize), (len - pos) as usize);
    }
}
