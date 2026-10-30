use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::UI::Shell::{
    IKnownFolderManager, KnownFolderManager, FFFP_EXACTMATCH, SIGDN_NORMALDISPLAY,
};

use super::{com_init, wide, Pidl};

const STORED_LOCALLY: u32 = 51553;
const PERSONAL: u32 = 16957;
const THIS_PC: u32 = 9216;

pub fn location(path: &str) -> String {
    com_init();
    if path.starts_with("::") {
        return storage_text(path);
    }
    let file = Path::new(path);
    if file.parent().is_none() {
        return crate::i18n::system_string("windows.storage.dll", THIS_PC);
    }
    if is_known(path) {
        return storage_text(path);
    }
    let mut tail = Vec::new();
    let mut current = file.parent();
    while let Some(folder) = current {
        let text = folder.to_string_lossy();
        if folder.parent().is_none() || is_known(&text) {
            tail.push(display(&text));
            break;
        }
        tail.push(
            folder
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        current = folder.parent();
    }
    tail.reverse();
    tail.join("\\")
}

pub fn grouping_label(group: i32) -> String {
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::UI::Shell::PropertiesSystem::PDFF_DEFAULT;
    let Some(desc) = super::columns::key("System.Home.Grouping")
        .and_then(|key| super::columns::description(&key))
    else {
        return String::new();
    };
    unsafe {
        desc.FormatForDisplay(&PROPVARIANT::from(group as u32), PDFF_DEFAULT)
            .map(|text| super::take_pwstr(text))
            .unwrap_or_default()
    }
}

fn storage_text(path: &str) -> String {
    let onedrive = onedrive_root();
    if onedrive
        .as_deref()
        .is_some_and(|root| super::cloud::contains(root, path))
    {
        let personal = crate::i18n::system_string("windows.storage.dll", PERSONAL);
        return match first_name() {
            Some(name) => format!("{name} \u{2013} {personal}"),
            None => personal,
        };
    }
    crate::i18n::system_string("windows.storage.dll", STORED_LOCALLY)
}

fn is_known(path: &str) -> bool {
    let path = wide(path);
    unsafe {
        windows::Win32::System::Com::CoCreateInstance::<_, IKnownFolderManager>(
            &KnownFolderManager,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
        .and_then(|manager| manager.FindFolderFromPath(PCWSTR(path.as_ptr()), FFFP_EXACTMATCH))
        .is_ok()
    }
}

fn display(path: &str) -> String {
    Pidl::parse(path)
        .map(|pidl| pidl.name(SIGDN_NORMALDISPLAY))
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_owned())
}

fn onedrive_root() -> Option<String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\OneDrive\Accounts\Personal")
        .ok()?
        .get_value::<String, _>("UserFolder")
        .ok()
        .filter(|root| !root.is_empty())
}

fn first_name() -> Option<String> {
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
    let mut buffer = [0u16; 256];
    let mut length = buffer.len() as u32;
    let name = unsafe {
        GetUserNameExW(
            NameDisplay,
            Some(windows::core::PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
    };
    if !name {
        return None;
    }
    String::from_utf16_lossy(&buffer[..length as usize])
        .split_whitespace()
        .next()
        .map(str::to_owned)
}
