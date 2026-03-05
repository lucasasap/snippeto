mod config;
mod expander;
mod keymap;

use evdev::{Device, EventSummary, KeyCode};
use keymap::{ShiftState, keycode_to_char};
use std::process::Command;
use std::sync::mpsc;
use std::thread;

struct KeyEvent {
    code: KeyCode,
    value: i32,
}

fn find_keyboards() -> Vec<(std::path::PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            // Skip our own virtual keyboard
            if device
                .name()
                .is_some_and(|n| n.contains("snippeto-virtual-keyboard"))
            {
                return false;
            }
            // Must support KEY_ENTER to be considered a keyboard
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::KEY_ENTER))
        })
        .collect()
}

fn check_dependencies() {
    for cmd in &["wl-copy", "wl-paste"] {
        if Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("snippeto: {cmd} not found, install wl-clipboard");
            std::process::exit(1);
        }
    }
}

fn main() {
    check_dependencies();

    let config = match config::load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("snippeto: failed to load config: {e}");
            eprintln!("snippeto: expected config at ~/.config/snippeto/snippets.yml");
            std::process::exit(1);
        }
    };

    if config.snippets.is_empty() {
        eprintln!("snippeto: no snippets defined, exiting");
        std::process::exit(1);
    }

    eprintln!("snippeto: loaded {} snippet(s)", config.snippets.len());

    let keyboards = find_keyboards();
    if keyboards.is_empty() {
        eprintln!("snippeto: no keyboard devices found");
        eprintln!("snippeto: are you in the 'input' group?");
        std::process::exit(1);
    }

    eprintln!(
        "snippeto: found {} keyboard device(s)",
        keyboards.len()
    );

    let mut expander = match expander::Expander::new(config.snippets) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("snippeto: failed to create virtual keyboard: {e}");
            eprintln!("snippeto: is the uinput module loaded? (modprobe uinput)");
            std::process::exit(1);
        }
    };

    let (tx, rx) = mpsc::channel::<KeyEvent>();

    for (path, mut device) in keyboards {
        let tx = tx.clone();
        let name = device.name().unwrap_or("unknown").to_string();
        eprintln!("snippeto: monitoring {name} at {}", path.display());

        thread::spawn(move || loop {
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        if let EventSummary::Key(_, code, value) = event.destructure() {
                            let _ = tx.send(KeyEvent { code, value });
                        }
                    }
                }
                Err(e) => {
                    eprintln!("snippeto: device error on {name}: {e}");
                    break;
                }
            }
        });
    }

    drop(tx);

    let mut shift = ShiftState::new();

    eprintln!("snippeto: running");

    for event in rx {
        // Update shift state (must happen before value filter for release events)
        if shift.update(event.code, event.value) {
            continue;
        }

        // Only process key press, ignore release and repeat
        if event.value != 1 {
            continue;
        }

        match event.code {
            KeyCode::KEY_BACKSPACE => {
                expander.pop_char();
                continue;
            }
            KeyCode::KEY_ENTER
            | KeyCode::KEY_TAB
            | KeyCode::KEY_ESC
            | KeyCode::KEY_LEFTCTRL
            | KeyCode::KEY_RIGHTCTRL
            | KeyCode::KEY_LEFTALT
            | KeyCode::KEY_RIGHTALT
            | KeyCode::KEY_LEFTMETA
            | KeyCode::KEY_RIGHTMETA => {
                expander.clear_buffer();
                continue;
            }
            _ => {}
        }

        if let Some((lower, upper)) = keycode_to_char(event.code) {
            let ch = if shift.active() { upper } else { lower };
            expander.push_char(ch);
        }
    }

    eprintln!("snippeto: all devices disconnected, exiting");
}
