use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

// --- xkbcommon FFI bindings (minimal set) ---

#[allow(non_camel_case_types)]
enum xkb_context {}
#[allow(non_camel_case_types)]
enum xkb_keymap {}
#[allow(non_camel_case_types)]
enum xkb_state {}

#[repr(C)]
struct xkb_rule_names {
    rules: *const c_char,
    model: *const c_char,
    layout: *const c_char,
    variant: *const c_char,
    options: *const c_char,
}

#[repr(C)]
#[allow(clippy::upper_case_acronyms)]
enum xkb_key_direction {
    UP,
    DOWN,
}

const XKB_CONTEXT_NO_FLAGS: u32 = 0;
const XKB_KEYMAP_COMPILE_NO_FLAGS: u32 = 0;

#[link(name = "xkbcommon")]
unsafe extern "C" {
    fn xkb_context_new(flags: u32) -> *mut xkb_context;
    fn xkb_context_unref(context: *mut xkb_context);
    fn xkb_keymap_new_from_names(
        context: *mut xkb_context,
        names: *const xkb_rule_names,
        flags: u32,
    ) -> *mut xkb_keymap;
    fn xkb_keymap_unref(keymap: *mut xkb_keymap);
    fn xkb_state_new(keymap: *mut xkb_keymap) -> *mut xkb_state;
    fn xkb_state_unref(state: *mut xkb_state);
    fn xkb_state_update_key(
        state: *mut xkb_state,
        key: u32,
        direction: xkb_key_direction,
    ) -> u32;
    fn xkb_state_key_get_utf8(
        state: *mut xkb_state,
        key: u32,
        buffer: *mut c_char,
        size: usize,
    ) -> i32;
}

/// Offset between evdev keycodes and XKB keycodes.
const EVDEV_OFFSET: u32 = 8;

/// xkbcommon-based key state that handles layout-aware character decoding
/// with full modifier tracking (Shift, Ctrl, Alt, Meta, CapsLock, NumLock, AltGr).
pub struct XkbState {
    context: *mut xkb_context,
    keymap: *mut xkb_keymap,
    state: *mut xkb_state,
}

// Safety: XkbState owns its raw pointers exclusively and each instance
// is used from a single thread.
unsafe impl Send for XkbState {}

impl XkbState {
    /// Create a new XkbState using the system's default keyboard layout.
    pub fn new() -> Result<Self, String> {
        let context = unsafe { xkb_context_new(XKB_CONTEXT_NO_FLAGS) };
        if context.is_null() {
            return Err("failed to create xkb context".into());
        }

        // Null pointers = use system defaults (XKB_DEFAULT_RULES env vars)
        let names = xkb_rule_names {
            rules: ptr::null(),
            model: ptr::null(),
            layout: ptr::null(),
            variant: ptr::null(),
            options: ptr::null(),
        };

        let keymap = unsafe {
            xkb_keymap_new_from_names(context, &names, XKB_KEYMAP_COMPILE_NO_FLAGS)
        };
        if keymap.is_null() {
            unsafe { xkb_context_unref(context) };
            return Err("failed to create xkb keymap".into());
        }

        let state = unsafe { xkb_state_new(keymap) };
        if state.is_null() {
            unsafe {
                xkb_keymap_unref(keymap);
                xkb_context_unref(context);
            }
            return Err("failed to create xkb state".into());
        }

        Ok(Self {
            context,
            keymap,
            state,
        })
    }

    /// Process a key event: decode the character at current modifier state,
    /// then update internal state. Returns the UTF-8 string produced by
    /// the key (empty for modifiers and special keys).
    ///
    /// `value`: 0 = release, 1 = press, 2 = repeat
    pub fn process_key(&self, evdev_code: u16, value: i32) -> String {
        let keycode = evdev_code as u32 + EVDEV_OFFSET;

        // Read character BEFORE updating state (current modifiers apply)
        let char_value = if value != 0 {
            self.key_get_utf8(keycode)
        } else {
            String::new()
        };

        // Update modifier tracking
        let direction = if value == 0 {
            xkb_key_direction::UP
        } else {
            xkb_key_direction::DOWN
        };
        unsafe { xkb_state_update_key(self.state, keycode, direction) };

        char_value
    }

    /// Simulate press+release to sync a toggled modifier (CapsLock, NumLock).
    pub fn sync_key_toggle(&self, evdev_code: u16) {
        let keycode = evdev_code as u32 + EVDEV_OFFSET;
        unsafe {
            xkb_state_update_key(self.state, keycode, xkb_key_direction::DOWN);
            xkb_state_update_key(self.state, keycode, xkb_key_direction::UP);
        }
    }

    /// Simulate key press to sync a held modifier (Shift, Ctrl, Alt, Meta).
    pub fn sync_key_down(&self, evdev_code: u16) {
        let keycode = evdev_code as u32 + EVDEV_OFFSET;
        unsafe {
            xkb_state_update_key(self.state, keycode, xkb_key_direction::DOWN);
        }
    }

    fn key_get_utf8(&self, keycode: u32) -> String {
        let mut buffer: [c_char; 16] = [0; 16];
        let len = unsafe {
            xkb_state_key_get_utf8(self.state, keycode, buffer.as_mut_ptr(), buffer.len())
        };
        if len <= 0 {
            return String::new();
        }
        let cstr = unsafe { CStr::from_ptr(buffer.as_ptr()) };
        cstr.to_string_lossy().into_owned()
    }
}

impl Drop for XkbState {
    fn drop(&mut self) {
        unsafe {
            xkb_state_unref(self.state);
            xkb_keymap_unref(self.keymap);
            xkb_context_unref(self.context);
        }
    }
}
