use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::io::{self, Read, Write};
use std::os::raw::c_char;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::keymap::{EVDEV_OFFSET, ffi};

const DEFAULT_KEY_DELAY_MS: u64 = 5;
const DEFAULT_PASTE_DELAY_MS: u64 = 50;
const DEFAULT_CLIPBOARD_TIMEOUT_MS: u64 = 2000;
const DEFAULT_RELEASE_POLL_MS: u64 = 50;
const DEFAULT_RELEASE_TIMEOUT_MS: u64 = 4000;
const COMMAND_POLL_INTERVAL_MS: u64 = 10;
const DEFAULT_WAYLAND_PASTE_SHORTCUT: &str = "shift+insert";

const MAX_VIRTUAL_KEYCODE: u16 = 255;
const MIN_XKB_KEYCODE: u32 = EVDEV_OFFSET;
const MAX_XKB_KEYCODE: u32 = 256;

const MODIFIER_KEYS: [u16; 10] = [
    KeyCode::KEY_LEFTSHIFT.code(),
    KeyCode::KEY_RIGHTSHIFT.code(),
    KeyCode::KEY_LEFTCTRL.code(),
    KeyCode::KEY_RIGHTCTRL.code(),
    KeyCode::KEY_LEFTALT.code(),
    KeyCode::KEY_RIGHTALT.code(),
    KeyCode::KEY_LEFTMETA.code(),
    KeyCode::KEY_RIGHTMETA.code(),
    KeyCode::KEY_CAPSLOCK.code(),
    KeyCode::KEY_NUMLOCK.code(),
];

#[derive(Clone, Debug)]
struct KeyRecord {
    evdev_code: u16,
    modifiers: Vec<u16>,
}

pub struct Injector {
    virtual_kbd: VirtualDevice,
    char_map: HashMap<String, KeyRecord>,
    pressed_keys: Arc<Mutex<HashSet<u16>>>,
    has_clipboard: bool,
    key_delay: Duration,
    paste_delay: Duration,
    clipboard_timeout: Duration,
    release_poll: Duration,
    release_timeout: Duration,
    paste_shortcut: Vec<u16>,
}

impl Injector {
    pub fn new(pressed_keys: Arc<Mutex<HashSet<u16>>>, has_clipboard: bool) -> io::Result<Self> {
        let virtual_kbd = Self::create_virtual_keyboard()?;
        // Give uinput time to register the device.
        thread::sleep(Duration::from_millis(200));

        let char_map = generate_char_map().map_err(io::Error::other)?;
        if char_map.is_empty() {
            return Err(io::Error::other("generated an empty character map"));
        }

        Ok(Self {
            virtual_kbd,
            char_map,
            pressed_keys,
            has_clipboard,
            key_delay: duration_from_env("SNIPPETO_KEY_DELAY_MS", DEFAULT_KEY_DELAY_MS),
            paste_delay: duration_from_env("SNIPPETO_PASTE_DELAY_MS", DEFAULT_PASTE_DELAY_MS),
            clipboard_timeout: duration_from_env(
                "SNIPPETO_CLIPBOARD_TIMEOUT_MS",
                DEFAULT_CLIPBOARD_TIMEOUT_MS,
            ),
            release_poll: duration_from_env("SNIPPETO_RELEASE_POLL_MS", DEFAULT_RELEASE_POLL_MS),
            release_timeout: duration_from_env(
                "SNIPPETO_RELEASE_TIMEOUT_MS",
                DEFAULT_RELEASE_TIMEOUT_MS,
            ),
            paste_shortcut: paste_shortcut_from_env(),
        })
    }

    fn create_virtual_keyboard() -> io::Result<VirtualDevice> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in 0..=MAX_VIRTUAL_KEYCODE {
            keys.insert(KeyCode::new(code));
        }

