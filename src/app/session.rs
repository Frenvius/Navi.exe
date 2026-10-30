use serde::{Deserialize, Serialize};

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
            SetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, WINDOWPLACEMENT,
        },
    },
};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const ROOT: &str = r"Software\Navi\Sessions";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub paths: Vec<String>,
    #[serde(default)]
    pub libraries: Vec<Option<crate::shell::library::Context>>,
    pub active: usize,
    pub bounds: [i32; 4],
    pub maximized: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    version: u32,
    pid: u32,
    born: u64,
    window: Window,
}
pub struct Tracker {
    id: String,
    born: u64,
    last: String,
    registered: bool,
    ending: bool,
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
            let name = crate::shell::wide(&format!(r"Local\NaviSessions-{:x}", hash.finish()));
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

pub struct Claim {
    pub tracker: Tracker,
    pub restored: Option<Window>,
    pub others: Vec<(String, Window)>,
}

impl Window {
    pub fn capture(
        hwnd: windows::Win32::Foundation::HWND,
        paths: Vec<String>,
        libraries: Vec<Option<crate::shell::library::Context>>,
        active: usize,
    ) -> Result<Self, String> {
        let p = crate::frame::normal_placement(hwnd).map_err(|error| error.to_string())?;
        let r = p.rcNormalPosition;
        Ok(Self {
            paths,
            libraries,
            active,
            bounds: [
                r.left,
                r.top,
                (r.right - r.left).max(200),
                (r.bottom - r.top).max(100),
            ],
            maximized: p.showCmd == SW_SHOWMAXIMIZED.0 as u32,
        })
    }

    pub fn place(&self, hwnd: windows::Win32::Foundation::HWND) -> Result<(), String> {
        let [x, y, width, height] = self.bounds;
        let placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            showCmd: if self.maximized {
                SW_SHOWMAXIMIZED.0 as u32
            } else {
                SW_SHOWNORMAL.0 as u32
            },
            rcNormalPosition: windows::Win32::Foundation::RECT {
                left: x,
                top: y,
                right: x + width,
                bottom: y + height,
            },
            ..Default::default()
        };
        unsafe { SetWindowPlacement(hwnd, &placement) }.map_err(|error| error.to_string())
    }
}

fn session_key() -> Result<RegKey, String> {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(ROOT)
        .map(|(key, _)| key)
        .map_err(|error| error.to_string())
}

fn lock() -> Result<Lock, String> {
    Lock::acquire().ok_or_else(|| "Could not acquire the session journal mutex".into())
}

fn write(key: &RegKey, id: &str, record: &Record) -> Result<String, String> {
    let text = serde_json::to_string(record).map_err(|error| error.to_string())?;
    key.set_value(id, &text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

fn remove(key: &RegKey, id: &str) -> Result<(), String> {
    match key.delete_value(id) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn claim(
    launch: &super::entry::Launch,
    initial: Window,
    identity: usize,
) -> Result<Claim, String> {
    let current_born = born(std::process::id()).unwrap_or(0);
    let suffix = if identity == 0 {
        String::new()
    } else {
        format!("-{identity:x}")
    };
    let mut tracker = Tracker {
        id: format!("window-{}-{}{suffix}", std::process::id(), current_born),
        born: current_born,
        last: String::new(),
        registered: false,
        ending: false,
    };
    let _lock = lock()?;
    let key = session_key()?;
    let old = records(&key);
    let on = enabled();
    let chosen = if on {
        if let Some(id) = &launch.restore {
            old.iter().find(|(name, _)| name == id).cloned()
        } else if launch.mode == super::entry::Mode::Default
            && launch.path.is_none()
            && !old.iter().any(|(_, record)| live(record))
        {
            old.first().cloned()
        } else {
            None
        }
    } else {
        None
    };
    let restored = chosen.as_ref().map(|(id, record)| {
        tracker.id = id.clone();
        record.window.clone()
    });
    if on {
        tracker.last = write(
            &key,
            &tracker.id,
            &Record {
                version: 1,
                pid: std::process::id(),
                born: current_born,
                window: restored.clone().unwrap_or(initial),
            },
        )?;
    }
    let mut others = Vec::new();
    for (id, mut record) in old {
        if chosen.as_ref().is_some_and(|(name, _)| *name == id) || live(&record) {
            continue;
        }
        if restored.is_some() && launch.restore.is_none() {
            record.pid = std::process::id();
            record.born = current_born;
            write(&key, &id, &record)?;
            others.push((id, record.window));
        } else if launch.restore.is_none() {
            remove(&key, &id)?;
        }
    }
    Ok(Claim {
        tracker,
        restored,
        others,
    })
}

pub fn release_claim(id: &str) -> Result<(), String> {
    let _lock = lock()?;
    let key = session_key()?;
    if let Some((_, mut record)) = records(&key).into_iter().find(|(name, _)| name == id) {
        record.pid = 0;
        record.born = 0;
        write(&key, id, &record)?;
    }
    Ok(())
}

impl Tracker {
    pub fn checkpoint(&mut self, window: Window) -> Result<(), String> {
        let on = enabled();
        if self.registered != on {
            unsafe {
                if on {
                    let command = crate::shell::wide("");
                    RegisterApplicationRestart(
                        PCWSTR(command.as_ptr()),
                        RESTART_NO_CRASH | RESTART_NO_HANG,
                    )
                    .map_err(|error| error.to_string())?;
                } else {
                    UnregisterApplicationRestart().map_err(|error| error.to_string())?;
                }
            }
            self.registered = on;
        }
        if !on {
            if !self.last.is_empty() {
                let _lock = lock()?;
                remove(&session_key()?, &self.id)?;
                self.last.clear();
            }
            return Ok(());
        }
        let record = Record {
            version: 1,
            pid: std::process::id(),
            born: self.born,
            window,
        };
        let text = serde_json::to_string(&record).map_err(|error| error.to_string())?;
        if text == self.last {
            return Ok(());
        }
        let _lock = lock()?;
        session_key()?
            .set_value(&self.id, &text)
            .map_err(|error| error.to_string())?;
        self.last = text;
        Ok(())
    }

    pub fn ending(&mut self) {
        self.ending = true;
    }

    pub fn finish(&mut self, window: Window) -> Result<(), String> {
        self.checkpoint(window)?;
        if !enabled() || self.ending {
            return Ok(());
        }
        let _lock = lock()?;
        let key = session_key()?;
        if records(&key)
            .iter()
            .any(|(id, record)| *id != self.id && live(record))
        {
            remove(&key, &self.id)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ids;
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
