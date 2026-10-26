//! Native window chrome: the client area extends over the caption (like Explorer's tabbed
//! title bar), Mica Alt shows through transparent pixels, and the caption buttons are hit-tested
//! as real non-client buttons so Snap Layouts and the system menu keep working.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmDefWindowProc, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TABBEDWINDOW,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
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

impl Button {
    pub fn index(self) -> i32 {
        match self {
            Button::None => 0,
            Button::Min => 1,
            Button::Max => 2,
            Button::Close => 3,
        }
    }
}

/// Layout the UI reports so hit testing matches what is drawn (logical pixels).
#[derive(Clone, Copy, Default)]
pub struct Layout {
    /// Height of the title bar (tab strip).
    pub caption_height: f32,
    /// Right edge of the last interactive element in the tab strip (the "+" button).
    pub tabs_end: f32,
}

pub trait Host {
    fn hover(&self, button: Button, pressed: bool);
    fn maximized_changed(&self, maximized: bool);
    fn active_changed(&self, active: bool);
    /// Mouse back (false) / forward (true) buttons.
    fn x_button(&self, forward: bool);
    /// The wheel turned (with Ctrl held or not) at a client point (logical pixels), by `delta`
    /// (120 a notch). Returns whether it was used.
    fn wheel(&self, x: f32, y: f32, delta: i32, ctrl: bool) -> bool;
    /// The system switched between the light and dark themes.
    fn theme_changed(&self);
    /// Folder Options changed (hidden items, extensions, check boxes...): the shell's
    /// "ShellState" notice, sent when Explorer or the dialog applies them.
    fn settings_changed(&self);
}

thread_local! {
    /// Our menu popup windows: activating one must not make the main window look inactive.
    static POPUPS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    static POPUP_OWNER: Cell<Option<HWND>> = const { Cell::new(None) };
    static POPUP_DISMISS: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    static HOST: RefCell<Option<Box<dyn Host>>> = const { RefCell::new(None) };
    static LAYOUT: Cell<Layout> = Cell::new(Layout { caption_height: 39.0, tabs_end: 0.0 });
    static PRESSED: Cell<Button> = const { Cell::new(Button::None) };
    static MAIN: Cell<Option<HWND>> = const { Cell::new(None) };
    /// Full screen (F11): the window's place and style to go back to.
    static FULL_SCREEN: Cell<Option<(WINDOWPLACEMENT, i32)>> = const { Cell::new(None) };
    // Set while a menu hands the window its real (inactive) look as focus leaves the app.
    static LEAVING: Cell<bool> = const { Cell::new(false) };
    // The system theme is dark: DWM's frame and backdrop follow it.
    static DARK: Cell<bool> = const { Cell::new(true) };
    static HOVER: Cell<Button> = const { Cell::new(Button::None) };
}

pub fn set_tabs_end(x: f32) {
    LAYOUT.with(|l| {
        let mut layout = l.get();
        layout.tabs_end = x;
        l.set(layout);
    });
}

const CAPTION_W: [f32; 3] = [45.0, 45.0, 46.0];
const CAPTION_H: f32 = 29.0;

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

