mod config;
mod expander;
mod injector;
mod keymap;
mod snippet;

use evdev::{Device, EventSummary, KeyCode, LedCode};
use keymap::XkbState;
use snippet::Snippet;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime};

const CONFIG_WATCH_INTERVAL_MS: u64 = 500;
const DEVICE_POLL_INTERVAL_MS: u64 = 10;
const WORKER_LOOP_POLL_INTERVAL_MS: u64 = 50;
const WORKER_RESPAWN_DELAY_MS: u64 = 1000;

struct KeyEvent {
    code: KeyCode,
    value: i32,
    char_value: String,
}

enum WorkerCommand {
    Shutdown,
}

enum WorkerExit {
    ShutdownRequested,
    StartupFailure(String),
    AllInputStreamsClosed,
}

impl std::fmt::Display for WorkerExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShutdownRequested => write!(f, "shutdown requested"),
            Self::StartupFailure(error) => write!(f, "startup failure: {error}"),
            Self::AllInputStreamsClosed => write!(f, "all input streams closed"),
        }
    }
}

enum SupervisorEvent {
    ConfigChanged,
    WorkerExited { worker_id: u64, exit: WorkerExit },
}

struct WorkerHandle {
    id: u64,
    command_tx: mpsc::Sender<WorkerCommand>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ConfigFingerprint {
    exists: bool,
    modified: Option<SystemTime>,
    len: u64,
}

fn find_keyboards() -> Vec<(PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            // Skip our own virtual keyboard.
            if device
                .name()
                .is_some_and(|n| n.contains("snippeto-virtual-keyboard"))
            {
                return false;
            }

            // Must support KEY_ENTER to be considered a keyboard.
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::KEY_ENTER))
        })
        .collect()
}

fn check_clipboard_available() -> bool {
    let has_wl_copy = Command::new("which")
        .arg("wl-copy")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    let has_wl_paste = Command::new("which")
        .arg("wl-paste")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    if has_wl_copy && has_wl_paste {
        true
    } else {
        eprintln!(
            "snippeto: wl-clipboard not fully available; typed injection will still work, but unmappable characters cannot use clipboard fallback"
        );
        false
    }
}

fn sync_initial_modifiers(device: &Device, xkb: &XkbState) {
    // Sync toggled modifiers via LED state.
    if let Ok(leds) = device.get_led_state() {
        if leds.contains(LedCode::LED_CAPSL) {
            xkb.sync_key_toggle(KeyCode::KEY_CAPSLOCK.code());
        }
        if leds.contains(LedCode::LED_NUML) {
            xkb.sync_key_toggle(KeyCode::KEY_NUMLOCK.code());
        }
    }

    // Sync held modifiers via key state.
    if let Ok(keys) = device.get_key_state() {
        for key in [
            KeyCode::KEY_LEFTSHIFT,
            KeyCode::KEY_RIGHTSHIFT,
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_RIGHTCTRL,
            KeyCode::KEY_LEFTALT,
            KeyCode::KEY_RIGHTALT,
            KeyCode::KEY_LEFTMETA,
            KeyCode::KEY_RIGHTMETA,
        ] {
            if keys.contains(key) {
                xkb.sync_key_down(key.code());
            }
        }
    }
}

