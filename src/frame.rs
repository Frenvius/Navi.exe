use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmDefWindowProc, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TABBEDWINDOW,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Caption button under the pointer, as the UI draws it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Button {
    #[default]
    None,
    Min,
    Max,
    Close,
}

/// Layout the UI reports so hit testing matches what is drawn (logical pixels).
#[derive(Clone, Copy, Default)]
pub struct Layout {
    /// Height of the title bar (tab strip).
    pub caption_height: f32,
    /// Right edge of the last interactive element in the tab strip (the "+" button).
    pub tabs_end: f32,
}

thread_local! {
    static NATIVE: RefCell<std::collections::HashMap<usize, Native>> = RefCell::new(std::collections::HashMap::new());
    static DARK: Cell<bool> = const { Cell::new(true) };
}

struct Native {
    layout: Layout,
    full_screen: Option<(WINDOWPLACEMENT, i32)>,
}

pub fn native_window(hwnd: HWND) {
    NATIVE.with(|windows| {
        windows.borrow_mut().insert(
            hwnd.0 as usize,
            Native {
                layout: Layout {
                    caption_height: 40.0,
                    tabs_end: 287.0,
                },
                full_screen: None,
            },
        );
    });
}

pub fn remove_native_window(hwnd: HWND) {
    NATIVE.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
}

pub fn set_native_tabs_end(hwnd: HWND, end: f32) {
    NATIVE.with(|windows| {
        if let Some(window) = windows.borrow_mut().get_mut(&(hwnd.0 as usize)) {
            window.layout.tabs_end = end;
        }
    });
}

pub fn is_full_screen_for(hwnd: HWND) -> bool {
    NATIVE.with(|windows| {
        windows
            .borrow()
            .get(&(hwnd.0 as usize))
            .is_some_and(|window| window.full_screen.is_some())
    })
}

fn full_screen(hwnd: HWND) -> Option<(WINDOWPLACEMENT, i32)> {
    NATIVE.with(|windows| {
        windows
            .borrow()
            .get(&(hwnd.0 as usize))
            .and_then(|window| window.full_screen)
    })
}

fn replace_full_screen(
    hwnd: HWND,
    value: Option<(WINDOWPLACEMENT, i32)>,
) -> Option<(WINDOWPLACEMENT, i32)> {
    let native = NATIVE.with(|windows| {
        windows
            .borrow_mut()
            .get_mut(&(hwnd.0 as usize))
            .map(|window| std::mem::replace(&mut window.full_screen, value))
    });
    native.flatten()
}

pub const CAPTION_W: [f32; 3] = [45.0, 45.0, 46.0];
pub const CAPTION_H: f32 = 29.0;

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

fn frame_thickness(hwnd: HWND) -> (i32, i32) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        (
            GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad,
            GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad,
        )
    }
}

fn is_maximized(hwnd: HWND) -> bool {
    unsafe { IsZoomed(hwnd).as_bool() }
}

fn client_point(hwnd: HWND, lparam: LPARAM) -> POINT {
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut pt);
    }
    pt
}

pub fn button_at(hwnd: HWND, pt: POINT) -> Button {
    let s = scale(hwnd);
    let mut rc = RECT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
    }
    if pt.y < 0 || pt.y as f32 >= CAPTION_H * s {
        return Button::None;
    }
    let mut right = rc.right as f32;
    for (i, w) in CAPTION_W.iter().enumerate().rev() {
        let left = right - w * s;
        if pt.x as f32 >= left && (pt.x as f32) < right {
            return [Button::Min, Button::Max, Button::Close][i];
        }
        right = left;
    }
    Button::None
}

fn hit_test(hwnd: HWND, lparam: LPARAM) -> u32 {
    if is_full_screen_for(hwnd) {
        return HTCLIENT;
    }
    let pt = client_point(hwnd, lparam);
    let s = scale(hwnd);
    let layout = NATIVE
        .with(|windows| {
            windows
                .borrow()
                .get(&(hwnd.0 as usize))
                .map(|window| window.layout)
        })
        .unwrap_or_default();
    let (_, fy) = frame_thickness(hwnd);
    if !is_maximized(hwnd) && pt.y >= 0 && pt.y < fy {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let (fx, _) = frame_thickness(hwnd);
        if pt.x < fx * 2 {
            return HTTOPLEFT;
        }
        if pt.x >= rc.right - fx * 2 {
            return HTTOPRIGHT;
        }
        if pt.y < (fy / 2).max(1) || button_at(hwnd, pt) == Button::None {
            return HTTOP;
        }
    }
    match button_at(hwnd, pt) {
        Button::Min => return HTMINBUTTON,
        Button::Max => return HTMAXBUTTON,
        Button::Close => return HTCLOSE,
        Button::None => {}
    }
    if (pt.y as f32) < layout.caption_height * s && (pt.x as f32) > layout.tabs_end * s {
        return HTCAPTION;
    }
    HTCLIENT
}