        VirtualDevice::builder()?
            .name("snippeto-virtual-keyboard")
            .with_keys(&keys)?
            .build()
    }

    pub fn expand(&mut self, trigger_len: usize, replacement: &str) -> io::Result<()> {
        self.wait_for_key_release();

        if self.can_type_text(replacement) {
            self.send_backspaces(trigger_len)?;
            self.type_string(replacement)?;
            return Ok(());
        }

        if !self.has_clipboard {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "replacement includes unmappable characters and clipboard fallback is unavailable",
            ));
        }

        let saved_clipboard = self.get_clipboard();

        let inject_result = (|| -> io::Result<()> {
            self.send_backspaces(trigger_len)?;
            self.set_clipboard(replacement)?;
            thread::sleep(self.paste_delay);
            self.send_paste()?;
            Ok(())
        })();

        // Wait before restoring so the paste target can consume clipboard data first.
        if saved_clipboard.is_some() {
            thread::sleep(self.paste_delay);
        }

        if let Some(saved) = saved_clipboard
            && let Err(error) = self.set_clipboard(&saved)
        {
            eprintln!("snippeto: failed to restore clipboard: {error}");
        }

        inject_result
    }

    fn can_type_text(&self, text: &str) -> bool {
        text.chars()
            .all(|ch| self.char_map.contains_key(&ch.to_string()))
    }

    fn type_string(&mut self, text: &str) -> io::Result<()> {
        let mut active_modifiers = Vec::new();

        for ch in text.chars() {
            let key = self
                .char_map
                .get(&ch.to_string())
                .cloned()
                .ok_or_else(|| io::Error::other(format!("no key mapping for `{ch}`")))?;
            self.update_modifiers(&mut active_modifiers, &key.modifiers)?;
            self.tap_key(key.evdev_code)?;
        }

        for modifier in active_modifiers.into_iter().rev() {
            self.key_up(modifier)?;
        }

        Ok(())
    }

    fn update_modifiers(&mut self, active: &mut Vec<u16>, target: &[u16]) -> io::Result<()> {
        for modifier in active.iter().rev() {
            if !target.contains(modifier) {
                self.key_up(*modifier)?;
            }
        }

        active.retain(|modifier| target.contains(modifier));

        for modifier in target {
            if !active.contains(modifier) {
                self.key_down(*modifier)?;
                active.push(*modifier);
            }
        }

        Ok(())
    }

    fn send_backspaces(&mut self, count: usize) -> io::Result<()> {
        for _ in 0..count {
            self.tap_key(KeyCode::KEY_BACKSPACE.code())?;
        }
        Ok(())
    }

    fn send_paste(&mut self) -> io::Result<()> {
        let combo = self.paste_shortcut.clone();
        let Some((main_key, modifiers)) = combo.split_last() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "paste shortcut cannot be empty",
            ));
        };

        for modifier in modifiers {
            self.key_down(*modifier)?;
        }

        let tap_result = self.tap_key(*main_key);
        let mut release_error = None;
        for modifier in modifiers.iter().rev() {
            if let Err(error) = self.key_up(*modifier)
                && release_error.is_none()
            {
                release_error = Some(error);
            }
        }

        tap_result?;
        if let Some(error) = release_error {
            return Err(error);
        }

        Ok(())
    }

    fn tap_key(&mut self, code: u16) -> io::Result<()> {
        self.key_down(code)?;
        self.key_up(code)
    }

    fn key_down(&mut self, code: u16) -> io::Result<()> {
        self.emit_key(code, 1)
    }

    fn key_up(&mut self, code: u16) -> io::Result<()> {
        self.emit_key(code, 0)
    }

    fn emit_key(&mut self, code: u16, value: i32) -> io::Result<()> {
        let event = InputEvent::new(EventType::KEY.0, code, value);
        self.virtual_kbd.emit(&[event])?;
        thread::sleep(self.key_delay);
        Ok(())
    }

    fn wait_for_key_release(&self) {
        let start = Instant::now();

        loop {
            let has_pressed_keys = self
                .pressed_keys
                .lock()
                .map(|keys| !keys.is_empty())
                .unwrap_or(false);
            if !has_pressed_keys {
                return;
            }

            if start.elapsed() >= self.release_timeout {
                eprintln!("snippeto: timeout waiting for keys to be released, injecting anyway");
                return;
            }

            thread::sleep(self.release_poll);
        }
    }

    fn get_clipboard(&self) -> Option<String> {
        match self.get_clipboard_with_timeout() {
            Ok(text) => Some(text),
            Err(error) => {
                eprintln!("snippeto: failed to read clipboard: {error}");
                None
            }
        }
    }

    fn get_clipboard_with_timeout(&self) -> io::Result<String> {
        let mut child = Command::new("wl-paste")
            .arg("--no-newline")
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| io::Error::other(format!("failed to run wl-paste: {e}")))?;

        let status = wait_for_child_with_timeout(&mut child, self.clipboard_timeout, "wl-paste")?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "wl-paste exited unsuccessfully: {status}"
            )));
        }

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("wl-paste did not expose stdout"))?;
        let mut output = Vec::new();
        stdout.read_to_end(&mut output)?;
        String::from_utf8(output)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("utf8 error: {e}")))
    }

    fn set_clipboard(&self, text: &str) -> io::Result<()> {
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| io::Error::other(format!("failed to run wl-copy: {e}")))?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        // Close stdin so wl-copy can complete after consuming the payload.
        drop(child.stdin.take());

        let status = wait_for_child_with_timeout(&mut child, self.clipboard_timeout, "wl-copy")?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "wl-copy exited unsuccessfully: {status}"
            )))
        }
    }
}

