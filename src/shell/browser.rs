use super::*;
use windows::core::{implement, Interface, Ref};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::*;

pub fn handles(path: &str) -> bool {
    if is_control_panel(path) {
        return true;
    }
    let path = path.to_ascii_lowercase();
    let path = path.strip_prefix("shell:").unwrap_or(&path);
    [
        "::{bd84b380-8ca2-1069-ab1d-08000948f534}",
        "::{4234d49b-0245-4df3-b780-3893943456e1}",
        "::{2227a280-3aea-1069-a2de-08002b30309d}",
        "::{7007acc7-3202-11d1-aad2-00805fc1270e}",
    ]
    .iter()
    .any(|id| path == *id || path.starts_with(&format!("{id}\\")))
        || matches!(
            path,
            "fonts" | "appsfolder" | "printersfolder" | "connectionsfolder"
        )
        || std::env::var("SystemRoot")
            .is_ok_and(|root| path.eq_ignore_ascii_case(&format!("{root}\\Fonts")))
}
pub fn is_control_panel(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let path = path.strip_prefix("shell:").unwrap_or(&path);
    path.starts_with("::{26ee0668-a00a-44d7-9371-beb064c98683}")
        || path.starts_with("::{21ec2020-3aea-1069-a2dd-08002b30309d}")
        || matches!(path, "controlpanelfolder" | "controlpanelcategory")
}

#[implement(IExplorerBrowserEvents)]
struct Events {
    navigate: Box<dyn Fn(String)>,
}
impl IExplorerBrowserEvents_Impl for Events_Impl {
    fn OnNavigationPending(&self, _: *const Common::ITEMIDLIST) -> windows::core::Result<()> {
        app_mode(1);
        Ok(())
    }
    fn OnViewCreated(&self, view: Ref<'_, IShellView>) -> windows::core::Result<()> {
        if let Some(view) = view.as_ref() {
            unsafe {
                let _ = view.UIActivate(SVUIA_ACTIVATE_NOFOCUS.0 as u32);
            }
        }
        Ok(())
    }
    fn OnNavigationFailed(&self, p: *const Common::ITEMIDLIST) -> windows::core::Result<()> {
        app_mode(1);
        eprintln!("Native navigation failed {}", unsafe {
            super::name_of(p, SIGDN_DESKTOPABSOLUTEPARSING)
        });
        Ok(())
    }
    fn OnNavigationComplete(&self, p: *const Common::ITEMIDLIST) -> windows::core::Result<()> {
        let path = unsafe { super::name_of(p, SIGDN_DESKTOPABSOLUTEPARSING) };
        if !path.is_empty() {
            (self.navigate)(path);
        }
        Ok(())
    }
}