fn main() {
    let has_clipboard = check_clipboard_available();

    let mut snippets = match config::load_config() {
        Ok(c) => c.snippets,
        Err(e) => {
            eprintln!("snippeto: failed to load config: {e}");
            eprintln!("snippeto: expected config at ~/.config/snippeto/snippets.yml");
            std::process::exit(1);
        }
    };

    if snippets.is_empty() {
        eprintln!("snippeto: no snippets defined, exiting");
        std::process::exit(1);
    }

    eprintln!("snippeto: loaded {} snippet(s)", snippets.len());

    let (supervisor_tx, supervisor_rx) = mpsc::channel::<SupervisorEvent>();
    spawn_config_watcher(config::config_path(), supervisor_tx.clone());

    let mut next_worker_id = 1;
    let mut worker = spawn_worker(
        next_worker_id,
        snippets.clone(),
        has_clipboard,
        supervisor_tx.clone(),
    );
    next_worker_id += 1;

    let mut restart_pending = false;
    let mut pending_snippets: Option<Vec<Snippet>> = None;

    eprintln!("snippeto: supervisor running");

    loop {
        let event = match supervisor_rx.recv() {
            Ok(event) => event,
            Err(_) => break,
        };

        match event {
            SupervisorEvent::ConfigChanged => match config::load_config() {
                Ok(config) => {
                    if config.snippets.is_empty() {
                        eprintln!("snippeto: config reload ignored: no snippets defined");
                        continue;
                    }

                    eprintln!("snippeto: config change detected, restarting worker...");
                    pending_snippets = Some(config.snippets);

                    if !restart_pending {
                        restart_pending = true;
                        if worker.command_tx.send(WorkerCommand::Shutdown).is_err() {
                            eprintln!(
                                "snippeto: could not signal worker shutdown, waiting for worker exit"
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!("snippeto: config reload failed, keeping current config: {error}");
                }
            },
            SupervisorEvent::WorkerExited { worker_id, exit } => {
                if worker_id != worker.id {
                    continue;
                }

                if restart_pending {
                    restart_pending = false;

                    if let Some(updated_snippets) = pending_snippets.take() {
                        snippets = updated_snippets;
                        eprintln!("snippeto: loaded {} snippet(s)", snippets.len());
                    }

                    if !matches!(exit, WorkerExit::ShutdownRequested) {
                        eprintln!("snippeto: worker exited during reload ({exit}), restarting...");
                    }
                } else {
                    eprintln!("snippeto: worker exited unexpectedly ({exit}), restarting...");
                    thread::sleep(Duration::from_millis(WORKER_RESPAWN_DELAY_MS));
                }

                worker = spawn_worker(
                    next_worker_id,
                    snippets.clone(),
                    has_clipboard,
                    supervisor_tx.clone(),
                );
                next_worker_id += 1;
            }
        }
    }
}

fn spawn_config_watcher(config_path: PathBuf, supervisor_tx: mpsc::Sender<SupervisorEvent>) {
    thread::spawn(move || {
        let mut fingerprint = config_fingerprint(&config_path);

        loop {
            thread::sleep(Duration::from_millis(CONFIG_WATCH_INTERVAL_MS));

            let current = config_fingerprint(&config_path);
            if current != fingerprint {
                fingerprint = current;

                if supervisor_tx.send(SupervisorEvent::ConfigChanged).is_err() {
                    break;
                }
            }
        }
    });
}

fn config_fingerprint(path: &Path) -> ConfigFingerprint {
    match std::fs::metadata(path) {
        Ok(metadata) => ConfigFingerprint {
            exists: true,
            modified: metadata.modified().ok(),
            len: metadata.len(),
        },
        Err(_) => ConfigFingerprint {
            exists: false,
            modified: None,
            len: 0,
        },
    }
}

fn spawn_worker(
    worker_id: u64,
    snippets: Vec<Snippet>,
    has_clipboard: bool,
    supervisor_tx: mpsc::Sender<SupervisorEvent>,
) -> WorkerHandle {
    let (command_tx, command_rx) = mpsc::channel();

    thread::spawn(move || {
        let exit = worker_main(snippets, has_clipboard, command_rx);
        let _ = supervisor_tx.send(SupervisorEvent::WorkerExited { worker_id, exit });
    });

    WorkerHandle {
        id: worker_id,
        command_tx,
    }
}

fn worker_main(
    snippets: Vec<Snippet>,
    has_clipboard: bool,
    command_rx: mpsc::Receiver<WorkerCommand>,
) -> WorkerExit {
    let keyboards = find_keyboards();
    if keyboards.is_empty() {
        return WorkerExit::StartupFailure(
            "no keyboard devices found (are you in the 'input' group?)".to_string(),
        );
    }

    eprintln!("snippeto: found {} keyboard device(s)", keyboards.len());

    let pressed_keys = Arc::new(Mutex::new(HashSet::new()));

    let mut expander = match expander::Expander::new(snippets, pressed_keys.clone(), has_clipboard)
    {
        Ok(expander) => expander,
        Err(error) => {
            return WorkerExit::StartupFailure(format!(
                "failed to initialize expander: {error} (is uinput loaded?)"
            ));
        }
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let (event_tx, event_rx) = mpsc::channel::<KeyEvent>();
    let mut keyboard_threads = Vec::new();

    for (path, device) in keyboards {
        if let Err(error) = device.set_nonblocking(true) {
            eprintln!(
                "snippeto: failed to enable nonblocking mode on {}: {error}",
                path.display()
            );
            continue;
        }

        let xkb = match XkbState::new() {
            Ok(state) => state,
            Err(error) => {
                eprintln!("snippeto: failed to create xkb state: {error}");
                continue;
            }
        };

        sync_initial_modifiers(&device, &xkb);

        let name = device.name().unwrap_or("unknown").to_string();
        let pressed_keys = pressed_keys.clone();
        let stop_flag = stop_flag.clone();
        let event_tx = event_tx.clone();

        eprintln!("snippeto: monitoring {name} at {}", path.display());

        let handle = thread::spawn(move || {
            keyboard_event_loop(name, device, xkb, pressed_keys, stop_flag, event_tx);
        });
        keyboard_threads.push(handle);
    }

    drop(event_tx);

    if keyboard_threads.is_empty() {
        return WorkerExit::StartupFailure("no usable keyboard devices were available".to_string());
    }

    eprintln!("snippeto: worker running");

    let exit_reason = loop {
        match command_rx.try_recv() {
            Ok(WorkerCommand::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                break WorkerExit::ShutdownRequested;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match event_rx.recv_timeout(Duration::from_millis(WORKER_LOOP_POLL_INTERVAL_MS)) {
            Ok(event) => process_key_event(&mut expander, event),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break WorkerExit::AllInputStreamsClosed;
            }
        }
    };

    stop_flag.store(true, Ordering::Relaxed);

    for handle in keyboard_threads {
        let _ = handle.join();
    }

    exit_reason
}

fn keyboard_event_loop(
    name: String,
    mut device: Device,
    xkb: XkbState,
    pressed_keys: Arc<Mutex<HashSet<u16>>>,
    stop_flag: Arc<AtomicBool>,
    event_tx: mpsc::Sender<KeyEvent>,
) {
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        match device.fetch_events() {
            Ok(events) => {
                for event in events {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    if let EventSummary::Key(_, code, value) = event.destructure() {
                        if let Ok(mut pressed) = pressed_keys.lock() {
                            match value {
                                0 => {
                                    pressed.remove(&code.code());
                                }
                                1 | 2 => {
                                    pressed.insert(code.code());
                                }
                                _ => {}
                            }
                        }

                        let char_value = xkb.process_key(code.code(), value);
                        if event_tx
                            .send(KeyEvent {
                                code,
                                value,
                                char_value,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(DEVICE_POLL_INTERVAL_MS));
            }
            Err(error) => {
                eprintln!("snippeto: device error on {name}: {error}");
                break;
            }
        }
    }
}

fn process_key_event(expander: &mut expander::Expander, event: KeyEvent) {
    // Only process key press, ignore release and repeat.
    if event.value != 1 {
        return;
    }

    match event.code {
        KeyCode::KEY_BACKSPACE => {
            expander.pop_char();
            return;
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
            return;
        }
        _ => {}
    }

    for ch in event.char_value.chars() {
        if !ch.is_control() {
            expander.push_char(ch);
        }
    }
}
