use windows::core::{GUID, PCWSTR};
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
    pub group_id: Option<u32>,
    pub namespace_clsid: Option<GUID>,
}

impl NavNode {
    pub fn namespace_id(&self) -> Option<GUID> {
        self.namespace_clsid
            .filter(|id| *id != GUID::zeroed())
            .or_else(|| parsing_namespace(&self.id))
    }
}

fn parsing_namespace(id: &str) -> Option<GUID> {
    let id = id.strip_prefix("shell:").unwrap_or(id);
    GUID::try_from(id.strip_prefix("::{")?.strip_suffix('}')?).ok()
}

const KEEP_CLSIDS: [GUID; 5] = [
    GUID::from_u128(0x20d04fe0_3aea_1069_a2d8_08002b30309d),
    GUID::from_u128(0xf02c1a0d_be21_4350_88b0_7367fc96ef3c),
    GUID::from_u128(0xf874310e_b6b7_47dc_bc84_b9e6b38f5903),
    GUID::from_u128(0x679f85cb_0220_4080_b29b_5540cc05aab6),
    GUID::from_u128(0xe88865ea_0e1c_4e20_9aa6_edcd0212c87c),
];

fn property_key(name: &str) -> Option<PROPERTYKEY> {
    let name = wide(name);
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
        let home_key = if at_root {
            None
        } else {
            property_key("System.Home.IsPinned")
        };
        let group_key = property_key("System.Namespace.GroupId");
        let namespace_key = property_key("System.NamespaceCLSID");

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
            let abs = ILCombine(Some(parent.ptr()), Some(crel));
            let node_id = name_of(abs, SIGDN_DESKTOPABSOLUTEPARSING);
            let item = SHCreateItemFromIDList::<IShellItem2>(abs).ok();
            let get_bool = |key: &PROPERTYKEY| {
                item.as_ref()
                    .and_then(|it| it.GetBool(key).ok())
                    .map(|b| b.as_bool())
                    .unwrap_or(false)
            };
            let namespace_clsid = namespace_key
                .as_ref()
                .and_then(|key| item.as_ref()?.GetCLSID(key).ok())
                .filter(|id| *id != GUID::zeroed());

            let keep;
            let is_pinned = if at_root {
                let clsid = namespace_clsid.or_else(|| parsing_namespace(&node_id));
                let pinned = get_bool(&PKEY_IsPinnedToNameSpaceTree);
                keep = if clsid == Some(KEEP_CLSIDS[0]) {
                    options.this_pc
                } else if clsid == Some(KEEP_CLSIDS[1]) {
                    options.network
                } else if clsid == Some(GUID::from_u128(0x031e4825_7b94_4dc3_b131_e946b44c8dd5)) {
                    options.all || options.libraries
                } else {
                    options.all || pinned || clsid.is_some_and(|id| KEEP_CLSIDS.contains(&id))
                };
                pinned
            } else {
                keep = true;
                home_key.as_ref().map(get_bool).unwrap_or(false)
            };

            if keep {
                let name = display_name(&folder, crel, SHGDN_NORMAL);
                let mut attrs = (SFGAO_HASSUBFOLDER | SFGAO_FILESYSTEM).0;
                let _ = folder.GetAttributesOf(&[crel as *const _], &mut attrs);
                out.push(NavNode {
                    name,
                    id: node_id,
                    icon: icons::icon_of_pidl(abs),
                    has_children: attrs & SFGAO_HASSUBFOLDER.0 != 0,
                    is_fs: attrs & SFGAO_FILESYSTEM.0 != 0,
                    is_pinned,
                    group_id: group_key
                        .as_ref()
                        .and_then(|key| item.as_ref()?.GetUInt32(key).ok()),
                    namespace_clsid,
                });
            }
            ILFree(Some(abs));
            CoTaskMemFree(Some(crel as *const _));
        }
        Ok(out)
    }
}

pub fn set_pinned(id: &str, pinned: bool) -> Result<(), String> {
    com_init();
    let pidl = Pidl::parse(id)?;
    set_pinned_pidl(&pidl, pinned)
}

pub fn set_pinned_pidl(pidl: &Pidl, pinned: bool) -> Result<(), String> {
    if !pinned {
        return super::quick_access::unpin(pidl);
    }
    let verb = wide("pintohome");
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

pub fn desktop() -> Option<NavNode> {
    let root = Pidl::parse("").ok()?;
    Some(NavNode {
        name: root.name(windows::Win32::UI::Shell::SIGDN_NORMALDISPLAY),
        id: String::new(),
        icon: unsafe { icons::icon_of_pidl(root.ptr()) },
        has_children: true,
        is_fs: false,
        is_pinned: false,
        group_id: None,
        namespace_clsid: None,
    })
}
