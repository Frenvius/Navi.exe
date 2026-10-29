pub const SETTINGS: &str = concat!(env!("CARGO_PKG_NAME"), ":settings");
pub fn is_settings(path: &str) -> bool {
    path.eq_ignore_ascii_case(SETTINGS)
}

pub const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
pub const HOME: &str = "::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}";
pub const GALLERY: &str = "::{E88865EA-0E1C-4E20-9AA6-EDCD0212C87C}";
pub const NETWORK: &str = "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}";
pub const RECYCLE_BIN: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";
pub const LIBRARIES: &str = "::{031E4825-7B94-4DC3-B131-E946B44C8DD5}";
/// Quick access pins only resolve through the shell: moniker.
pub const QUICK_ACCESS: &str = "shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}";

pub fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

pub fn canonical(id: &str) -> String {
    if is_settings(id) {
        return SETTINGS.into();
    }
    match id.to_ascii_lowercase().as_str() {
        "shell:mycomputerfolder" | "shell:::{20d04fe0-3aea-1069-a2d8-08002b30309d}" => {
            THIS_PC.to_string()
        }
        "shell:::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}" => HOME.to_string(),
        "shell:gallery" | "shell:::{e88865ea-0e1c-4e20-9aa6-edcd0212c87c}" => GALLERY.to_string(),
        "shell:recyclebinfolder" | "shell:::{645ff040-5081-101b-9f08-00aa002f954e}" => {
            RECYCLE_BIN.to_string()
        }
        "shell:libraries" | "shell:::{031e4825-7b94-4dc3-b131-e946b44c8dd5}" => {
            LIBRARIES.to_string()
        }
        "shell:networkplacesfolder" | "shell:::{f02c1a0d-be21-4350-88b0-7367fc96ef3c}" => {
            NETWORK.to_string()
        }
        _ if crate::shell::browser::handles(id) => crate::shell::Pidl::parse(id)
            .map(|p| p.name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING))
            .ok()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.to_string()),
        _ => id.to_string(),
    }
}

/// Virtual folders have no file system path (This PC, Home, Network, search results...).
pub fn is_virtual(id: &str) -> bool {
    is_settings(id)
        || id.starts_with("::")
        || id.starts_with("shell:")
        || is_search(id)
        || crate::shell::archive::root(id).is_some()
        || crate::shell::network::is_computer(id)
}

/// Where a new window or tab opens: Folder Options' "Abrir o Explorador de Arquivos em"
/// (Advanced\LaunchTo: 1 This PC, 3 Downloads, else Home).
pub fn start_location() -> String {
    use winreg::enums::HKEY_CURRENT_USER;
    let launch: u32 = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced")
        .and_then(|k| k.get_value("LaunchTo"))
        .unwrap_or(2);
    match launch {
        1 => THIS_PC.to_string(),
        3 => "shell:Downloads".to_string(),
        _ => HOME.to_string(),
    }
}

/// Search results are a history location of their own: `search|<scope>|<query>` (paths
/// cannot contain '|').
const SEARCH: &str = "search|";

pub fn search_id(scope: &str, query: &str) -> String {
    format!("{SEARCH}{scope}|{query}")
}

pub fn is_search(id: &str) -> bool {
    id.starts_with(SEARCH)
}

pub fn search_parts(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(SEARCH)?.split_once('|')
}

pub fn is_drive_root(id: &str) -> bool {
    let b = id.as_bytes();
    (b.len() == 2 || b.len() == 3)
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || b[2] == b'\\' || b[2] == b'/')
}

/// Library namespace items parse to the library definition, not its save directory.
pub fn is_library(id: &str) -> bool {
    id.to_ascii_lowercase().ends_with(".library-ms")
}

/// Address bar root glyph: 1 desktop, 2 home, 3 gallery, 4 network, 5 this pc, 0 shell icon.
pub fn root_kind(id: &str) -> i32 {
    if is_settings(id) {
        6
    } else if id.is_empty() {
        1
    } else if same(id, HOME) {
        2
    } else if same(id, GALLERY) {
        3
    } else if same(id, NETWORK) {
        4
    } else if same(id, THIS_PC) || same(id, LIBRARIES) {
        5
    } else {
        0
    }
}

/// Locations without a chevron (they have no navigable children in the address bar).
pub fn crumb_has_children(id: &str) -> bool {
    !is_settings(id) && !same(id, HOME) && !same(id, GALLERY)
}

pub fn parent_of(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    match trimmed.rfind('\\') {
        None => String::new(),
        Some(cut) => {
            let parent = &trimmed[..cut];
            if parent.len() == 2 && parent.ends_with(':') {
                format!("{parent}\\")
            } else {
                parent.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_ids_ignore_case_and_canonicalize() {
        assert!(is_settings("Navi:Settings"));
        assert_eq!(canonical("Navi:Settings"), SETTINGS);
        assert_eq!(canonical(SETTINGS), SETTINGS);
        assert!(!is_settings("other:settings"));
    }
}
