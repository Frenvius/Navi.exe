use windows::core::Result;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegNotifyChangeKeyValue, HKEY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const CHANGED: u32 = WM_APP + 2;

struct Observed {
    key: winreg::RegKey,
    event: HANDLE,
}

impl Observed {
    fn arm(&self) -> Result<()> {
        unsafe {
            RegNotifyChangeKeyValue(
                HKEY(self.key.raw_handle() as _),
                true,
                REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME,
                Some(self.event),
                true,
            )
            .ok()
        }
    }
}

impl Drop for Observed {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseHandle(self.event) } {
            eprintln!("native preferences event cleanup: {error}");
        }
    }
}

pub struct Watch {
    stop: HANDLE,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    pub fn start(hwnd: HWND) -> Result<Self> {
        let stop = unsafe { CreateEventW(None, true, false, None)? };
        let address = hwnd.0 as usize;
        let stop_address = stop.0 as usize;
        let worker = std::thread::Builder::new()
            .name("native-preferences".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let mut observed = Vec::new();
                    for path in [
                        r"Software\Microsoft\Windows\CurrentVersion\Explorer",
                        r"Software\Classes",
                    ] {
                        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                            .open_subkey_with_flags(path, winreg::enums::KEY_NOTIFY)
                            .map_err(|error| {
                                windows::core::Error::new(
                                    windows::Win32::Foundation::E_FAIL,
                                    error.to_string(),
                                )
                            })?;
                        let event = unsafe { CreateEventW(None, false, false, None)? };
                        observed.push(Observed { key, event });
                    }
                    let mut handles = vec![HANDLE(stop_address as _)];
                    handles.extend(observed.iter().map(|observed| observed.event));
                    for observed in &observed {
                        observed.arm()?;
                    }
                    loop {
                        let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                        if result.0 == 0 {
                            return Ok(());
                        }
                        let Some(observed) = result
                            .0
                            .checked_sub(1)
                            .and_then(|index| observed.get(index as usize))
                        else {
                            return Err(windows::core::Error::from_win32());
                        };
                        observed.arm()?;
                        unsafe {
                            PostMessageW(Some(HWND(address as _)), CHANGED, WPARAM(0), LPARAM(0))?;
                        }
                    }
                })();
                if let Err(error) = result {
                    eprintln!("native preferences watcher: {error}");
                }
            })
            .map_err(|error| {
                if let Err(close) = unsafe { CloseHandle(stop) } {
                    eprintln!("native preferences stop cleanup: {close}");
                }
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Err(error) = unsafe { SetEvent(self.stop) } {
            eprintln!("native preferences stop: {error}");
        }
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            eprintln!("native preferences worker panicked");
        }
        if let Err(error) = unsafe { CloseHandle(self.stop) } {
            eprintln!("native preferences stop release: {error}");
        }
    }
}
