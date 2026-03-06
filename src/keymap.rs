use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

pub(crate) mod ffi {
    use std::os::raw::c_char;

    #[allow(non_camel_case_types)]
    pub enum xkb_context {}
    #[allow(non_camel_case_types)]
    pub enum xkb_keymap {}
    #[allow(non_camel_case_types)]
    pub enum xkb_state {}

    #[repr(C)]
    pub struct xkb_rule_names {
        pub rules: *const c_char,
        pub model: *const c_char,
        pub layout: *const c_char,
        pub variant: *const c_char,
        pub options: *const c_char,
    }

    #[repr(C)]
    #[allow(clippy::upper_case_acronyms)]
    pub enum xkb_key_direction {
        UP,
        DOWN,
    }

    pub const XKB_CONTEXT_NO_FLAGS: u32 = 0;
    pub const XKB_KEYMAP_COMPILE_NO_FLAGS: u32 = 0;

    #[link(name = "xkbcommon")]
    unsafe extern "C" {
        pub fn xkb_context_new(flags: u32) -> *mut xkb_context;
        pub fn xkb_context_unref(context: *mut xkb_context);
        pub fn xkb_keymap_new_from_names(
            context: *mut xkb_context,
            names: *const xkb_rule_names,
            flags: u32,
        ) -> *mut xkb_keymap;
        pub fn xkb_keymap_unref(keymap: *mut xkb_keymap);
        pub fn xkb_state_new(keymap: *mut xkb_keymap) -> *mut xkb_state;
        pub fn xkb_state_unref(state: *mut xkb_state);
        pub fn xkb_state_update_key(
            state: *mut xkb_state,
            key: u32,
            direction: xkb_key_direction,
        ) -> u32;
        pub fn xkb_state_key_get_utf8(
            state: *mut xkb_state,
            key: u32,
            buffer: *mut c_char,
            size: usize,
        ) -> i32;
    }
}

/// Offset between evdev keycodes and XKB keycodes.
pub(crate) const EVDEV_OFFSET: u32 = 8;

/// xkbcommon-based key state that handles layout-aware character decoding
/// with full modifier tracking (Shift, Ctrl, Alt, Meta, CapsLock, NumLock, AltGr).
pub struct XkbState {
    context: *mut ffi::xkb_context,
    keymap: *mut ffi::xkb_keymap,
    state: *mut ffi::xkb_state,
}

// Safety: XkbState owns its raw pointers exclusively and each instance
// is used from a single thread.
unsafe impl Send for XkbState {}

impl XkbState {
    /// Create a new XkbState using the system's default keyboard layout.
    pub fn new() -> Result<Self, String> {
        let context = unsafe { ffi::xkb_context_new(ffi::XKB_CONTEXT_NO_FLAGS) };
        if context.is_null() {
            return Err("failed to create xkb context".into());
        }

        // Null pointers = use system defaults (XKB_DEFAULT_RULES env vars)
        let names = ffi::xkb_rule_names {
            rules: ptr::null(),
            model: ptr::null(),
            layout: ptr::null(),
            variant: ptr::null(),
            options: ptr::null(),
        };

        let keymap = unsafe {
            ffi::xkb_keymap_new_from_names(context, &names, ffi::XKB_KEYMAP_COMPILE_NO_FLAGS)
        };
        if keymap.is_null() {
            unsafe { ffi::xkb_context_unref(context) };
            return Err("failed to create xkb keymap".into());
        }

        let state = unsafe { ffi::xkb_state_new(keymap) };
        if state.is_null() {
            unsafe {
                ffi::xkb_keymap_unref(keymap);
                ffi::xkb_context_unref(context);
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
            ffi::xkb_key_direction::UP
        } else {
            ffi::xkb_key_direction::DOWN
        };
        unsafe { ffi::xkb_state_update_key(self.state, keycode, direction) };

        char_value
    }

    /// Simulate press+release to sync a toggled modifier (CapsLock, NumLock).
    pub fn sync_key_toggle(&self, evdev_code: u16) {
        let keycode = evdev_code as u32 + EVDEV_OFFSET;
        unsafe {
            ffi::xkb_state_update_key(self.state, keycode, ffi::xkb_key_direction::DOWN);
            ffi::xkb_state_update_key(self.state, keycode, ffi::xkb_key_direction::UP);
        }
    }

    /// Simulate key press to sync a held modifier (Shift, Ctrl, Alt, Meta).
    pub fn sync_key_down(&self, evdev_code: u16) {
        let keycode = evdev_code as u32 + EVDEV_OFFSET;
        unsafe {
            ffi::xkb_state_update_key(self.state, keycode, ffi::xkb_key_direction::DOWN);
        }
    }

    fn key_get_utf8(&self, keycode: u32) -> String {
        let mut buffer: [c_char; 16] = [0; 16];
        let len = unsafe {
            ffi::xkb_state_key_get_utf8(self.state, keycode, buffer.as_mut_ptr(), buffer.len())
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
            ffi::xkb_state_unref(self.state);
            ffi::xkb_keymap_unref(self.keymap);
            ffi::xkb_context_unref(self.context);
        }
    }
}
