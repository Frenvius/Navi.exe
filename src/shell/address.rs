//! The address bar's suggestions, from the same places Explorer's come from: the locations
//! typed into it (TypedPaths, most recent first), the Run dialog's history (RunMRU) and the
//! items of the folder being typed.

use std::os::windows::fs::MetadataExt;

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const TYPED_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\TypedPaths";
const RUN_MRU: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\RunMRU";
/// Explorer keeps this many typed locations.
const TYPED_MAX: usize = 25;

/// The typed locations, most recent first.
pub fn typed_paths() -> Vec<String> {
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(TYPED_PATHS) else {
        return Vec::new();
    };
    (1..=TYPED_MAX)
        .map_while(|i| key.get_value::<String, _>(format!("url{i}")).ok())
        .collect()
}

/// Puts a location typed into the address bar first among the typed ones.
pub fn push_typed_path(text: &str) {
    let mut paths = typed_paths();
    paths.retain(|p| p.to_lowercase() != text.to_lowercase());
    paths.insert(0, text.to_string());
    paths.truncate(TYPED_MAX);
    let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey_with_flags(TYPED_PATHS, KEY_READ | KEY_WRITE)
    else {
        return;
    };
    for (i, p) in paths.iter().enumerate() {
        let _ = key.set_value(format!("url{}", i + 1), p);
    }
    for i in paths.len() + 1..=TYPED_MAX {
        let _ = key.delete_value(format!("url{i}"));
    }
}

/// "Excluir Histórico": forgets the typed locations.
pub fn clear_typed_paths() {
    let Ok(key) =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(TYPED_PATHS, KEY_READ | KEY_WRITE)
    else {
        return;
    };
    for i in 1..=TYPED_MAX {
        let _ = key.delete_value(format!("url{i}"));
    }
}

/// The Run dialog's history, most recent first (its entries end in "\1").
fn run_history() -> Vec<String> {
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_MRU) else {
        return Vec::new();
    };
    let order: String = key.get_value("MRUList").unwrap_or_default();
    order
        .chars()
        .filter_map(|c| key.get_value::<String, _>(c.to_string()).ok())
        .map(|v| v.strip_suffix(r"\1").map(str::to_string).unwrap_or(v))
        .collect()
}

/// The suggestions for the text being typed: with nothing typed, the typed locations as they
/// are; otherwise everything that starts with the text (typed locations, the Run history and
/// the visible items of the folder it names), in case-insensitive order.
pub fn suggestions(text: &str) -> Vec<String> {
    if text.is_empty() {
        return typed_paths();
    }
    let lower = text.to_lowercase();
    let mut out: Vec<String> = typed_paths()
        .into_iter()
        .chain(run_history())
        .filter(|s| s.to_lowercase().starts_with(&lower))
        .collect();
    if let Some(cut) = text.rfind(['\\', '/']) {
        let (dir, prefix) = text.split_at(cut + 1);
        let prefix = prefix.to_lowercase();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                // Hidden and system items are left out, as Explorer leaves them.
                let hidden = entry
                    .metadata()
                    .map(|m| m.file_attributes() & 0x6 != 0)
                    .unwrap_or(true);
                let name = entry.file_name().to_string_lossy().into_owned();
                if !hidden && name.to_lowercase().starts_with(&prefix) {
                    out.push(format!("{dir}{name}"));
                }
            }
        }
    }
    out.sort_by_key(|s| s.to_lowercase());
    out.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    out
}
