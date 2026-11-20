//! Pass-through exports for XInput 1.3.
//!
//! The game imports `XInputGetState` / `XInputGetCapabilities` from xinput1_3.dll, which is
//! what gets this DLL loaded. Every export forwards unchanged to the real system copy.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

struct RealXInput {
    get_state: Option<unsafe extern "system" fn(u32, *mut c_void) -> u32>,
    set_state: Option<unsafe extern "system" fn(u32, *mut c_void) -> u32>,
    get_capabilities: Option<unsafe extern "system" fn(u32, u32, *mut c_void) -> u32>,
    enable: Option<unsafe extern "system" fn(i32)>,
    get_dsound_audio_device_guids: Option<unsafe extern "system" fn(u32, *mut c_void, *mut c_void) -> u32>,
    get_battery_information: Option<unsafe extern "system" fn(u32, u8, *mut c_void) -> u32>,
    get_keystroke: Option<unsafe extern "system" fn(u32, u32, *mut c_void) -> u32>,
    get_state_ex: Option<unsafe extern "system" fn(u32, *mut c_void) -> u32>,
    wait_for_guide_button: Option<unsafe extern "system" fn(u32, u32, *mut c_void) -> u32>,
    cancel_guide_button_wait: Option<unsafe extern "system" fn(u32) -> u32>,
    power_off_controller: Option<unsafe extern "system" fn(u32) -> u32>,
}

static REAL: OnceLock<RealXInput> = OnceLock::new();

/// Loads `<system dir>\xinput1_3.dll`. For a 32-bit process, WOW64 file-system redirection
/// maps the system directory to SysWOW64, so this is always the matching 32-bit copy.
fn load_real_module() -> HMODULE {
    let mut buf = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if len == 0 || len >= buf.len() {
        crate::log::write("proxy: GetSystemDirectoryW failed; XInput will report no controllers");
        return std::ptr::null_mut();
    }
    let path: Vec<u16> = buf[..len].iter().copied().chain("\\xinput1_3.dll\0".encode_utf16()).collect();
    let module = unsafe { LoadLibraryW(path.as_ptr()) };
    if module.is_null() {
        crate::log::write("proxy: could not load the system xinput1_3.dll; XInput will report no controllers");
    }
    module
}

fn real() -> &'static RealXInput {
    REAL.get_or_init(|| {
        let module = load_real_module();
        // Resolves an export by name (null-terminated) or by ordinal (MAKEINTRESOURCE-style pointer).
        let lookup = |name: *const u8| unsafe {
            if module.is_null() { None } else { GetProcAddress(module, name) }
        };
        let by_ordinal = |ordinal: usize| lookup(ordinal as *const u8);
        unsafe {
            RealXInput {
                get_state: std::mem::transmute(lookup(c"XInputGetState".as_ptr().cast())),
                set_state: std::mem::transmute(lookup(c"XInputSetState".as_ptr().cast())),
                get_capabilities: std::mem::transmute(lookup(c"XInputGetCapabilities".as_ptr().cast())),
                enable: std::mem::transmute(lookup(c"XInputEnable".as_ptr().cast())),
                get_dsound_audio_device_guids: std::mem::transmute(lookup(c"XInputGetDSoundAudioDeviceGuids".as_ptr().cast())),
                get_battery_information: std::mem::transmute(lookup(c"XInputGetBatteryInformation".as_ptr().cast())),
                get_keystroke: std::mem::transmute(lookup(c"XInputGetKeystroke".as_ptr().cast())),
                get_state_ex: std::mem::transmute(by_ordinal(100)),
                wait_for_guide_button: std::mem::transmute(by_ordinal(101)),
                cancel_guide_button_wait: std::mem::transmute(by_ordinal(102)),
                power_off_controller: std::mem::transmute(by_ordinal(103)),
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_get_state(user: u32, state: *mut c_void) -> u32 {
    // The game polls XInput from its input task each frame, outside the battle simulation:
    // a safe point for Reset Battle's capture/restore (checked against the main thread there).
    let enabled = crate::ENABLED.load(std::sync::atomic::Ordering::Acquire);
    if user == 0 && enabled {
        crate::takeover::on_frame_boundary();
    }
    let result = real().get_state.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, state));
    if enabled && result == 0 && !state.is_null() {
        // XINPUT_STATE: dwPacketNumber (u32), then XINPUT_GAMEPAD.wButtons (u16).
        let buttons = std::ptr::read_unaligned((state as *const u8).add(4) as *const u16);
        // XINPUT_GAMEPAD: wButtons (u16), bLeftTrigger (u8), bRightTrigger (u8).
        let left_trigger = *(state as *const u8).add(6);
        let right_trigger = *(state as *const u8).add(7);
        crate::takeover::on_xinput_buttons(user, buttons);
        crate::seek::on_xinput(user, buttons, left_trigger, right_trigger);
    }
    result
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_set_state(user: u32, vibration: *mut c_void) -> u32 {
    real().set_state.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, vibration))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_get_capabilities(user: u32, flags: u32, caps: *mut c_void) -> u32 {
    real().get_capabilities.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, flags, caps))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_enable(enable: i32) {
    if let Some(f) = real().enable {
        f(enable)
    }
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_get_dsound_audio_device_guids(user: u32, render: *mut c_void, capture: *mut c_void) -> u32 {
    real().get_dsound_audio_device_guids.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, render, capture))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_get_battery_information(user: u32, device_type: u8, info: *mut c_void) -> u32 {
    real().get_battery_information.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, device_type, info))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_get_keystroke(user: u32, reserved: u32, keystroke: *mut c_void) -> u32 {
    real().get_keystroke.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, reserved, keystroke))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_get_state_ex(user: u32, state: *mut c_void) -> u32 {
    real().get_state_ex.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, state))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_wait_for_guide_button(user: u32, flags: u32, state: *mut c_void) -> u32 {
    real().wait_for_guide_button.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user, flags, state))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_cancel_guide_button_wait(user: u32) -> u32 {
    real().cancel_guide_button_wait.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user))
}

#[no_mangle]
pub unsafe extern "system" fn proxy_xinput_power_off_controller(user: u32) -> u32 {
    real().power_off_controller.map_or(ERROR_DEVICE_NOT_CONNECTED, |f| f(user))
}
