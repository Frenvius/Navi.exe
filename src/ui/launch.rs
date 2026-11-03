use crate::app::entry::{self, Action, Request};
use crate::explorer_host::presentation::{self, Destination, Gate};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use windows::core::{w, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const READY: u32 = WM_APP + 46;
const ACTIVATE: usize = 81;
const BOOTSTRAP: usize = 92;

pub fn bootstrap(hwnd: HWND) {
    HOST_UNPUBLISHED.with(|windows| windows.borrow_mut().insert(hwnd.0 as usize));
    HOST_BOOTSTRAP.with(|windows| windows.borrow_mut().insert(hwnd.0 as usize));
    super::input::arm(hwnd, BOOTSTRAP, 1200);
}

pub fn activated(hwnd: HWND) {
    let time = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            w!("NaviLastActivated"),
            Some(HANDLE(time.max(1) as _)),
        )
    } {
        eprintln!("launch activation order: {error}");
    }
}

thread_local! {
    static ROUTES: RefCell<HashMap<usize, Vec<String>>> = Default::default();
    static MAILBOX: RefCell<HashMap<usize, VecDeque<Request>>> = Default::default();
    static RETRIES: RefCell<HashMap<usize, u8>> = Default::default();
    static HOST_PENDING: RefCell<HashMap<usize, HostPending>> = Default::default();
    static HOST_UNPUBLISHED: RefCell<HashSet<usize>> = Default::default();
    static HOST_BOOTSTRAP: RefCell<HashSet<usize>> = Default::default();
}

struct HostPending {
    request: crate::explorer_host::protocol::Request,
    gate: Gate,
    previous: Option<super::tabs::Tabs>,
    restoring: bool,
}

pub fn host_staging(hwnd: HWND) -> bool {
    HOST_PENDING.with(|pending| pending.borrow().contains_key(&(hwnd.0 as usize)))
}

pub fn host_unpublished(hwnd: HWND) -> bool {
    HOST_UNPUBLISHED.with(|windows| windows.borrow().contains(&(hwnd.0 as usize)))
}

fn host_settle(hwnd: HWND) {
    let pending = HOST_PENDING.with(|pending| pending.borrow_mut().remove(&(hwnd.0 as usize)));
    let Some(mut pending) = pending else {
        return;
    };
    match pending.gate.destination() {
        Destination::Navi => {
            let unpublished =
                HOST_UNPUBLISHED.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
            crate::explorer_host::transport::finish(&pending.request.id, true);
            super::shell_windows::sync(hwnd);
            if unpublished {
                let launch = entry::Launch {
                    mode: entry::Mode::NewWindow,
                    ..Default::default()
                };
                if let Err(error) = super::session::start(hwnd, &launch) {
                    eprintln!("Explorer host session: {error}");
                }
            }
            super::session::checkpoint(hwnd);
            activate(hwnd);
        }
        Destination::Native | Destination::NativeDone if !pending.restoring => {
            crate::explorer_host::transport::cancel(&pending.request.id);
            if let Some(previous) = pending.previous.take() {
                let window = super::WINDOW.with(|slot| slot.borrow_mut().take());
                if let Some(mut window) = window {
                    window.tabs = previous;
                    let result = super::tabs::show(&mut window, hwnd)
                        .and_then(|_| super::tabs::sync(&mut window, hwnd));
                    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
                    if let Err(error) = result {
                        super::fail(error);
                    }
                }
                pending.restoring = true;
                HOST_PENDING.with(|slot| slot.borrow_mut().insert(hwnd.0 as usize, pending));
                super::invalidate_all(hwnd);
            } else {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        _ => {
            HOST_PENDING.with(|slot| slot.borrow_mut().insert(hwnd.0 as usize, pending));
        }
    }
}

pub fn host_before_paint(hwnd: HWND) {
    host_settle(hwnd);
}

pub fn host_ready(hwnd: HWND) {
    host_settle(hwnd);
    for id in crate::explorer_host::transport::take() {
        let Some(request) = crate::explorer_host::transport::begin(&id) else {
            continue;
        };
        let gate = match Gate::open(&request) {
            Ok(gate) => gate,
            Err(_) => {
                crate::explorer_host::transport::cancel(&id);
                continue;
            }
        };
        if host_staging(hwnd) && !request.new_window {
            gate.cancel();
            crate::explorer_host::transport::cancel(&id);
            continue;
        }
        let mut target = hwnd;
        let result = (|| -> Result<()> {
            let bootstrap = !host_staging(hwnd)
                && super::WINDOW.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|window| {
                        entry::same(window.tabs.active().location.tab.path(), &request.path)
                    })
                })
                && HOST_BOOTSTRAP.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
            if request.new_window && !bootstrap {
                target = super::create_host_window(&request.path)?;
            }
            let _context = super::context::enter(target);
            let previous = if !request.new_window && unsafe { IsWindowVisible(target).as_bool() } {
                let window = super::WINDOW.with(|slot| slot.borrow_mut().take());
                let Some(mut window) = window else {
                    return Err(windows::core::Error::from_hresult(
                        windows::Win32::Foundation::E_FAIL,
                    ));
                };
                let result = super::tabs::remember(&mut window).map(|_| super::tabs::Tabs {
                    entries: window.tabs.entries.clone(),
                    current: window.tabs.current,
                });
                super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
                Some(result?)
            } else {
                None
            };
            HOST_PENDING.with(|pending| {
                pending.borrow_mut().insert(
                    target.0 as usize,
                    HostPending {
                        request: request.clone(),
                        gate: gate.clone(),
                        previous,
                        restoring: false,
                    },
                )
            });
            if !gate.stage(target).map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
            })? || !crate::explorer_host::transport::apply(&id)
            {
                gate.cancel();
                host_settle(target);
                return Ok(());
            }
            if gate.destination() != Destination::Pending {
                host_settle(target);
                return Ok(());
            }
            if request.new_tab && unsafe { IsWindowVisible(target).as_bool() } {
                super::tabs::open(target, &request.path)?;
            } else if !request.new_window && !bootstrap {
                super::navigate(target, super::Navigation::Open(request.path.clone()))?;
            }
            if !request.selection.is_empty() {
                select(target, request.selection.clone());
            }
            if !unsafe { IsWindowVisible(target).as_bool() } {
                crate::frame::restore_placement(target)?;
                unsafe {
                    let _ = ShowWindow(target, SW_SHOWNA);
                }
                super::input::stop(target, BOOTSTRAP);
            }
            host_settle(target);
            Ok(())
        })();
        if result.is_err() {
            gate.cancel();
            crate::explorer_host::transport::cancel(&request.id);
            let _context = super::context::enter(target);
            host_settle(target);
        }
    }
}

