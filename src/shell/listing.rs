use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, PROPERTYKEY};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Capacity, PKEY_DateCreated, PKEY_DateModified, PKEY_FreeSpace, PKEY_ItemTypeText,
    PKEY_Size,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemServices::{
    SFGAO_CANCOPY, SFGAO_CANDELETE, SFGAO_CANMOVE, SFGAO_CANRENAME, SFGAO_FILESYSTEM, SFGAO_FOLDER,
    SFGAO_GHOSTED, SFGAO_HASSUBFOLDER, SFGAO_HIDDEN, SFGAO_LINK, SFGAO_STREAM,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILCombine, ILFree, IShellItem2, SHCreateItemWithParent, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN,
    SHCONTF_INCLUDESUPERHIDDEN, SHCONTF_NONFOLDERS, SHGDNF, SHGDN_FORPARSING, SHGDN_INFOLDER,
    SHGDN_NORMAL, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
};

use super::{com_init, display_name, icons, take_pwstr, Pidl};

#[derive(Clone, Debug)]
pub struct ShellEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_fs: bool,
    pub is_link: bool,
    pub hidden: bool,
    pub size: u64,
    pub modified: u64,
    pub created: u64,
    pub type_name: String,
    pub icon: u32,
    pub icon_pending: bool,
    pub file_attributes: u32,
    pub library_origin: Option<super::library::Origin>,
    pub photo: Option<super::gallery::Photo>,
    pub total: u64,
    pub free: u64,
    pub can_move: bool,
    pub can_copy: bool,
    pub can_rename: bool,
    pub can_delete: bool,
    pub child: Vec<u8>,
    pub absolute: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExplorerSettings {
    pub show_hidden: bool,
    pub show_super_hidden: bool,
    pub hide_extensions: bool,
    pub check_boxes: bool,
    pub compact: bool,
    /// "Exibir o caminho completo na barra de título" (CabinetState\FullPath): file system
    /// folders title their tab and window with their path.
    pub full_path: bool,
    pub single_click: bool,
    pub underline_hover_only: bool,
    pub hover_ms: u32,
    pub new_window: bool,
    pub type_search: bool,
    pub icons_only: bool,
    pub type_overlay: bool,
    pub info_tips: bool,
    pub folder_size_tips: bool,
    pub compressed_color: bool,
    pub status_bar: bool,
    pub preview_handlers: bool,
    /// Options enforced by the native enumerator/menus/provider; track changes so
    /// their current views and commands are refreshed as well.
    pub native_options: [u32; 5],
}

const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";

pub fn settings() -> ExplorerSettings {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(ADVANCED).ok();
    let get = |name: &str, default: u32| -> u32 {
        key.as_ref()
            .and_then(|k| k.get_value::<u32, _>(name).ok())
            .unwrap_or(default)
    };
    let mut shell_state = windows::Win32::UI::Shell::SHELLSTATEA::default();
    unsafe {
        windows::Win32::UI::Shell::SHGetSetSettings(
            Some(&mut shell_state),
            windows::Win32::UI::Shell::SSF_DOUBLECLICKINWEBVIEW,
            false,
        );
    }
    let explorer = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer")
        .ok();
    // Shell32's per-process SHGetSetSettings cache can outlive an external General-tab
    // change. The persisted SHELLSTATE is size-prefixed; its first flag DWORD follows it.
    let click_flags = explorer
        .as_ref()
        .and_then(|k| k.get_raw_value("ShellState").ok())
        .filter(|v| v.vtype == winreg::enums::RegType::REG_BINARY)
        .and_then(|v| {
            v.bytes
                .get(4..8)
                .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
        })
        .unwrap_or(shell_state._bitfield1);
    let mut hover_ms = 400u32;
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETMOUSEHOVERTIME,
            0,
            Some((&mut hover_ms as *mut u32).cast()),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    ExplorerSettings {
        new_window: RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\CabinetState")
            .and_then(|k| k.get_raw_value("Settings"))
            .ok()
            .and_then(|v| {
                v.bytes
                    .get(4..8)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            })
            .is_some_and(|flags| flags & 0x20 != 0),
        type_search: get("TypeAhead", 0) == 1,
        icons_only: get("IconsOnly", 0) == 1,
        type_overlay: get("ShowTypeOverlay", 1) == 1,
        info_tips: get("ShowInfoTip", 1) == 1,
        folder_size_tips: get("FolderContentsInfoTip", 1) == 1,
        compressed_color: get("ShowEncryptCompressedColor", 0) == 1,
        status_bar: get("ShowStatusBar", 1) == 1,
        preview_handlers: get("ShowPreviewHandlers", 1) == 1,
        native_options: [
            get("SharingWizardOn", 1),
            get("HideMergeConflicts", 1),
            get("HideDrivesWithNoMedia", 1),
            get("ShowSyncProviderNotifications", 1),
            get("NavPaneShowStatus", 0),
        ],
        single_click: click_flags & (1 << 5) == 0,
        hover_ms: hover_ms.max(1),
        underline_hover_only: RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer")
            .and_then(|k| k.get_value::<u32, _>("IconUnderline"))
            .unwrap_or(2)
            == 2,
        show_hidden: get("Hidden", 2) == 1,
        show_super_hidden: get("ShowSuperHidden", 0) == 1,
        hide_extensions: get("HideFileExt", 1) == 1,
        check_boxes: get("AutoCheckSelect", 0) == 1,
        compact: get("UseCompactMode", 0) == 1,
        full_path: RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\CabinetState")
            .and_then(|k| k.get_value::<u32, _>("FullPath"))
            .unwrap_or(0)
            == 1,
    }
}

