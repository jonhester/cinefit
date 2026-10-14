//! Key presses from the remote and keyboards, read straight from
//! `/dev/input`: an appliance has no terminal. The remote is a Harmony
//! driving a flirc, which presents as a USB keyboard, so the remote and a real
//! keyboard are the same thing here. Reading needs only the `input` group.
//!
//! The map follows Kodi's keys, which the flirc's Kodi profile sends, as the
//! C++ version measured them against the owner's remote (its #126):
//!
//! | Button | Key | Action |
//! |---|---|---|
//! | Menu | C (remapped on the Harmony) | menu |
//! | Info | I | info |
//! | OK | keypad Enter | select |
//! | Back, Exit | Backspace | back |
//! | arrows | arrows | up, down, left, right |
//!
//! plus a keyboard's Enter, E, Esc, B, M and D, which Void Video used.
//!
//! Each keyboard is grabbed while it is read, or the kernel would also type
//! the keys into the login prompt on the console behind the picture. The grab
//! ends when the device closes, so a crash cannot leave a keyboard captured.
//! Only devices with navigation keys are taken: the power button is a key
//! device too, and stays logind's.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use evdev::{Device, EventType, KeyCode};

/// What a key press means to cinefit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Menu,
    Info,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Up => "up",
            Action::Down => "down",
            Action::Left => "left",
            Action::Right => "right",
            Action::Select => "select",
            Action::Back => "back",
            Action::Menu => "menu",
            Action::Info => "info",
        }
    }
}

/// The action for a key, or `None` for a key cinefit does not use.
pub fn action_for(key: KeyCode) -> Option<Action> {
    Some(match key {
        KeyCode::KEY_UP => Action::Up,
        KeyCode::KEY_DOWN => Action::Down,
        KeyCode::KEY_LEFT => Action::Left,
        KeyCode::KEY_RIGHT => Action::Right,
        KeyCode::KEY_ENTER | KeyCode::KEY_KPENTER | KeyCode::KEY_E => Action::Select,
        KeyCode::KEY_BACKSPACE | KeyCode::KEY_ESC | KeyCode::KEY_B => Action::Back,
        KeyCode::KEY_C | KeyCode::KEY_M => Action::Menu,
        KeyCode::KEY_I | KeyCode::KEY_D => Action::Info,
        _ => return None,
    })
}

/// One key event in, at most one action out. `value` is evdev's: 1 press, 2
/// autorepeat, 0 release. Releases never act; autorepeat acts only for the
/// arrows, so holding a direction scrolls but holding OK does not select
/// twice.
pub fn on_key(key: KeyCode, value: i32) -> Option<Action> {
    let a = action_for(key)?;
    match value {
        1 => Some(a),
        2 if matches!(a, Action::Up | Action::Down | Action::Left | Action::Right) => Some(a),
        _ => None,
    }
}

/// How often the thread looks for keyboards that came and went: a receiver
/// plugged in later, or a wireless keyboard waking.
const RESCAN: Duration = Duration::from_secs(2);
/// How often it reads the devices it has. A key press waits at most this.
const POLL: Duration = Duration::from_millis(15);

