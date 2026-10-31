pub mod address;
pub mod archive;
pub mod assoc;
pub mod backdrop;
pub mod browser;
pub mod cloud;
pub mod columns;
pub mod extmenu;
pub mod folder_options;
pub mod gallery;
pub mod home;
pub mod http;
pub mod icons;
pub mod jumplist;
pub mod library;
pub mod listing;
pub mod menu;
pub mod nav;
pub mod nav_options;
pub mod network;
pub mod newmenu;
pub mod open_site;
pub mod ops;
pub mod preview;
pub mod quick_access;
pub mod search;
pub mod share;
pub mod shell_windows;
pub mod text;
pub mod theme;
pub mod thumb_cache;
pub mod viewstate;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{
    ILClone, ILCombine, ILFindLastID, ILFree, ILRemoveLastID, IShellFolder, SHBindToObject,
    SHGetDesktopFolder, SHGetNameFromIDList, SHParseDisplayName, StrRetToStrW, SHGDNF, SIGDN,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

thread_local! {
    static COM_APARTMENT: Option<ComApartment> = unsafe {
        let result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if result.is_ok() {
            Some(ComApartment)
        } else {
            if result != windows::Win32::Foundation::RPC_E_CHANGED_MODE {
                eprintln!("shell COM initialization: {}", windows::core::Error::from_hresult(result));
            }
            None
        }
    };
}

pub fn com_init() {
    COM_APARTMENT.with(|_| {});
}

#[cfg(test)]
mod com_lifetime_tests {
    use super::*;
    use windows::Win32::System::Com::{CoUninitialize, COINIT_MULTITHREADED};