const DETAILS_CONTAINER: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\Modules\GlobalSettings\DetailsContainer";

/// Explorer's shared "Painel de detalhes" / "Painel de visualização" state: two DWORDs,
/// details then preview, 1 shown and 2 hidden.
pub fn panes() -> (bool, bool) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let value = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(DETAILS_CONTAINER)
        .and_then(|k| k.get_raw_value("DetailsContainer"))
        .map(|v| v.bytes)
        .unwrap_or_default();
    let dword = |i: usize| {
        value
            .get(i * 4..i * 4 + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    (dword(0) == Some(1), dword(1) == Some(1))
}

/// Saves the panes for every Explorer window, as toggling one in Explorer does.
pub fn set_panes(details: bool, preview: bool) {
    use winreg::enums::{RegType, HKEY_CURRENT_USER};
    use winreg::{RegKey, RegValue};
    let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(DETAILS_CONTAINER) else {
        return;
    };
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&(if details { 1u32 } else { 2 }).to_le_bytes());
    bytes.extend_from_slice(&(if preview { 1u32 } else { 2 }).to_le_bytes());
    let _ = key.set_raw_value(
        "DetailsContainer",
        &RegValue {
            bytes,
            vtype: RegType::REG_BINARY,
        },
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderOption {
    HiddenItems,
    Extensions,
    CheckBoxes,
    Compact,
}

pub fn set_option(option: FolderOption, on: bool) {
    use windows::Win32::UI::Shell::{
        SHGetSetSettings, SHELLSTATEA, SSF_AUTOCHECKSELECT, SSF_SHOWALLOBJECTS, SSF_SHOWEXTENSIONS,
    };
    if option == FolderOption::Compact {
        use winreg::enums::HKEY_CURRENT_USER;
        let changed = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(ADVANCED)
            .and_then(|(key, _)| key.set_value("UseCompactMode", &(on as u32)));
        if let Err(error) = changed {
            eprintln!("compact view preference: {error}");
            return;
        }
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::*;
            if SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                windows::Win32::Foundation::WPARAM(0),
                LPARAM(windows::core::w!("ShellState").as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                100,
                None,
            )
            .0 == 0
            {
                eprintln!(
                    "compact view notification: {}",
                    windows::core::Error::from_win32()
                );
            }
        }
        return;
    }
    let (mask, first, second) = match option {
        FolderOption::HiddenItems => (SSF_SHOWALLOBJECTS, 1 << 0, 0),
        FolderOption::Extensions => (SSF_SHOWEXTENSIONS, 1 << 1, 0),
        _ => (SSF_AUTOCHECKSELECT, 0, 1 << 3),
    };
    let set = |v: i32, bits: i32| if on { v | bits } else { v & !bits };
    unsafe {
        let mut state: SHELLSTATEA = std::mem::zeroed();
        SHGetSetSettings(Some(&mut state), mask, false);
        state._bitfield1 = set(state._bitfield1, first);
        state._bitfield2 = set(state._bitfield2, second);
        SHGetSetSettings(Some(&mut state), mask, true);
    }
}

fn filetime_to_ms(ft: FILETIME) -> u64 {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    ticks.saturating_sub(116_444_736_000_000_000) / 10_000
}

unsafe fn string_prop(item: &IShellItem2, key: &PROPERTYKEY) -> String {
    item.GetString(key)
        .map(|p| take_pwstr(p))
        .unwrap_or_default()
}

pub fn list(id: &str) -> Result<Vec<ShellEntry>, String> {
    com_init();
    list_pidl(&Pidl::parse(id)?)
}

/// Lists the children of a folder given by its absolute PIDL (also used for search folders).
pub fn list_pidl(parent: &Pidl) -> Result<Vec<ShellEntry>, String> {
    let mut entries = Vec::new();
    for_each_pidl(parent, |entry| {
        entries.push(entry);
        true
    })?;
    Ok(entries)
}

pub fn for_each_pidl(
    parent: &Pidl,
    mut emit: impl FnMut(ShellEntry) -> bool,
) -> Result<(), String> {
    com_init();
    let opts = settings();
    let parent_path = parent.name(SIGDN_DESKTOPABSOLUTEPARSING);
    let library = super::library::is_library(&parent_path);
    let origins = if library {
        super::library::origins(&parent_path)
    } else {
        Vec::new()
    };
    unsafe {
        let folder = parent.folder()?;

        let mut flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32;
        if opts.show_hidden {
            flags |= SHCONTF_INCLUDEHIDDEN.0 as u32;
        }
        if opts.show_super_hidden {
            flags |= SHCONTF_INCLUDESUPERHIDDEN.0 as u32;
        }

        let mut penum = None;
        folder
            .EnumObjects(HWND::default(), flags, &mut penum)
            .ok()
            .map_err(|e| e.message())?;

        let Some(en) = penum else { return Ok(()) };
        let virtual_folder = library
            || parent
                .name(windows::Win32::UI::Shell::SIGDN_FILESYSPATH)
                .is_empty()
            || super::archive::root(&parent_path).is_some();

        let reader = EntryReader {
            parent,
            folder,
            parent_path,
            origins,
            virtual_folder,
        };
        loop {
            let mut child: [*mut ITEMIDLIST; 1] = [std::ptr::null_mut()];
            let mut fetched = 0u32;
            let _ = en.Next(&mut child, Some(&mut fetched));
            if fetched == 0 {
                break;
            }
            let child = child[0];

            let keep_going = emit(reader.read(child));

            CoTaskMemFree(Some(child as *const _));
            if !keep_going {
                break;
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Crumb {
    /// Native desktop ancestor for the tree, before address-only drive rebasing.
    pub nav_root: Option<String>,
    pub id: String,
    pub name: String,
    pub icon: u32,
    /// Whether it says it has subfolders (the last crumb's chevron goes by it).
    pub has_subfolders: bool,
}

/// The chain of ancestors (root first, desktop excluded) shown in the address bar.
pub fn crumbs(id: &str) -> Result<Vec<Crumb>, String> {
    com_init();
    let mut chain = Vec::new();
    let mut pidl = Pidl::parse(id)?;
    let mut nav_root = None;
    // A path in the profile goes by its drive, as Explorer shows it, rather than under the
    // user's own folder (which only shows as such when reached from the desktop).
    if is_profile_path(id) {
        if let Ok(by_drive) = Pidl::parse_in_this_pc(id) {
            nav_root = top_of(pidl).map(|p| p.name(SIGDN_DESKTOPABSOLUTEPARSING));
            pidl = by_drive;
        }
    }
    let mut cur = Some(pidl);
    while let Some(p) = cur {
        if p.is_desktop() {
            if chain.is_empty() {
                chain.push(Crumb {
                    nav_root: None,
                    id: String::new(),
                    name: p.name(SIGDN_NORMALDISPLAY),
                    icon: unsafe { icons::icon_of_pidl(p.ptr()) },
                    has_subfolders: true,
                });
            }
            break;
        }
        chain.push(Crumb {
            nav_root: None,
            id: p.name(SIGDN_DESKTOPABSOLUTEPARSING),
            name: p.name(SIGDN_NORMALDISPLAY),
            icon: unsafe { icons::icon_of_pidl(p.ptr()) },
            has_subfolders: has_subfolders(&p),
        });
        cur = p.parent();
    }
    chain.reverse();
    if let Some(first) = chain.first_mut() {
        first.nav_root = nav_root;
    }
    Ok(chain)
}

/// A location's name as the shell shows it (its parsing name when it doesn't parse).
pub fn display_name_of(id: &str) -> String {
    com_init();
    match Pidl::parse(id) {
        Ok(p) => p.name(SIGDN_NORMALDISPLAY),
        Err(_) => id.to_string(),
    }
}

/// An item's folder as Explorer shows it ("Local do arquivo"): with the folders' display
/// names, "C:\Usuários\frenv" for C:\Users\frenv (System.ItemFolderPathDisplay).
pub fn folder_path_display(path: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::EnhancedStorage::PKEY_ItemFolderPathDisplay;
    use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
    com_init();
    let w = super::wide(path);
    unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        item.GetString(&PKEY_ItemFolderPathDisplay)
            .ok()
            .map(|p| take_pwstr(p))
            .filter(|s| !s.is_empty())
    }
}

/// Whether an item says it has subfolders (SFGAO_HASSUBFOLDER, asked of its parent).
fn has_subfolders(p: &Pidl) -> bool {
    let Some(parent) = p.parent() else {
        return true;
    };
    let Ok(folder) = parent.folder() else {
        return true;
    };
    let mut attrs = SFGAO_HASSUBFOLDER.0;
    unsafe {
        folder.GetAttributesOf(&[p.last_id()], &mut attrs).is_ok()
            && attrs & SFGAO_HASSUBFOLDER.0 != 0
    }
}

/// A folder's subfolders as the address bar's dropdowns list them: as the folder itself
/// orders them (CompareIDs, so the desktop's own items come first in their set order), with
/// archives among them under their names without the extension. (name, parsing name).
pub fn subfolders(id: &str) -> Result<Vec<(String, String)>, String> {
    com_init();
    let opts = settings();
    let parent = Pidl::parse(id)?;
    unsafe {
        let folder = parent.folder()?;
        let mut flags = SHCONTF_FOLDERS.0 as u32;
        if opts.show_hidden {
            flags |= SHCONTF_INCLUDEHIDDEN.0 as u32;
        }
        if opts.show_super_hidden {
            flags |= SHCONTF_INCLUDESUPERHIDDEN.0 as u32;
        }
        let mut penum = None;
        folder
            .EnumObjects(HWND::default(), flags, &mut penum)
            .ok()
            .map_err(|e| e.message())?;
        let Some(en) = penum else {
            return Ok(Vec::new());
        };
        let mut kids: Vec<*mut ITEMIDLIST> = Vec::new();
        loop {
            let mut child: [*mut ITEMIDLIST; 1] = [std::ptr::null_mut()];
            let mut fetched = 0u32;
            let _ = en.Next(&mut child, Some(&mut fetched));
            if fetched == 0 {
                break;
            }
            kids.push(child[0]);
        }
        kids.sort_by(|a, b| (folder.CompareIDs(LPARAM(0), *a, *b).0 as i16).cmp(&0));
        let out = kids
            .iter()
            .map(|&child| {
                let mut name =
                    display_name(&folder, child, SHGDNF(SHGDN_INFOLDER.0 | SHGDN_NORMAL.0));
                let mut path = display_name(&folder, child, SHGDN_FORPARSING);
                if path.is_empty() {
                    let abs = ILCombine(Some(parent.ptr()), Some(child));
                    path = super::name_of(abs, SIGDN_DESKTOPABSOLUTEPARSING);
                    ILFree(Some(abs));
                }
                let mut attrs = SFGAO_STREAM.0;
                let _ = folder.GetAttributesOf(&[child as *const _], &mut attrs);
                if attrs & SFGAO_STREAM.0 != 0 {
                    if let Some(ext) = std::path::Path::new(&path)
                        .extension()
                        .and_then(|e| e.to_str())
                    {
                        if let Some(stem) = name.strip_suffix(&format!(".{ext}")) {
                            name = stem.to_string();
                        }
                    }
                }
                CoTaskMemFree(Some(child as *const _));
                (name, path)
            })
            .collect();
        Ok(out)
    }
}

/// Whether a path is the profile folder or in it, filed under the profile folder itself
/// (a sync root in it, such as OneDrive's, keeps its own place).
fn is_profile_path(id: &str) -> bool {
    let Ok(profile) = std::env::var("USERPROFILE") else {
        return false;
    };
    let (lower, profile) = (id.to_lowercase(), profile.to_lowercase());
    if lower != profile && !lower.starts_with(&format!("{profile}\\")) {
        return false;
    }
    let Ok(pidl) = Pidl::parse(id) else {
        return false;
    };
    top_of(pidl).is_some_and(|top| top.name(SIGDN_DESKTOPABSOLUTEPARSING).to_lowercase() == profile)
}

/// The item under the desktop a PIDL descends from.
fn top_of(mut cur: Pidl) -> Option<Pidl> {
    loop {
        let parent = cur.parent()?;
        if parent.is_desktop() {
            return Some(cur);
        }
        cur = parent;
    }
}

/// Which of an item's property lists to read (its file type's registry lists).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropList {
    Content,
    ContentSearch,
    Tile,
    Pane,
}

/// An item's infotip as Explorer shows it: its shell folder's tip (a folder's size, its
/// first subfolders and files, "Pasta vazia"...), lines split.
pub fn info_tip(path: &str) -> Vec<String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, IQueryInfo, SHCreateItemFromParsingName, QITIPF_DEFAULT, QITIPF_USESLOWTIP,
    };
    com_init();
    let w = super::wide(path);
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, windows::Win32::UI::Shell::IShellItem>(
            PCWSTR(w.as_ptr()),
            None,
        ) else {
            return Vec::new();
        };
        let Ok(info) = item.BindToHandler::<_, IQueryInfo>(None, &BHID_SFUIObject) else {
            return Vec::new();
        };
        let flags = if settings().folder_size_tips {
            QITIPF_DEFAULT | QITIPF_USESLOWTIP
        } else {
            QITIPF_DEFAULT
        };
        let Ok(tip) = info.GetInfoTip(flags) else {
            return Vec::new();
        };
        let text = take_pwstr(tip);
        text.lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    }
}