pub fn track_nc_leave(hwnd: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
    };
    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut tme);
    }
}

pub fn hit_button(ht: u32) -> Button {
    match ht {
        HTMINBUTTON => Button::Min,
        HTMAXBUTTON => Button::Max,
        HTCLOSE => Button::Close,
        _ => Button::None,
    }
}

pub unsafe fn non_client(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    default_proc: unsafe fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
) -> Option<LRESULT> {
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 => {
            let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let res = default_proc(hwnd, msg, wparam, lparam);
            params.rgrc[0].top = top;
            if is_maximized(hwnd) {
                let (_, fy) = frame_thickness(hwnd);
                params.rgrc[0].top += fy;
            }
            return Some(res);
        }
        WM_NCHITTEST => {
            let mut res = LRESULT(0);
            if DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut res).as_bool() {
                return Some(res);
            }
            let def = default_proc(hwnd, msg, wparam, lparam);
            if def.0 as u32 != HTCLIENT {
                return Some(def);
            }
            return Some(LRESULT(hit_test(hwnd, lparam) as isize));
        }
        _ => {}
    }
    None
}

pub fn theme_changed(lparam: LPARAM) -> bool {
    lparam.0 != 0
        && unsafe { windows::core::PCWSTR(lparam.0 as *const u16).to_string() }
            .is_ok_and(|name| name == "ImmersiveColorSet")
}

pub fn show_system_menu(hwnd: HWND, lparam: LPARAM) {
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        if menu.is_invalid() {
            return;
        }
        let max = is_maximized(hwnd);
        let set = |id: u32, enabled: bool| {
            let _ = EnableMenuItem(menu, id, if enabled { MF_ENABLED } else { MF_GRAYED });
        };
        set(SC_RESTORE, max);
        set(SC_MOVE, !max);
        set(SC_SIZE, !max);
        set(SC_MINIMIZE, true);
        set(SC_MAXIMIZE, !max);
        set(SC_CLOSE, true);
        let x = (lparam.0 & 0xFFFF) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            Some(0),
            hwnd,
            None,
        );
        if cmd.0 != 0 {
            let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd.0 as usize), LPARAM(0));
        }
    }
}

pub fn menu_region(hwnd: HWND, pixels: &[u8], width: usize, height: usize, scale: f32) {
    use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, RGN_OR};
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        for y in 0..height {
            let row = &pixels[y * width * 4..(y + 1) * width * 4];
            let first = (0..width).find(|&x| row[x * 4 + 3] != 0);
            let last = (0..width).rfind(|&x| row[x * 4 + 3] != 0);
            if let (Some(l), Some(r)) = (first, last) {
                let band = CreateRectRgn(
                    (l as f32 * scale).round() as i32,
                    (y as f32 * scale).round() as i32,
                    ((r + 1) as f32 * scale).round() as i32,
                    ((y + 1) as f32 * scale).round() as i32,
                );
                let _ = CombineRgn(Some(region), Some(region), Some(band), RGN_OR);
                let _ = DeleteObject(band.into());
            }
        }
        if windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
        }
    }
}

pub fn focus_debug(hwnd: HWND, zone: usize) {
    if hwnd.is_invalid() {
        return;
    }
    let name = crate::shell::wide("SlintFocusZone");
    unsafe {
        let _ = SetPropW(
            hwnd,
            windows::core::PCWSTR(name.as_ptr()),
            Some(windows::Win32::Foundation::HANDLE(zone as _)),
        );
    }
}

/// Read-only automation diagnostics on our own HWND; absent from normal sessions.
/// DWM's 1px frame around flyouts (0x00BBGGRR): MenuFlyouts (dropdowns) versus context menus.
pub fn flyout_border(dropdown: bool) -> u32 {
    match (DARK.with(|d| d.get()), dropdown) {
        (true, true) => 0x0012_1212,
        (true, false) => 0x003a_3a3a,
        (false, true) => 0x00ee_eeee,
        (false, false) => 0x00ec_ecec,
    }
}

/// Screen rectangle of the monitor work area containing a point (physical pixels).
pub fn work_area(x: i32, y: i32) -> RECT {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        info.rcWork
    }
}

pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    let mut pt = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }
    (pt.x, pt.y)
}

/// Opens the system menu under the window icon position (Alt+Space style), in screen pixels.
pub fn system_menu_at(hwnd: HWND, x: i32, y: i32) {
    let lparam = LPARAM(((y as u16 as isize) << 16) | x as u16 as isize);
    show_system_menu(hwnd, lparam);
}

