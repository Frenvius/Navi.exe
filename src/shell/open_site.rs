use std::ffi::c_void;

use windows::core::{
    implement, IUnknown, IUnknownImpl, Interface, Result, BOOL, GUID, PCSTR, PCWSTR,
};
use windows::Win32::Foundation::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, HWND, POINT, PROPERTYKEY,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{IPersist_Impl, IServiceProvider, IServiceProvider_Impl};
use windows::Win32::System::Ole::IObjectWithSite;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuDefaultItem, GET_MENU_DEFAULT_ITEM_FLAGS, HMENU,
};

use super::Pidl;

pub struct View {
    pub folder: String,
    pub selected: i32,
    pub sort: Vec<(PROPERTYKEY, bool)>,
    pub columns: Vec<PROPERTYKEY>,
}

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

pub struct Command {
    context: IContextMenu,
    id: usize,
    _site: IUnknown,
    _menu: Menu,
}

impl Command {
    pub fn invoke(self, hwnd: HWND) -> std::result::Result<(), String> {
        unsafe { super::ops::invoke(&self.context, hwnd, None, self.id, false, None) }
    }
}

pub fn prepare(hwnd: HWND, path: &str, view: &View) -> std::result::Result<Command, String> {
    let folder = Pidl::parse(&view.folder)?;
    let site: IUnknown = Site {
        folder,
        selected: view.selected,
        sort: view
            .sort
            .iter()
            .map(|(propkey, descending)| SORTCOLUMN {
                propkey: *propkey,
                direction: if *descending {
                    SORT_DESCENDING
                } else {
                    SORT_ASCENDING
                },
            })
            .collect(),
        columns: view.columns.clone(),
    }
    .into();
    unsafe {
        let context = super::ops::context_menu(hwnd, &view.folder, &[path.to_owned()])?;
        context
            .cast::<IObjectWithSite>()
            .and_then(|object| object.SetSite(&site))
            .map_err(|error| error.message())?;
        let menu = Menu(CreatePopupMenu().map_err(|error| error.message())?);
        context
            .QueryContextMenu(menu.0, 0, 1, 0x7fff, CMF_DEFAULTONLY)
            .ok()
            .map_err(|error| error.message())?;
        let id = GetMenuDefaultItem(menu.0, 0, GET_MENU_DEFAULT_ITEM_FLAGS(0));
        if id == u32::MAX {
            return Err("No default shell command".into());
        }
        Ok(Command {
            context,
            id: (id - 1) as usize,
            _site: site,
            _menu: menu,
        })
    }
}

macro_rules! unsupported {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $result:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> Result<$result> {
            $(let _ = $arg;)*
            Err(E_NOTIMPL.into())
        })*
    };
}

#[implement(
    IServiceProvider,
    IFolderView2,
    IPersistIDList,
    IColumnManager,
    Agile = false
)]
struct Site {
    folder: Pidl,
    selected: i32,
    sort: Vec<SORTCOLUMN>,
    columns: Vec<PROPERTYKEY>,
}

impl IServiceProvider_Impl for Site_Impl {
    fn QueryService(
        &self,
        service: *const GUID,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> Result<()> {
        if service.is_null() || iid.is_null() || out.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *out = std::ptr::null_mut();
            if *service == IFolderView::IID || *service == IFolderView2::IID {
                return self.QueryInterface(iid, out).ok();
            }
        }
        Err(E_NOINTERFACE.into())
    }
}

impl IPersist_Impl for Site_Impl {
    unsupported! { GetClassID() -> GUID; }
}

impl IPersistIDList_Impl for Site_Impl {
    unsupported! { SetIDList(pidl: *const ITEMIDLIST) -> (); }

    fn GetIDList(&self) -> Result<*mut ITEMIDLIST> {
        let copy = unsafe { ILClone(self.folder.ptr()) };
        if copy.is_null() {
            Err(E_OUTOFMEMORY.into())
        } else {
            Ok(copy)
        }
    }
}

impl IFolderView_Impl for Site_Impl {
    unsupported! {
        GetCurrentViewMode() -> u32;
        SetCurrentViewMode(mode: u32) -> ();
        GetFolder(iid: *const GUID, out: *mut *mut c_void) -> ();
        Item(index: i32) -> *mut ITEMIDLIST;
        ItemCount(flags: &_SVGIO) -> i32;
        Items(flags: &_SVGIO, iid: *const GUID, out: *mut *mut c_void) -> ();
        GetSelectionMarkedItem() -> i32;
        GetFocusedItem() -> i32;
        GetItemPosition(pidl: *const ITEMIDLIST) -> POINT;
        GetSpacing(point: *mut POINT) -> ();
        GetDefaultSpacing() -> POINT;
        GetAutoArrange() -> ();
        SelectItem(index: i32, flags: u32) -> ();
        SelectAndPositionItems(count: u32, ids: *const *const ITEMIDLIST, points: *const POINT, flags: u32) -> ();
    }
}

