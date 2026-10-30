use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantToDouble, PropVariantToFileTime, PropVariantToStringAlloc,
};
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyDescription, PSGetPropertyDescription, PDFF_DEFAULT, PDVF_RIGHTALIGN,
    PDVF_SORTDESCENDING,
};
use windows::Win32::UI::Shell::{
    IShellFolder, IShellItem2, SHCreateItemFromParsingName, SHCreateItemWithParent,
    SIGDN_FILESYSPATH,
};

use super::{com_init, take_pwstr, wide, Pidl};

/// A column's header text, whether its values are right-aligned, its default width (pixels
/// at 100%: the description's width in characters, 8px each), and whether its first sort is
/// descending (dates and sizes).
#[derive(Clone, Debug)]
pub struct Column {
    pub key: PROPERTYKEY,
    pub label: String,
    pub right: bool,
    pub width: f32,
    pub descending: bool,
}

pub fn description(key: &PROPERTYKEY) -> Option<IPropertyDescription> {
    use windows::core::Interface;
    let mut desc: Option<IPropertyDescription> = None;
    unsafe {
        PSGetPropertyDescription(
            key,
            &IPropertyDescription::IID,
            &mut desc as *mut _ as *mut _,
        )
        .ok()?
    };
    desc
}

pub fn describe(key: &PROPERTYKEY) -> Option<Column> {
    com_init();
    unsafe {
        let desc = description(key)?;
        let label = desc
            .GetDisplayName()
            .map(|p| take_pwstr(p))
            .unwrap_or_default();
        if label.is_empty() {
            return None;
        }
        let flags = desc.GetViewFlags().map(|f| f.0).unwrap_or(0);
        let right = flags & PDVF_RIGHTALIGN.0 != 0;
        let descending = flags & PDVF_SORTDESCENDING.0 != 0;
        let chars = desc.GetDefaultColumnWidth().unwrap_or(20);
        Some(Column {
            key: *key,
            label,
            right,
            width: chars as f32 * 8.0,
            descending,
        })
    }
}

/// Makes a folder's items for reading their values. Outside the file system the folder makes
/// them itself: the Recycle Bin's Local Original and Data de Exclusão only come through it,
/// not through the deleted file's own path.
pub struct Items {
    folder: Option<IShellFolder>,
    /// A file system folder bound once, so its items are found by name: parsing each full
    /// path walks the namespace from the desktop, milliseconds per item under a sync root.
    directory: Option<(String, IShellFolder)>,
    /// The items' IDs in the folder, by path (from the listing): Network's only give their
    /// values through them.
    children: std::collections::HashMap<String, Vec<u8>>,
    native_details: bool,
}

impl Items {
    pub fn new(folder: &str, children: std::collections::HashMap<String, Vec<u8>>) -> Items {
        com_init();
        let native_details = super::archive::root(folder).is_some();
        let parsed = Pidl::parse(folder).ok();
        let is_virtual = parsed.as_ref().is_some_and(|p| {
            p.name(SIGDN_FILESYSPATH).is_empty()
                || super::archive::root(
                    &p.name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING),
                )
                .is_some()
        });
        let bound = parsed.and_then(|p| p.folder().ok());
        let (folder, directory) = if is_virtual {
            (bound, None)
        } else {
            (None, bound.map(|bound| (folder.to_owned(), bound)))
        };
        Items {
            folder,
            directory,
            children,
            native_details,
        }
    }

    fn item(&self, path: &str) -> Option<IShellItem2> {
        unsafe {
            if let (Some(folder), Some(child)) = (&self.folder, self.children.get(path)) {
                if let Ok(item) = SHCreateItemWithParent(None, folder, child.as_ptr() as *const _) {
                    return Some(item);
                }
            }
            if let Some(item) = self.folder.as_ref().and_then(|folder| child_item(folder, path)) {
                return Some(item);
            }
            if let Some(item) = self.directory_item(path) {
                return Some(item);
            }
            let w = wide(path);
            SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(w.as_ptr()), None).ok()
        }
    }

    fn directory_item(&self, path: &str) -> Option<IShellItem2> {
        let (directory, folder) = self.directory.as_ref()?;
        let file = std::path::Path::new(path);
        if !file.parent()?.to_str()?.eq_ignore_ascii_case(directory) {
            return None;
        }
        child_item(folder, file.file_name()?.to_str()?)
    }
}

/// An item of a bound folder by the name it parses (a full path for a virtual folder, the
/// file name for a file system one).
fn child_item(folder: &IShellFolder, name: &str) -> Option<IShellItem2> {
    let w = wide(name);
    let mut child = std::ptr::null_mut();
    unsafe {
        folder
            .ParseDisplayName(
                HWND::default(),
                None,
                PCWSTR(w.as_ptr()),
                None,
                &mut child,
                std::ptr::null_mut(),
            )
            .ok()?;
        let item = SHCreateItemWithParent(None, folder, child).ok();
        CoTaskMemFree(Some(child as *const _));
        item
    }
}

