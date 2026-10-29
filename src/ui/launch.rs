use crate::app::entry::{self, Action, Request};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use windows::core::{w, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const READY: u32 = WM_APP + 46;
const ACTIVATE: usize = 81;

thread_local! {
    static ROUTES: RefCell<HashMap<usize, Vec<String>>> = Default::default();
    static MAILBOX: RefCell<HashMap<usize, VecDeque<Request>>> = Default::default();
    static RETRIES: RefCell<HashMap<usize, u8>> = Default::default();
}

pub fn routes(hwnd: HWND, tabs: &super::tabs::Tabs) {
    let paths = tabs
        .entries
        .iter()
        .map(|saved| saved.location.tab.path().to_owned())
        .collect();
    ROUTES.with(|routes| routes.borrow_mut().insert(hwnd.0 as usize, paths));
}

fn matched(hwnd: HWND, path: &str) -> Option<usize> {
    ROUTES.with(|routes| {
        routes.borrow().get(&(hwnd.0 as usize)).and_then(|paths| {
            paths
                .iter()
                .position(|candidate| entry::same(candidate, path))
        })
    })
}

pub fn message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let request = match entry::incoming(message, wparam, lparam)? {
        Ok(request) => request,
        Err(()) => return Some(LRESULT(0)),
    };
    let index = matched(hwnd, &request.path);
    if matches!(request.action, Action::Probe) {
        return Some(LRESULT(isize::from(index.is_some())));
    }
    if matches!(request.action, Action::Match) && index.is_none() {
        return Some(LRESULT(0));
    }
    let queued = MAILBOX.with(|mailbox| {
        let mut mailbox = mailbox.borrow_mut();
        let requests = mailbox.entry(hwnd.0 as usize).or_default();
        if requests.len() >= 64 {
            return false;
        }
        requests.push_back(request);
        true
    });
    if !queued {
        return Some(LRESULT(0));
    }
    match unsafe { PostMessageW(Some(hwnd), READY, WPARAM(0), LPARAM(0)) } {
        Ok(()) => {
            activate(hwnd);
            Some(LRESULT(1))
        }
        Err(error) => {
            eprintln!("native launch queue: {error}");
            MAILBOX.with(|mailbox| {
                if let Some(requests) = mailbox.borrow_mut().get_mut(&(hwnd.0 as usize)) {
                    requests.pop_back();
                }
            });
            Some(LRESULT(0))
        }
    }
}

fn activate_once(hwnd: HWND) -> bool {
    if unsafe { GetPropW(hwnd, w!("NaviMain")) }.0 as usize
        != crate::app::tab_transfer::MARKER
        || !crate::app::tab_transfer::same_executable(hwnd)
    {
        return true;
    }
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            super::invalidate_all(hwnd);
        }
        let active = SetForegroundWindow(hwnd).as_bool();
        if let Err(error) = SetPropW(
            hwnd,
            w!("NativeLaunchActivated"),
            Some(HANDLE((usize::from(active) + 1) as _)),
        ) {
            eprintln!("launch activation diagnostic: {error}");
        }
        active
    }
}

pub fn activate(hwnd: HWND) {
    if activate_once(hwnd) {
        RETRIES.with(|retries| retries.borrow_mut().remove(&(hwnd.0 as usize)));
        super::input::stop(hwnd, ACTIVATE);
        return;
    }
    RETRIES.with(|retries| retries.borrow_mut().insert(hwnd.0 as usize, 2));
    super::input::arm(hwnd, ACTIVATE, 100);
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id != ACTIVATE {
        return false;
    }
    super::input::stop(hwnd, ACTIVATE);
    let attempts = RETRIES.with(|retries| retries.borrow_mut().remove(&(hwnd.0 as usize)));
    if let Some(attempts) = attempts {
        if !activate_once(hwnd) && attempts > 1 {
            RETRIES.with(|retries| retries.borrow_mut().insert(hwnd.0 as usize, attempts - 1));
            super::input::arm(hwnd, ACTIVATE, 100);
        }
    }
    true
}

pub fn ready(hwnd: HWND) {
    if !super::WINDOW.with(|slot| slot.borrow().is_some()) {
        return;
    }
    let requests = MAILBOX
        .with(|mailbox| mailbox.borrow_mut().remove(&(hwnd.0 as usize)))
        .unwrap_or_default();
    for request in requests {
        let result: Result<()> = (|| {
            match request.action {
                Action::Probe => {}
                Action::Match => {
                    if let Some(index) = matched(hwnd, &request.path) {
                        super::tabs::select(hwnd, index)?;
                    } else {
                        return Ok(());
                    }
                }
                Action::Navigate => super::navigate(hwnd, super::Navigation::Open(request.path))?,
                Action::NewTab => super::tabs::open(hwnd, &request.path)?,
            }
            if let Some(path) = request.select {
                select(hwnd, path);
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("native launch delivery: {error}");
        }
    }
}

pub fn select(hwnd: HWND, path: String) {
    super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return;
        };
        let selection = crate::app::selection::Selection {
            selected: std::collections::HashSet::from([path.clone()]),
            anchor: Some(path.clone()),
            focused: Some(path.clone()),
            ..Default::default()
        };
        if window.list.folder.is_none() {
            window.pending_visit = Some(selection);
        } else {
            window.list.interaction.selection = selection;
            window.list.interaction.mark_all();
        }
        window.reveal_path = Some(path);
    });
    super::chrome::set_focus(hwnd, super::chrome::Focus::List);
    super::invalidate_all(hwnd);
}

pub fn after_paint(hwnd: HWND) {
    let pending = MAILBOX.with(|mailbox| {
        mailbox
            .borrow()
            .get(&(hwnd.0 as usize))
            .is_some_and(|requests| !requests.is_empty())
    });
    if pending {
        if let Err(error) = unsafe { PostMessageW(Some(hwnd), READY, WPARAM(0), LPARAM(0)) } {
            eprintln!("native deferred launch: {error}");
        }
    }
}

pub fn close(hwnd: HWND) {
    ROUTES.with(|routes| routes.borrow_mut().remove(&(hwnd.0 as usize)));
    MAILBOX.with(|mailbox| mailbox.borrow_mut().remove(&(hwnd.0 as usize)));
    RETRIES.with(|retries| retries.borrow_mut().remove(&(hwnd.0 as usize)));
}
