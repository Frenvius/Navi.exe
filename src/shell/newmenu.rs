//! The shell's own "Novo" menu (CLSID_NewMenu): the same entries Explorer offers, including
//! the ones packaged apps register, created through the shell so templates and handlers run.

use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{IContextMenu, IContextMenu3, IShellExtInit, CMF_NORMAL};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, GetSubMenu, HMENU,
    MENUITEMINFOW, MFT_SEPARATOR, MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU, WM_INITMENUPOPUP,
};

use super::{com_init, ops, Pidl};

const CLSID_NEW_MENU: GUID = GUID::from_u128(0xD969A300_E7FF_11D0_A93B_00A0C90F2719);
const FIRST_ID: u32 = 1;

#[derive(Clone, Debug)]
pub struct NewEntry {
    pub label: String,
    /// The shell verb ("NewFolder", "NewLink" or the extension, e.g. ".txt").
    pub verb: String,
}

struct Menu {
    cm: IContextMenu,
    menu: HMENU,
    popup: HMENU,
}

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.menu);
        }
    }
}

unsafe fn build(hwnd: HWND, folder: &str) -> Result<Menu, String> {
    let cm: IContextMenu =
        CoCreateInstance(&CLSID_NEW_MENU, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
    let pidl = Pidl::parse(folder)?;
    cm.cast::<IShellExtInit>()
        .map_err(|e| e.message())?
        .Initialize(Some(pidl.ptr()), None, Default::default())
        .map_err(|e| e.message())?;
    let menu = CreatePopupMenu().map_err(|e| e.message())?;
    let _ = cm.QueryContextMenu(menu, 0, FIRST_ID, 0x7FFF, CMF_NORMAL);
    // The handler adds one "Novo" popup and fills it when it is about to open.
    let popup = GetSubMenu(menu, 0);
    if popup.is_invalid() {
        let _ = DestroyMenu(menu);
        return Err("no New menu".into());
    }
    if let Ok(cm3) = cm.cast::<IContextMenu3>() {
        let _ = cm3.HandleMenuMsg2(WM_INITMENUPOPUP, WPARAM(popup.0 as usize), LPARAM(0), None);
    }
    let _ = hwnd;
    Ok(Menu { cm, menu, popup })
}

unsafe fn verb_of(cm: &IContextMenu, id: u32) -> String {
    let mut buf = [0u16; 256];
    let ok = cm.GetCommandString(
        (id - FIRST_ID) as usize,
        windows::Win32::UI::Shell::GCS_VERBW,
        None,
        windows::core::PSTR(buf.as_mut_ptr() as *mut u8),
        buf.len() as u32,
    );
    if ok.is_err() {
        return String::new();
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..len])
}

pub fn entries(hwnd: HWND, folder: &str) -> Result<Vec<NewEntry>, String> {
    com_init();
    unsafe {
        let menu = build(hwnd, folder)?;
        let mut out = Vec::new();
        for i in 0..GetMenuItemCount(Some(menu.popup)).max(0) {
            let mut text = [0u16; 260];
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_ID | MIIM_STRING | MIIM_SUBMENU,
                dwTypeData: PWSTR(text.as_mut_ptr()),
                cch: text.len() as u32,
                ..Default::default()
            };
            if GetMenuItemInfoW(menu.popup, i as u32, true, &mut info).is_err()
                || info.fType.contains(MFT_SEPARATOR)
            {
                continue;
            }
            let len = text.iter().position(|c| *c == 0).unwrap_or(0);
            let label = String::from_utf16_lossy(&text[..len]).replace('&', "");
            if label.is_empty() || info.wID < FIRST_ID {
                continue;
            }
            out.push(NewEntry {
                label,
                verb: verb_of(&menu.cm, info.wID),
            });
        }
        Ok(out)
    }
}

/// Runs the entry with `verb` in `folder` (the shell creates the item and names it).
pub fn create(hwnd: HWND, folder: &str, verb: &str) -> Result<(), String> {
    com_init();
    unsafe {
        let menu = build(hwnd, folder)?;
        for i in 0..GetMenuItemCount(Some(menu.popup)).max(0) {
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_ID,
                ..Default::default()
            };
            if GetMenuItemInfoW(menu.popup, i as u32, true, &mut info).is_ok()
                && info.wID >= FIRST_ID
                && verb_of(&menu.cm, info.wID) == verb
            {
                return ops::invoke(
                    &menu.cm,
                    hwnd,
                    None,
                    (info.wID - FIRST_ID) as usize,
                    false,
                    None,
                );
            }
        }
        Err(format!("no New entry for {verb}"))
    }
}
