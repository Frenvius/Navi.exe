//! Navigation pane preferences shared with Explorer and its Folder Options dialog.
use super::{com_init, wide, Pidl};
use windows::core::GUID;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::IRegTreeItem;
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const LIBRARIES: &str = r"Software\Classes\CLSID\{031E4825-7B94-4dc3-B131-E946B44C8DD5}";
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
    Ok(())
}
