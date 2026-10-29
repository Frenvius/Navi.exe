use super::{com_init, Pidl};
use windows::{
    core::{IUnknown, IUnknown_Vtbl, Interface, GUID, HRESULT, PCWSTR},
    Win32::{
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::Shell::{IShellItem, SHCreateItemFromIDList},
    },
};

#[repr(transparent)]
#[derive(Clone)]
struct Automatic(IUnknown);
unsafe impl Interface for Automatic {
    type Vtable = AutomaticVtbl;
    const IID: GUID = GUID::from_u128(0xe9c5ef8d_fd41_4f72_ba87_eb03bad5817c);
}
#[repr(C)]
struct AutomaticVtbl {
    base: IUnknown_Vtbl,
    initialize: unsafe extern "system" fn(*mut std::ffi::c_void, PCWSTR, PCWSTR, PCWSTR) -> HRESULT,
    has_list: usize,
    get_list: usize,
    add_usage: usize,
    pin: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, i32) -> HRESULT,
    index: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        *mut std::ffi::c_void,
        *mut i32,
    ) -> HRESULT,
    remove: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> HRESULT,
}
impl Automatic {
    fn new() -> windows::core::Result<Self> {
        com_init();
        unsafe {
            let list: Self = CoCreateInstance(
                &GUID::from_u128(0xf0ae1542_f497_484b_a175_a20db09144ba),
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            (list.vtable().initialize)(
                list.as_raw(),
                windows::core::w!("Microsoft.Windows.Explorer"),
                PCWSTR::null(),
                PCWSTR::null(),
            )
            .ok()?;
            Ok(list)
        }
    }
}
pub fn index(path: &str) -> Result<i32, String> {
    let list = Automatic::new().map_err(|e| e.to_string())?;
    let pidl = Pidl::parse(path)?;
    unsafe {
        let item: IShellItem = SHCreateItemFromIDList(pidl.ptr()).map_err(|e| e.to_string())?;
        let mut index = -1;
        (list.vtable().index)(list.as_raw(), item.as_raw(), &mut index)
            .ok()
            .map_err(|e| e.to_string())?;
        Ok(index)
    }
}

/// A final pin index, after removing the dragged sources from their old positions.
pub fn pin_at(paths: &[String], at: usize) -> Result<(), String> {
    apply_plan(
        &paths
            .iter()
            .enumerate()
            .map(|(offset, path)| (path.clone(), at + offset))
            .collect::<Vec<_>>(),
    )
}

fn apply_plan(plan: &[(String, usize)]) -> Result<(), String> {
    let list = Automatic::new().map_err(|e| e.to_string())?;
    for (path, at) in plan {
        let pidl = Pidl::parse(path)?;
        unsafe {
            let item: IShellItem = SHCreateItemFromIDList(pidl.ptr()).map_err(|e| e.to_string())?;
            (list.vtable().pin)(
                list.as_raw(),
                item.as_raw(),
                (*at).try_into().map_err(|_| "Invalid pin index")?,
            )
            .ok()
            .map_err(|e| e.to_string())?;
        }
    }
    notify_changed();
    Ok(())
}

pub fn unpin(pidl: &Pidl) -> Result<(), String> {
    let list = Automatic::new().map_err(|error| error.to_string())?;
    unsafe {
        let item: IShellItem =
            SHCreateItemFromIDList(pidl.ptr()).map_err(|error| error.to_string())?;
        (list.vtable().remove)(list.as_raw(), item.as_raw())
            .ok()
            .map_err(|error| error.to_string())?;
    }
    notify_changed();
    Ok(())
}

pub fn notify_changed() {
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_IDLIST};
        if let Ok(module) = GetModuleHandleW(windows::core::w!("shlwapi.dll")) {
            if let Some(proc) = GetProcAddress(module, windows::core::PCSTR(633usize as *const u8))
            {
                let increment: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(proc);
                increment(windows::Win32::UI::Shell::GLOBALCOUNTER_COMMONPLACES_LIST_CACHE.0);
            }
        }
        for id in [
            "shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}",
            "::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}",
        ] {
            if let Ok(pidl) = Pidl::parse(id) {
                SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_IDLIST, Some(pidl.ptr().cast()), None);
            }
        }
    }
}

fn insertion_index(pins: &[String], sources: &[String], boundary: usize) -> usize {
    pins.iter()
        .take(boundary)
        .filter(|pin| {
            !sources
                .iter()
                .any(|source| source.eq_ignore_ascii_case(pin))
        })
        .count()
}

pub fn insert(paths: &[String], boundary: usize) -> Result<(), String> {
    let pins = super::nav::children("shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}")?
        .into_iter()
        .filter(|n| n.is_pinned)
        .map(|n| n.id)
        .collect::<Vec<_>>();
    apply_plan(&insertion_plan(&pins, paths, boundary))
}

fn insertion_plan(pins: &[String], sources: &[String], boundary: usize) -> Vec<(String, usize)> {
    let at = insertion_index(pins, sources, boundary);
    let mut plan = Vec::new();
    if sources.len() > 1 {
        // Stage only the dragged pins at the end, preserving their order. Moving
        // discontiguous sources directly to final indices can swap an untouched pin
        // into the middle of the dragged block. No other pin is passed to PinItem.
        let mut known = pins
            .iter()
            .map(|p| p.to_lowercase())
            .collect::<std::collections::HashSet<_>>();
        let mut count = pins.len();
        for source in sources {
            let existing = !known.insert(source.to_lowercase());
            plan.push((source.clone(), count - usize::from(existing)));
            if !existing {
                count += 1;
            }
        }
    }
    plan.extend(sources.iter().enumerate().map(|(i, p)| (p.clone(), at + i)));
    plan
}

