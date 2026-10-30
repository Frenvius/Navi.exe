use super::{viewstate::ViewState, Pidl};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::{
    core::{w, Interface},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};

static OPEN: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn sheet_callback(hwnd: HWND, message: u32, _data: LPARAM) -> i32 {
    if message == windows::Win32::UI::Controls::PSCB_INITIALIZED {
        let _ = SetWindowSubclass(hwnd, Some(position_message), 20, SHEET_OWNER.get() as usize);
    }
    0
}
unsafe extern "system" fn position_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if message == WM_SHOWWINDOW && wparam.0 != 0 {
        let _ = RemoveWindowSubclass(hwnd, Some(position_message), 20);
        let owner = HWND(data as _);
        let mut rect = RECT::default();
        if GetWindowRect(owner, &mut rect).is_ok() {
            // Measured at two owner origins: native Folder Options is anchored
            // at visible (31,152), raw (24,152), relative to the raw owner origin
            // at 96 DPI. Its invisible left sizing border accounts for the 7px.
            let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(owner).max(96) as i32;
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left + 24 * dpi / 96,
                rect.top + 152 * dpi / 96,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    result
}
thread_local! {
static SHEET_OWNER:std::cell::Cell<isize>=const{std::cell::Cell::new(0)};
static VIEW:std::cell::Cell<*const IShellView>=const{std::cell::Cell::new(std::ptr::null())};
static APPLYING:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
static ACCEPTED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
static DID_APPLY:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
}
unsafe extern "system" fn page_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let message = &*(lparam.0 as *const CWPSTRUCT);
        if message.message == WM_INITDIALOG && GetDlgItem(Some(message.hwnd), 30123).is_ok() {
            VIEW.with(|view| {
                if !view.get().is_null() {
                    let _ = SetWindowSubclass(
                        message.hwnd,
                        Some(page_message),
                        18,
                        view.get() as usize,
                    );
                }
            });
        }
        if (message.message == WM_INITDIALOG
            || message.message == WM_SHOWWINDOW
            || message.message == WM_WINDOWPOSCHANGED)
            && APPLYING.get()
            && GetDlgItem(Some(message.hwnd), IDYES.0).is_ok()
            && GetDlgItem(Some(message.hwnd), IDCANCEL.0).is_ok()
        {
            let _ = SetWindowSubclass(message.hwnd, Some(confirmation_message), 19, 0);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
unsafe extern "system" fn confirmation_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // The measured native question uses Yes=6 and No/Cancel=2 (not IDNO=7).
    if msg == WM_COMMAND && wparam.0 & 0xffff == IDYES.0 as usize {
        ACCEPTED.set(true);
    }
    if msg == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(confirmation_message), 19);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}
unsafe extern "system" fn page_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_COMMAND && wparam.0 & 0xffff == 30123 {
        ACCEPTED.set(false);
        APPLYING.set(true);
        let result = DefSubclassProc(hwnd, msg, wparam, lparam);
        APPLYING.set(false);
        if ACCEPTED.replace(false) {
            let view = &*(data as *const IShellView);
            match view
                .cast::<windows::Win32::System::Ole::IOleCommandTarget>()
                .and_then(|target| {
                    target.Exec(&CGID_DefView, 0, 0, std::ptr::null(), std::ptr::null_mut())
                }) {
                Ok(()) => DID_APPLY.set(true),
                Err(e) => eprintln!("Apply to Folders: {e}"),
            }
        }
        return result;
    }
    if msg == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(page_message), 18);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}
unsafe extern "system" fn add_page(
    page: windows::Win32::UI::Controls::HPROPSHEETPAGE,
    data: LPARAM,
) -> windows::core::BOOL {
    (&mut *(data.0 as *mut Vec<windows::Win32::UI::Controls::HPROPSHEETPAGE>)).push(page);
    true.into()
}
struct Context {
    host: HWND,
    browser: IExplorerBrowser,
}
impl Context {
    fn new(path: &str, view: &ViewState, dimensions: (i32, i32)) -> Result<Self, String> {
        unsafe {
            let host = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_POPUP,
                0,
                0,
                dimensions.0.max(200),
                dimensions.1.max(100),
                None,
                None,
                None,
                None,
            )
            .map_err(|e| e.message())?;
            let initialize = || -> windows::core::Result<IExplorerBrowser> {
                let browser: IExplorerBrowser =
                    CoCreateInstance(&ExplorerBrowser, None, CLSCTX_INPROC_SERVER)?;
                let result = (|| {
                    browser.Initialize(
                        host,
                        &RECT {
                            left: 0,
                            top: 0,
                            right: dimensions.0.max(200),
                            bottom: dimensions.1.max(100),
                        },
                        None,
                    )?;
                    let pidl = Pidl::parse(path).map_err(|e| {
                        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e)
                    })?;
                    browser.BrowseToIDList(pidl.0, SBSP_ABSOLUTE)?;
                    let native: IFolderView2 = browser.GetCurrentView()?;
                    let mode = match view.logical_mode.unwrap_or(1) {
                        1 => FVM_DETAILS,
                        2 => FVM_TILE,
                        4 => FVM_LIST,
                        5 => FVM_CONTENT,
                        _ => FVM_ICON,
                    };
                    native.SetViewModeAndIconSize(mode, view.icon_size.unwrap_or(16) as i32)?;
                    if let Some(group) = view.group {
                        let (key, descending) = group.unwrap_or_default();
                        native.SetGroupBy(&key, !descending)?;
                    }
                    if !view.sort.is_empty() {
                        let sort = view
                            .sort
                            .iter()
                            .map(|(key, desc)| SORTCOLUMN {
                                propkey: *key,
                                direction: SORTDIRECTION(if *desc { -1 } else { 1 }),
                            })
                            .collect::<Vec<_>>();
                        native.SetSortColumns(&sort)?;
                    }
                    if !view.columns.is_empty() {
                        let columns: IColumnManager = native.cast()?;
                        columns.SetColumns(
                            &view.columns.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                        )?;
                        for (key, width) in &view.columns {
                            let info = CM_COLUMNINFO {
                                cbSize: std::mem::size_of::<CM_COLUMNINFO>() as u32,
                                dwMask: CM_MASK_WIDTH.0 as u32,
                                uWidth: *width,
                                ..Default::default()
                            };
                            columns.SetColumnInfo(key, &info)?;
                        }
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => Ok(browser),
                    Err(e) => {
                        let _ = browser.Destroy();
                        Err(e)
                    }
                }
            };
            match initialize() {
                Ok(browser) => Ok(Self { host, browser }),
                Err(e) => {
                    let _ = DestroyWindow(host);
                    Err(e.message())
                }
            }
        }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            let _ = self.browser.Destroy();
            let _ = DestroyWindow(self.host);
        }
    }
}

