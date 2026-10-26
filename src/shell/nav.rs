//! Navigation pane namespace: the same roots and children Explorer's tree shows.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY};
use windows::Win32::Storage::EnhancedStorage::PKEY_IsPinnedToNameSpaceTree;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_HASSUBFOLDER};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
use windows::Win32::UI::Shell::{
    ILCombine, ILFree, IShellItem2, SHCreateItemFromIDList, ShellExecuteExW, SEE_MASK_INVOKEIDLIST,
    SHCONTF_FOLDERS, SHCONTF_NAVIGATION_ENUM, SHELLEXECUTEINFOW, SHGDN_NORMAL,
    SIGDN_DESKTOPABSOLUTEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::{com_init, display_name, icons, name_of, wide, Pidl};

#[derive(Clone, Debug)]
pub struct NavNode {
    pub name: String,
    pub id: String,
    pub icon: u32,
    pub has_children: bool,
    pub is_fs: bool,
    pub is_pinned: bool,
}

/// Desktop children that Explorer always shows in the tree (This PC, Network, Home, Quick
/// access, Gallery), besides the ones pinned to the namespace tree.
const KEEP_CLSIDS: [&str; 5] = [
    "20D04FE0-3AEA-1069",
    "F02C1A0D-BE21-4350",
    "F874310E-B6B7-47DC",
    "679F85CB-0220-4080",
    "E88865EA-0E1C-4E20",
];

fn home_pinned_key() -> Option<PROPERTYKEY> {
    let name = wide("System.Home.IsPinned");
    let mut key = PROPERTYKEY::default();
    unsafe {
        PSGetPropertyKeyFromName(PCWSTR(name.as_ptr()), &mut key)
            .ok()
            .map(|_| key)
    }
}

pub fn children(id: &str) -> Result<Vec<NavNode>, String> {
    com_init();
    let at_root = id.is_empty();
    let options = if at_root {
        super::nav_options::read()
    } else {
        Default::default()
    };
    unsafe {
        let parent = Pidl::parse(id)?;
        let folder = parent.folder()?;
        let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NAVIGATION_ENUM.0) as u32;
        let mut penum = None;
        folder
            .EnumObjects(HWND::default(), flags, &mut penum)
            .ok()
            .map_err(|e| e.message())?;
        let home_key = if at_root { None } else { home_pinned_key() };

        let mut out = Vec::new();
        let Some(en) = penum else { return Ok(out) };
        let mut children = Vec::new();
        loop {
            let mut child = [std::ptr::null_mut()];
            let mut fetched = 0;
            let _ = en.Next(&mut child, Some(&mut fetched));
            if fetched == 0 {
                break;
            }
            children.push(child[0]);
        }
        children.sort_by(|a, b| (folder.CompareIDs(LPARAM(0), *a, *b).0 as i16).cmp(&0));
        for crel in children {
            let name = display_name(&folder, crel, SHGDN_NORMAL);
            let mut attrs = (SFGAO_HASSUBFOLDER | SFGAO_FILESYSTEM).0;
            let _ = folder.GetAttributesOf(&[crel as *const _], &mut attrs);
            let abs = ILCombine(Some(parent.ptr()), Some(crel));
            let node_id = name_of(abs, SIGDN_DESKTOPABSOLUTEPARSING);
            let item = SHCreateItemFromIDList::<IShellItem2>(abs).ok();
            let get_bool = |key: &PROPERTYKEY| {
                item.as_ref()
                    .and_then(|it| it.GetBool(key).ok())
                    .map(|b| b.as_bool())
                    .unwrap_or(false)
            };

            let keep;
            let is_pinned = if at_root {
                let up = node_id.to_uppercase();
                let pinned = get_bool(&PKEY_IsPinnedToNameSpaceTree);
                keep = if up.contains("20D04FE0-3AEA-1069") {
                    options.this_pc
                } else if up.contains("F02C1A0D-BE21-4350") {
                    options.network
                } else if up.contains("031E4825-7B94-4DC3") {
                    options.all || options.libraries
                } else {
                    options.all || pinned || KEEP_CLSIDS.iter().any(|c| up.contains(c))
                };
                pinned
            } else {
                keep = true;
                home_key.as_ref().map(get_bool).unwrap_or(false)
            };

            if keep {
                out.push(NavNode {
                    name,
                    id: node_id,
                    icon: icons::icon_of_pidl(abs),
                    has_children: attrs & SFGAO_HASSUBFOLDER.0 != 0,
                    is_fs: attrs & SFGAO_FILESYSTEM.0 != 0,
                    is_pinned,
                });
            }
            ILFree(Some(abs));
            CoTaskMemFree(Some(crel as *const _));
        }
        Ok(out)
    }
}

/// Pins or unpins a folder to Home (Quick access) through the shell verbs Explorer uses.
pub fn set_pinned(id: &str, pinned: bool) -> Result<(), String> {
    com_init();
    let pidl = Pidl::parse(id)?;
    set_pinned_pidl(&pidl, pinned)
}

pub fn set_pinned_pidl(pidl: &Pidl, pinned: bool) -> Result<(), String> {
    let verb = wide(if pinned { "pintohome" } else { "unpinfromhome" });
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpIDList: pidl.ptr() as *mut _,
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info).map_err(|e| e.message()) }
}

/// The user's accent color as 0xRRGGBB.
pub fn accent_color() -> u32 {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\DWM")
        .and_then(|k| k.get_value::<u32, _>("AccentColor"))
        .map(|abgr| ((abgr & 0xFF) << 16) | (abgr & 0xFF00) | ((abgr >> 16) & 0xFF))
        .unwrap_or(0x0078D4)
}

/// The accent shade WinUI fills with (AccentFillColorDefault) as 0xRRGGBB: AccentLight2 on dark
/// backgrounds, AccentDark1 on light ones.
pub fn accent_fill(dark: bool) -> u32 {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let palette = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent")
        .and_then(|k| k.get_raw_value("AccentPalette"));
    // The palette is 8 RGBA shades, Light3 first: Light2 is the second, Dark1 the fifth.
    let at = if dark { 4 } else { 16 };
    match palette {
        Ok(v) if v.bytes.len() >= at + 4 => {
            let b = &v.bytes[at..at + 4];
            ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32
        }
        _ if dark => 0x60CDFF,
        _ => 0x005FB8,
    }
}

/// The desktop root appears only in "Show all folders", initially expanded.
pub fn desktop() -> Option<NavNode> {
    let root = Pidl::parse("").ok()?;
    Some(NavNode {
        name: root.name(windows::Win32::UI::Shell::SIGDN_NORMALDISPLAY),
        id: String::new(),
        icon: unsafe { icons::icon_of_pidl(root.ptr()) },
        has_children: true,
        is_fs: false,
        is_pinned: false,
    })
}
