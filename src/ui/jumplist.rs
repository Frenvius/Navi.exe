use std::{cell::RefCell, collections::HashMap, sync::mpsc};
use windows::Win32::Foundation::HWND;

struct Tracker {
    sender: mpsc::Sender<String>,
    enabled: bool,
    paths: HashMap<usize, String>,
}

thread_local! {
    static TRACKER: RefCell<Option<Tracker>> = const { RefCell::new(None) };
}

pub fn start(hwnd: HWND) -> windows::core::Result<()> {
    crate::shell::jumplist::window(hwnd)?;
    TRACKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let tracker = slot.get_or_insert_with(|| {
            let sender = crate::shell::jumplist::start();
            send(&sender, String::new());
            Tracker {
                sender,
                enabled: crate::shell::jumplist::history_enabled(),
                paths: HashMap::new(),
            }
        });
        tracker.paths.insert(hwnd.0 as usize, String::new());
    });
    Ok(())
}

fn send(sender: &mpsc::Sender<String>, path: String) {
    if let Err(error) = sender.send(path) {
        eprintln!("native jump list update: {error}");
    }
}

pub fn visit(hwnd: HWND, path: &str) {
    TRACKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(tracker) = slot.as_mut() else {
            return;
        };
        let previous = tracker.paths.entry(hwnd.0 as usize).or_default();
        if crate::app::ids::same(previous, path) {
            return;
        }
        *previous = path.to_owned();
        if tracker.enabled && !crate::app::ids::is_virtual(path) {
            send(&tracker.sender, path.to_owned());
        }
    });
}

pub fn preferences() {
    let enabled = crate::shell::jumplist::history_enabled();
    TRACKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(tracker) = slot.as_mut() {
            if tracker.enabled != enabled {
                tracker.enabled = enabled;
                send(&tracker.sender, String::new());
            }
        }
    });
}

pub fn close(hwnd: HWND) {
    crate::shell::jumplist::close(hwnd);
    TRACKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(tracker) = slot.as_mut() {
            tracker.paths.remove(&(hwnd.0 as usize));
            if tracker.paths.is_empty() {
                *slot = None;
            }
        }
    });
}