fn wait_for_child_with_timeout(
    child: &mut Child,
    timeout: Duration,
    command_name: &str,
) -> io::Result<ExitStatus> {
    let start = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if start.elapsed() >= timeout {
            eprintln!(
                "snippeto: {command_name} timed out after {}ms, killing process",
                timeout.as_millis()
            );

            if let Err(error) = child.kill() {
                eprintln!("snippeto: failed to kill {command_name}: {error}");
            }

            if let Err(error) = child.wait() {
                eprintln!("snippeto: failed to reap {command_name}: {error}");
            }

            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{command_name} timed out"),
            ));
        }

        thread::sleep(Duration::from_millis(COMMAND_POLL_INTERVAL_MS));
    }
}

fn paste_shortcut_from_env() -> Vec<u16> {
    let default = vec![KeyCode::KEY_LEFTSHIFT.code(), KeyCode::KEY_INSERT.code()];
    let Ok(raw) = std::env::var("SNIPPETO_PASTE_SHORTCUT") else {
        return default;
    };

    match parse_paste_shortcut(&raw) {
        Some(combo) => combo,
        None => {
            eprintln!(
                "snippeto: invalid SNIPPETO_PASTE_SHORTCUT `{raw}`, using {DEFAULT_WAYLAND_PASTE_SHORTCUT}"
            );
            default
        }
    }
}

fn parse_paste_shortcut(raw: &str) -> Option<Vec<u16>> {
    let mut combo = Vec::new();
    for token in raw.split('+') {
        let key = parse_paste_shortcut_token(token.trim())?;
        combo.push(key);
    }

    if combo.is_empty() { None } else { Some(combo) }
}

fn parse_paste_shortcut_token(token: &str) -> Option<u16> {
    let token = token.to_ascii_lowercase();
    Some(match token.as_str() {
        "ctrl" | "control" | "leftctrl" => KeyCode::KEY_LEFTCTRL.code(),
        "rightctrl" => KeyCode::KEY_RIGHTCTRL.code(),
        "shift" | "leftshift" => KeyCode::KEY_LEFTSHIFT.code(),
        "rightshift" => KeyCode::KEY_RIGHTSHIFT.code(),
        "alt" | "leftalt" => KeyCode::KEY_LEFTALT.code(),
        "rightalt" | "altgr" => KeyCode::KEY_RIGHTALT.code(),
        "meta" | "super" | "win" | "leftmeta" => KeyCode::KEY_LEFTMETA.code(),
        "rightmeta" => KeyCode::KEY_RIGHTMETA.code(),
        "insert" | "ins" => KeyCode::KEY_INSERT.code(),
        "v" => KeyCode::KEY_V.code(),
        _ => return None,
    })
}

fn generate_char_map() -> Result<HashMap<String, KeyRecord>, String> {
    let context = unsafe { ffi::xkb_context_new(ffi::XKB_CONTEXT_NO_FLAGS) };
    if context.is_null() {
        return Err("failed to create xkb context".into());
    }

    let names = ffi::xkb_rule_names {
        rules: std::ptr::null(),
        model: std::ptr::null(),
        layout: std::ptr::null(),
        variant: std::ptr::null(),
        options: std::ptr::null(),
    };

    let keymap = unsafe {
        ffi::xkb_keymap_new_from_names(context, &names, ffi::XKB_KEYMAP_COMPILE_NO_FLAGS)
    };
    if keymap.is_null() {
        unsafe {
            ffi::xkb_context_unref(context);
        }
        return Err("failed to create xkb keymap".into());
    }

    let mut char_map = HashMap::new();
    for combo in generate_modifier_combos() {
        let state = unsafe { ffi::xkb_state_new(keymap) };
        if state.is_null() {
            unsafe {
                ffi::xkb_keymap_unref(keymap);
                ffi::xkb_context_unref(context);
            }
            return Err("failed to create xkb state".into());
        }

        apply_modifier_combo(state, &combo);

        for xkb_code in MIN_XKB_KEYCODE..=MAX_XKB_KEYCODE {
            let value = key_get_utf8(state, xkb_code);
            if value.is_empty() || value.chars().count() != 1 {
                continue;
            }

            let evdev_code = (xkb_code - EVDEV_OFFSET) as u16;
            char_map.entry(value).or_insert_with(|| KeyRecord {
                evdev_code,
                modifiers: combo.clone(),
            });
        }

        unsafe {
            ffi::xkb_state_unref(state);
        }
    }

    unsafe {
        ffi::xkb_keymap_unref(keymap);
        ffi::xkb_context_unref(context);
    }

    Ok(char_map)
}

