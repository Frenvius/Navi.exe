use std::cell::{Cell, RefCell};

use windows::core::{
    implement, ComObject, Error, IUnknown, IUnknownImpl, Interface, Ref, Result, BOOL, BSTR,
    GUID, HRESULT, PCWSTR, PWSTR,
};
use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, LRESULT, RECT, SHANDLE_PTR, S_FALSE, S_OK,
    VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE, WPARAM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, IDispatch, IDispatch_Impl, IServiceProvider, IServiceProvider_Impl, IStream,
    ITypeInfo, CLSCTX_ALL, CLSCTX_INPROC_SERVER, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO,
};
use windows::Win32::System::Ole::{
    DispGetIDsOfNames, DispInvoke, IEnumVARIANT, IEnumVARIANT_Impl, IOleWindow_Impl,
    LoadRegTypeLib, OLECMDEXECOPT, OLECMDF, OLECMDID, OLEMENUGROUPWIDTHS, READYSTATE,
    READYSTATE_COMPLETE,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::{
    InitVariantFromBuffer, VariantGetElementCount, VariantToBuffer, VARENUM, VARIANT, VT_ARRAY,
    VT_UI1,
};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{HMENU, MSG};

use super::{name_of, Pidl};

pub enum Request {
    Select(String, u32),
    Navigate(String),
    Quit,
}

#[derive(Default)]
pub struct View {
    pub selected: Vec<String>,
    pub focused: Option<String>,
}

#[derive(Clone, Copy)]
pub struct Host {
    pub request: fn(HWND, &str, Request),
    pub view: fn(HWND, &str) -> Option<View>,
}

pub struct Registration {
    windows: IShellWindows,
    cookie: i32,
    browser: ComObject<Browser>,
}

impl Registration {
    pub fn new(hwnd: HWND, path: &str, host: Host) -> Result<Self> {
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
            let browser = ComObject::new(Browser {
                hwnd,
                path: RefCell::new(path.to_owned()),
                folder: RefCell::new(pidl),
                host,
            });
            let dispatch: IDispatch = browser.to_interface::<IWebBrowser2>().cast()?;
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
        *self.browser.path.borrow_mut() = path.to_owned();
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

fn shell() -> Result<IShellDispatch> {
    unsafe { CoCreateInstance(&Shell, None, CLSCTX_INPROC_SERVER) }
}

fn folder_item(shell: &IShellDispatch, path: &str) -> Result<FolderItem> {
    let item = parse(path)?;
    let parent = item.parent().ok_or_else(|| Error::from(E_INVALIDARG))?;
    let child = unsafe { name_of(item.last_id(), SIGDN_PARENTRELATIVEPARSING) };
    unsafe {
        let folder =
            shell.NameSpace(&VARIANT::from(parent.name(SIGDN_DESKTOPABSOLUTEPARSING).as_str()))?;
        folder.ParseName(&BSTR::from(child))
    }
}

fn url_path(url: &str) -> String {
    if !url.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:")) {
        return url.to_owned();
    }
    let wide = super::wide(url);
    let mut path = [0u16; 32768];
    let mut length = path.len() as u32;
    match unsafe {
        PathCreateFromUrlW(
            PCWSTR(wide.as_ptr()),
            PWSTR(path.as_mut_ptr()),
            &mut length,
            0,
        )
    } {
        Ok(()) => String::from_utf16_lossy(&path[..length as usize]),
        Err(_) => url.to_owned(),
    }
}

fn navigation_target(target: &VARIANT) -> Result<Pidl> {
    if unsafe { target.Anonymous.Anonymous.vt } == VARENUM(VT_ARRAY.0 | VT_UI1.0) {
        let size = unsafe { VariantGetElementCount(target) } as usize;
        let mut bytes = vec![0u8; size];
        unsafe { VariantToBuffer(target, bytes.as_mut_ptr().cast(), size as u32)? };
        if !terminated(&bytes) {
            return Err(E_INVALIDARG.into());
        }
        return Ok(Pidl(unsafe { ILClone(bytes.as_ptr().cast()) }));
    }
    if let Ok(dispatch) = IDispatch::try_from(target) {
        return parse(&unsafe { dispatch.cast::<FolderItem>()?.Path()? }.to_string());
    }
    parse(&url_path(&BSTR::try_from(target)?.to_string()))
}

fn terminated(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while let Some(size) = bytes.get(offset..offset + 2) {
        let size = u16::from_le_bytes([size[0], size[1]]) as usize;
        if size == 0 {
            return true;
        }
        if size < 2 {
            return false;
        }
        offset += size;
    }
    false
}

struct Library {
    id: GUID,
    major: u16,
    minor: u16,
}

const SHDOCVW: Library = Library {
    id: GUID::from_u128(0xeab22ac0_30c1_11cf_a7eb_0000c05bae0b),
    major: 1,
    minor: 1,
};

const SHELL32: Library = Library {
    id: GUID::from_u128(0x50a7e9b0_70ef_11d1_b75a_00a0c90564fe),
    major: 1,
    minor: 0,
};

fn type_info(library: &Library, interface: &GUID) -> Result<ITypeInfo> {
    unsafe {
        let dispatch = LoadRegTypeLib(&library.id, library.major, library.minor, 0)?
            .GetTypeInfoOfGuid(interface)?;
        dispatch.GetRefTypeInfo(dispatch.GetRefTypeOfImplType(u32::MAX)?)
    }
}

macro_rules! dispatch {
    ($object:ty, $interface:ty, $library:expr) => {
        impl IDispatch_Impl for $object {
            fn GetTypeInfoCount(&self) -> Result<u32> {
                Ok(1)
            }
            fn GetTypeInfo(&self, _: u32, _: u32) -> Result<ITypeInfo> {
                type_info(&$library, &<$interface>::IID)
            }
            fn GetIDsOfNames(
                &self,
                _: *const GUID,
                names: *const PCWSTR,
                count: u32,
                _: u32,
                ids: *mut i32,
            ) -> Result<()> {
                unsafe {
                    DispGetIDsOfNames(
                        &type_info(&$library, &<$interface>::IID)?,
                        names,
                        count,
                        ids,
                    )
                }
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
                let this = self.as_interface::<$interface>();
                unsafe {
                    DispInvoke(
                        this.as_raw(),
                        &type_info(&$library, &<$interface>::IID)?,
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
    };
}

macro_rules! not_implemented {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        $(fn $name(&self, $(_: $arg),*) -> Result<$ret> {
            Err(E_NOTIMPL.into())
        })*
    };
}

#[implement(
    IWebBrowser2,
    IServiceProvider,
    IShellBrowser,
    IShellView,
    Agile = false
)]
pub struct Browser {
    hwnd: HWND,
    path: RefCell<String>,
    folder: RefCell<Pidl>,
    host: Host,
}

impl Browser_Impl {
    fn folder_name(&self, kind: SIGDN) -> String {
        unsafe { name_of(self.folder.borrow().ptr(), kind) }
    }

    fn request(&self, request: Request) {
        let path = self.path.borrow().clone();
        (self.host.request)(self.hwnd, &path, request);
    }

    fn view(&self) -> Result<View> {
        let path = self.path.borrow().clone();
        (self.host.view)(self.hwnd, &path).ok_or_else(|| E_FAIL.into())
    }

    fn navigate_to(&self, target: Pidl) -> Result<()> {
        let path = target.name(SIGDN_DESKTOPABSOLUTEPARSING);
        if path.is_empty() || !super::ops::is_folder(&path) {
            return Err(E_INVALIDARG.into());
        }
        self.request(Request::Navigate(path));
        Ok(())
    }
}

dispatch!(Browser_Impl, IWebBrowser2, SHDOCVW);

impl IWebBrowser_Impl for Browser_Impl {
    not_implemented! {
        GoBack() -> ();
        GoForward() -> ();
        GoHome() -> ();
        GoSearch() -> ();
        Refresh() -> ();
        Refresh2(*const VARIANT) -> ();
        Stop() -> ();
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
    fn Navigate(
        &self,
        url: &BSTR,
        _: *const VARIANT,
        _: *const VARIANT,
        _: *const VARIANT,
        _: *const VARIANT,
    ) -> Result<()> {
        self.navigate_to(parse(&url_path(&url.to_string()))?)
    }
    fn Application(&self) -> Result<IDispatch> {
        shell()?.cast()
    }
    fn Parent(&self) -> Result<IDispatch> {
        shell()?.cast()
    }
    fn Document(&self) -> Result<IDispatch> {
        let view = ComObject::new(FolderView {
            browser: self.to_object(),
        });
        view.to_interface::<IShellFolderViewDual>().cast()
    }
    fn TopLevelContainer(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn LocationName(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.folder_name(SIGDN_NORMALDISPLAY)))
    }
    fn LocationURL(&self) -> Result<BSTR> {
        let path = self.folder_name(SIGDN_FILESYSPATH);
        if path.is_empty() {
            return Ok(BSTR::new());
        }
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
        ClientToWindow(*mut i32, *mut i32) -> ();
        PutProperty(&BSTR, &VARIANT) -> ();
        GetProperty(&BSTR) -> VARIANT;
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
    fn Quit(&self) -> Result<()> {
        self.request(Request::Quit);
        Ok(())
    }
    fn Name(&self) -> Result<BSTR> {
        Ok(BSTR::from("Navi"))
    }
    fn FullName(&self) -> Result<BSTR> {
        let path =
            std::env::current_exe().map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        Ok(BSTR::from(path.to_string_lossy().as_ref()))
    }
    fn Path(&self) -> Result<BSTR> {
        let path =
            std::env::current_exe().map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        let folder = path.parent().map(|folder| format!("{}\\", folder.display()));
        Ok(BSTR::from(folder.unwrap_or_default()))
    }
    fn HWND(&self) -> Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(self.hwnd.0 as isize))
    }
    fn Visible(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
}

impl IWebBrowser2_Impl for Browser_Impl {
    not_implemented! {
        QueryStatusWB(OLECMDID) -> OLECMDF;
        ExecWB(OLECMDID, OLECMDEXECOPT, *const VARIANT, *mut VARIANT) -> ();
        ShowBrowserBar(*const VARIANT, *const VARIANT, *const VARIANT) -> ();
        SetOffline(VARIANT_BOOL) -> ();
        SetSilent(VARIANT_BOOL) -> ();
        SetRegisterAsBrowser(VARIANT_BOOL) -> ();
        SetRegisterAsDropTarget(VARIANT_BOOL) -> ();
        SetTheaterMode(VARIANT_BOOL) -> ();
        SetAddressBar(VARIANT_BOOL) -> ();
        SetResizable(VARIANT_BOOL) -> ();
    }
    fn Navigate2(
        &self,
        url: *const VARIANT,
        _: *const VARIANT,
        _: *const VARIANT,
        _: *const VARIANT,
        _: *const VARIANT,
    ) -> Result<()> {
        let url = unsafe { url.as_ref() }.ok_or_else(|| Error::from(E_INVALIDARG))?;
        self.navigate_to(navigation_target(url)?)
    }
    fn ReadyState(&self) -> Result<READYSTATE> {
        Ok(READYSTATE_COMPLETE)
    }
    fn Offline(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn Silent(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn RegisterAsBrowser(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn RegisterAsDropTarget(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn TheaterMode(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn AddressBar(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn Resizable(&self) -> Result<VARIANT_BOOL> {
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
        self.request(Request::Select(path, flags));
        Ok(())
    }
}

#[implement(IShellFolderViewDual, IServiceProvider, Agile = false)]
struct FolderView {
    browser: ComObject<Browser>,
}

dispatch!(FolderView_Impl, IShellFolderViewDual, SHELL32);

impl FolderView_Impl {
    fn item_path(&self, item: &VARIANT) -> Result<String> {
        if let Ok(dispatch) = IDispatch::try_from(item) {
            let item: FolderItem = dispatch.cast()?;
            return Ok(unsafe { item.Path()? }.to_string());
        }
        let name = BSTR::try_from(item)?;
        if let Ok(item) = unsafe { self.Folder()?.ParseName(&name) } {
            return Ok(unsafe { item.Path()? }.to_string());
        }
        Ok(name.to_string())
    }
}

impl IShellFolderViewDual_Impl for FolderView_Impl {
    fn Application(&self) -> Result<IDispatch> {
        shell()?.cast()
    }
    fn Parent(&self) -> Result<IDispatch> {
        self.browser.to_interface::<IWebBrowser2>().cast()
    }
    fn Folder(&self) -> Result<Folder> {
        let path = self.browser.folder_name(SIGDN_DESKTOPABSOLUTEPARSING);
        unsafe { shell()?.NameSpace(&VARIANT::from(path.as_str())) }
    }
    fn SelectedItems(&self) -> Result<FolderItems> {
        let items = ComObject::new(Items {
            paths: self.browser.view()?.selected,
        });
        Ok(items.to_interface())
    }
    fn FocusedItem(&self) -> Result<FolderItem> {
        match self.browser.view()?.focused {
            Some(path) => folder_item(&shell()?, &path),
            None => Err(S_FALSE.into()),
        }
    }
    fn SelectItem(&self, item: *const VARIANT, flags: i32) -> Result<()> {
        let item = unsafe { item.as_ref() }.ok_or_else(|| Error::from(E_INVALIDARG))?;
        let path = self.item_path(item)?;
        self.browser.request(Request::Select(path, flags as u32));
        Ok(())
    }
    not_implemented! {
        PopupItemMenu(Ref<'_, FolderItem>, &VARIANT, &VARIANT) -> BSTR;
        Script() -> IDispatch;
        ViewOptions() -> i32;
    }
}

impl IServiceProvider_Impl for FolderView_Impl {
    fn QueryService(
        &self,
        service: *const GUID,
        riid: *const GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        IServiceProvider_Impl::QueryService(&*self.browser, service, riid, ppv)
    }
}

#[implement(FolderItems, Agile = false)]
struct Items {
    paths: Vec<String>,
}

dispatch!(Items_Impl, FolderItems, SHELL32);

impl FolderItems_Impl for Items_Impl {
    fn Count(&self) -> Result<i32> {
        Ok(self.paths.len() as i32)
    }
    fn Application(&self) -> Result<IDispatch> {
        shell()?.cast()
    }
    fn Parent(&self) -> Result<IDispatch> {
        shell()?.cast()
    }
    fn Item(&self, index: &VARIANT) -> Result<FolderItem> {
        let path = match i32::try_from(index) {
            Ok(index) => usize::try_from(index)
                .ok()
                .and_then(|index| self.paths.get(index)),
            Err(_) => {
                let name = BSTR::try_from(index)?.to_string().to_lowercase();
                self.paths.iter().find(|path| {
                    let path = path.to_lowercase();
                    path == name || path.rsplit('\\').next() == Some(name.as_str())
                })
            }
        };
        let path = path.ok_or_else(|| Error::from(E_INVALIDARG))?;
        folder_item(&shell()?, path)
    }
    fn _NewEnum(&self) -> Result<IUnknown> {
        let items = ComObject::new(ItemsEnum {
            paths: self.paths.clone(),
            next: Cell::new(0),
        });
        Ok(items.to_interface::<IEnumVARIANT>().into())
    }
}

#[implement(IEnumVARIANT, Agile = false)]
struct ItemsEnum {
    paths: Vec<String>,
    next: Cell<usize>,
}

impl IEnumVARIANT_Impl for ItemsEnum_Impl {
    fn Next(&self, count: u32, items: *mut VARIANT, fetched: *mut u32) -> HRESULT {
        let Ok(shell) = shell() else {
            return E_FAIL;
        };
        let mut written = 0;
        while written < count as usize && self.next.get() < self.paths.len() {
            let path = &self.paths[self.next.get()];
            self.next.set(self.next.get() + 1);
            let Ok(item) = folder_item(&shell, path).and_then(|item| item.cast::<IDispatch>())
            else {
                continue;
            };
            unsafe { items.add(written).write(VARIANT::from(item)) };
            written += 1;
        }
        if let Some(fetched) = unsafe { fetched.as_mut() } {
            *fetched = written as u32;
        }
        if written == count as usize {
            S_OK
        } else {
            S_FALSE
        }
    }
    fn Skip(&self, count: u32) -> HRESULT {
        let next = self.next.get().saturating_add(count as usize);
        self.next.set(next.min(self.paths.len()));
        if next <= self.paths.len() {
            S_OK
        } else {
            S_FALSE
        }
    }
    fn Reset(&self) -> Result<()> {
        self.next.set(0);
        Ok(())
    }
    fn Clone(&self) -> Result<IEnumVARIANT> {
        let items = ComObject::new(ItemsEnum {
            paths: self.paths.clone(),
            next: Cell::new(self.next.get()),
        });
        Ok(items.to_interface())
    }
}

#[cfg(test)]
mod tests {
    use super::terminated;

    #[test]
    fn buffer_pidls_must_end_inside_the_buffer() {
        assert!(terminated(&[0, 0]));
        assert!(terminated(&[4, 0, 7, 7, 0, 0]));
        assert!(!terminated(&[]));
        assert!(!terminated(&[4, 0, 7, 7]));
        assert!(!terminated(&[9, 0, 7, 7, 0, 0]));
        assert!(!terminated(&[1, 0, 0, 0]));
    }
}
