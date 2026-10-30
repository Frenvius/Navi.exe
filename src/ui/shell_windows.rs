use std::{cell::RefCell, collections::HashMap};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, RegisterWindowMessageW, WM_APP};

use crate::app::ids::same;
use crate::shell::shell_windows::{Host, Registration, Request, View};

pub const REQUEST: u32 = WM_APP + 48;
const QUEUE_LIMIT: usize = 4096;
const HOST: Host = Host {
    request: queue,
    view,
};

struct Published {
    path: String,
    registration: Option<Registration>,
}

thread_local! {
    static PUBLISHED: RefCell<HashMap<usize, Published>> = RefCell::new(HashMap::new());
    static REQUESTS: RefCell<HashMap<usize, Vec<(String, Request)>>> = RefCell::new(HashMap::new());
    static TASKBAR_CREATED: u32 = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
}

fn publishable(path: &str) -> bool {
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
    let registration = if !publishable(&path) {
        drop(previous);
        None
    } else {
        match previous.map(|registration| registration.navigate(&path).map(|()| registration)) {
            Some(Ok(registration)) => Some(registration),
            moved => {
                if let Some(Err(error)) = moved {
                    eprintln!("shell windows navigate: {error}");
                }
                register(hwnd, &path)
            }
        }
    };
    PUBLISHED.with(|slot| {
        slot.borrow_mut()
            .insert(key, Published { path, registration });
    });
}

fn register(hwnd: HWND, path: &str) -> Option<Registration> {
    Registration::new(hwnd, path, HOST)
        .inspect_err(|error| eprintln!("shell windows register: {error}"))
        .ok()
}

pub fn taskbar_created() -> u32 {
    TASKBAR_CREATED.with(|message| *message)
}

pub fn restart(hwnd: HWND) {
    let published = PUBLISHED.with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)));
    drop(published);
    sync(hwnd);
}

fn queue(hwnd: HWND, folder: &str, request: Request) {
    let first = REQUESTS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let requests = slot.entry(hwnd.0 as usize).or_default();
        if requests.len() >= QUEUE_LIMIT {
            return false;
        }
        requests.push((folder.to_owned(), request));
        requests.len() == 1
    });
    if !first {
        return;
    }
    if let Err(error) = unsafe { PostMessageW(Some(hwnd), REQUEST, WPARAM(0), LPARAM(0)) } {
        eprintln!("shell windows request queue: {error}");
        REQUESTS.with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)));
    }
}

fn view(hwnd: HWND, folder: &str) -> Option<View> {
    let _entered = super::context::enter(hwnd);
    super::WINDOW.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let window = slot.as_ref()?;
        if !same(window.tabs.active().location.tab.path(), folder) {
            return Some(View::default());
        }
        let selection = &window.list.interaction.selection;
        let selected = window.list.folder.as_ref().map_or_else(Vec::new, |listed| {
            listed
                .order
                .iter()
                .map(|&index| &listed.entries[index].path)
                .filter(|path| selection.selected.contains(*path))
                .cloned()
                .collect()
        });
        Some(View {
            selected,
            focused: selection.focused.clone(),
        })
    })
}

fn tab_index(folder: &str) -> Option<usize> {
    super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let tabs = &slot.as_ref()?.tabs;
        if same(tabs.active().location.tab.path(), folder) {
            return Some(tabs.current);
        }
        tabs.entries
            .iter()
            .position(|saved| same(saved.location.tab.path(), folder))
    })
}

fn show_tab(hwnd: HWND, folder: &str) -> bool {
    let Some(index) = tab_index(folder) else {
        return false;
    };
    if let Err(error) = super::tabs::select(hwnd, index) {
        eprintln!("shell windows tab: {error}");
        return false;
    }
    true
}

pub fn requested(hwnd: HWND) {
    let requests = REQUESTS
        .with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)))
        .unwrap_or_default();
    let mut selects: Option<(String, Vec<(String, u32)>)> = None;
    for (folder, request) in requests {
        match request {
            Request::Select(path, flags) => match &mut selects {
                Some((current, pending)) if *current == folder => pending.push((path, flags)),
                _ => {
                    if let Some((current, pending)) = selects.replace((folder, vec![(path, flags)]))
                    {
                        select(hwnd, &current, &pending);
                    }
                }
            },
            other => {
                if let Some((current, pending)) = selects.take() {
                    select(hwnd, &current, &pending);
                }
                handle(hwnd, &folder, other);
            }
        }
    }
    if let Some((current, pending)) = selects {
        select(hwnd, &current, &pending);
    }
}

fn select(hwnd: HWND, folder: &str, requests: &[(String, u32)]) {
    if !show_tab(hwnd, folder) {
        return;
    }
    let paths = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        Some(crate::app::selection::shell_selected(
            &window.list.interaction.selection,
            requests,
        ))
    });
    let Some(paths) = paths.filter(|paths| !paths.is_empty()) else {
        return;
    };
    super::launch::select(hwnd, paths);
    super::launch::activate(hwnd);
}

fn handle(hwnd: HWND, folder: &str, request: Request) {
    match request {
        Request::Select(path, flags) => select(hwnd, folder, &[(path, flags)]),
        Request::Navigate(target) => {
            if !show_tab(hwnd, folder) {
                return;
            }
            if let Err(error) = super::navigate(hwnd, super::Navigation::Open(target)) {
                eprintln!("shell windows navigate request: {error}");
            }
        }
        Request::Quit => {
            let Some(index) = tab_index(folder) else {
                return;
            };
            if let Err(error) = super::tabs::close(hwnd, index) {
                eprintln!("shell windows quit: {error}");
            }
        }
    }
}

pub fn close(hwnd: HWND) {
    let key = hwnd.0 as usize;
    REQUESTS.with(|slot| slot.borrow_mut().remove(&key));
    let published = PUBLISHED.with(|slot| slot.borrow_mut().remove(&key));
    drop(published);
}