/// Shell-owned classic views use an opaque client instead of the tabbed Mica surface.
pub fn classic_view(hwnd:HWND,classic:bool){
    thread_local!{static CLASSIC:Cell<bool>=const{Cell::new(false)};}
    thread_local!{static HAD_CLIP:Cell<bool>=const{Cell::new(false)};}
    if hwnd.0.is_null()||CLASSIC.with(|value|value.replace(classic)==classic){return;}
    unsafe{
        // The software renderer must not repaint over shell child HWNDs.
        let style=GetWindowLongPtrW(hwnd,GWL_STYLE);
        if classic{HAD_CLIP.with(|value|value.set(style&WS_CLIPCHILDREN.0 as isize!=0));SetWindowLongPtrW(hwnd,GWL_STYLE,style|WS_CLIPCHILDREN.0 as isize);}
        else if !HAD_CLIP.with(|value|value.get()){SetWindowLongPtrW(hwnd,GWL_STYLE,style&!(WS_CLIPCHILDREN.0 as isize));}
        // Winit enabled per-pixel alpha with an empty blur region when this window
        // was created. GDI shell child text writes no alpha; disable that policy in
        // an opaque native view, then restore Winit's exact empty-region policy.
        use windows::Win32::Graphics::Dwm::{DwmEnableBlurBehindWindow,DWM_BLURBEHIND,DWM_BB_ENABLE,DWM_BB_BLURREGION};
        let region=if classic{Default::default()}else{windows::Win32::Graphics::Gdi::CreateRectRgn(0,0,-1,-1)};
        let _=DwmEnableBlurBehindWindow(hwnd,&DWM_BLURBEHIND{dwFlags:DWM_BB_ENABLE|if classic{0}else{DWM_BB_BLURREGION},fEnable:(!classic).into(),hRgnBlur:region,fTransitionOnMaximized:false.into()});
        if !region.is_invalid(){let _=windows::Win32::Graphics::Gdi::DeleteObject(region.into());}
        let backdrop=if classic{windows::Win32::Graphics::Dwm::DWMSBT_NONE}else{DWMSBT_TABBEDWINDOW};
        let _=DwmSetWindowAttribute(hwnd,DWMWA_SYSTEMBACKDROP_TYPE,&backdrop as *const _ as _,std::mem::size_of_val(&backdrop) as u32);
        let v=if classic{0}else{-1};
        let _=DwmExtendFrameIntoClientArea(hwnd,&MARGINS{cxLeftWidth:v,cxRightWidth:v,cyTopHeight:v,cyBottomHeight:v});
    }
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

fn button_at(hwnd: HWND, pt: POINT) -> Button {
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
    // Full screen has no caption, buttons nor borders: it's all content.
    if is_full_screen() {
        return HTCLIENT;
    }
    let pt = client_point(hwnd, lparam);
    let s = scale(hwnd);
    let layout = LAYOUT.with(|l| l.get());
    let (_, fy) = frame_thickness(hwnd);
    if !is_maximized(hwnd) && pt.y >= 0 && pt.y < fy {
        // The caption covers the top resize border, so give part of it back.
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

fn with_host(f: impl FnOnce(&dyn Host)) {
    HOST.with(|h| {
        if let Some(host) = h.borrow().as_ref() {
            f(host.as_ref());
        }
    });
}

fn set_hover(button: Button) {
    let pressed = PRESSED.with(|p| p.get());
    if HOVER.with(|h| h.replace(button)) != button || pressed != Button::None {
        with_host(|h| h.hover(button, pressed != Button::None && pressed == button));
    }
}

fn track_nc_leave(hwnd: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
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

fn hit_button(ht: u32) -> Button {
    match ht {
        HTMINBUTTON => Button::Min,
        HTMAXBUTTON => Button::Max,
        HTCLOSE => Button::Close,
        _ => Button::None,
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if let Some(result)=crate::app::entry::message(hwnd,msg,wparam,lparam){return result;}
    if let Some(result)=crate::app::tab_transfer::message(hwnd,msg,wparam,lparam){return result;}
    match msg {
        WM_QUERYENDSESSION => {
            crate::app::with_app(|app|{crate::app::session::ending(app,false);app.save_view();});
            return LRESULT(1);
        }
        WM_ENDSESSION if wparam.0!=0 => {
            crate::app::with_app(|app|{crate::app::session::ending(app,true);app.save_view();});
        }
        WM_CLOSE => save_placement(hwnd),
        crate::watch::SHELL_CHANGE_MSG => {
            crate::watch::shell_notified(wparam.0, lparam.0);
            return LRESULT(0);
        }
        WM_SETTINGCHANGE if lparam.0 != 0 && windows::core::PCWSTR(lparam.0 as *const u16).to_string().is_ok_and(|s| s == "ShellState") => {
            with_host(|h| h.settings_changed());
        }
        WM_SETTINGCHANGE if lparam.0 != 0 && windows::core::PCWSTR(lparam.0 as *const u16).to_string().is_ok_and(|s| s == "ImmersiveColorSet") => {
            let dark = crate::shell::theme::dark();
            if dark != DARK.with(|d| d.get()) {
                set_dark(dark);
                with_host(|h| h.theme_changed());
            }
        }
        WM_NCCALCSIZE if wparam.0 != 0 => {
            let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let res = DefSubclassProc(hwnd, msg, wparam, lparam);
            // Drop the caption (and the top border) the default handler reserved.
            params.rgrc[0].top = top;
            if is_maximized(hwnd) {
                let (_, fy) = frame_thickness(hwnd);
                params.rgrc[0].top += fy;
            }
            return res;
        }
        WM_NCHITTEST => {
            let mut res = LRESULT(0);
            if DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut res).as_bool() {
                return res;
            }
            let def = DefSubclassProc(hwnd, msg, wparam, lparam);
            if def.0 as u32 != HTCLIENT {
                return def;
            }
            return LRESULT(hit_test(hwnd, lparam) as isize);
        }
        WM_NCMOUSEMOVE => {
            track_nc_leave(hwnd);
            set_hover(hit_button(wparam.0 as u32));
            if hit_button(wparam.0 as u32) != Button::None {
                return LRESULT(0);
            }
        }
        WM_NCMOUSELEAVE | WM_MOUSELEAVE => {
            PRESSED.with(|p| p.set(Button::None));
            set_hover(Button::None);
        }
        WM_MOUSEMOVE => {
            if HOVER.with(|h| h.get()) != Button::None {
                PRESSED.with(|p| p.set(Button::None));
                set_hover(Button::None);
            }
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            let b = hit_button(wparam.0 as u32);
            if b != Button::None {
                PRESSED.with(|p| p.set(b));
                with_host(|h| h.hover(b, true));
                return LRESULT(0);
            }
        }
        WM_NCLBUTTONUP => {
            let b = hit_button(wparam.0 as u32);
            let pressed = PRESSED.with(|p| p.replace(Button::None));
            if b != Button::None {
                with_host(|h| h.hover(b, false));
                if b == pressed {
                    let cmd = match b {
                        Button::Min => SC_MINIMIZE,
                        Button::Max if is_maximized(hwnd) => SC_RESTORE,
                        Button::Max => SC_MAXIMIZE,
                        _ => SC_CLOSE,
                    };
                    let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
                }
                return LRESULT(0);
            }
        }
        WM_NCRBUTTONUP if wparam.0 as u32 == HTCAPTION => {
            show_system_menu(hwnd, lparam);
            return LRESULT(0);
        }
        WM_SIZE => {
            let max = wparam.0 as u32 == SIZE_MAXIMIZED;
            with_host(|h| h.maximized_changed(max));
            // The tabs' titles are cut to their room, which follows the width.
            crate::app::post(|app| app.sync_tabs());
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let mut pt = POINT { x: (lparam.0 & 0xFFFF) as u16 as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32 };
            let _ = ScreenToClient(hwnd, &mut pt);
            let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let mut used = false;
            with_host(|h| used = h.wheel(pt.x as f32 / scale, pt.y as f32 / scale, delta, wparam.0 & 0x0008 != 0));
            if used {
                return LRESULT(0);
            }
        }
        // Escape during a drag of ours (its loop runs on another thread) cancels it.
        WM_KEYDOWN if wparam.0 == 0x1B && crate::dnd::cancel_drag() => {
            return LRESULT(0);
        }
        // Keep New Window available after focus moves between native/Slint views,
        // including This PC's tiles. Consume the event to avoid a duplicate launch.
        WM_KEYDOWN if wparam.0 == 0x4E && ctrl_only() => {
            crate::app::post(|app|app.new_window());
            return LRESULT(0);
        }
        // Win32 system arrow keys do not consistently reach Slint's focused view.
        // In particular a hover-selected item/empty destination must keep Alt+Up working.
        WM_SYSKEYDOWN if matches!(wparam.0,0x25..=0x27) && alt_only() => {
            let key=wparam.0;
            let _=slint::invoke_from_event_loop(move||crate::app::with_app(|a|{
                a.ui().invoke_consume_alt();
                match key {0x25=>a.back(),0x27=>a.forward(),_=>a.up()}
            }));
            return LRESULT(0);
        }
        // Ctrl+Shift+1..8 picks a view by the key itself: on some layouts (ABNT2's 6) the
        // shifted digit is a dead key, which types nothing.
        WM_KEYDOWN if (0x31..=0x38).contains(&wparam.0) && ctrl_shift() => {
            let mode = wparam.0 as i32 - 0x30;
            let _ = slint::invoke_from_event_loop(move || crate::app::with_app(|a| a.set_view(mode)));
            return LRESULT(0);
        }
        // Ctrl with the keypad's + sizes the details columns to fit.
        WM_KEYDOWN if wparam.0 == 0x6B && ctrl_only() => {
            let _ = slint::invoke_from_event_loop(|| crate::app::with_app(|a| a.fit_all_key()));
            return LRESULT(0);
        }
        WM_XBUTTONUP => {
            let forward = ((wparam.0 >> 16) & 0xFFFF) as u16 == 2;
            with_host(|h| h.x_button(forward));
            return LRESULT(1);
        }
        // A menu of ours taking activation leaves the window (and its Mica) looking active,
        // as Explorer's own flyouts, which live in its window, do.
        WM_NCACTIVATE if wparam.0 == 0 && popup_shown() && !LEAVING.with(|l| l.get()) => {
            return DefSubclassProc(hwnd, msg, WPARAM(1), lparam);
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE;
            let to_popup = !active && is_popup(HWND(lparam.0 as _));
            if !to_popup {
                with_host(|h| h.active_changed(active));
            }
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

fn show_system_menu(hwnd: HWND, lparam: LPARAM) {
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
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, Some(0), hwnd, None);
        if cmd.0 != 0 {
            let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd.0 as usize), LPARAM(0));
        }
    }
}

/// Runs `make` (which creates a menu window) with `owner` as the owner of the windows it
/// creates; see [`popup_owner`].
pub fn creating_popup<T>(owner: HWND, make: impl FnOnce() -> T) -> T {
    POPUP_OWNER.with(|o| o.set(Some(owner)));
    let made = make();
    POPUP_OWNER.with(|o| o.set(None));
    made
}

/// The owner for a window being created as a menu, if one is.
pub fn popup_owner() -> Option<HWND> {
    POPUP_OWNER.with(|o| o.get())
}

/// A transparent drag preview is owned, non-activating, and never intercepts OLE input.
pub fn style_drop_preview(hwnd: HWND) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let required = WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize | WS_EX_TRANSPARENT.0 as isize;
        if ex & required == required && ex & WS_EX_APPWINDOW.0 as isize == 0 { return; }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE,
            (ex | required)
                & !(WS_EX_APPWINDOW.0 as isize));
        let _ = SetWindowSubclass(hwnd, Some(drop_preview_subclass), 1, 0);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}

unsafe extern "system" fn drop_preview_subclass(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}

fn is_popup(hwnd: HWND) -> bool {
    !hwnd.is_invalid() && POPUPS.with(|p| p.borrow().contains(&hwnd))
}

fn popup_shown() -> bool {
    POPUPS.with(|p| p.borrow().iter().any(|&h| unsafe { IsWindowVisible(h).as_bool() }))
}

/// Before a focused menu level hides, hands activation to `to` (the menu below it or the
/// main window); otherwise Windows activates whatever window is next in the z-order.
pub fn hand_focus_back(closing: HWND, to: HWND) {
    unsafe {
        if !closing.is_invalid() && GetForegroundWindow() == closing {
            let _ = SetForegroundWindow(to);
        }
    }
}

unsafe extern "system" fn popup_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // The whole popup is client area; its compact DWM treatment never resizes.
    if msg == WM_NCCALCSIZE && wparam.0 != 0 {
        return LRESULT(0);
    }
    if msg == WM_NCHITTEST {
        if data == 1 {
            let mut point = POINT { x: (lparam.0 as i16) as i32, y: ((lparam.0 >> 16) as i16) as i32 };
            let _ = ScreenToClient(hwnd, &mut point);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as f32 / 96.0;
            let (x, y) = (point.x as f32 / scale - 10.0, point.y as f32 / scale - 2.0);
            let (w, h) = (rect.right as f32 / scale - 20.0, rect.bottom as f32 / scale - 20.0);
            let (cx, cy) = (x.clamp(8.0, w - 8.0), y.clamp(8.0, h - 8.0));
            if x < 0.0 || y < 0.0 || x >= w || y >= h || (x - cx).powi(2) + (y - cy).powi(2) > 64.0 {
                return LRESULT(HTTRANSPARENT as isize);
            }
        }
        return LRESULT(HTCLIENT as isize);
    }
    // DefWindowProc paints a classic caption straight over the client on activation;
    // lparam -1 keeps the activation handling without the repaint.
    if msg == WM_NCACTIVATE {
        return DefSubclassProc(hwnd, msg, wparam, LPARAM(-1));
    }
    if msg == WM_NCPAINT {
        return LRESULT(0);
    }
    // winit re-applies its own styles now and then; keep the popup's.
    if msg == WM_STYLECHANGING && wparam.0 as i32 == GWL_STYLE.0 {
        let change = &mut *(lparam.0 as *mut STYLESTRUCT);
        change.styleNew = popup_style(change.styleNew);
    }
    if msg == WM_STYLECHANGING && wparam.0 as i32 == GWL_EXSTYLE.0 {
        let change = &mut *(lparam.0 as *mut STYLESTRUCT);
        change.styleNew = (change.styleNew | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0;
    }
    if msg == WM_ACTIVATE && (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE && !is_popup(HWND(lparam.0 as _)) {
        // Focus left for another app: the window, kept looking active, now looks inactive.
        if let Some(main) = MAIN.with(|m| m.get()).filter(|&m| m != HWND(lparam.0 as _)) {
            LEAVING.with(|l| l.set(true));
            SendMessageW(main, WM_NCACTIVATE, Some(WPARAM(0)), Some(LPARAM(0)));
            LEAVING.with(|l| l.set(false));
            with_host(|h| h.active_changed(false));
        }
        // Light dismiss: focus went somewhere other than another menu level.
        let _ = slint::invoke_from_event_loop(|| POPUP_DISMISS.with(|d| {
            if let Some(f) = d.borrow().as_ref() {
                f();
            }
        }));
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Menus use the compact popup treatment, without a resizable window frame.
fn popup_style(style: u32) -> u32 {
    let without = WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_THICKFRAME.0;
    style & !without
}

pub fn set_popup_dismiss(f: Box<dyn Fn()>) {
    POPUP_DISMISS.with(|d| *d.borrow_mut() = Some(f));
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut std::ffi::c_void,
    size: usize,
}

/// Acrylic blur behind a borderless window (the system backdrop types need a frame),
/// through user32's SetWindowCompositionAttribute. `tint` is 0xAABBGGRR.
fn acrylic(hwnd: HWND, tint: u32) {
    use windows::core::s;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    unsafe {
        let Ok(user32) = GetModuleHandleA(s!("user32.dll")) else { return };
        let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return };
        let set: unsafe extern "system" fn(HWND, *mut CompositionData) -> i32 = std::mem::transmute(f);
        // ACCENT_ENABLE_ACRYLICBLURBEHIND = 4, WCA_ACCENT_POLICY = 19
        let mut policy = AccentPolicy { state: 4, flags: 0, gradient: tint, animation: 0 };
        let mut data = CompositionData {
            attribute: 19,
            data: &mut policy as *mut _ as *mut _,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set(hwnd, &mut data);
    }
}

/// Turns a borderless Slint window into a flyout: owned by `owner`, no taskbar button,
/// acrylic backdrop, rounded corners and the theme's frame.
pub fn style_popup(hwnd: HWND, owner: HWND) {
    if std::env::var("DEBUG_POPUP").is_ok() {
        eprintln!("style_popup {:?} owner {:?}", hwnd.0, owner.0);
    }
    use windows::Win32::Graphics::Dwm::{DWMSBT_TRANSIENTWINDOW, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL,
        DWMWA_NCRENDERING_POLICY, DWMNCRP_DISABLED};
    POPUPS.with(|p| p.borrow_mut().push(hwnd));
    // The popup activated before it was known as ours: the main window is still the active one.
    with_host(|h| h.active_changed(true));
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize));
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, popup_style(style as u32) as isize);
        SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner.0 as isize);
        let dark = DARK.with(|d| d.get()) as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        let backdrop = DWMSBT_TRANSIENTWINDOW;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const _, std::mem::size_of_val(&backdrop) as u32);
        let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR, &flyout_border(true) as *const _ as *const _, 4);
        // Menus appear in place, without a window's open and close animations.
        let off: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_TRANSITIONS_FORCEDISABLED, &off as *const _ as *const _, 4);
        // ROUND uses the long shadow of a full window. ROUNDSMALL uses the compact
        // popup profile (about 18px below), while keeping the native acrylic backdrop.
        let policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, &policy as *const _ as *const _, 4);
        let corner = DWMWCP_ROUNDSMALL;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, std::mem::size_of_val(&corner) as u32);
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let _ = SetWindowSubclass(hwnd, Some(popup_subclass), 0x5E2, 0);
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Menus provide their own measured RGBA chrome. DWM's small corner preset couples
/// a 4px radius to the compact shadow; the round preset adds a full-window shadow.
pub fn style_menu_popup(hwnd: HWND, owner: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMSBT_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
    style_popup(hwnd, owner);
    unsafe {
        let backdrop = DWMSBT_NONE;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const _, 4);
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        let border = 0xFFFFFFFEu32;
        let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
        let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS::default());
        let _ = SetWindowSubclass(hwnd, Some(popup_subclass), 0x5E2, 1);
    }
}

