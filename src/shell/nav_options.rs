use super::{com_init, wide, Pidl};
use windows::core::GUID;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::IRegTreeItem;
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const LIBRARIES: &str = r"Software\Classes\CLSID\{031E4825-7B94-4dc3-B131-E946B44C8DD5}";

fn pane_store(
    bytes: &[u8],
) -> windows::core::Result<windows::Win32::UI::Shell::PropertiesSystem::INamedPropertyStore> {
    use windows::core::Interface;
    use windows::Win32::UI::Shell::PropertiesSystem::{
        INamedPropertyStore, IPersistSerializedPropStorage, PSCreateMemoryPropertyStore,
        PCUSERIALIZEDPROPSTORAGE,
    };
    com_init();
    unsafe {
        let mut raw = std::ptr::null_mut();
        PSCreateMemoryPropertyStore(&INamedPropertyStore::IID, &mut raw)?;
        let store = INamedPropertyStore::from_raw(raw);
        if !bytes.is_empty() {
            if bytes.len() < 8 {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            let persist: IPersistSerializedPropStorage = store.cast()?;
            persist.SetPropertyStorage(
                PCUSERIALIZEDPROPSTORAGE(bytes[4..].as_ptr() as isize),
                (bytes.len() - 4) as u32,
            )?;
        }
        Ok(store)
    }
}

pub fn pane_visible() -> bool {
    let raw = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Modules\GlobalSettings\ProperTreeModuleInner").and_then(|key| key.get_raw_value("ProperTreeModuleInner"));
    let Ok(raw) = raw else { return true };
    let result = pane_store(&raw.bytes).and_then(|store| unsafe {
        store.GetNamedValue(windows::core::w!("NavPane_ShowLibraryPane"))
    });
    match result {
        Ok(value) => value.to_string() != "0",
        Err(error) => {
            eprintln!("navigation pane visibility read: {error}");
            true
        }
    }
}

pub fn set_pane_visible(visible: bool) -> Result<(), String> {
    use windows::core::Interface;
    use windows::Win32::UI::Shell::PropertiesSystem::IPersistSerializedPropStorage;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Modules\GlobalSettings\ProperTreeModuleInner").map_err(|error| error.to_string())?;
    let original = match key.get_raw_value("ProperTreeModuleInner") {
        Ok(value) => value.bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    let result = (|| -> windows::core::Result<Vec<u8>> {
        let store = pane_store(&original)?;
        unsafe {
            store.SetNamedValue(
                windows::core::w!("NavPane_ShowLibraryPane"),
                &windows::Win32::System::Com::StructuredStorage::PROPVARIANT::from(visible),
            )?;
            let persist: IPersistSerializedPropStorage = store.cast()?;
            let mut data = std::ptr::null_mut();
            let mut size = 0;
            persist.GetPropertyStorage(&mut data, &mut size)?;
            if data.is_null() || size < 4 {
                windows::Win32::System::Com::CoTaskMemFree(Some(data.cast()));
                return Err(windows::Win32::Foundation::E_FAIL.into());
            }
            let mut bytes = (size + 4).to_le_bytes().to_vec();
            bytes.extend_from_slice(std::slice::from_raw_parts(data.cast::<u8>(), size as usize));
            windows::Win32::System::Com::CoTaskMemFree(Some(data.cast()));
            Ok(bytes)
        }
    })()
    .map_err(|error| error.to_string())?;
    key.set_raw_value(
        "ProperTreeModuleInner",
        &winreg::RegValue {
            vtype: winreg::enums::REG_BINARY,
            bytes: result,
        },
    )
    .map_err(|error| error.to_string())?;
    notify();
    Ok(())
}

pub fn pane_width() -> f32 {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Modules\GlobalSettings\Sizer",
        )
        .and_then(|key| key.get_raw_value("PageSpaceControlSizer"))
        .ok()
        .and_then(|value| {
            value
                .bytes
                .get(..4)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u32::from_le_bytes)
        })
        .map(|width| (width as f32 - 6.0).max(54.0))
        .unwrap_or(196.0)
}

pub fn set_pane_width(width: f32) -> Result<(), String> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Modules\GlobalSettings\Sizer",
        )
        .map_err(|error| error.to_string())?;
    let mut value = key
        .get_raw_value("PageSpaceControlSizer")
        .map_err(|error| error.to_string())?;
    if value.bytes.len() != 16 || value.vtype != winreg::enums::REG_BINARY {
        return Err("Unexpected navigation pane width storage".into());
    }
    value.bytes[..4].copy_from_slice(&((width + 6.0).round() as u32).to_le_bytes());
    key.set_raw_value("PageSpaceControlSizer", &value)
        .map_err(|error| error.to_string())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Option {
    ThisPc,
    Network,
    Libraries,
    AllFolders,
    Expand,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub this_pc: bool,
    pub network: bool,
    pub libraries: bool,
    pub all: bool,
    pub expand: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            this_pc: true,
            network: true,
            libraries: false,
            all: false,
            expand: false,
        }
    }
}
fn handler(option: Option) -> windows::core::Result<IRegTreeItem> {
    let clsid = match option {
        Option::ThisPc => GUID::from_u128(0xecf2dcaa_e718_4e0c_b5eb_3be48efc4d5a),
        _ => GUID::from_u128(0x726bd46b_a435_43e0_a054_1a11ad3af044),
    };
    com_init();
    unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }
}
pub fn read() -> Options {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let get = |path: &str, name: &str| {
        user.open_subkey(path)
            .and_then(|k| k.get_value::<u32, _>(name))
            .unwrap_or(0)
            != 0
    };
    let custom = |option| {
        handler(option)
            .and_then(|h| unsafe { h.GetCheckState() })
            .map(|b| b.as_bool())
            .unwrap_or(true)
    };
    Options {
        this_pc: custom(Option::ThisPc),
        network: custom(Option::Network),
        libraries: get(LIBRARIES, "System.IsPinnedToNameSpaceTree"),
        all: get(ADVANCED, "NavPaneShowAllFolders"),
        expand: get(ADVANCED, "NavPaneExpandToCurrentFolder"),
    }
}
pub fn set(option: Option, on: bool) -> Result<(), String> {
    match option {
        Option::ThisPc | Option::Network => unsafe {
            handler(option)
                .and_then(|h| h.SetCheckState(on))
                .map_err(|e| e.to_string())?
        },
        _ => {
            let (path, name) = match option {
                Option::Libraries => (LIBRARIES, "System.IsPinnedToNameSpaceTree"),
                Option::AllFolders => (ADVANCED, "NavPaneShowAllFolders"),
                _ => (ADVANCED, "NavPaneExpandToCurrentFolder"),
            };
            RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(path)
                .and_then(|(k, _)| k.set_value(name, &(on as u32)))
                .map_err(|e| e.to_string())?;
        }
    }
    notify();
    Ok(())
}

fn notify() {
    unsafe {
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::{
                Shell::{SHChangeNotify, SHCNE_UPDATEITEM, SHCNF_IDLIST},
                WindowsAndMessaging::*,
            },
        };
        if let Ok(root) = Pidl::parse("") {
            SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_IDLIST, Some(root.0 as _), None);
        }
        let message = wide("ShellState");
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(message.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            1000,
            None,
        );
    }
}