/// One property of an item as the shell shows it: its label and display value (direction
/// marks dropped), when it has one.
pub fn property(path: &str, name: &str) -> Option<(String, String)> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::PDFF_DEFAULT;
    use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
    com_init();
    let w = super::wide(path);
    let pk = super::columns::key(name)?;
    unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let desc = super::columns::description(&pk)?;
        let v = item.GetProperty(&pk).ok().filter(|v| !v.is_empty())?;
        let value: String = take_pwstr(desc.FormatForDisplay(&v, PDFF_DEFAULT).ok()?)
            .chars()
            .filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'))
            .collect();
        let label = take_pwstr(desc.GetDisplayName().ok()?);
        (!value.is_empty()).then_some((label, value))
    }
}

/// An item's property list as Explorer lays it out: per slot, its label (empty when the
/// list hides it) and display value, or None for a placeholder or an empty value. The
/// name, first in the content list, is left out.
pub fn prop_list(path: &str, list: PropList) -> Vec<Option<(String, String)>> {
    prop_list_with_id(path, list, &[])
}

pub fn prop_list_with_id(path: &str, list: PropList, id: &[u8]) -> Vec<Option<(String, String)>> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_ItemNameDisplay, PKEY_PropList_ContentViewModeForBrowse,
        PKEY_PropList_ContentViewModeForSearch, PKEY_PropList_TileInfo,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyDescription, IPropertyDescriptionList, PDFF_DEFAULT, PDVF_HIDELABEL,
    };
    use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
    com_init();
    let key = match list {
        PropList::Content => PKEY_PropList_ContentViewModeForBrowse,
        PropList::ContentSearch => PKEY_PropList_ContentViewModeForSearch,
        PropList::Tile => PKEY_PropList_TileInfo,
        PropList::Pane => windows::Win32::Storage::EnhancedStorage::PKEY_PropList_PreviewDetails,
    };
    let w = super::wide(path);
    let mut out = Vec::new();
    unsafe {
        let item = if id.is_empty() {
            SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(w.as_ptr()), None)
        } else {
            windows::Win32::UI::Shell::SHCreateItemFromIDList::<IShellItem2>(id.as_ptr().cast())
        };
        let Ok(item) = item else { return out };
        let Ok(descs) = item.GetPropertyDescriptionList::<IPropertyDescriptionList>(&key) else {
            return out;
        };
        let count = descs.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(desc) = descs.GetAt::<IPropertyDescription>(i) else {
                out.push(None);
                continue;
            };
            let mut pk = PROPERTYKEY::default();
            if desc.GetPropertyKey(&mut pk).is_err() {
                out.push(None);
                continue;
            }
            if pk == PKEY_ItemNameDisplay {
                continue;
            }
            let value = item
                .GetProperty(&pk)
                .ok()
                // An unset value would format as "Sem classificação" and such; Explorer leaves it out.
                .filter(|v| !v.is_empty())
                .and_then(|v| desc.FormatForDisplay(&v, PDFF_DEFAULT).ok())
                .map(|p| take_pwstr(p))
                .unwrap_or_default();
            if value.is_empty() {
                out.push(None);
                continue;
            }
            let hide = desc
                .GetViewFlags()
                .map(|f| f.0 & PDVF_HIDELABEL.0 != 0)
                .unwrap_or(false);
            let label = if hide {
                String::new()
            } else {
                desc.GetDisplayName()
                    .map(|p| take_pwstr(p))
                    .unwrap_or_default()
            };
            out.push(Some((label, value)));
        }
    }
    out
}