impl IFolderView2_Impl for Site_Impl {
    unsupported! {
        SetGroupBy(key: *const PROPERTYKEY, ascending: BOOL) -> ();
        SetViewProperty(pidl: *const ITEMIDLIST, key: *const PROPERTYKEY, value: *const PROPVARIANT) -> ();
        GetViewProperty(pidl: *const ITEMIDLIST, key: *const PROPERTYKEY) -> PROPVARIANT;
        SetTileViewProperties(pidl: *const ITEMIDLIST, properties: &PCWSTR) -> ();
        SetExtendedTileViewProperties(pidl: *const ITEMIDLIST, properties: &PCWSTR) -> ();
        SetText(kind: FVTEXTTYPE, text: &PCWSTR) -> ();
        SetCurrentFolderFlags(mask: u32, flags: u32) -> ();
        GetCurrentFolderFlags() -> u32;
        SetSortColumns(columns: *const SORTCOLUMN, count: i32) -> ();
        GetItem(index: i32, iid: *const GUID, out: *mut *mut c_void) -> ();
        GetVisibleItem(start: i32, previous: BOOL) -> i32;
        GetSelection(none_implies_folder: BOOL) -> IShellItemArray;
        GetSelectionState(pidl: *const ITEMIDLIST) -> u32;
        InvokeVerbOnSelection(verb: &PCSTR) -> ();
        SetViewModeAndIconSize(mode: FOLDERVIEWMODE, size: i32) -> ();
        SetGroupSubsetCount(count: u32) -> ();
        GetGroupSubsetCount() -> u32;
        SetRedraw(redraw: BOOL) -> ();
        IsMoveInSameFolder() -> ();
        DoRename() -> ();
    }

    fn GetGroupBy(&self, key: *mut PROPERTYKEY, ascending: *mut BOOL) -> Result<()> {
        if key.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *key = PROPERTYKEY::default();
            if !ascending.is_null() {
                *ascending = true.into();
            }
        }
        Ok(())
    }

    fn GetSortColumnCount(&self) -> Result<i32> {
        Ok(self.sort.len() as i32)
    }

    fn GetSortColumns(&self, columns: *mut SORTCOLUMN, count: i32) -> Result<()> {
        if count < 0 || count as usize != self.sort.len() {
            return Err(E_INVALIDARG.into());
        }
        if count > 0 {
            if columns.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(self.sort.as_ptr(), columns, count as usize);
            }
        }
        Ok(())
    }

    fn GetSelectedItem(&self, start: i32) -> Result<i32> {
        Ok(if self.selected >= start {
            self.selected
        } else {
            -1
        })
    }

    fn GetViewModeAndIconSize(&self, mode: *mut FOLDERVIEWMODE, size: *mut i32) -> Result<()> {
        unsafe {
            if !mode.is_null() {
                *mode = FVM_DETAILS;
            }
            if !size.is_null() {
                *size = 16;
            }
        }
        Ok(())
    }
}

impl IColumnManager_Impl for Site_Impl {
    unsupported! {
        SetColumnInfo(key: *const PROPERTYKEY, info: *const CM_COLUMNINFO) -> ();
        GetColumnInfo(key: *const PROPERTYKEY, info: *mut CM_COLUMNINFO) -> ();
        SetColumns(keys: *const PROPERTYKEY, count: u32) -> ();
    }

    fn GetColumnCount(&self, _: CM_ENUM_FLAGS) -> Result<u32> {
        Ok(self.columns.len() as u32)
    }

    fn GetColumns(&self, _: CM_ENUM_FLAGS, keys: *mut PROPERTYKEY, count: u32) -> Result<()> {
        if count as usize != self.columns.len() {
            return Err(E_INVALIDARG.into());
        }
        if count > 0 {
            if keys.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(self.columns.as_ptr(), keys, count as usize);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Storage::EnhancedStorage::{PKEY_DateModified, PKEY_ItemNameDisplay};

    #[test]
    fn native_consumer_can_query_the_site_on_its_sta_and_own_the_returned_pidl() {
        std::thread::spawn(|| {
            crate::shell::com_init();
            let folder = std::env::var("WINDIR").unwrap();
            let expected = Pidl::parse(&folder).unwrap();
            let site: IServiceProvider = Site {
                folder: Pidl::parse(&folder).unwrap(),
                selected: 2,
                sort: vec![
                    SORTCOLUMN {
                        propkey: PKEY_DateModified,
                        direction: SORT_DESCENDING,
                    },
                    SORTCOLUMN {
                        propkey: PKEY_ItemNameDisplay,
                        direction: SORT_ASCENDING,
                    },
                ],
                columns: vec![PKEY_ItemNameDisplay, PKEY_DateModified],
            }
            .into();
            unsafe {
                let view: IFolderView2 = site.QueryService(&IFolderView::IID).unwrap();
                assert_eq!(view.GetSelectedItem(0).unwrap(), 2);
                assert_eq!(view.GetSelectedItem(3).unwrap(), -1);
                let persistence: IPersistIDList = view.cast().unwrap();
                let copy = Pidl(persistence.GetIDList().unwrap());
                assert_ne!(copy.ptr(), expected.ptr());
                assert!(ILIsEqual(copy.ptr(), expected.ptr()).as_bool());
                let mut sorts =
                    vec![SORTCOLUMN::default(); view.GetSortColumnCount().unwrap() as usize];
                view.GetSortColumns(&mut sorts).unwrap();
                assert_eq!(sorts[0].propkey, PKEY_DateModified);
                assert_eq!(sorts[0].direction, SORT_DESCENDING);
                assert_eq!(sorts[1].propkey, PKEY_ItemNameDisplay);
                assert_eq!(sorts[1].direction, SORT_ASCENDING);
                let columns: IColumnManager = view.cast().unwrap();
                let mut keys = vec![
                    PROPERTYKEY::default();
                    columns.GetColumnCount(CM_ENUM_VISIBLE).unwrap() as usize
                ];
                columns.GetColumns(CM_ENUM_VISIBLE, &mut keys).unwrap();
                assert_eq!(keys, vec![PKEY_ItemNameDisplay, PKEY_DateModified]);
                let agile = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
                let mut out = std::ptr::null_mut();
                assert_eq!(site.query(&agile, &mut out), E_NOINTERFACE);
                assert!(out.is_null());
                drop(columns);
                drop(persistence);
                drop(view);
                drop(site);
                assert!(ILIsEqual(copy.ptr(), expected.ptr()).as_bool());
            }
        })
        .join()
        .unwrap();
    }
}
