//! Read-only Gallery item properties; sources remain owned by the Windows collection.
use super::*;
#[derive(Clone, Copy, Debug, Default)]
pub struct Photo {
    pub date: u64,
    pub width: u32,
    pub height: u32,
}
pub fn photo(item: &windows::Win32::UI::Shell::IShellItem2) -> Photo {
    unsafe {
        let number = |name: &str| {
            super::columns::key(name)
                .and_then(|key| item.GetUInt32(&key).ok())
                .unwrap_or(0)
        };
        let date = item
            .GetFileTime(&windows::Win32::Storage::EnhancedStorage::PKEY_ItemDate)
            .map(|t| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64)
            .unwrap_or(0);
        Photo {
            date: date.saturating_sub(116_444_736_000_000_000) / 10_000,
            width: number("System.Image.HorizontalSize"),
            height: number("System.Image.VerticalSize"),
        }
    }
}
pub fn collection_roots() -> Vec<String> {
    // Gallery's registered FolderScope is FOLDERID_PicturesLibrary, not a
    // recursively enumerated ad-hoc list of user folders.
    super::library::origins("shell:::{031E4825-7B94-4DC3-B131-E946B44C8DD5}\\Pictures.library-ms")
        .into_iter()
        .map(|o| o.path)
        .collect()
}