/// The binary window region only removes fully transparent pixels. Partial corner
/// coverage remains in the RGBA surface, avoiding the jaggies of CreateRoundRectRgn.
pub fn menu_region(hwnd: HWND, pixels: &[u8], width: usize, height: usize, scale: f32) {
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, CombineRgn, DeleteObject, RGN_OR};
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        for y in 0..height {
            let row = &pixels[y * width * 4..(y + 1) * width * 4];
            let first = (0..width).find(|&x| row[x * 4 + 3] != 0);
            let last = (0..width).rfind(|&x| row[x * 4 + 3] != 0);
            if let (Some(l), Some(r)) = (first, last) {
                let band = CreateRectRgn((l as f32 * scale).round() as i32, (y as f32 * scale).round() as i32,
                    ((r + 1) as f32 * scale).round() as i32, ((y + 1) as f32 * scale).round() as i32);
                let _ = CombineRgn(Some(region), Some(region), Some(band), RGN_OR);
                let _ = DeleteObject(band.into());
            }
        }
        if windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
        }
    }
}

pub fn activate(hwnd: HWND) {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Read-only automation diagnostics on our own HWND; absent from normal sessions.
pub fn focus_debug(hwnd: HWND, zone: usize) {
    if hwnd.is_invalid(){return;}
    let name=crate::shell::wide("SlintFocusZone");
    unsafe {let _=SetPropW(hwnd,windows::core::PCWSTR(name.as_ptr()),Some(windows::Win32::Foundation::HANDLE(zone as _)));}
}

/// Read-only automation diagnostics on our own HWND; absent from normal sessions.
pub fn pc_scroll_debug(hwnd: HWND, offset: f32, view: f32, extent: f32, units: f32, focus: i32, group: i32, selected: usize) {
    if hwnd.is_invalid() || std::env::var_os("PC_SCROLL_TRACE").is_none() {return;}
    for (name,value) in [("SlintPcOffset",offset.round() as i32),("SlintPcView",view.round() as i32),
        ("SlintPcExtent",extent.round() as i32),("SlintPcUnits",units.round() as i32),
        ("SlintPcFocus",focus),("SlintPcGroup",group),("SlintPcSelected",selected as i32)] {
        let name=crate::shell::wide(name);
        unsafe {let _=SetPropW(hwnd,windows::core::PCWSTR(name.as_ptr()),Some(windows::Win32::Foundation::HANDLE(value as isize as _)));}
    }
}

/// Hides a window from the screen while it keeps rendering (DWMWA_CLOAK).
pub fn cloak(hwnd: HWND, on: bool) {
    let value: i32 = on as i32;
    unsafe {
        // Let DWM take in the frame just presented first.
        if !on {
            let _ = windows::Win32::Graphics::Dwm::DwmFlush();
        }
        let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_CLOAK, &value as *const _ as *const _, 4);
    }
}