struct EntryReader<'a> {
    parent: &'a Pidl,
    folder: windows::Win32::UI::Shell::IShellFolder,
    parent_path: String,
    origins: Vec<super::library::Origin>,
    virtual_folder: bool,
}

impl EntryReader<'_> {
    unsafe fn read(&self, child: *mut ITEMIDLIST) -> ShellEntry {
        let mut attrs = (SFGAO_FOLDER
            | SFGAO_STREAM
            | SFGAO_FILESYSTEM
            | SFGAO_HIDDEN
            | SFGAO_LINK
            | SFGAO_GHOSTED
            | SFGAO_CANMOVE
            | SFGAO_CANCOPY
            | SFGAO_CANRENAME
            | SFGAO_CANDELETE)
            .0;
        let _ = self
            .folder
            .GetAttributesOf(&[child as *const _], &mut attrs);
        let has = |f: u32| attrs & f != 0;

        let abs = ILCombine(Some(self.parent.ptr()), Some(child));
        let name = display_name(
            &self.folder,
            child,
            SHGDNF(SHGDN_INFOLDER.0 | SHGDN_NORMAL.0),
        );
        let mut path = display_name(&self.folder, child, SHGDN_FORPARSING);
        if path.is_empty() {
            path = super::name_of(abs, SIGDN_DESKTOPABSOLUTEPARSING);
        }
        let is_link = has(SFGAO_LINK.0);

        let is_drive = path.len() <= 3 && path.ends_with(":\\");
        let item: Option<IShellItem2> = SHCreateItemWithParent(None, &self.folder, child).ok();
        let (size, modified, created, type_name, total, free) = match &item {
            Some(it) => (
                it.GetUInt64(&PKEY_Size).unwrap_or(0),
                it.GetFileTime(&PKEY_DateModified)
                    .map(filetime_to_ms)
                    .unwrap_or(0),
                it.GetFileTime(&PKEY_DateCreated)
                    .map(filetime_to_ms)
                    .unwrap_or(0),
                string_prop(it, &PKEY_ItemTypeText),
                if is_drive {
                    it.GetUInt64(&PKEY_Capacity).unwrap_or(0)
                } else {
                    0
                },
                if is_drive {
                    it.GetUInt64(&PKEY_FreeSpace).unwrap_or(0)
                } else {
                    0
                },
            ),
            None => (0, 0, 0, String::new(), 0, 0),
        };

        let is_dir = has(SFGAO_FOLDER.0) && !has(SFGAO_STREAM.0);
        // Overlay handlers and shortcut targets cost milliseconds per item, so a file system
        // item's real icon is resolved only once its row is shown.
        let deferred = has(SFGAO_FILESYSTEM.0) && !is_drive;
        let (icon, icon_pending) = if deferred {
            let placeholder = if is_dir {
                icons::folder_icon()
            } else {
                icons::generic_icon(&path)
            };
            (placeholder, true)
        } else {
            (icons::icon_of(&self.folder, child, abs, is_link), false)
        };
        let file_attributes = item
            .as_ref()
            .and_then(|it| {
                it.GetUInt32(&windows::Win32::Storage::EnhancedStorage::PKEY_FileAttributes)
                    .ok()
            })
            .unwrap_or(0);

        let entry = ShellEntry {
            photo: if self
                .parent_path
                .to_ascii_lowercase()
                .contains("e88865ea-0e1c-4e20-9aa6-edcd0212c87c")
            {
                item.as_ref().map(super::gallery::photo)
            } else {
                None
            },
            library_origin: super::library::origin_for(&path, &self.origins),
            name,
            path,
            is_dir,
            is_fs: has(SFGAO_FILESYSTEM.0),
            is_link,
            hidden: has(SFGAO_HIDDEN.0) || has(SFGAO_GHOSTED.0),
            size,
            modified,
            created,
            type_name,
            icon,
            icon_pending,
            file_attributes,
            total,
            free,
            can_move: has(SFGAO_CANMOVE.0),
            can_copy: has(SFGAO_CANCOPY.0),
            can_rename: has(SFGAO_CANRENAME.0),
            can_delete: has(SFGAO_CANDELETE.0),
            child: if self.virtual_folder {
                std::slice::from_raw_parts(
                    child as *const u8,
                    windows::Win32::UI::Shell::ILGetSize(Some(child)) as usize,
                )
                .to_vec()
            } else {
                Vec::new()
            },
            absolute: if self.virtual_folder || !has(SFGAO_FILESYSTEM.0) {
                std::slice::from_raw_parts(
                    abs as *const u8,
                    windows::Win32::UI::Shell::ILGetSize(Some(abs)) as usize,
                )
                .to_vec()
            } else {
                Vec::new()
            },
        };

        ILFree(Some(abs));
        entry
    }
}