pub fn show(
    owner: HWND,
    path: String,
    view: ViewState,
    dimensions: (i32, i32),
    done: impl FnOnce(bool) + Send + 'static,
) -> Result<(), String> {
    if OPEN.swap(true, Ordering::AcqRel) {
        return Err("Folder Options already open".into());
    }
    let owner = owner.0 as isize;
    std::thread::spawn(move || {
        super::com_init();
        let before = super::viewstate::default_fingerprint(&path);
        let context = Context::new(&path, &view, dimensions);
        if let Err(e) = &context {
            eprintln!("Folder Options view context: {e}");
        }
        unsafe {
            let result = (|| -> windows::core::Result<()> {
                let command: IShellPropSheetExt = CoCreateInstance(
                    &windows::core::GUID::from_u128(0x6d5313c0_8c62_11d1_b2cd_006097df8c11),
                    None,
                    CLSCTX_INPROC_SERVER,
                )?;
                let native: Option<IShellView> = context
                    .as_ref()
                    .ok()
                    .and_then(|context| context.browser.GetCurrentView().ok());
                if let Some(native) = &native {
                    let site: windows::Win32::System::Ole::IObjectWithSite = command.cast()?;
                    site.SetSite(native)?;
                }
                let mut pages = Vec::<windows::Win32::UI::Controls::HPROPSHEETPAGE>::new();
                command.AddPages(Some(add_page), LPARAM(&mut pages as *mut _ as isize))?;
                if pages.is_empty() {
                    return Err(windows::core::Error::from_win32());
                }
                let caption = super::wide(&crate::i18n::system_string("explorerframe.dll", 1030));
                let mut header = windows::Win32::UI::Controls::PROPSHEETHEADERW_V2 {
                    dwSize: std::mem::size_of::<windows::Win32::UI::Controls::PROPSHEETHEADERW_V2>()
                        as u32,
                    dwFlags: windows::Win32::UI::Controls::PSH_NOCONTEXTHELP
                        | windows::Win32::UI::Controls::PSH_USECALLBACK,
                    hwndParent: HWND(owner as _),
                    pszCaption: windows::core::PCWSTR(caption.as_ptr()),
                    nPages: pages.len() as u32,
                    pfnCallback: Some(sheet_callback),
                    ..Default::default()
                };
                header.Anonymous3.phpage = pages.as_mut_ptr();
                VIEW.with(|view| {
                    view.set(native.as_ref().map_or(std::ptr::null(), |v| v as *const _))
                });
                let hook = match SetWindowsHookExW(
                    WH_CALLWNDPROC,
                    Some(page_hook),
                    None,
                    windows::Win32::System::Threading::GetCurrentThreadId(),
                ) {
                    Ok(hook) => hook,
                    Err(error) => {
                        VIEW.with(|view| view.set(std::ptr::null()));
                        for page in pages {
                            let _ = windows::Win32::UI::Controls::DestroyPropertySheetPage(page);
                        }
                        return Err(error);
                    }
                };
                SHEET_OWNER.set(owner);
                let result = windows::Win32::UI::Controls::PropertySheetW(&mut header);
                SHEET_OWNER.set(0);
                let _ = UnhookWindowsHookEx(hook);
                VIEW.with(|view| view.set(std::ptr::null()));
                if result == -1 {
                    return Err(windows::core::Error::from_win32());
                }
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("Folder Options: {e}");
            }
        }
        drop(context);
        let changed =
            DID_APPLY.replace(false) || before != super::viewstate::default_fingerprint(&path);
        OPEN.store(false, Ordering::Release);
        done(changed);
    });
    Ok(())
}
