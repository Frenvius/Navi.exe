use std::{cell::RefCell, collections::HashMap};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::UI::{Shell::*, WindowsAndMessaging::WM_APP};

pub const CHANGED: u32 = WM_APP + 47;
const REFRESH: usize = 82;

thread_local! {
    static PENDING:RefCell<HashMap<usize,String>>=RefCell::new(HashMap::new());
    static WATCHES: RefCell<HashMap<usize, (String, super::navigation::Notifications)>> = RefCell::new(HashMap::new());
}

pub fn sync(hwnd: HWND) {
    let path = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|window| window.tabs.active().location.tab.path().to_owned())
    });
    let Some(path) = path else {
        return;
    };
    let existing = WATCHES.with(|slot| {
        slot.borrow()
            .get(&(hwnd.0 as usize))
            .map(|(path, _)| path.clone())
    });
    if existing.as_deref() == Some(path.as_str()) {
        return;
    }
    let watched = crate::app::ids::is_virtual(&path)
        && !crate::app::ids::is_search(&path)
        && !crate::app::ids::is_settings(&path)
        && !crate::shell::browser::handles(&path);
    if !watched {
        close(hwnd);
        return;
    }
    match super::navigation::Notifications::location(hwnd, &path) {
        Ok(watch) => {
            let old = WATCHES.with(|slot| slot.borrow_mut().insert(hwnd.0 as usize, (path, watch)));
            drop(old);
        }
        Err(error) => eprintln!("native open-location watch: {error}"),
    }
}

pub fn changed(hwnd: HWND, wparam: usize, lparam: isize) {
    unsafe {
        let mut pidls = std::ptr::null_mut();
        let mut event = 0;
        let locked = SHChangeNotification_Lock(
            HANDLE(wparam as _),
            lparam as u32,
            Some(&mut pidls),
            Some(&mut event),
        );
        if locked.is_invalid() {
            eprintln!("native open-location change lock failed");
            return;
        }
        if !SHChangeNotification_Unlock(HANDLE(locked.0)).as_bool() {
            eprintln!("native open-location change unlock failed");
        }
    }
    let path = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        if window.list.search.busy() {
            return None;
        }
        let path = window.tabs.active().location.tab.path();
        WATCHES.with(|slot| {
            slot.borrow()
                .get(&(hwnd.0 as usize))
                .filter(|(watched, _)| crate::app::ids::same(watched, path))
                .map(|(path, _)| path.clone())
        })
    });
    if let Some(path) = path {
        PENDING.with(|slot| slot.borrow_mut().insert(hwnd.0 as usize, path));
        super::input::arm(hwnd, REFRESH, 150);
    }
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id != REFRESH {
        return false;
    }
    super::input::stop(hwnd, REFRESH);
    let path = PENDING.with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)));
    let result = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return Ok(());
        };
        if window.list.search.busy()
            || !path.is_some_and(|path| {
                crate::app::ids::same(&path, window.tabs.active().location.tab.path())
            })
        {
            return Ok(());
        }
        window.list.refresh(&window.scroll)
    });
    if let Err(error) = result {
        eprintln!("native open-location refresh: {error}");
    }
    super::search::sync(hwnd);
    super::invalidate_all(hwnd);
    true
}

pub fn close(hwnd: HWND) {
    PENDING.with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)));
    super::input::stop(hwnd, REFRESH);
    let old = WATCHES.with(|slot| slot.borrow_mut().remove(&(hwnd.0 as usize)));
    drop(old);
}