fn generate_modifier_combos() -> Vec<Vec<u16>> {
    let mut combos = Vec::with_capacity(176);

    combos.push(Vec::new());

    for i in 0..MODIFIER_KEYS.len() {
        combos.push(vec![MODIFIER_KEYS[i]]);
    }

    for i in 0..MODIFIER_KEYS.len() {
        for j in (i + 1)..MODIFIER_KEYS.len() {
            combos.push(vec![MODIFIER_KEYS[i], MODIFIER_KEYS[j]]);
        }
    }

    for i in 0..MODIFIER_KEYS.len() {
        for j in (i + 1)..MODIFIER_KEYS.len() {
            for k in (j + 1)..MODIFIER_KEYS.len() {
                combos.push(vec![MODIFIER_KEYS[i], MODIFIER_KEYS[j], MODIFIER_KEYS[k]]);
            }
        }
    }

    combos
}

fn apply_modifier_combo(state: *mut ffi::xkb_state, combo: &[u16]) {
    for modifier in combo {
        let xkb_code = u32::from(*modifier) + EVDEV_OFFSET;
        if *modifier == KeyCode::KEY_CAPSLOCK.code() || *modifier == KeyCode::KEY_NUMLOCK.code() {
            unsafe {
                ffi::xkb_state_update_key(state, xkb_code, ffi::xkb_key_direction::DOWN);
                ffi::xkb_state_update_key(state, xkb_code, ffi::xkb_key_direction::UP);
            }
        } else {
            unsafe {
                ffi::xkb_state_update_key(state, xkb_code, ffi::xkb_key_direction::DOWN);
            }
        }
    }
}

fn key_get_utf8(state: *mut ffi::xkb_state, keycode: u32) -> String {
    let mut buffer: [c_char; 16] = [0; 16];
    let len =
        unsafe { ffi::xkb_state_key_get_utf8(state, keycode, buffer.as_mut_ptr(), buffer.len()) };
    if len <= 0 {
        return String::new();
    }

    let cstr = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    cstr.to_string_lossy().into_owned()
}

fn duration_from_env(name: &str, default_ms: u64) -> Duration {
    let Ok(raw) = std::env::var(name) else {
        return Duration::from_millis(default_ms);
    };

    match raw.parse::<u64>() {
        Ok(value) => Duration::from_millis(value),
        Err(_) => {
            eprintln!("snippeto: invalid value for {name}: `{raw}`, using {default_ms}ms");
            Duration::from_millis(default_ms)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{generate_modifier_combos, parse_paste_shortcut};
    use evdev::KeyCode;

    #[test]
    fn modifier_combo_generation_matches_expected_size() {
        let combos = generate_modifier_combos();
        assert_eq!(combos.len(), 176);
        assert!(combos.iter().any(|combo| combo.is_empty()));
        assert!(combos.iter().all(|combo| combo.len() <= 3));
    }

    #[test]
    fn parses_shift_insert_paste_shortcut() {
        assert_eq!(
            parse_paste_shortcut("Shift+Insert"),
            Some(vec![
                KeyCode::KEY_LEFTSHIFT.code(),
                KeyCode::KEY_INSERT.code()
            ])
        );
    }

    #[test]
    fn parses_ctrl_v_paste_shortcut() {
        assert_eq!(
            parse_paste_shortcut("ctrl+v"),
            Some(vec![KeyCode::KEY_LEFTCTRL.code(), KeyCode::KEY_V.code()])
        );
    }

    #[test]
    fn rejects_invalid_paste_shortcut() {
        assert!(parse_paste_shortcut("ctrl+banana").is_none());
    }
}
