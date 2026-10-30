use std::cell::RefCell;

use windows::core::{
    implement, Error, IUnknownImpl, Interface, Ref, Result, BOOL, BSTR, GUID, HRESULT, PCWSTR,
    PWSTR,
};
use windows::Win32::Foundation::{
    E_FAIL, E_NOTIMPL, HWND, LPARAM, LRESULT, RECT, SHANDLE_PTR, S_FALSE, VARIANT_BOOL,
    VARIANT_FALSE, VARIANT_TRUE, WPARAM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, IDispatch, IDispatch_Impl, IServiceProvider, IServiceProvider_Impl, IStream,
    ITypeInfo, CLSCTX_ALL, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO,
};
use windows::Win32::System::Ole::{
    DispGetIDsOfNames, DispInvoke, IOleWindow_Impl, LoadRegTypeLib, OLEMENUGROUPWIDTHS,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::{InitVariantFromBuffer, VARIANT};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{HMENU, MSG};

use super::{name_of, Pidl};

pub type OnSelect = fn(HWND, String, u32);

pub struct Registration {
    windows: IShellWindows,
    cookie: i32,
    browser: windows::core::ComObject<Browser>,
}

impl Registration {
    pub fn new(hwnd: HWND, path: &str, on_select: OnSelect) -> Result<Self> {
        let pidl = parse(path)?;
        let location = location(&pidl)?;
        unsafe {
            let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
            let pending = windows.RegisterPending(
                GetCurrentThreadId() as i32,
                &location,
                &VARIANT::default(),
                SWC_BROWSER,
            )?;
            let browser = windows::core::ComObject::new(Browser {
                hwnd,
                folder: RefCell::new(pidl),
                on_select,
            });
            let dispatch: IDispatch = browser.to_interface::<IWebBrowserApp>().cast()?;
            match windows.Register(&dispatch, hwnd.0 as i32, SWC_BROWSER) {
                Ok(cookie) => Ok(Self {
                    windows,
                    cookie,
                    browser,
                }),
                Err(error) => {
                    if let Err(revoke) = windows.Revoke(pending) {
                        eprintln!("shell windows pending revoke: {revoke}");
                    }
                    Err(error)
                }
            }
        }
    }

    pub fn navigate(&self, path: &str) -> Result<()> {
        let pidl = parse(path)?;
        unsafe { self.windows.OnNavigate(self.cookie, &location(&pidl)?)? };
        *self.browser.folder.borrow_mut() = pidl;
        Ok(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.windows.Revoke(self.cookie) } {
            eprintln!("shell windows revoke: {error}");
        }
    }
}

fn parse(path: &str) -> Result<Pidl> {
    Pidl::parse(path).map_err(|error| Error::new(E_FAIL, error))
}

fn location(pidl: &Pidl) -> Result<VARIANT> {
    unsafe { InitVariantFromBuffer(pidl.ptr() as _, ILGetSize(Some(pidl.ptr()))) }
}

#[implement(
    IWebBrowserApp,
    IServiceProvider,
    IShellBrowser,
    IShellView,
    Agile = false
)]
pub struct Browser {
    hwnd: HWND,
    folder: RefCell<Pidl>,
    on_select: OnSelect,
}

impl Browser_Impl {
    fn folder_name(&self, kind: SIGDN) -> String {
        unsafe { name_of(self.folder.borrow().ptr(), kind) }
    }
}

macro_rules! not_implemented {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        $(fn $name(&self, $(_: $arg),*) -> Result<$ret> {
            Err(E_NOTIMPL.into())
        })*
    };
}

const SHDOCVW: GUID = GUID::from_u128(0xeab22ac0_30c1_11cf_a7eb_0000c05bae0b);

fn type_info() -> Result<ITypeInfo> {
    unsafe {
        let dispatch =
            LoadRegTypeLib(&SHDOCVW, 1, 1, 0)?.GetTypeInfoOfGuid(&IWebBrowserApp::IID)?;
        dispatch.GetRefTypeInfo(dispatch.GetRefTypeOfImplType(u32::MAX)?)
    }
}

impl IDispatch_Impl for Browser_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(1)
    }
    fn GetTypeInfo(&self, _: u32, _: u32) -> Result<ITypeInfo> {
        type_info()
    }
    fn GetIDsOfNames(
        &self,
        _: *const GUID,
        names: *const PCWSTR,
        count: u32,
        _: u32,
        ids: *mut i32,
    ) -> Result<()> {
        unsafe { DispGetIDsOfNames(&type_info()?, names, count, ids) }
    }
    fn Invoke(
        &self,
        id: i32,
        _: *const GUID,
        _: u32,
        flags: DISPATCH_FLAGS,
        params: *const DISPPARAMS,
        result: *mut VARIANT,
        exception: *mut EXCEPINFO,
        argument: *mut u32,
    ) -> Result<()> {
        let browser = self.as_interface::<IWebBrowserApp>();
        unsafe {
            DispInvoke(
                browser.as_raw(),
                &type_info()?,
                id,
                flags.0,
                params.cast_mut(),
                result,
                exception,
                argument,
            )
        }
    }
}