pub fn read_file(path: &str) -> Result<Option<ShellEntry>, String> {
    com_init();
    if !std::path::Path::new(path)
        .try_exists()
        .map_err(|error| format!("{path}: {error}"))?
    {
        return Ok(None);
    }
    let parent_path = std::path::Path::new(path)
        .parent()
        .ok_or_else(|| format!("missing parent: {path}"))?
        .to_string_lossy()
        .into_owned();
    let parent = Pidl::parse(&parent_path)?;
    let absolute = Pidl::parse(path)?;
    let reader = EntryReader {
        parent: &parent,
        folder: parent.folder()?,
        parent_path,
        origins: Vec::new(),
        virtual_folder: false,
    };
    let child = unsafe { windows::Win32::UI::Shell::ILFindLastID(absolute.ptr()) };
    let entry = unsafe { reader.read(child as *mut _) };
    let options = settings();
    Ok(((!entry.hidden || options.show_hidden)
        && (entry.file_attributes & 4 == 0 || !entry.hidden || options.show_super_hidden))
        .then_some(entry))
}

#[cfg(test)]
mod native_update_tests {
    use super::*;

    #[test]
    fn single_entry_refresh_keeps_shell_metadata_without_enumerating_siblings() {
        let root = std::env::temp_dir().join(format!("navi-single-entry-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("native-file.txt");
        std::fs::write(&file, b"before").unwrap();
        let path = file.to_str().unwrap();
        let before = list(root.to_str().unwrap()).unwrap().pop().unwrap();
        let single = read_file(path).unwrap().unwrap();
        assert_eq!(
            (
                &before.name,
                &before.type_name,
                before.size,
                before.modified,
                before.created,
                before.file_attributes,
                before.icon
            ),
            (
                &single.name,
                &single.type_name,
                single.size,
                single.modified,
                single.created,
                single.file_attributes,
                single.icon
            )
        );
        std::fs::write(&file, b"after: changed size").unwrap();
        assert_eq!(read_file(path).unwrap().unwrap().size, 19);
        std::fs::remove_file(&file).unwrap();
        assert!(read_file(path).unwrap().is_none());
        std::fs::remove_dir(root).unwrap();
    }
}
