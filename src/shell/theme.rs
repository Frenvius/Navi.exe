//! Visual style parts drawn by uxtheme, the way Explorer's own controls draw them (the
//! navigation pane is a tree view in the "Explorer" style, "DarkMode_Explorer" when dark).

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows::Win32::UI::Controls::{CloseThemeData, DrawThemeBackground, OpenThemeData};

use super::icons::Rgba;
use super::wide;

/// Whether apps use the dark theme (Settings > Personalization > Colors), which Explorer
/// follows. Without the value Windows is light.
pub fn dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .is_ok_and(|light| light == 0)
}

/// The tree view's visual style class for the theme.
pub fn tree(dark: bool) -> &'static str {
    if dark {
        "DarkMode_Explorer::TreeView"
    } else {
        "Explorer::TreeView"
    }
}
pub const TVP_TREEITEM: i32 = 1;
pub const TVP_GLYPH: i32 = 2;
pub const TVP_HOTGLYPH: i32 = 4;
pub const TREIS_HOT: i32 = 2;
pub const TREIS_SELECTED: i32 = 3;
pub const TREIS_SELECTEDNOTFOCUS: i32 = 5;
pub const TREIS_HOTSELECTED: i32 = 6;
pub const GLPS_CLOSED: i32 = 1;
pub const GLPS_OPENED: i32 = 2;

/// A theme part drawn at `w` x `h` pixels, as premultiplied RGBA.
pub fn part(class: &str, part: i32, state: i32, w: i32, h: i32) -> Option<Rgba> {
    unsafe {
        let name = wide(class);
        let theme = OpenThemeData(Some(HWND::default()), windows::core::PCWSTR(name.as_ptr()));
        if theme.is_invalid() {
            return None;
        }
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            let _ = CloseThemeData(theme);
            return None;
        };
        let old = SelectObject(dc, bitmap.into());
        let rect = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let _ = DrawThemeBackground(theme, dc, part, state, &rect, None);
        let px = std::slice::from_raw_parts(bits as *const [u8; 4], (w * h) as usize);
        let pixels = px.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        let _ = CloseThemeData(theme);
        Some(Rgba {
            width: w as u32,
            height: h as u32,
            pixels,
        })
    }
}
