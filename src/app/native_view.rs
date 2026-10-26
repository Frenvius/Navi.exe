//! The Control Panel owns its links, categories and applet activation.
use super::*;
use windows::Win32::Foundation::RECT;
impl App {
    pub fn native_viewport(&self, x: f32, y: f32, w: f32, h: f32) {
        let scale = self.ui().window().scale_factor();
        let rect = RECT {
            left: (x * scale).round() as i32,
            top: (y * scale).round() as i32,
            right: ((x + w) * scale).round() as i32,
            bottom: ((y + h) * scale).round() as i32,
        };
        self.native_rect.set(rect);
        if let Some(browser) = self.native_browser.borrow().as_ref() {
            browser.rect(rect);
        }
    }
    pub fn sync_native_view(&self) {
        let path = self.current_path();
        crate::frame::classic_view(self.hwnd.get(), crate::shell::browser::handles(&path));
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetPropW(
                self.hwnd.get(),
                windows::core::w!("SlintNativeView"),
                Some(windows::Win32::Foundation::HANDLE(
                    crate::shell::browser::handles(&path) as usize as _,
                )),
            );
        }
        if !crate::shell::browser::handles(&path) {
            self.native_browser.borrow_mut().take();
            return;
        }
        if self.hwnd.get().0.is_null() {
            return;
        }
        let mut slot = self.native_browser.borrow_mut();
        if slot.is_none() {
            install_keys();
            match crate::shell::browser::Browser::new(
                self.hwnd.get(),
                self.native_rect.get(),
                |path| {
                    let _ = slint::invoke_from_event_loop(move || {
                        with_app(|app| app.native_navigation_complete(path))
                    });
                },
            ) {
                Ok(browser) => *slot = Some(browser),
                Err(e) => {
                    eprintln!("Native shell view: {e}");
                    return;
                }
            }
        }
        if let Some(browser) = slot.as_mut() {
            if let Err(e) = browser.navigate(&path) {
                eprintln!("Native shell navigation: {e}");
            }
        }
    }
    pub fn native_navigation_complete(self: &Rc<Self>, path: String) {
        if !crate::shell::browser::handles(&self.current_path()) {
            return;
        }
        if let Some(browser) = self.native_browser.borrow_mut().as_mut() {
            browser.path = path.clone();
        }
        if !ids::same(&self.current_path(), &path) {
            self.navigate(&path);
        }
    }
    pub fn close_native_view(&self) {
        self.native_browser.borrow_mut().take();
    }
    pub fn refresh_native_view(&self) -> bool {
        if let Some(browser) = self.native_browser.borrow().as_ref() {
            browser.refresh();
            true
        } else {
            false
        }
    }
    pub fn set_native_view(&self, mode: i32) -> bool {
        if let Some(browser) = self.native_browser.borrow().as_ref() {
            browser.set_view(mode);
            true
        } else {
            false
        }
    }
    pub fn focus_native_view(&self) {
        if let Some(browser) = self.native_browser.borrow().as_ref() {
            browser.focus();
        }
        self.note_focus("list");
    }
}

// ExplorerBrowser's accelerator contract is a host responsibility. Limit this hook
// to the native child HWNDs of our marked main window; popups and other apps keep
// their own message dispatch. App actions are queued, outside any COM borrow.
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::*,
};
fn install_keys() {
    thread_local! {static HOOK:std::cell::Cell<HHOOK>=const{std::cell::Cell::new(HHOOK(std::ptr::null_mut()))};}
    HOOK.with(|hook| {
        if hook.get().0.is_null() {
            unsafe {
                if let Ok(value) = SetWindowsHookExW(
                    WH_GETMESSAGE,
                    Some(keys),
                    None,
                    windows::Win32::System::Threading::GetCurrentThreadId(),
                ) {
                    hook.set(value);
                }
            }
        }
    });
}
unsafe extern "system" fn keys(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        let msg = &mut *(lparam.0 as *mut MSG);
        let main = GetAncestor(msg.hwnd, GA_ROOT);
        if msg.hwnd != main
            && GetPropW(main, windows::core::w!("SlintNativeView")).0 as usize == 1
            && matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN)
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
            };
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let alt = GetKeyState(VK_MENU.0 as i32) < 0;
            let key = msg.wParam.0;
            let action = if (ctrl && key == 0x4c) || (alt && key == 0x44) || (!alt && key == 0x73) {
                1
            } else if alt && key == 0x25 {
                2
            } else if alt && key == 0x27 {
                3
            } else if alt && key == 0x26 {
                4
            } else if key == 0x74 {
                5
            } else if ctrl && key == 0x54 {
                6
            } else if ctrl && key == 0x57 {
                7
            } else if ctrl && key == 0x09 {
                8
            } else if key == 0x75 {
                9
            } else if ctrl && matches!(key, 0x45 | 0x46) {
                10
            } else {
                0
            };
            let mut handled = action != 0;
            if matches!(action, 1 | 9 | 10) {
                // Transfer Win32 focus as well as Slint focus before the next key is
                // delivered. Queuing only the visual focus leaves typing in DefView.
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(main));
                with_app(|app| {
                    app.ui().invoke_focus_target(
                        (if action == 1 {
                            "address"
                        } else if action == 10 || shift {
                            "search"
                        } else {
                            "crumbs"
                        })
                        .into(),
                    )
                });
            } else if handled {
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| match action {
                        1 => app.ui().invoke_focus_target("address".into()),
                        2 => app.back(),
                        3 => app.forward(),
                        4 => app.up(),
                        5 => {
                            app.refresh_native_view();
                        }
                        6 => app.new_tab(&ids::start_location()),
                        7 => {
                            let index = app.state.borrow().current;
                            app.close_tab(index);
                        }
                        8 => app.cycle_tab(if shift { -1 } else { 1 }),
                        _ => {}
                    })
                });
            } else {
                with_app(|app| {
                    if let Ok(browser) = app.native_browser.try_borrow() {
                        if let Some(browser) = browser.as_ref() {
                            handled = browser.translate(msg);
                        }
                    }
                });
            }
            if handled {
                msg.message = WM_NULL;
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