/// Reads every keyboard on a thread of its own; [`KeyReader::drain`] takes
/// what was pressed. Nothing here runs on the frame path.
pub struct KeyReader {
    pending: Arc<Mutex<Vec<Action>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl KeyReader {
    /// Starts reading the keyboards in `dir` (`/dev/input` but in a test).
    /// No keyboard at all is normal: one can be plugged in later. `log` gets
    /// a line for each device taken or lost.
    pub fn start(dir: &Path, log: impl Fn(String) + Send + 'static) -> Self {
        let pending = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s, dir) = (Arc::clone(&pending), Arc::clone(&stop), dir.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("input".into())
            .spawn(move || run(&dir, &p, &s, &log))
            .ok();
        Self {
            pending,
            stop,
            thread,
        }
    }

    /// Everything pressed since the last call, oldest first.
    pub fn drain(&self) -> Vec<Action> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Drop for KeyReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn is_keyboard(d: &Device) -> bool {
    d.supported_keys().is_some_and(|k| {
        k.contains(KeyCode::KEY_UP)
            || k.contains(KeyCode::KEY_ENTER)
            || k.contains(KeyCode::KEY_KPENTER)
    })
}

fn run(dir: &Path, pending: &Mutex<Vec<Action>>, stop: &AtomicBool, log: &dyn Fn(String)) {
    let mut devices: Vec<(PathBuf, Device)> = Vec::new();
    // Nodes looked at and passed over (not keyboards, or not openable), so a
    // rescan does not reopen them every time; forgotten when they go away.
    let mut passed: HashSet<PathBuf> = HashSet::new();
    let mut scanned: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        if scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            scanned = Some(Instant::now());
            scan(dir, &mut devices, &mut passed, log);
        }
        let mut got = Vec::new();
        devices.retain_mut(|(path, d)| {
            let failed = match d.fetch_events() {
                Ok(events) => {
                    for ev in events.filter(|e| e.event_type() == EventType::KEY) {
                        got.extend(on_key(KeyCode::new(ev.code()), ev.value()));
                    }
                    None
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => None,
                Err(e) => Some(e),
            };
            match failed {
                None => true,
                Some(e) => {
                    log(format!(
                        "input: {} ({}) gone: {e}",
                        d.name().unwrap_or("?"),
                        path.display()
                    ));
                    false
                }
            }
        });
        if !got.is_empty() {
            pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(got);
        }
        std::thread::sleep(POLL);
    }
}

fn scan(
    dir: &Path,
    devices: &mut Vec<(PathBuf, Device)>,
    passed: &mut HashSet<PathBuf>,
    log: &dyn Fn(String),
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut present = HashSet::new();
    for path in entries.flatten().map(|e| e.path()) {
        let is_node = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("event"));
        if !is_node {
            continue;
        }
        present.insert(path.clone());
        if passed.contains(&path) || devices.iter().any(|(p, _)| *p == path) {
            continue;
        }
        // Not a device node (a test's plain file), not readable, or not a
        // keyboard: passed over until it goes away.
        let Ok(mut d) = Device::open(&path) else {
            passed.insert(path);
            continue;
        };
        if !is_keyboard(&d) || d.set_nonblocking(true).is_err() {
            passed.insert(path);
            continue;
        }
        let grabbed = match d.grab() {
            Ok(()) => String::new(),
            Err(e) => format!(", not grabbed ({e}): its keys also reach the console"),
        };
        log(format!(
            "input: {} ({}){grabbed}",
            d.name().unwrap_or("?"),
            path.display()
        ));
        devices.push((path, d));
    }
    passed.retain(|p| present.contains(p));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remotes_buttons_as_measured() {
        // The owner's remote (the C++ version's #126).
        assert_eq!(on_key(KeyCode::KEY_UP, 1), Some(Action::Up));
        assert_eq!(on_key(KeyCode::KEY_DOWN, 1), Some(Action::Down));
        assert_eq!(on_key(KeyCode::KEY_LEFT, 1), Some(Action::Left));
        assert_eq!(on_key(KeyCode::KEY_RIGHT, 1), Some(Action::Right));
        assert_eq!(on_key(KeyCode::KEY_KPENTER, 1), Some(Action::Select), "OK");
        assert_eq!(
            on_key(KeyCode::KEY_BACKSPACE, 1),
            Some(Action::Back),
            "Back, Exit"
        );
        assert_eq!(on_key(KeyCode::KEY_C, 1), Some(Action::Menu), "Menu");
        assert_eq!(on_key(KeyCode::KEY_I, 1), Some(Action::Info), "Info");
        // A keyboard's.
        assert_eq!(on_key(KeyCode::KEY_ENTER, 1), Some(Action::Select));
        assert_eq!(on_key(KeyCode::KEY_E, 1), Some(Action::Select));
        assert_eq!(on_key(KeyCode::KEY_ESC, 1), Some(Action::Back));
        assert_eq!(on_key(KeyCode::KEY_B, 1), Some(Action::Back));
        assert_eq!(on_key(KeyCode::KEY_M, 1), Some(Action::Menu));
        assert_eq!(on_key(KeyCode::KEY_D, 1), Some(Action::Info));
    }

    #[test]
    fn other_keys_releases_and_held_buttons_do_nothing() {
        // What else the remote sends, its power-off (KEY_SLEEP) included.
        for k in [
            KeyCode::KEY_F2,
            KeyCode::KEY_Z,
            KeyCode::KEY_TAB,
            KeyCode::KEY_1,
            KeyCode::KEY_SLEEP,
            KeyCode::KEY_POWER,
        ] {
            assert_eq!(on_key(k, 1), None, "{k:?}");
        }
        assert_eq!(on_key(KeyCode::KEY_UP, 0), None, "release");
        assert_eq!(
            on_key(KeyCode::KEY_DOWN, 2),
            Some(Action::Down),
            "a held arrow repeats"
        );
        for k in [
            KeyCode::KEY_KPENTER,
            KeyCode::KEY_C,
            KeyCode::KEY_I,
            KeyCode::KEY_BACKSPACE,
        ] {
            assert_eq!(on_key(k, 2), None, "held {k:?}");
        }
    }

    #[test]
    fn the_reader_ignores_what_is_not_a_keyboard() {
        let dir = std::env::temp_dir().join(format!("cinefit-input-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Named like a node, but a plain file: passed over, not choked on.
        std::fs::write(dir.join("event0"), b"").unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let l = Arc::clone(&lines);
        let r = KeyReader::start(&dir, move |s| l.lock().unwrap().push(s));
        std::thread::sleep(Duration::from_millis(100));
        assert!(r.drain().is_empty());
        drop(r);
        assert!(
            lines.lock().unwrap().is_empty(),
            "{:?}",
            lines.lock().unwrap()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