/// DWM's 1px frame around flyouts (0x00BBGGRR): MenuFlyouts (dropdowns) versus context menus.
pub fn flyout_border(dropdown: bool) -> u32 {
    match (DARK.with(|d| d.get()), dropdown) {
        (true, true) => 0x0012_1212,
        (true, false) => 0x003a_3a3a,
        (false, true) => 0x00ee_eeee,
        (false, false) => 0x00ec_ecec,
    }
}

/// Switches DWM's frame and backdrop of our windows, and the classic menus, to the theme.
pub fn set_dark(dark: bool) {
    DARK.with(|d| d.set(dark));
    let value = dark as i32;
    let windows: Vec<HWND> = MAIN.with(|m| m.get()).into_iter().chain(POPUPS.with(|p| p.borrow().clone())).collect();
    for hwnd in windows {
        unsafe {
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &value as *const _ as *const _, 4);
        }
    }
    flush_menu_themes();
}

pub fn set_border_color(hwnd: HWND, color: u32) {
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR, &color as *const _ as *const _, 4);
    }
}

/// Screen rectangle of the monitor work area containing a point (physical pixels).
pub fn work_area(x: i32, y: i32) -> RECT {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
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

/// Dark classic menus (TrackPopupMenu, shell context menus) like Explorer's, through
/// uxtheme's unnamed exports: SetPreferredAppMode (135), AllowDarkModeForWindow (133) and
/// Whether the window is full screen (F11).
pub fn is_full_screen() -> bool {
    FULL_SCREEN.with(|f| f.get().is_some())
}

/// F11, as Explorer: the window covers its monitor without a frame (the app hides its tabs,
/// address and command bars), and F11 again puts it back where it was. Returns whether it's
/// full screen now.
pub fn toggle_full_screen(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        match FULL_SCREEN.with(|f| f.take()) {
            Some((placement, style)) => {
                SetWindowLongW(hwnd, GWL_STYLE, style);
                let _ = SetWindowPlacement(hwnd, &placement);
                let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED);
                false
            }
            None => {
                let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                let _ = GetWindowPlacement(hwnd, &mut placement);
                let style = GetWindowLongW(hwnd, GWL_STYLE);
                let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
                FULL_SCREEN.with(|f| f.set(Some((placement, style))));
                SetWindowLongW(hwnd, GWL_STYLE, (style & !(WS_CAPTION.0 | WS_THICKFRAME.0) as i32) | WS_POPUP.0 as i32);
                let r = info.rcMonitor;
                let _ = SetWindowPos(hwnd, Some(HWND_TOP), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOOWNERZORDER | SWP_FRAMECHANGED);
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
        let Ok(lib) = LoadLibraryA(s!("uxtheme.dll")) else { return };
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

/// Makes classic menus pick up a theme change: uxtheme's RefreshImmersiveColorPolicyState (104)
/// and FlushMenuThemes (136).
fn flush_menu_themes() {
    use windows::core::{s, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    unsafe {
        let Ok(lib) = LoadLibraryA(s!("uxtheme.dll")) else { return };
        for n in [104usize, 136] {
            if let Some(f) = GetProcAddress(lib, PCSTR(n as *const u8)) {
                let call: unsafe extern "system" fn() = std::mem::transmute(f);
                call();
            }
        }
    }
}

pub fn install(hwnd: HWND, host: Box<dyn Host>) {
    HOST.with(|h| *h.borrow_mut() = Some(host));
    allow_dark_menus(hwnd);
    DARK.with(|d| d.set(crate::shell::theme::dark()));
    unsafe {
        let dark = DARK.with(|d| d.get()) as i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        let backdrop = DWMSBT_TABBEDWINDOW;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            std::mem::size_of_val(&backdrop) as u32,
        );
        if std::env::var("NO_EXTEND").is_err() {
            let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        }
        // Without WS_SYSMENU DWM stops drawing its own caption buttons over the extended
        // frame; ours are hit-tested as HTMINBUTTON/HTMAXBUTTON/HTCLOSE instead.
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_SYSMENU.0 as isize));
        MAIN.with(|m| m.set(Some(hwnd)));
        let _ = SetWindowSubclass(hwnd, Some(subclass), 0x5E1, 0);
        restore_placement(hwnd);
        // Recompute the frame now that WM_NCCALCSIZE is ours.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

const SETTINGS_KEY: windows::core::PCWSTR = windows::core::w!(r"Software\slint-win-explorer");
const PLACEMENT_VALUE: windows::core::PCWSTR = windows::core::w!("WindowPlacement");

/// The window's place and size (and whether maximized) for the next start, as Explorer opens
/// a new window where the last one closed. In full screen, the place it had before.
fn save_placement(hwnd: HWND) {
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY};
    let mut placement = FULL_SCREEN.with(|f| f.get()).map(|(p, _)| p).unwrap_or_else(|| {
        let mut p = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        let _ = unsafe { GetWindowPlacement(hwnd, &mut p) };
        p
    });
    if placement.showCmd == SW_SHOWMINIMIZED.0 as u32 {
        placement.showCmd = SW_SHOWNORMAL.0 as u32;
    }
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            PLACEMENT_VALUE,
            REG_BINARY.0,
            Some(&placement as *const _ as *const _),
            std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        );
    }
}