pub fn toggle_full_screen(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        match replace_full_screen(hwnd, None) {
            Some((placement, style)) => {
                SetWindowLongW(hwnd, GWL_STYLE, style);
                let _ = SetWindowPlacement(hwnd, &placement);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
                false
            }
            None => {
                let mut placement = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                let _ = GetWindowPlacement(hwnd, &mut placement);
                let style = GetWindowLongW(hwnd, GWL_STYLE);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ =
                    GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
                replace_full_screen(hwnd, Some((placement, style)));
                SetWindowLongW(
                    hwnd,
                    GWL_STYLE,
                    (style & !(WS_CAPTION.0 | WS_THICKFRAME.0) as i32) | WS_POPUP.0 as i32,
                );
                let r = info.rcMonitor;
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
                true
            }
        }
    }
}

/// FlushMenuThemes (136).
fn allow_dark_menus(hwnd: HWND) {
    use windows::core::{s, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    unsafe {
        let Ok(lib) = LoadLibraryA(s!("uxtheme.dll")) else {
            return;
        };
        let ordinal = |n: usize| GetProcAddress(lib, PCSTR(n as *const u8));
        if let Some(f) = ordinal(135) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(1); // AllowDark: follow the system theme
        }
        if let Some(f) = ordinal(133) {
            let allow: unsafe extern "system" fn(HWND, i32) -> i32 = std::mem::transmute(f);
            allow(hwnd, 1);
        }
        if let Some(f) = ordinal(136) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

pub fn apply_dark(hwnd: HWND, dark: bool) -> windows::core::Result<()> {
    DARK.with(|value| value.set(dark));
    let value = dark as i32;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const _ as _,
            std::mem::size_of_val(&value) as u32,
        )
    }
}

pub fn prepare(hwnd: HWND) -> windows::core::Result<()> {
    allow_dark_menus(hwnd);
    let dark = crate::shell::theme::dark();
    DARK.with(|value| value.set(dark));
    apply_dark(hwnd, dark)?;
    unsafe {
        let backdrop = DWMSBT_TABBEDWINDOW;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as _,
            std::mem::size_of_val(&backdrop) as u32,
        )?;
        if std::env::var("NO_EXTEND").is_err() {
            DwmExtendFrameIntoClientArea(
                hwnd,
                &MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                },
            )?;
        }
        let menu = GetSystemMenu(hwnd, false);
        if menu.is_invalid() {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "System menu could not be initialized",
            ));
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
        if SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_SYSMENU.0 as isize)) == 0 {
            let error = windows::Win32::Foundation::GetLastError();
            if error.0 != 0 {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

const SETTINGS_KEY: windows::core::PCWSTR = windows::core::w!(r"Software\slint-win-explorer");
const PLACEMENT_VALUE: windows::core::PCWSTR = windows::core::w!("WindowPlacement");

pub fn normal_placement(hwnd: HWND) -> windows::core::Result<WINDOWPLACEMENT> {
    if let Some((placement, _)) = full_screen(hwnd) {
        return Ok(placement);
    }
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe {
        GetWindowPlacement(hwnd, &mut placement)?;
    }
    Ok(placement)
}

pub fn save_placement(hwnd: HWND) -> windows::core::Result<()> {
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY};
    let mut placement = normal_placement(hwnd)?;
    if placement.showCmd == SW_SHOWMINIMIZED.0 as u32 {
        placement.showCmd = SW_SHOWNORMAL.0 as u32;
    }
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            PLACEMENT_VALUE,
            REG_BINARY.0,
            Some(&placement as *const _ as *const _),
            std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        )
        .ok()
    }
}

pub fn restore_placement(hwnd: HWND) -> windows::core::Result<()> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut placement = WINDOWPLACEMENT::default();
    let mut size = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            PLACEMENT_VALUE,
            RRF_RT_REG_BINARY,
            None,
            Some(&mut placement as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    if matches!(
        read,
        windows::Win32::Foundation::ERROR_FILE_NOT_FOUND
            | windows::Win32::Foundation::ERROR_PATH_NOT_FOUND
            | windows::Win32::Foundation::ERROR_UNSUPPORTED_TYPE
            | windows::Win32::Foundation::ERROR_MORE_DATA
    ) {
        return Ok(());
    }
    read.ok()?;
    if size != std::mem::size_of::<WINDOWPLACEMENT>() as u32 || placement.length != size {
        return Ok(());
    }
    if placement.showCmd != SW_SHOWMAXIMIZED.0 as u32 {
        placement.showCmd = SW_SHOWNORMAL.0 as u32;
    }
    unsafe { SetWindowPlacement(hwnd, &placement) }
}
