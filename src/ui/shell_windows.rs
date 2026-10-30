use std::{cell::RefCell, collections::HashMap};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::shell::shell_windows::Registration;

pub const SELECT: u32 = WM_APP + 48;
const QUEUE_LIMIT: usize = 4096;

struct Published {
    path: String,
    registration: Option<Registration>,
}

thread_local! {
    static PUBLISHED: RefCell<HashMap<usize, Published>> = RefCell::new(HashMap::new());
    static REQUESTS: RefCell<HashMap<usize, Vec<(String, u32)>>> = RefCell::new(HashMap::new());
}

fn published(path: &str) -> bool {
    !crate::app::ids::is_settings(path) && !crate::app::ids::is_search(path)
}

pub fn sync(hwnd: HWND) {
    let key = hwnd.0 as usize;
    let path = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|window| window.tabs.active().location.tab.path().to_owned())
    });
    let Some(path) = path else {
        return;
    };
    let unchanged = PUBLISHED.with(|slot| {
        slot.borrow()
            .get(&key)
            .is_some_and(|published| published.path == path)
    });
    if unchanged {
        return;
    }
    let previous = PUBLISHED
        .with(|slot| slot.borrow_mut().remove(&key))
        .and_then(|published| published.registration);
    let registration = if !published(&path) {
        drop(previous);
        None
    } else {
        match previous.map(|registration| registration.navigate(&path).map(|()| registration)) {
            Some(Ok(registration)) => Some(registration),
            moved => {
                if let Some(Err(error)) = moved {
                    eprintln!("shell windows navigate: {error}");
                }
                Registration::new(hwnd, &path, queue)
                    .inspect_err(|error| eprintln!("shell windows register: {error}"))
                    .ok()
            }
        }
    };
    REQUESTS.with(|slot| slot.borrow_mut().remove(&key));
    PUBLISHED.with(|slot| {
        slot.borrow_mut()
            .insert(key, Published { path, registration });
    });
}

fn queue(hwnd: HWND, path: String, flags: u32) {
    let first = REQUESTS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let requests = slot.entry(hwnd.0 as usize).or_default();
        if requests.len() >= QUEUE_LIMIT {
            return false;
        }
        requests.push((path, flags));
        requests.len() == 1
    });
    if !first {
        return;
    }
    if let Err(error) = unsafe { PostMessageW(Some(hwnd), SELECT, WPARAM(0), LPARAM(0)) } {
        eprintln!("shell windows selection queue: {error}");
        REQUESTS.with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)));
    }
}

pub fn selected(hwnd: HWND) {
    let key = hwnd.0 as usize;
    let requests = REQUESTS
        .with(|slot| slot.borrow_mut().remove(&key))
        .unwrap_or_default();
    let registered = PUBLISHED.with(|slot| {
        slot.borrow()
            .get(&key)
            .map(|published| published.path.clone())
    });
    let paths = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let active = window.tabs.active().location.tab.path();
        if !registered.is_some_and(|path| crate::app::ids::same(&path, active)) {
            return None;
        }
        Some(crate::app::selection::shell_selected(
            &window.list.interaction.selection,
            &requests,
        ))
    });
    let Some(paths) = paths.filter(|paths| !paths.is_empty()) else {
        return;
    };
    super::launch::select(hwnd, paths);
    super::launch::activate(hwnd);
}

pub fn close(hwnd: HWND) {
    let key = hwnd.0 as usize;
    REQUESTS.with(|slot| slot.borrow_mut().remove(&key));
    let published = PUBLISHED.with(|slot| slot.borrow_mut().remove(&key));
    drop(published);
}