use std::cell::{Cell, RefCell};
use windows::{
    core::{implement, Ref},
    Win32::{
        Foundation::POINTL,
        System::{
            Com::{
                IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
            },
            DataExchange::RegisterClipboardFormatW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT},
            Ole::{IDropTarget, IDropTarget_Impl, DROPEFFECT, DROPEFFECT_LINK, DROPEFFECT_NONE},
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::Shell::{DROPDESCRIPTION, DROPIMAGE_LINK},
    },
};

#[implement(IDropTarget)]
struct PinDrop {
    boundary: usize,
    allowed: Cell<bool>,
    data: RefCell<Option<IDataObject>>,
}
impl PinDrop {
    fn effect(&self, effect: *mut DROPEFFECT) {
        unsafe {
            *effect = if self.allowed.get() && (*effect).0 & DROPEFFECT_LINK.0 != 0 {
                DROPEFFECT_LINK
            } else {
                DROPEFFECT_NONE
            };
        }
    }
    fn describe(&self) {
        let data = self.data.borrow();
        let Some(data) = data.as_ref() else { return };
        unsafe {
            let Ok(global) = GlobalAlloc(
                GMEM_MOVEABLE | GMEM_ZEROINIT,
                std::mem::size_of::<DROPDESCRIPTION>(),
            ) else {
                return;
            };
            let ptr = GlobalLock(global) as *mut DROPDESCRIPTION;
            if ptr.is_null() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(global));
                return;
            }
            let mut description = DROPDESCRIPTION::default();
            description.r#type = DROPIMAGE_LINK;
            let label = crate::i18n::tr("menu.pin-home");
            let mut message = [0u16; 260];
            for (slot, value) in message.iter_mut().take(259).zip(label.encode_utf16()) {
                *slot = value;
            }
            description.szMessage = message;
            ptr.write_unaligned(description);
            let _ = GlobalUnlock(global);
            let format = FORMATETC {
                cfFormat: RegisterClipboardFormatW(windows::core::w!("DropDescription")) as u16,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            let medium = STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 { hGlobal: global },
                pUnkForRelease: std::mem::ManuallyDrop::new(None),
            };
            if data.SetData(&format, &medium, true).is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(global));
            }
        }
    }
}
impl IDropTarget_Impl for PinDrop_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let paths = data
            .as_ref()
            .map(super::ops::drag_items)
            .unwrap_or_default();
        self.allowed.set(
            !paths.is_empty()
                && paths
                    .iter()
                    .all(|p| std::path::Path::new(p).is_dir() || super::ops::is_folder(p)),
        );
        *self.data.borrow_mut() = data.cloned();
        self.effect(effect);
        if self.allowed.get() {
            self.describe();
        }
        Ok(())
    }
    fn DragOver(
        &self,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.effect(effect);
        if self.allowed.get() {
            self.describe();
        }
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        self.data.borrow_mut().take();
        Ok(())
    }
    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.effect(effect);
        if unsafe { *effect } != DROPEFFECT_NONE {
            let paths = data
                .as_ref()
                .map(super::ops::drag_items)
                .unwrap_or_default();
            if let Err(error) = insert(&paths, self.boundary) {
                unsafe { *effect = DROPEFFECT_NONE };
                eprintln!("Quick access drop: {error}");
                return Err(windows::core::Error::from_win32());
            }
        }
        self.data.borrow_mut().take();
        Ok(())
    }
}
pub fn drop_target(boundary: usize) -> IDropTarget {
    PinDrop {
        boundary,
        allowed: Cell::new(false),
        data: RefCell::new(None),
    }
    .into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn namespace_drag_identity_survives_without_a_filesystem_drop_format() {
        super::com_init();
        let pc = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
        let data = super::super::ops::data_object(
            windows::Win32::Foundation::HWND::default(),
            &[pc.into()],
        )
        .unwrap();
        assert!(super::super::ops::dropped_files(&data).is_empty());
        let items = super::super::ops::drag_items(&data);
        assert_eq!(items.len(), 1);
        assert!(items[0].eq_ignore_ascii_case(pc));
    }
    #[test]
    fn discontiguous_and_new_sources_keep_their_order_without_repinning_other_items() {
        for (sources, boundary, expected) in [
            (vec!["d", "b"], 4, vec!["a", "c", "d", "b"]),
            (vec!["b", "new", "d"], 0, vec!["b", "new", "d", "a", "c"]),
            (vec!["d", "b"], 2, vec!["a", "d", "b", "c"]),
        ] {
            let mut pins = ["a", "b", "c", "d"].map(String::from).to_vec();
            let sources = sources.into_iter().map(String::from).collect::<Vec<_>>();
            for (path, at) in super::insertion_plan(&pins, &sources, boundary) {
                assert!(sources.contains(&path));
                if let Some(old) = pins.iter().position(|p| p == &path) {
                    pins.remove(old);
                }
                pins.insert(at, path);
            }
            assert_eq!(pins, expected);
        }
    }
    #[test]
    fn removing_dragged_pins_adjusts_the_insertion_boundary() {
        let pins = ["a", "b", "c", "d"].map(String::from);
        assert_eq!(super::insertion_index(&pins, &["A".into()], 4), 3);
        assert_eq!(super::insertion_index(&pins, &["d".into()], 1), 1);
        assert_eq!(
            super::insertion_index(&pins, &["a".into(), "c".into()], 4),
            2
        );
        assert_eq!(super::insertion_index(&pins, &["new".into()], 2), 2);
    }
}
