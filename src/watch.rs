//! Watches the active folder and tells the UI thread when it changes.

use std::sync::Mutex;

use notify::{EventKind, RecursiveMode, Watcher};

static WATCHER: Mutex<Option<notify::RecommendedWatcher>> = Mutex::new(None);

pub fn watch(path: Option<String>) {
    let paths = path.iter().cloned().collect();
    watch_many(path.unwrap_or_default(), paths);
}

pub fn watch_many(folder: String, paths: Vec<String>) {
    let mut guard = WATCHER.lock().unwrap();
    *guard = None;
    if paths.is_empty() { return; }
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            if !matches!(event.kind, EventKind::Access(_)) {
                let folder = folder.clone();
                crate::app::post(move |app| app.fs_changed(folder));
            }
        }
    });
    if let Ok(mut watcher) = watcher {
        let mut watched = false;
        for path in paths {
            if std::path::Path::new(&path).is_dir() {
                watched |= watcher.watch(std::path::Path::new(&path), RecursiveMode::NonRecursive).is_ok();
            }
        }
        if watched { *guard = Some(watcher); }
    }
}

/// The window message the shell's change notifications come as (see `watch_shell`).
pub const SHELL_CHANGE_MSG: u32 = 0x8000 + 0x51;

thread_local! {
    static SHELL_WATCH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    // The folder registered, kept while it is.
    static SHELL_WATCHED: std::cell::RefCell<Option<crate::shell::Pidl>> = const { std::cell::RefCell::new(None) };
}

/// Folders outside the file system (the Recycle Bin, This PC, Network) change without files
/// to watch: the shell's notifications for them come to the window as SHELL_CHANGE_MSG.
pub fn watch_shell(hwnd: windows::Win32::Foundation::HWND, path: Option<&str>) {
    use windows::Win32::UI::Shell::{
        SHChangeNotifyDeregister, SHChangeNotifyEntry, SHChangeNotifyRegister, SHCNE_ALLEVENTS, SHCNRF_InterruptLevel, SHCNRF_NewDelivery,
        SHCNRF_ShellLevel,
    };
    let old = SHELL_WATCH.with(|w| w.replace(0));
    if old != 0 {
        unsafe {
            let _ = SHChangeNotifyDeregister(old);
        }
    }
    let Some(path) = path else { return };
    let Ok(pidl) = crate::shell::Pidl::parse(path) else { return };
    let entry = SHChangeNotifyEntry { pidl: pidl.ptr() as *mut _, fRecursive: false.into() };
    let id = unsafe {
        SHChangeNotifyRegister(
            hwnd,
            SHCNRF_ShellLevel | SHCNRF_InterruptLevel | SHCNRF_NewDelivery,
            SHCNE_ALLEVENTS.0 as i32,
            SHELL_CHANGE_MSG,
            1,
            &entry,
        )
    };
    SHELL_WATCH.with(|w| w.set(id));
    SHELL_WATCHED.with(|w| *w.borrow_mut() = Some(pidl));
}

/// Takes in one of the shell's notifications (SHCNRF_NewDelivery hands it locked).
pub fn shell_notified(wparam: usize, lparam: isize) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::UI::Shell::{SHChangeNotification_Lock, SHChangeNotification_Unlock};
    unsafe {
        let mut pidls = std::ptr::null_mut();
        let mut event = 0i32;
        let lock = SHChangeNotification_Lock(HANDLE(wparam as _), lparam as u32, Some(&mut pidls), Some(&mut event));
        if !lock.is_invalid() {
            let _ = SHChangeNotification_Unlock(HANDLE(lock.0));
        }
    }
    crate::app::post(|app| {
        let path = app.state.borrow().path.clone();
        app.fs_changed(path);
    });
}
