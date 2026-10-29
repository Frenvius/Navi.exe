use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const READY: u32 = WM_APP + 45;

struct GdiHost(HWND);

unsafe extern "system" fn gdi_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn local_rect(rect: RECT) -> RECT {
    RECT {
        right: rect.right - rect.left,
        bottom: rect.bottom - rect.top,
        ..Default::default()
    }
}

impl GdiHost {
    fn new(parent: HWND, rect: RECT) -> windows::core::Result<Self> {
        unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(gdi_message),
                hInstance: instance.into(),
                hbrBackground: windows::Win32::Graphics::Gdi::GetSysColorBrush(
                    windows::Win32::Graphics::Gdi::COLOR_WINDOW,
                ),
                lpszClassName: w!("SwShellView"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0
                && windows::Win32::Foundation::GetLastError()
                    != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
            {
                return Err(windows::core::Error::from_win32());
            }
            let window = Self(CreateWindowExW(
                WS_EX_LAYERED,
                w!("SwShellView"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                Some(parent),
                None,
                Some(instance.into()),
                None,
            )?);
            SetLayeredWindowAttributes(
                window.0,
                windows::Win32::Foundation::COLORREF(0),
                255,
                LWA_ALPHA,
            )?;
            Ok(window)
        }
    }

    fn place(&self, rect: RECT) -> windows::core::Result<()> {
        unsafe {
            SetWindowPos(
                self.0,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
    }
}

impl Drop for GdiHost {
    fn drop(&mut self) {
        if let Err(error) = unsafe { DestroyWindow(self.0) } {
            eprintln!("hosted child cleanup: {error}");
        }
    }
}

struct Host {
    instance: u64,
    rect: RECT,
    browser: crate::shell::browser::Browser,
    window: GdiHost,
}

thread_local! {
    static HOSTS: RefCell<HashMap<isize, Host>> = Default::default();
    static BUSY: RefCell<HashSet<isize>> = Default::default();
    static QUEUE: RefCell<VecDeque<(isize, u64, String)>> = Default::default();
    static INSTANCE: std::cell::Cell<u64> = const {std::cell::Cell::new(0)};
}

fn completed(hwnd: HWND, instance: u64, path: String) {
    QUEUE.with(|queue| {
        queue
            .borrow_mut()
            .push_back((hwnd.0 as isize, instance, path))
    });
    if let Err(error) = unsafe { PostMessageW(Some(hwnd), READY, WPARAM(0), LPARAM(0)) } {
        eprintln!("hosted navigation notification: {error}");
    }
}

pub fn ready(hwnd: HWND) {
    if BUSY.with(|busy| busy.borrow().contains(&(hwnd.0 as isize))) {
        return;
    }
    let requests = QUEUE.with(|queue| {
        let mut queue = queue.borrow_mut();
        let mut own = Vec::new();
        queue.retain(|(owner, instance, path)| {
            if *owner == hwnd.0 as isize {
                own.push((*instance, path.clone()));
                false
            } else {
                true
            }
        });
        own
    });
    for (instance, path) in requests {
        let accepted = HOSTS.with(|hosts| {
            let mut hosts = hosts.borrow_mut();
            let Some(host) = hosts
                .get_mut(&(hwnd.0 as isize))
                .filter(|host| host.instance == instance)
            else {
                return false;
            };
            host.browser.path = path.clone();
            true
        });
        if !accepted {
            continue;
        }
        let navigate = super::WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                let current = window.tabs.active().location.tab.path();
                crate::shell::browser::handles(current) && !crate::app::ids::same(current, &path)
            })
        });
        if navigate {
            if let Err(error) = super::navigate(hwnd, super::Navigation::Open(path)) {
                super::fail(error);
            }
        }
    }
}