pub struct Browser {
    browser: IExplorerBrowser,
    cookie: u32,
    pub path: String,
}
impl Browser {
    pub fn new(
        hwnd: HWND,
        rect: RECT,
        navigate: impl Fn(String) + 'static,
    ) -> Result<Self, String> {
        unsafe {
            install_palette_hook();
            let browser: IExplorerBrowser =
                CoCreateInstance(&ExplorerBrowser, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| e.message())?;
            browser
                .SetPropertyBag(windows::core::w!("Shell"))
                .map_err(|e| e.message())?;
            browser
                .SetOptions(EBO_NOBORDER | EBO_NOTRAVELLOG | EBO_SHOWFRAMES)
                .map_err(|e| e.message())?;
            let settings = FOLDERSETTINGS {
                ViewMode: FVM_AUTO.0 as u32,
                fFlags: (FWF_AUTOARRANGE | FWF_FULLROWSELECT | FWF_NOHEADERINALLVIEWS).0 as u32,
            };
            app_mode(1);
            let initialized = browser.Initialize(hwnd, &rect, Some(&settings));
            app_mode(1);
            if let Err(error) = initialized {
                let _ = browser.Destroy();
                return Err(error.message());
            }
            let events: IExplorerBrowserEvents = Events {
                navigate: Box::new(navigate),
            }
            .into();
            let cookie = match browser.Advise(&events) {
                Ok(cookie) => cookie,
                Err(error) => {
                    let _ = browser.Destroy();
                    return Err(error.message());
                }
            };
            Ok(Self {
                browser,
                cookie,
                path: String::new(),
            })
        }
    }
    pub fn navigate(&mut self, path: &str) -> Result<(), String> {
        if self.path.eq_ignore_ascii_case(path) {
            return Ok(());
        }
        let pidl = Pidl::parse(path)?;
        app_mode(1);
        unsafe {
            if let Err(error) = self.browser.BrowseToIDList(pidl.ptr(), SBSP_ABSOLUTE) {
                app_mode(1);
                return Err(error.message());
            }
        }
        self.path = path.to_string();
        Ok(())
    }
    pub fn rect(&self, rect: RECT) {
        unsafe {
            let _ = self.browser.SetRect(None, rect);
        }
    }
    pub fn refresh(&self) {
        unsafe {
            if let Ok(view) = self.browser.GetCurrentView::<IShellView>() {
                let _ = view.Refresh();
            }
        }
    }
    pub fn set_view(&self, mode: i32) {
        let (mode, size) = match mode {
            1 => (FVM_ICON, 256),
            2 => (FVM_ICON, 96),
            3 => (FVM_ICON, 48),
            4 => (FVM_SMALLICON, 16),
            5 => (FVM_LIST, 16),
            6 => (FVM_DETAILS, 16),
            7 => (FVM_TILE, 48),
            8 => (FVM_CONTENT, 32),
            _ => return,
        };
        unsafe {
            if let Ok(view) = self.browser.GetCurrentView::<IFolderView2>() {
                let _ = view.SetViewModeAndIconSize(mode, size);
            }
        }
    }
    pub fn focus(&self) {
        unsafe {
            if let Ok(view) = self.browser.GetCurrentView::<IShellView>() {
                let _ = view.UIActivate(SVUIA_ACTIVATE_FOCUS.0 as u32);
            }
        }
    }
    pub fn translate(&self, msg: &windows::Win32::UI::WindowsAndMessaging::MSG) -> bool {
        unsafe {
            let Ok(input) = self.browser.cast::<IInputObject>() else {
                return false;
            };
            (windows::core::Interface::vtable(&input).TranslateAcceleratorIO)(
                windows::core::Interface::as_raw(&input),
                msg,
            )
            .0 == 0
        }
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        app_mode(1);
        unsafe {
            let _ = self.browser.Unadvise(self.cookie);
            let _ = self.browser.Destroy();
        }
    }
}

// Classic Explorer's frame stays light even when its item view/tree are dark.
// Theme only the outer DirectUI frame; DefView and the namespace tree keep their
// independent Explorer themes.
fn app_mode(value: i32) {
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        if let Ok(module) = GetModuleHandleW(windows::core::w!("uxtheme.dll")) {
            if let Some(function) = GetProcAddress(module, windows::core::PCSTR(135usize as _)) {
                let mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(function);
                mode(value);
            }
        }
    }
}

fn install_palette_hook() {
    use windows::Win32::UI::WindowsAndMessaging::*;
    thread_local! {static HOOK:std::cell::Cell<HHOOK>=const{std::cell::Cell::new(HHOOK(std::ptr::null_mut()))};}
    unsafe extern "system" fn created(
        code: i32,
        w: windows::Win32::Foundation::WPARAM,
        l: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        if code == HCBT_CREATEWND as i32 {
            let hwnd = HWND(w.0 as _);
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class);
            if String::from_utf16_lossy(&class[..n as usize]) == "SHELLDLL_DefView" {
                let create = &*(l.0 as *const CBT_CREATEWNDW);
                let parent = (*create.lpcs).hwndParent;
                let root = GetAncestor(parent, GA_ROOT);
                if GetPropW(root, windows::core::w!("NaviNativeView")).0 as usize == 1 {
                    app_mode(1);
                }
            }
        }
        CallNextHookEx(None, code, w, l)
    }
    HOOK.with(|hook| {
        if hook.get().0.is_null() {
            unsafe {
                if let Ok(value) = SetWindowsHookExW(
                    WH_CBT,
                    Some(created),
                    None,
                    windows::Win32::System::Threading::GetCurrentThreadId(),
                ) {
                    hook.set(value);
                }
            }
        }
    });
}
