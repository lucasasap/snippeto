use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use std::thread;
use std::time::Duration;

const MAX_BUFFER_SIZE: usize = 128;
const KEY_DELAY: Duration = Duration::from_millis(5);
const PASTE_DELAY: Duration = Duration::from_millis(50);

pub struct Expander {
    buffer: String,
    snippets: HashMap<String, String>,
    virtual_kbd: VirtualDevice,
}

impl Expander {
    pub fn new(snippets: HashMap<String, String>) -> std::io::Result<Self> {
        let virtual_kbd = Self::create_virtual_keyboard()?;
        // Give uinput time to register the device
        thread::sleep(Duration::from_millis(200));
        Ok(Self {
            buffer: String::with_capacity(MAX_BUFFER_SIZE),
            snippets,
            virtual_kbd,
        })
    }

    fn create_virtual_keyboard() -> std::io::Result<VirtualDevice> {
        let mut keys = AttributeSet::<KeyCode>::new();
        keys.insert(KeyCode::KEY_BACKSPACE);
        keys.insert(KeyCode::KEY_LEFTCTRL);
        keys.insert(KeyCode::KEY_V);

        VirtualDevice::builder()?
            .name("snippeto-virtual-keyboard")
            .with_keys(&keys)?
            .build()
    }

    /// Process a typed character. Returns true if an expansion was triggered.
    pub fn push_char(&mut self, ch: char) {
        self.buffer.push(ch);

        if self.buffer.len() > MAX_BUFFER_SIZE {
            let excess = self.buffer.len() - MAX_BUFFER_SIZE;
            self.buffer.drain(..excess);
        }

        if let Some((trigger_len, replacement)) = self.find_match() {
            self.expand(trigger_len, &replacement);
            self.buffer.clear();
        }
    }

    fn find_match(&self) -> Option<(usize, String)> {
        for (trigger, replacement) in &self.snippets {
            if self.buffer.ends_with(trigger.as_str()) {
                return Some((trigger.chars().count(), replacement.clone()));
            }
        }
        None
    }

    fn expand(&mut self, trigger_len: usize, replacement: &str) {
        let saved_clipboard = self.get_clipboard();

        self.send_backspaces(trigger_len);
        self.set_clipboard(replacement);
        thread::sleep(PASTE_DELAY);
        self.send_paste();
        thread::sleep(PASTE_DELAY);

        if let Some(ref saved) = saved_clipboard {
            self.set_clipboard(saved);
        }
    }

    fn send_backspaces(&mut self, count: usize) {
        for _ in 0..count {
            let down = InputEvent::new(EventType::KEY.0, KeyCode::KEY_BACKSPACE.code(), 1);
            self.virtual_kbd.emit(&[down]).ok();
            thread::sleep(KEY_DELAY);

            let up = InputEvent::new(EventType::KEY.0, KeyCode::KEY_BACKSPACE.code(), 0);
            self.virtual_kbd.emit(&[up]).ok();
            thread::sleep(KEY_DELAY);
        }
    }

    fn send_paste(&mut self) {
        let ctrl_down = InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 1);
        let v_down = InputEvent::new(EventType::KEY.0, KeyCode::KEY_V.code(), 1);
        let v_up = InputEvent::new(EventType::KEY.0, KeyCode::KEY_V.code(), 0);
        let ctrl_up = InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.code(), 0);

        self.virtual_kbd.emit(&[ctrl_down]).ok();
        thread::sleep(KEY_DELAY);
        self.virtual_kbd.emit(&[v_down]).ok();
        thread::sleep(KEY_DELAY);
        self.virtual_kbd.emit(&[v_up]).ok();
        thread::sleep(KEY_DELAY);
        self.virtual_kbd.emit(&[ctrl_up]).ok();
        thread::sleep(KEY_DELAY);
    }

    fn get_clipboard(&self) -> Option<String> {
        Command::new("wl-paste")
            .arg("--no-newline")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok()
                } else {
                    None
                }
            })
    }

    fn set_clipboard(&self, text: &str) {
        let mut child = match Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("snippeto: failed to run wl-copy: {e}");
                return;
            }
        };
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes()).ok();
        }
        child.wait().ok();
    }

    pub fn pop_char(&mut self) {
        self.buffer.pop();
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
    }
}