/// Puts the window back where it last closed (the system keeps it on a screen).
fn restore_placement(hwnd: HWND) {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut placement = WINDOWPLACEMENT::default();
    let mut size = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    let read = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, PLACEMENT_VALUE, RRF_RT_REG_BINARY, None, Some(&mut placement as *mut _ as *mut _), Some(&mut size))
    };
    if read.is_err() || size != std::mem::size_of::<WINDOWPLACEMENT>() as u32 || placement.length != size {
        return;
    }
    if placement.showCmd != SW_SHOWMAXIMIZED.0 as u32 {
        placement.showCmd = SW_SHOWNORMAL.0 as u32;
    }
    let _ = unsafe { SetWindowPlacement(hwnd, &placement) };
}

/// Ctrl held, Shift and Alt not.
fn ctrl_only() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    down(VK_CONTROL) && !down(VK_SHIFT) && !down(VK_MENU)
}

fn alt_only() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState,VK_CONTROL,VK_MENU,VK_SHIFT};
    unsafe {GetKeyState(VK_MENU.0 as i32)<0 && GetKeyState(VK_CONTROL.0 as i32)>=0 && GetKeyState(VK_SHIFT.0 as i32)>=0}
}

/// Ctrl and Shift held, Alt not.
fn ctrl_shift() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    down(VK_CONTROL) && down(VK_SHIFT) && !down(VK_MENU)
}