impl IWebBrowser_Impl for Browser_Impl {
    not_implemented! {
        GoBack() -> ();
        GoForward() -> ();
        GoHome() -> ();
        GoSearch() -> ();
        Navigate(&BSTR, *const VARIANT, *const VARIANT, *const VARIANT, *const VARIANT) -> ();
        Refresh() -> ();
        Refresh2(*const VARIANT) -> ();
        Stop() -> ();
        Application() -> IDispatch;
        Parent() -> IDispatch;
        Container() -> IDispatch;
        Type() -> BSTR;
        Left() -> i32;
        SetLeft(i32) -> ();
        Top() -> i32;
        SetTop(i32) -> ();
        Width() -> i32;
        SetWidth(i32) -> ();
        Height() -> i32;
        SetHeight(i32) -> ();
    }
    fn Document(&self) -> Result<IDispatch> {
        self.to_interface::<IWebBrowserApp>().cast()
    }
    fn TopLevelContainer(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn LocationName(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.folder_name(SIGDN_NORMALDISPLAY)))
    }
    fn LocationURL(&self) -> Result<BSTR> {
        let path = self.folder_name(SIGDN_DESKTOPABSOLUTEPARSING);
        let wide = super::wide(&path);
        let mut url = [0u16; 2084];
        let mut length = url.len() as u32;
        let created = unsafe {
            UrlCreateFromPathW(
                PCWSTR(wide.as_ptr()),
                PWSTR(url.as_mut_ptr()),
                &mut length,
                0,
            )
        };
        Ok(match created {
            Ok(()) => BSTR::from_wide(&url[..length as usize]),
            Err(_) => BSTR::from(path),
        })
    }
    fn Busy(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
}

impl IWebBrowserApp_Impl for Browser_Impl {
    not_implemented! {
        Quit() -> ();
        ClientToWindow(*mut i32, *mut i32) -> ();
        PutProperty(&BSTR, &VARIANT) -> ();
        GetProperty(&BSTR) -> VARIANT;
        Path() -> BSTR;
        SetVisible(VARIANT_BOOL) -> ();
        StatusBar() -> VARIANT_BOOL;
        SetStatusBar(VARIANT_BOOL) -> ();
        StatusText() -> BSTR;
        SetStatusText(&BSTR) -> ();
        ToolBar() -> i32;
        SetToolBar(i32) -> ();
        MenuBar() -> VARIANT_BOOL;
        SetMenuBar(VARIANT_BOOL) -> ();
        FullScreen() -> VARIANT_BOOL;
        SetFullScreen(VARIANT_BOOL) -> ();
    }
    fn Name(&self) -> Result<BSTR> {
        Ok(BSTR::from("Navi"))
    }
    fn FullName(&self) -> Result<BSTR> {
        let path =
            std::env::current_exe().map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        Ok(BSTR::from(path.to_string_lossy().as_ref()))
    }
    fn HWND(&self) -> Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(self.hwnd.0 as isize))
    }
    fn Visible(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
}

impl IServiceProvider_Impl for Browser_Impl {
    fn QueryService(
        &self,
        _: *const GUID,
        riid: *const GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        unsafe { self.QueryInterface(riid, ppv).ok() }
    }
}

impl IOleWindow_Impl for Browser_Impl {
    fn GetWindow(&self) -> Result<HWND> {
        Ok(self.hwnd)
    }
    fn ContextSensitiveHelp(&self, _: BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IShellBrowser_Impl for Browser_Impl {
    not_implemented! {
        InsertMenusSB(HMENU, *mut OLEMENUGROUPWIDTHS) -> ();
        SetMenuSB(HMENU, isize, HWND) -> ();
        RemoveMenusSB(HMENU) -> ();
        SetStatusTextSB(&PCWSTR) -> ();
        EnableModelessSB(BOOL) -> ();
        TranslateAcceleratorSB(*const MSG, u16) -> ();
        BrowseObject(*const ITEMIDLIST, u32) -> ();
        GetViewStateStream(u32) -> IStream;
        GetControlWindow(u32) -> HWND;
        SendControlMsg(u32, u32, WPARAM, LPARAM, *mut LRESULT) -> ();
        SetToolbarItems(*const TBBUTTON, u32, u32) -> ();
    }
    fn QueryActiveShellView(&self) -> Result<IShellView> {
        Ok(self.to_interface())
    }
    fn OnViewWindowActive(&self, _: Ref<'_, IShellView>) -> Result<()> {
        Ok(())
    }
}

impl IShellView_Impl for Browser_Impl {
    fn TranslateAccelerator(&self, _: *const MSG) -> HRESULT {
        S_FALSE
    }
    not_implemented! {
        EnableModeless(BOOL) -> ();
        UIActivate(u32) -> ();
        Refresh() -> ();
        CreateViewWindow(Ref<'_, IShellView>, *const FOLDERSETTINGS, Ref<'_, IShellBrowser>, *const RECT) -> HWND;
        DestroyViewWindow() -> ();
        GetCurrentInfo() -> FOLDERSETTINGS;
        AddPropertySheetPages(u32, LPFNSVADDPROPSHEETPAGE, LPARAM) -> ();
        SaveViewState() -> ();
        GetItemObject(&_SVGIO, *const GUID, *mut *mut core::ffi::c_void) -> ();
    }
    fn SelectItem(&self, child: *const ITEMIDLIST, flags: u32) -> Result<()> {
        let item = Pidl(unsafe { ILCombine(Some(self.folder.borrow().ptr()), Some(child)) });
        let path = item.name(SIGDN_DESKTOPABSOLUTEPARSING);
        if path.is_empty() {
            return Err(E_FAIL.into());
        }
        (self.on_select)(self.hwnd, path, flags);
        Ok(())
    }
}