    #[test]
    fn repeated_shell_initialization_owns_only_one_apartment_reference() {
        std::thread::spawn(|| {
            for _ in 0..64 {
                com_init();
            }
            let mut releases = 0;
            loop {
                unsafe {
                    CoUninitialize();
                }
                releases += 1;
                assert!(releases <= 64);
                let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                result.ok().unwrap();
                if result == windows::core::HRESULT(0) {
                    break;
                }
                unsafe {
                    CoUninitialize();
                }
            }
            if releases != 1 {
                unsafe {
                    CoUninitialize();
                }
            }
            assert_eq!(releases, 1);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn an_existing_mta_keeps_its_original_apartment_reference() {
        std::thread::spawn(|| {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            }
            for _ in 0..64 {
                com_init();
            }
            unsafe {
                CoUninitialize();
            }
            let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            result.ok().unwrap();
            unsafe {
                CoUninitialize();
            }
            assert_eq!(result, windows::core::HRESULT(0));
        })
        .join()
        .unwrap();
    }
}

/// Owned absolute PIDL, freed on drop.
pub struct Pidl(pub *mut ITEMIDLIST);

impl Pidl {
    /// Parses a desktop-absolute parsing name (a file system path or a `::{CLSID}` path).
    /// An empty string yields the desktop root.
    pub fn parse(id: &str) -> Result<Pidl, String> {
        unsafe {
            if id.is_empty() {
                let empty = ITEMIDLIST::default();
                return Ok(Pidl(ILClone(&empty)));
            }
            let w = wide(id);
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if let Err(first) =
                SHParseDisplayName(PCWSTR::from_raw(w.as_ptr()), None, &mut pidl, 0, None)
            {
                // Some registered virtual folders (notably AppsFolder) return a
                // desktop-absolute parsing name which only the shell: moniker can
                // bind again. Preserve the canonical identity, not an FS guess.
                if !pidl.is_null() {
                    ILFree(Some(pidl));
                    pidl = std::ptr::null_mut();
                }
                if !id.starts_with("::{") {
                    return Err(format!("{id}: {}", first.message()));
                }
                let moniker = wide(&format!("shell:{id}"));
                SHParseDisplayName(PCWSTR(moniker.as_ptr()), None, &mut pidl, 0, None)
                    .map_err(|e| format!("{id}: {}", e.message()))?;
            }
            Ok(Pidl(pidl))
        }
    }

    pub fn ptr(&self) -> *const ITEMIDLIST {
        self.0
    }

    pub fn is_desktop(&self) -> bool {
        unsafe { self.0.is_null() || (*self.0).mkid.cb == 0 }
    }

    pub fn name(&self, kind: SIGDN) -> String {
        unsafe { name_of(self.0, kind) }
    }

    pub fn parent(&self) -> Option<Pidl> {
        if self.is_desktop() {
            return None;
        }
        unsafe {
            let copy = ILClone(self.0);
            let _ = ILRemoveLastID(Some(copy));
            Some(Pidl(copy))
        }
    }

    pub fn last_id(&self) -> *const ITEMIDLIST {
        unsafe { ILFindLastID(self.0) }
    }

    /// Parses a file system path through "Este Computador", so that it goes by its drive
    /// (SHParseDisplayName files a path in the profile under the user's own folder).
    pub fn parse_in_this_pc(path: &str) -> Result<Pidl, String> {
        unsafe {
            let pc = Pidl::parse("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}")?;
            let folder = pc.folder()?;
            let w = wide(path);
            let mut rel: *mut ITEMIDLIST = std::ptr::null_mut();
            folder
                .ParseDisplayName(
                    Default::default(),
                    None,
                    PCWSTR::from_raw(w.as_ptr()),
                    None,
                    &mut rel,
                    std::ptr::null_mut(),
                )
                .map_err(|e| format!("{path}: {}", e.message()))?;
            let rel = Pidl(rel);
            Ok(Pidl(ILCombine(Some(pc.0), Some(rel.0))))
        }
    }

    /// Binds to the folder this PIDL points at.
    pub fn folder(&self) -> Result<IShellFolder, String> {
        unsafe {
            let desktop = SHGetDesktopFolder().map_err(|e| e.message())?;
            if self.is_desktop() {
                return Ok(desktop);
            }
            SHBindToObject(&desktop, self.0, None).map_err(|e| e.message())
        }
    }
}

#[cfg(test)]
mod namespace_roundtrip_tests {
    use super::*;
    #[test]
    fn native_virtual_parsing_names_roundtrip_to_the_same_folder() {
        com_init();
        for alias in [
            "shell:AppsFolder",
            "shell:ControlPanelFolder",
            "shell:Fonts",
        ] {
            let original = Pidl::parse(alias).unwrap();
            let name = original.name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING);
            assert!(!name.is_empty(), "{alias}");
            let parsed = Pidl::parse(&name).unwrap();
            assert!(
                unsafe {
                    windows::Win32::UI::Shell::ILIsEqual(original.ptr(), parsed.ptr()).as_bool()
                },
                "{alias}: {name}"
            );
            parsed.folder().unwrap();
        }
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ILFree(Some(self.0)) };
        }
    }
}

pub unsafe fn name_of(pidl: *const ITEMIDLIST, kind: SIGDN) -> String {
    SHGetNameFromIDList(pidl, kind)
        .ok()
        .map(|p| take_pwstr(p))
        .unwrap_or_default()
}

pub unsafe fn take_pwstr(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

pub unsafe fn display_name(
    folder: &IShellFolder,
    child: *const ITEMIDLIST,
    flags: SHGDNF,
) -> String {
    let mut strret = STRRET::default();
    if folder.GetDisplayNameOf(child, flags, &mut strret).is_err() {
        return String::new();
    }
    let mut psz = PWSTR::null();
    if StrRetToStrW(&mut strret, Some(child), &mut psz).is_err() {
        return String::new();
    }
    take_pwstr(psz)
}

/// Splits a list of parsing names into (parent folder, child pidls). All items must share
/// the same parent, which is how the shell builds a multi-item context menu or data object.
pub struct ItemSet {
    pub parent: Pidl,
    pub folder: IShellFolder,
    pub items: Vec<Pidl>,
}

impl ItemSet {
    pub fn new(paths: &[String]) -> Result<ItemSet, String> {
        let items = paths
            .iter()
            .map(|p| Pidl::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        let first = items.first().ok_or("no items")?;
        let parent = first.parent().ok_or("item has no parent")?;
        let folder = parent.folder()?;
        Ok(ItemSet {
            parent,
            folder,
            items,
        })
    }

    pub fn children(&self) -> Vec<*const ITEMIDLIST> {
        self.items.iter().map(|p| p.last_id()).collect()
    }
}