pub fn sync(hwnd: HWND) {
    let owner = hwnd.0 as isize;
    if BUSY.with(|busy| !busy.borrow_mut().insert(owner)) {
        return;
    }
    let desired = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let path = window.tabs.active().location.tab.path();
        if !crate::shell::browser::handles(path) {
            return None;
        }
        let mut rect = RECT::default();
        if let Err(error) = unsafe { GetClientRect(hwnd, &mut rect) } {
            eprintln!("hosted viewport: {error}");
            return None;
        }
        rect.top = if crate::frame::is_full_screen_for(hwnd) {
            0
        } else {
            ((if crate::shell::browser::is_control_panel(path) {
                86.0
            } else {
                136.0
            }) * unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32
                / 96.0)
                .round() as i32
        };
        Some((path.to_owned(), rect))
    });
    let mut hosted = HOSTS.with(|hosts| hosts.borrow_mut().remove(&owner));
    let was_hosted = hosted.is_some();
    if let Some((path, rect)) = desired {
        if !was_hosted {
            if let Err(error) =
                unsafe { SetPropW(hwnd, w!("NaviNativeView"), Some(HANDLE(1 as _))) }
            {
                eprintln!("hosted palette marker: {error}");
            }
        }
        if hosted
            .as_ref()
            .is_some_and(|host| !crate::app::ids::same(&host.browser.path, &path))
        {
            drop(hosted.take());
        }
        if let Some(host) = &mut hosted {
            if host.rect != rect {
                if let Err(error) = host.window.place(rect) {
                    eprintln!("hosted child placement: {error}");
                }
                host.browser.rect(local_rect(rect));
                host.rect = rect;
            }
        } else {
            let instance = INSTANCE.with(|next| {
                let value = next.get().wrapping_add(1);
                next.set(value);
                value
            });
            let created = GdiHost::new(hwnd, rect).and_then(|window| {
                crate::shell::browser::Browser::new(window.0, local_rect(rect), move |path| {
                    completed(hwnd, instance, path)
                })
                .map(|browser| (window, browser))
                .map_err(|error| {
                    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                })
            });
            match created {
                Ok((window, mut browser)) => match browser.navigate(&path) {
                    Ok(()) => {
                        hosted = Some(Host {
                            instance,
                            rect,
                            browser,
                            window,
                        })
                    }
                    Err(error) => eprintln!("hosted shell navigation {path}: {error}"),
                },
                Err(error) => eprintln!("hosted shell view {path}: {error}"),
            }
        }
    } else if was_hosted {
        drop(hosted.take());
        if let Err(error) = unsafe { RemovePropW(hwnd, w!("NaviNativeView")) } {
            eprintln!("hosted palette cleanup: {error}");
        }
        if unsafe { GetForegroundWindow() } == hwnd {
            if let Err(error) =
                unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd)) }
            {
                if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } != hwnd {
                    eprintln!("hosted focus restore: {error}");
                }
            }
        }
    }
    if let Some(host) = hosted {
        HOSTS.with(|hosts| hosts.borrow_mut().insert(owner, host));
    }
    resume(hwnd);
}

fn resume(hwnd: HWND) {
    let owner = hwnd.0 as isize;
    BUSY.with(|busy| busy.borrow_mut().remove(&owner));
    if QUEUE.with(|queue| queue.borrow().iter().any(|(window, _, _)| *window == owner)) {
        if let Err(error) = unsafe { PostMessageW(Some(hwnd), READY, WPARAM(0), LPARAM(0)) } {
            eprintln!("hosted deferred navigation: {error}");
        }
    }
}

fn with_host<T>(hwnd: HWND, action: impl FnOnce(&mut Host) -> T) -> Option<T> {
    let owner = hwnd.0 as isize;
    if BUSY.with(|busy| !busy.borrow_mut().insert(owner)) {
        return None;
    }
    let Some(mut host) = HOSTS.with(|hosts| hosts.borrow_mut().remove(&owner)) else {
        resume(hwnd);
        return None;
    };
    let result = action(&mut host);
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        HOSTS.with(|hosts| hosts.borrow_mut().insert(owner, host));
    }
    resume(hwnd);
    Some(result)
}

pub fn refresh(hwnd: HWND) -> bool {
    with_host(hwnd, |host| host.browser.refresh()).is_some()
}

pub fn focus(hwnd: HWND) {
    with_host(hwnd, |host| host.browser.focus());
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    let mods = super::input::modifiers();
    if mods.ctrl && mods.shift && matches!(key, 0x31..=0x38) {
        return with_host(hwnd, |host| host.browser.set_view((key - 0x30) as i32)).is_some();
    }
    if key == 0x74 {
        return refresh(hwnd);
    }
    false
}

pub fn message(message: &MSG) -> bool {
    if !matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
        return false;
    }
    let owner = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
    if owner == message.hwnd {
        return false;
    }
    let ours = HOSTS.with(|hosts| hosts.borrow().contains_key(&(owner.0 as isize)));
    if !ours {
        return false;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(message.hwnd, &mut rect) }.is_err() {
        return false;
    }
    let mut point = windows::Win32::Foundation::POINT {
        x: rect.left,
        y: rect.top,
    };
    if !unsafe { windows::Win32::Graphics::Gdi::ScreenToClient(owner, &mut point) }.as_bool() {
        return false;
    }
    let inside = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(owner.0 as isize))
            .is_some_and(|host| point.y >= host.rect.top)
    });
    if !inside {
        return false;
    }
    if matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let key = message.wParam.0 as u32;
        let mods = super::input::modifiers();
        let alt = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
        if key == 0x75
            || key == 0x74
            || alt && matches!(key, 0x44 | 0x50 | 0x25 | 0x27 | 0x26)
            || mods.ctrl
                && (matches!(key, 0x4c | 0x45 | 0x46 | 0x4e | 0x54 | 0x57 | 0x09)
                    || mods.shift && matches!(key, 0x31..=0x38))
        {
            let _context = super::context::enter(owner);
            super::chrome::list_focus(owner);
            unsafe {
                SendMessageW(
                    owner,
                    message.message,
                    Some(message.wParam),
                    Some(message.lParam),
                );
            }
            return true;
        }
    }
    with_host(owner, |host| host.browser.translate(message)).unwrap_or(false)
}

pub fn close(hwnd: HWND) {
    let host = HOSTS.with(|hosts| hosts.borrow_mut().remove(&(hwnd.0 as isize)));
    drop(host);
    QUEUE.with(|queue| {
        queue
            .borrow_mut()
            .retain(|(owner, _, _)| *owner != hwnd.0 as isize)
    });
}