fn host_painted(hwnd: HWND) {
    let restoring = HOST_PENDING.with(|pending| {
        pending
            .borrow()
            .get(&(hwnd.0 as usize))
            .is_some_and(|pending| pending.restoring)
    });
    if restoring {
        HOST_PENDING.with(|pending| pending.borrow_mut().remove(&(hwnd.0 as usize)));
        if let Err(error) = presentation::cloak(hwnd, false) {
            eprintln!("Explorer host restore: {error}");
        }
        super::session::checkpoint(hwnd);
        return;
    }
    HOST_PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let Some(pending) = pending.get_mut(&(hwnd.0 as usize)) else {
            return;
        };
        let request = &pending.request;
        let painted = super::WINDOW.with(|slot| {
            let slot = slot.borrow();
            slot.as_ref().is_some_and(|window| {
                window.list.folder.as_ref().is_some_and(|folder| {
                    request
                        .selection
                        .iter()
                        .all(|path| folder.entries.iter().any(|entry| entry.path == *path))
                }) && entry::same(window.tabs.active().location.tab.path(), &request.path)
                    && request
                        .selection
                        .iter()
                        .all(|path| window.list.interaction.selection.selected.contains(path))
            })
        });
        if painted {
            pending.gate.ready();
        }
    });
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
    if unsafe { GetPropW(hwnd, w!("NaviMain")) }.0 as usize != crate::app::tab_transfer::MARKER
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
    if id == BOOTSTRAP {
        HOST_BOOTSTRAP.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
        super::input::stop(hwnd, BOOTSTRAP);
        if !unsafe { IsWindowVisible(hwnd).as_bool() } {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        return true;
    }
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
                select(hwnd, vec![path]);
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("native launch delivery: {error}");
        }
    }
}

pub fn select(hwnd: HWND, paths: Vec<String>) {
    let Some(path) = paths.first().cloned() else {
        return;
    };
    super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return;
        };
        let selection = crate::app::selection::Selection {
            selected: paths.into_iter().collect(),
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
    host_painted(hwnd);
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
    HOST_BOOTSTRAP.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
    HOST_UNPUBLISHED.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
    let next = super::context::handles()
        .into_iter()
        .find(|handle| *handle != hwnd)
        .unwrap_or_default();
    crate::explorer_host::transport::retarget(hwnd, next);
    HOST_PENDING.with(|pending| {
        if let Some(pending) = pending.borrow_mut().remove(&(hwnd.0 as usize)) {
            pending.gate.cancel();
            crate::explorer_host::transport::cancel(&pending.request.id);
        }
    });
    ROUTES.with(|routes| routes.borrow_mut().remove(&(hwnd.0 as usize)));
    MAILBOX.with(|mailbox| mailbox.borrow_mut().remove(&(hwnd.0 as usize)));
    RETRIES.with(|retries| retries.borrow_mut().remove(&(hwnd.0 as usize)));
    if !unsafe { GetPropW(hwnd, w!("NaviLastActivated")) }.is_invalid() {
        if let Err(error) = unsafe { RemovePropW(hwnd, w!("NaviLastActivated")) } {
            eprintln!("launch activation stamp cleanup: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_stamp_is_owned_by_window_lifetime() {
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("NaviLaunchLifetimeTest"),
                Default::default(),
                0,
                0,
                1,
                1,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .unwrap();
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                if let Err(error) = unsafe { DestroyWindow(self.0) } {
                    eprintln!("launch lifetime test cleanup: {error}");
                }
            }
        }
        let _window = Window(hwnd);
        bootstrap(hwnd);
        assert!(host_unpublished(hwnd));
        assert!(HOST_BOOTSTRAP.with(|windows| windows.borrow().contains(&(hwnd.0 as usize))));
        activated(hwnd);
        assert!(unsafe { GetPropW(hwnd, w!("NaviLastActivated")) }.0 as usize > 0);
        close(hwnd);
        assert!(!host_unpublished(hwnd));
        assert!(!HOST_BOOTSTRAP.with(|windows| windows.borrow().contains(&(hwnd.0 as usize))));
        assert!(unsafe { GetPropW(hwnd, w!("NaviLastActivated")) }.is_invalid());
    }
}