pub fn texts_with_id(
    items: &Items,
    path: &str,
    keys: &[PROPERTYKEY],
    absolute: &[u8],
) -> Vec<String> {
    unsafe {
        let retained: Option<IShellItem2> = if absolute.is_empty() {
            None
        } else {
            windows::Win32::UI::Shell::SHCreateItemFromIDList(absolute.as_ptr().cast()).ok()
        };
        let Some(item) = retained.or_else(|| items.item(path)) else {
            return vec![String::new(); keys.len()];
        };
        keys.iter()
            .map(|key| {
                if items.native_details && canonical(key) != "System.ZipFolder.CompressedSize" {
                    use windows::core::Interface;
                    if let (Some(folder), Some(child)) = (&items.folder, items.children.get(path)) {
                        if let Ok(folder) =
                            folder.cast::<windows::Win32::UI::Shell::IShellFolder2>()
                        {
                            for column in 0..64 {
                                let mut mapped = PROPERTYKEY::default();
                                if folder.MapColumnToSCID(column, &mut mapped).is_err() {
                                    break;
                                }
                                if mapped == *key {
                                    let mut details =
                                        windows::Win32::UI::Shell::Common::SHELLDETAILS::default();
                                    if folder
                                        .GetDetailsOf(child.as_ptr().cast(), column, &mut details)
                                        .is_ok()
                                    {
                                        let mut value = windows::core::PWSTR::null();
                                        let mut text = details.str;
                                        if windows::Win32::UI::Shell::StrRetToStrW(
                                            &mut text,
                                            Some(child.as_ptr().cast()),
                                            &mut value,
                                        )
                                        .is_ok()
                                        {
                                            return take_pwstr(value);
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
                let Some(desc) = description(key) else {
                    return String::new();
                };
                item.GetProperty(key)
                    .ok()
                    .filter(|v| !v.is_empty())
                    .and_then(|v| {
                        desc.FormatForDisplay(
                            &v,
                            if items.native_details
                                && canonical(key) == "System.ZipFolder.CompressedSize"
                            {
                                windows::Win32::UI::Shell::PropertiesSystem::PDFF_ALWAYSKB
                            } else {
                                PDFF_DEFAULT
                            },
                        )
                        .ok()
                    })
                    .map(|p| take_pwstr(p))
                    .unwrap_or_default()
            })
            .collect()
    }
}

fn canonical(key: &PROPERTYKEY) -> String {
    unsafe {
        windows::Win32::UI::Shell::PropertiesSystem::PSGetNameFromPropertyKey(key)
            .map(|p| take_pwstr(p))
            .unwrap_or_default()
    }
}

/// An item's value of a property for sorting: its text, and its number when the value is
/// numeric or a date (compared as such; a date's is in FILETIME ticks).
#[derive(Clone, Debug, Default)]
pub struct SortValue {
    pub text: String,
    pub number: Option<f64>,
    pub date: bool,
}

impl SortValue {
    pub fn is_empty(&self) -> bool {
        self.number.is_none() && self.text.is_empty()
    }
}

pub fn sort_value(items: &Items, path: &str, key: &PROPERTYKEY) -> SortValue {
    unsafe {
        let Some(item) = items.item(path) else {
            return SortValue::default();
        };
        let Ok(value) = item.GetProperty(key) else {
            return SortValue::default();
        };
        if value.is_empty() {
            return SortValue::default();
        }
        let date = value.vt() == VT_FILETIME;
        let number = match value.vt() {
            VT_FILETIME => PropVariantToFileTime(&value, PSTF_UTC)
                .ok()
                .map(|ft| (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) as f64),
            VT_I1 | VT_UI1 | VT_I2 | VT_UI2 | VT_I4 | VT_UI4 | VT_I8 | VT_UI8 | VT_INT
            | VT_UINT | VT_R4 | VT_R8 | VT_BOOL | VT_DATE | VT_DECIMAL | VT_CY => {
                PropVariantToDouble(&value).ok()
            }
            _ => None,
        };
        let text = PropVariantToStringAlloc(&value)
            .map(|p| take_pwstr(p))
            .unwrap_or_default();
        SortValue { text, number, date }
    }
}

/// The widest of these texts in the list's font (Segoe UI 12px, at 100%), as GDI's
/// DrawText measures them.
pub fn widest<'a>(texts: impl IntoIterator<Item = &'a str>) -> f32 {
    widths(texts).into_iter().fold(0.0, f32::max)
}

/// Each of these texts' width in the list's font (Segoe UI 12px, at 100%), as GDI's
/// DrawText measures it.
pub fn widths<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<f32> {
    widths_at(texts, 12)
}

/// The same in Segoe UI at `px` pixels (15 for content view names).
pub fn widths_at<'a>(texts: impl IntoIterator<Item = &'a str>, px: i32) -> Vec<f32> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateFontW, DeleteDC, DeleteObject, DrawTextW, SelectObject,
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT,
        DT_NOPREFIX, DT_SINGLELINE, FW_NORMAL, OUT_DEFAULT_PRECIS,
    };
    let face = wide("Segoe UI");
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontW(
            -px,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            PCWSTR(face.as_ptr()),
        );
        let old = SelectObject(dc, font.into());
        let widths = texts
            .into_iter()
            .map(|text| {
                if text.is_empty() {
                    return 0.0;
                }
                let mut w: Vec<u16> = text.encode_utf16().collect();
                let mut rect = RECT::default();
                DrawTextW(
                    dc,
                    &mut w,
                    &mut rect,
                    DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
                );
                (rect.right - rect.left) as f32
            })
            .collect();
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);
        widths
    }
}

/// A property's key by its canonical name ("System.Recycle.DeletedFrom").
pub fn key(name: &str) -> Option<PROPERTYKEY> {
    use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
    let w = wide(name);
    let mut k = PROPERTYKEY::default();
    unsafe { PSGetPropertyKeyFromName(PCWSTR(w.as_ptr()), &mut k) }
        .ok()
        .map(|_| k)
}
