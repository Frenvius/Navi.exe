//! The classic shell context menu ("Mostrar mais opções"), tracked natively on the main window.

use std::cell::RefCell;

use windows::core::{Interface, PSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Shell::{
    DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, RemoveWindowSubclass,
    SetWindowSubclass, CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL, GCS_VERBW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW,
    SetForegroundWindow, TrackPopupMenuEx, MF_BYPOSITION, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};

use super::{com_init, ops};

thread_local! {
    static ACTIVE: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

unsafe extern "system" fn forward(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if matches!(
        msg,
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
    ) {
        if let Some(cm) = ACTIVE.with(|a| a.borrow().clone()) {
            if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                let mut result = LRESULT(0);
                if cm3
                    .HandleMenuMsg2(msg, wparam, lparam, Some(&mut result))
                    .is_ok()
                {
                    return result;
                }
            } else if let Ok(cm2) = cm.cast::<IContextMenu2>() {
                if cm2.HandleMenuMsg(msg, wparam, lparam).is_ok() {
                    return LRESULT(0);
                }
            }
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe fn verb_of(cm: &IContextMenu, id: usize) -> String {
    let mut buf = [0u16; 256];
    let ok = cm.GetCommandString(
        id,
        GCS_VERBW,
        None,
        PSTR(buf.as_mut_ptr() as *mut u8),
        buf.len() as u32,
    );
    if ok.is_err() {
        return String::new();
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..len])
}

/// The canonical verbs of the classic menu's top-level rows for the items.
pub fn verbs(hwnd: HWND, folder: &str, paths: &[String]) -> Vec<String> {
    commands(hwnd, folder, paths)
        .into_iter()
        .map(|c| c.verb)
        .collect()
}

/// Read-only shell capabilities, localized titles and enabled state. Never invokes a verb.
#[derive(Debug, Clone)]
pub struct Command {
    pub verb: String,
    pub title: String,
    pub enabled: bool,
}

pub fn commands(hwnd: HWND, folder: &str, paths: &[String]) -> Vec<Command> {
    com_init();
    unsafe {
        let Ok(cm) = ops::context_menu(hwnd, folder, paths) else {
            return Vec::new();
        };
        let Ok(menu) = CreatePopupMenu() else {
            return Vec::new();
        };
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL);
        let mut verbs = Vec::new();
        for i in 0..GetMenuItemCount(Some(menu)).max(0) {
            let id = GetMenuItemID(menu, i);
            if id != u32::MAX && id > 0 {
                let verb = verb_of(&cm, (id - 1) as usize);
                if !verb.is_empty() {
                    let mut text = [0u16; 512];
                    let n = GetMenuStringW(menu, i as u32, Some(&mut text), MF_BYPOSITION).max(0)
                        as usize;
                    let title = String::from_utf16_lossy(&text[..n])
                        .split('\t')
                        .next()
                        .unwrap_or("")
                        .replace("&&", "\u{1}")
                        .replace('&', "")
                        .replace('\u{1}', "&");
                    let state = GetMenuState(menu, i as u32, MF_BYPOSITION);
                    verbs.push(Command {
                        verb,
                        title,
                        enabled: state & 3 == 0,
                    });
                }
            }
        }
        let _ = DestroyMenu(menu);
        verbs
    }
}

/// Shows the menu at a client point (physical pixels). Returns the canonical verb that was
/// picked, if any. "rename" is not invoked; the caller starts in-place editing instead.
pub fn show(
    hwnd: HWND,
    folder: &str,
    paths: &[String],
    x: i32,
    y: i32,
    shift: bool,
) -> Result<Option<String>, String> {
    show_impl(hwnd, folder, paths, x, y, shift, false)
}

/// Network computers use the native classic menu directly on this Explorer build.
/// Explorer contributes the two browser commands; handle those inside this app.
pub fn show_network(
    hwnd: HWND,
    folder: &str,
    paths: &[String],
    x: i32,
    y: i32,
    shift: bool,
) -> Result<Option<String>, String> {
    show_impl(hwnd, folder, paths, x, y, shift, true)
}
fn show_impl(
    hwnd: HWND,
    folder: &str,
    paths: &[String],
    x: i32,
    y: i32,
    shift: bool,
    network: bool,
) -> Result<Option<String>, String> {
    com_init();
    unsafe {
        let cm = ops::context_menu(hwnd, folder, paths)?;
        let menu = CreatePopupMenu().map_err(|e| e.message())?;
        let mut flags = CMF_NORMAL | CMF_CANRENAME;
        if shift {
            flags |= CMF_EXTENDEDVERBS;
        }
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, flags);

        const NEW_TAB: i32 = 0x8001;
        const NEW_WINDOW: i32 = 0x8002;
        if network {
            use windows::core::PCWSTR;
            use windows::Win32::UI::WindowsAndMessaging::{InsertMenuW, MF_STRING};
            let native = (0..GetMenuItemCount(Some(menu)).max(0))
                .filter_map(|i| {
                    let id = GetMenuItemID(menu, i);
                    (id > 0 && id != u32::MAX).then(|| verb_of(&cm, (id - 1) as usize))
                })
                .collect::<Vec<_>>();
            if !native.iter().any(|v| v == "opennewtab") {
                let text = super::wide(crate::i18n::tr("context.open-tab"));
                let _ = InsertMenuW(
                    menu,
                    1,
                    MF_BYPOSITION | MF_STRING,
                    NEW_TAB as usize,
                    PCWSTR(text.as_ptr()),
                );
            }
            if !native.iter().any(|v| v == "opennewwindow") {
                let text = super::wide(crate::i18n::tr("context.open-window"));
                let _ = InsertMenuW(
                    menu,
                    2,
                    MF_BYPOSITION | MF_STRING,
                    NEW_WINDOW as usize,
                    PCWSTR(text.as_ptr()),
                );
            }
        }

        let mut pt = POINT { x, y };
        let _ = ClientToScreen(hwnd, &mut pt);

        ACTIVE.with(|a| *a.borrow_mut() = Some(cm.clone()));
        let _ = SetWindowSubclass(hwnd, Some(forward), 1, 0);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            pt.x,
            pt.y,
            hwnd,
            None,
        )
        .0;
        let _ = RemoveWindowSubclass(hwnd, Some(forward), 1);

        let result = if network && cmd == NEW_TAB {
            Ok(Some("opennewtab".into()))
        } else if network && cmd == NEW_WINDOW {
            Ok(Some("opennewwindow".into()))
        } else if cmd > 0 {
            let id = (cmd - 1) as usize;
            let verb = verb_of(&cm, id);
            if verb == "rename"
                || (network && matches!(verb.as_str(), "open" | "opennewtab" | "opennewwindow"))
            {
                Ok(Some(verb))
            } else {
                ops::invoke(&cm, hwnd, None, id, shift, Some(pt)).map(|_| Some(verb))
            }
        } else {
            Ok(None)
        };

        ACTIVE.with(|a| *a.borrow_mut() = None);
        let _ = DestroyMenu(menu);
        result
    }
}
