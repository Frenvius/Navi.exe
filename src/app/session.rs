//! App-owned window journals. Explorer's PersistBrowsers is a preference only:
//! never write the real shell's restart streams or session records.
use super::{ids, App, Tab};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, rc::Rc, time::Duration};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE},
        System::{
            Recovery::{
                RegisterApplicationRestart, UnregisterApplicationRestart, RESTART_NO_CRASH,
                RESTART_NO_HANG,
            },
            Threading::{
                CreateMutexW, GetProcessTimes, OpenProcess, ReleaseMutex, WaitForSingleObject,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::WindowsAndMessaging::{
            GetWindowPlacement, SetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWNORMAL,
            WINDOWPLACEMENT,
        },
    },
};
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
    RegKey,
};

const ROOT: &str = r"Software\slint-win-explorer\Sessions";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Window {
    paths: Vec<String>,
    #[serde(default)]
    libraries: Vec<Option<crate::shell::library::Context>>,
    active: usize,
    bounds: [i32; 4],
    maximized: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    version: u32,
    pid: u32,
    born: u64,
    window: Window,
}
struct Tracker {
    id: String,
    born: u64,
    last: String,
    registered: bool,
    ending: bool,
}
thread_local! {
    static TRACKER:RefCell<Option<Tracker>>=const{RefCell::new(None)};
    static TIMER:slint::Timer=slint::Timer::default();
}
pub fn enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced")
        .and_then(|k| k.get_value::<u32, _>("PersistBrowsers"))
        .unwrap_or(0)
        != 0
}
struct Lock(HANDLE);
impl Lock {
    fn acquire() -> Option<Self> {
        unsafe {
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            std::env::var_os("USERPROFILE").hash(&mut hash);
            let name =
                crate::shell::wide(&format!(r"Local\SlintExplorerSessions-{:x}", hash.finish()));
            let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr())).ok()?;
            let wait = WaitForSingleObject(handle, 2000);
            if wait.0 != 0 && wait.0 != 0x80 {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Self(handle))
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}
fn born(pid: u32) -> Option<u64> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut create, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let result = GetProcessTimes(process, &mut create, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        result.ok()?;
        // An exited process is no longer live even while its handle remains observable.
        if exit.dwHighDateTime != 0 || exit.dwLowDateTime != 0 {
            return None;
        }
        Some((create.dwHighDateTime as u64) << 32 | create.dwLowDateTime as u64)
    }
}
fn decode(text: &str) -> Option<Record> {
    if text.len() > 1_048_576 {
        return None;
    }
    let record: Record = serde_json::from_str(text).ok()?;
    let w = &record.window;
    if record.version != 1
        || w.paths.is_empty()
        || w.paths.len() > 128
        || w.active >= w.paths.len()
        || w.paths
            .iter()
            .any(|p| p.is_empty() || p.len() > 32768 || p.contains('\0'))
        || w.bounds[2] < 200
        || w.bounds[3] < 100
        || w.bounds[2] > 32768
        || w.bounds[3] > 32768
        || w.bounds[0].checked_add(w.bounds[2]).is_none()
        || w.bounds[1].checked_add(w.bounds[3]).is_none()
    {
        return None;
    }
    Some(record)
}
fn records(key: &RegKey) -> Vec<(String, Record)> {
    let mut records = key
        .enum_values()
        .flatten()
        .filter_map(|(id, _)| {
            if !id.starts_with("window-") {
                return None;
            }
            let text: String = key.get_value(&id).ok()?;
            Some((id, decode(&text)?))
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|(_, r)| r.born);
    records
}
fn live(record: &Record) -> bool {
    born(record.pid) == Some(record.born)
}
fn snapshot(app: &App) -> Window {
    let s = app.state.borrow();
    let mut p = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let _ = unsafe { GetWindowPlacement(app.hwnd.get(), &mut p) };
    let r = p.rcNormalPosition;
    Window {
        paths: s.tabs.iter().map(|t| t.path().to_string()).collect(),
        libraries: s.tabs.iter().map(Tab::library).collect(),
        active: s.current,
        bounds: [
            r.left,
            r.top,
            (r.right - r.left).max(200),
            (r.bottom - r.top).max(100),
        ],
        maximized: p.showCmd == SW_SHOWMAXIMIZED.0 as u32,
    }
}
fn apply(app: &Rc<App>, window: &Window) {
    let tabs = window
        .paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let mut tab = Tab::new(path);
            tab.libraries[0] = window.libraries.get(index).cloned().flatten()
                .filter(|c| crate::shell::library::is_library(&c.library) && c.contains(path));
            if ids::is_settings(path) { tab.title=crate::i18n::tr("settings.title").into(); }
            else if let Ok(pidl) = crate::shell::Pidl::parse(path) {
                tab.title = pidl.name(windows::Win32::UI::Shell::SIGDN_NORMALDISPLAY);
                tab.icon = unsafe { crate::shell::icons::icon_of_pidl(pidl.ptr()) };
            } else {
                tab.title = path.clone();
            }
            tab
        })
        .collect();
    {
        let mut s = app.state.borrow_mut();
        s.tabs = tabs;
        s.current = window.active;
    }
    let [x, y, width, height] = window.bounds;
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        showCmd: if window.maximized {
            SW_SHOWMAXIMIZED.0 as u32
        } else {
            SW_SHOWNORMAL.0 as u32
        },
        ..Default::default()
    };
    placement.rcNormalPosition = windows::Win32::Foundation::RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    };
    let _ = unsafe { SetWindowPlacement(app.hwnd.get(), &placement) };
}
/// Restore once across concurrent launches. Explicit folder launches remain explicit;
/// children/restarters claim only their named window, never the whole session again.
pub fn start(app: &Rc<App>) {
    let launch=super::entry::options();
    let requested=launch.restore;
    let force_new=launch.mode!=super::entry::Mode::Default;
    let explicit=launch.path;
    let current_born = born(std::process::id()).unwrap_or(0);
    let id = format!("window-{}-{}", std::process::id(), current_born);
    let mut tracker = Tracker {
        id,
        born: current_born,
        last: String::new(),
        registered: false,
        ending: false,
    };
    let mut restored = false;
    if let Some(_lock) = Lock::acquire() {
        if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(ROOT) {
            let old = records(&key);
            let on = enabled();
            let chosen = if on {
                if let Some(id) = &requested {
                    old.iter().find(|(name, _)| name == id).cloned()
                } else if !force_new && explicit.is_none() && !old.iter().any(|(_, r)| live(r)) {
                    old.first().cloned()
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((id, record)) = &chosen {
                tracker.id = id.clone();
                apply(app, &record.window);
                restored = true;
            }
            if !restored {
                let start = explicit.clone().unwrap_or_else(ids::start_location);
                app.state.borrow_mut().tabs[0] = Tab::new(&start);
            }
            // Claim the primary before spawning other windows, under the same mutex.
            if on {
                let record = Record {
                    version: 1,
                    pid: std::process::id(),
                    born: current_born,
                    window: snapshot(app),
                };
                if let Ok(text) = serde_json::to_string(&record) {
                    let _ = key.set_value(&tracker.id, &text);
                    tracker.last = text;
                }
            }
            for (id, mut record) in old {
                if chosen.as_ref().is_some_and(|(name, _)| *name == id) || live(&record) {
                    continue;
                }
                if restored && requested.is_none() {
                    record.pid = std::process::id();
                    record.born = current_born;
                    if let Ok(text) = serde_json::to_string(&record) {
                        let _ = key.set_value(&id, &text);
                    }
                    let spawned = std::env::current_exe().ok().and_then(|exe| {
                        std::process::Command::new(exe)
                            .arg("--restore-session")
                            .arg(&id)
                            .env_remove("SLINT_TAB_WINDOW")
                            .spawn()
                            .ok()
                    });
                    if spawned.is_none() {
                        record.pid = 0;
                        record.born = 0;
                        if let Ok(text) = serde_json::to_string(&record) {
                            let _ = key.set_value(&id, &text);
                        }
                    }
                } else if requested.is_none() {
                    let _ = key.delete_value(&id);
                }
            }
        }
    }
    if !restored && app.state.borrow().tabs[0].path() == ids::HOME {
        let start = explicit.unwrap_or_else(ids::start_location);
        app.state.borrow_mut().tabs[0] = Tab::new(&start);
    }
    TRACKER.with(|t| *t.borrow_mut() = Some(tracker));
    checkpoint(app);
    let weak = Rc::downgrade(app);
    TIMER.with(|timer| {
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(500),
            move || {
                if let Some(app) = weak.upgrade() {
                    checkpoint(&app);
                }
            },
        )
    });
}
pub fn checkpoint(app: &App) {
    let on = enabled();
    TRACKER.with(|cell| {
        let mut tracker = cell.borrow_mut();
        let Some(t) = tracker.as_mut() else { return };
        if t.registered != on {
            unsafe {
                if on {
                    let command = crate::shell::wide(&format!("--restore-session {}", t.id));
                    t.registered = RegisterApplicationRestart(
                        PCWSTR(command.as_ptr()),
                        RESTART_NO_CRASH | RESTART_NO_HANG,
                    )
                    .is_ok();
                } else {
                    let _ = UnregisterApplicationRestart();
                    t.registered = false;
                }
            }
        }
        if !on {
            if !t.last.is_empty() {
                if let Ok(key) =
                    RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(ROOT, KEY_WRITE)
                {
                    let _ = key.delete_value(&t.id);
                }
                t.last.clear();
            }
            return;
        }
        let record = Record {
            version: 1,
            pid: std::process::id(),
            born: t.born,
            window: snapshot(app),
        };
        let Ok(text) = serde_json::to_string(&record) else {
            return;
        };
        if text == t.last {
            return;
        }
        let Some(_lock) = Lock::acquire() else { return };
        if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(ROOT) {
            if key.set_value(&t.id, &text).is_ok() {
                t.last = text;
            }
        }
    });
}
pub fn ending(app: &App, confirmed: bool) {
    checkpoint(app);
    if confirmed {
        TRACKER.with(|t| {
            if let Some(t) = t.borrow_mut().as_mut() {
                t.ending = true;
            }
        });
    }
}
pub fn finish(app: &App) {
    TIMER.with(|t| t.stop());
    checkpoint(app);
    TRACKER.with(|cell| {
        let tracker = cell.borrow();
        let Some(t) = tracker.as_ref() else { return };
        if !enabled() || t.ending {
            return;
        }
        let Some(_lock) = Lock::acquire() else { return };
        if let Ok(key) =
            RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(ROOT, KEY_READ | KEY_WRITE)
        {
            // Closing one window intentionally removes it; closing the last retains
            // its tabs for the user-requested app-reopen behavior.
            if records(&key)
                .iter()
                .any(|(id, r)| *id != t.id && r.pid != std::process::id() && live(r))
            {
                let _ = key.delete_value(&t.id);
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> Record {
        Record {
            version: 1,
            pid: 123,
            born: 456,
            window: Window {
                paths: vec![
                    r"C:\pasta com espaços\ação".into(),
                    ids::THIS_PC.into(),
                    "search|C:\\|nome".into(),
                ],
                libraries: Vec::new(),
                active: 1,
                bounds: [-1200, 30, 1116, 727],
                maximized: true,
            },
        }
    }
    #[test]
    fn session_preserves_unicode_virtual_search_tab_order_active_and_placement() {
        let r = record();
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(decode(&text).unwrap().window, r.window);
    }
    #[test]
    fn corrupt_future_and_unbounded_session_records_are_rejected() {
        assert!(decode("{").is_none());
        let mut r = record();
        r.version = 2;
        assert!(decode(&serde_json::to_string(&r).unwrap()).is_none());
        r = record();
        r.window.active = 3;
        assert!(decode(&serde_json::to_string(&r).unwrap()).is_none());
        r = record();
        r.window.paths = vec!["x".into(); 129];
        assert!(decode(&serde_json::to_string(&r).unwrap()).is_none());
        r = record();
        r.window.bounds[2] = i32::MAX;
        assert!(decode(&serde_json::to_string(&r).unwrap()).is_none());
    }
}
