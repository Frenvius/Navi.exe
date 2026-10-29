use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{IContextMenu, IContextMenu3, IShellExtInit, CMF_NORMAL};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, GetSubMenu, HMENU,
    MENUITEMINFOW, MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU,
    WM_INITMENUPOPUP,
};

use super::{com_init, ops, Pidl};

const CLSID_NEW_MENU: GUID = GUID::from_u128(0xD969A300_E7FF_11D0_A93B_00A0C90F2719);
const FIRST_ID: u32 = 1;

#[derive(Clone)]
pub struct NewEntry {
    pub label: String,
    pub verb: String,
    pub icon: Option<std::sync::Arc<super::icons::Rgba>>,
}

impl std::fmt::Debug for NewEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewEntry")
            .field("label", &self.label)
            .field("verb", &self.verb)
            .field(
                "icon",
                &self.icon.as_ref().map(|icon| (icon.width, icon.height)),
            )
            .finish()
    }
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
                fMask: MIIM_BITMAP | MIIM_FTYPE | MIIM_ID | MIIM_STRING | MIIM_SUBMENU,
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
            let verb = verb_of(&menu.cm, info.wID);
            let icon = if info.hbmpItem.0 as usize > 65535 && info.hbmpItem.0 as isize > 0 {
                Some(copy_bitmap(info.hbmpItem)?)
            } else {
                command_icon(&verb)
            };
            out.push(NewEntry {
                label,
                verb,
                icon: icon.map(std::sync::Arc::new),
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

unsafe fn copy_bitmap(
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
) -> Result<super::icons::Rgba, String> {
    use windows::Win32::Graphics::Gdi::*;
    let mut description = BITMAP::default();
    if GetObjectW(
        bitmap.into(),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut description as *mut _ as _),
    ) == 0
    {
        return Err(windows::core::Error::from_win32().message());
    }
    let (width, height) = (description.bmWidth, description.bmHeight.abs());
    if width <= 0 || height <= 0 || width > 512 || height > 512 {
        return Err("Invalid New menu bitmap dimensions".into());
    }
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = GetDC(None);
    if dc.is_invalid() {
        return Err(windows::core::Error::from_win32().message());
    }
    let rows = GetDIBits(
        dc,
        bitmap,
        0,
        height as u32,
        Some(pixels.as_mut_ptr() as _),
        &mut info,
        DIB_RGB_COLORS,
    );
    ReleaseDC(None, dc);
    if rows != height {
        return Err("Could not copy New menu bitmap".into());
    }
    let opaque = pixels.chunks_exact(4).all(|pixel| pixel[3] == 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if opaque {
            pixel[3] = 255;
        } else if pixel[3] > 0 {
            for index in 0..3 {
                pixel[index] = ((pixel[index] as u32 * 255 + pixel[3] as u32 / 2) / pixel[3] as u32)
                    .min(255) as u8;
            }
        }
    }
    Ok(super::icons::Rgba {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

fn registered_icon_location(verb: &str) -> Option<String> {
    use winreg::{enums::HKEY_CLASSES_ROOT, RegKey};
    let extension = if verb == "NewLink" {
        ".lnk"
    } else if verb.starts_with('.') {
        verb
    } else {
        return None;
    };
    let classes = RegKey::predef(HKEY_CLASSES_ROOT);
    let root = classes.open_subkey(extension).ok()?;
    if let Ok(path) = root
        .open_subkey("ShellNew")
        .and_then(|key| key.get_value::<String, _>("IconPath"))
    {
        return Some(path);
    }
    for creator in root.enum_keys().flatten() {
        let Ok(shell_new) = root.open_subkey(format!(r"{creator}\ShellNew")) else {
            continue;
        };
        let path = shell_new.get_value::<String, _>("IconPath").or_else(|_| {
            classes
                .open_subkey(format!(r"{creator}\DefaultIcon"))
                .and_then(|key| key.get_value::<String, _>(""))
        });
        if let Ok(path) = path {
            return Some(path);
        }
    }
    None
}

pub fn command_icon(verb: &str) -> Option<super::icons::Rgba> {
    registered_icon_location(verb).and_then(|path| super::icons::small_icon_at(&path))
}
