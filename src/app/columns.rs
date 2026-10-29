use std::cell::RefCell;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Author, PKEY_DateCreated, PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_ItemTypeText,
    PKEY_Keywords, PKEY_Size, PKEY_Title,
};

use super::format;
use crate::shell::columns::{self, Column};
use crate::shell::listing::ShellEntry;

/// The generic folder type's list, for types that have none.
const GENERIC: [(PROPERTYKEY, bool); 8] = [
    (PKEY_ItemNameDisplay, true),
    (PKEY_DateModified, true),
    (PKEY_ItemTypeText, true),
    (PKEY_Size, true),
    (PKEY_DateCreated, false),
    (PKEY_Author, false),
    (PKEY_Keywords, false),
    (PKEY_Title, false),
];

/// Explorer's default widths of the generic type's shown columns.
fn default_width(key: &PROPERTYKEY) -> Option<f32> {
    [
        (PKEY_ItemNameDisplay, 272.0),
        (PKEY_DateModified, 144.0),
        (PKEY_ItemTypeText, 120.0),
        (PKEY_Size, 80.0),
    ]
    .into_iter()
    .find(|(k, _)| k == key)
    .map(|(_, w)| w)
}

thread_local! {
    static DESCRIPTIONS: RefCell<Vec<Column>> = const { RefCell::new(Vec::new()) };
    /// The open folder's own header texts, when it gives them (the Recycle Bin's).
    static FOLDER_LABELS: RefCell<Vec<(PROPERTYKEY, String)>> = const { RefCell::new(Vec::new()) };
    static FOLDER_RIGHT: RefCell<Vec<PROPERTYKEY>> = const { RefCell::new(Vec::new()) };
}

/// The header texts the open folder gives its columns, over the property system's, and the
/// columns it right-aligns.
pub fn set_folder_labels(labels: Vec<(PROPERTYKEY, String)>, right: Vec<PROPERTYKEY>) {
    FOLDER_LABELS.with(|l| *l.borrow_mut() = labels);
    FOLDER_RIGHT.with(|r| *r.borrow_mut() = right);
}

/// A column's description, read once, with the open folder's header text.
fn describe(key: &PROPERTYKEY) -> Option<Column> {
    let own = FOLDER_LABELS.with(|l| {
        l.borrow()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, label)| label.clone())
    });
    let right = FOLDER_RIGHT.with(|r| r.borrow().contains(key));
    let with_own = |mut c: Column| {
        if let Some(label) = own {
            c.label = label;
        }
        c.right |= right;
        c
    };
    if let Some(hit) = DESCRIPTIONS.with(|d| d.borrow().iter().find(|c| c.key == *key).cloned()) {
        return Some(with_own(hit));
    }
    let column = columns::describe(key)?;
    DESCRIPTIONS.with(|d| d.borrow_mut().push(column.clone()));
    Some(with_own(column))
}

/// A column's header text, as the folder or the property system names it.
pub fn label(key: &PROPERTYKEY) -> Option<String> {
    describe(key).map(|c| c.label)
}

/// The values the listing already has (the rest are read from each item).
pub(super) fn known(key: &PROPERTYKEY) -> bool {
    [
        PKEY_ItemNameDisplay,
        PKEY_DateModified,
        PKEY_DateCreated,
        PKEY_ItemTypeText,
        PKEY_Size,
    ]
    .contains(key)
}

/// A shown column: its key, header text, alignment and width.
#[derive(Clone, Debug)]
pub struct Col {
    pub key: PROPERTYKEY,
    pub label: String,
    pub right: bool,
    pub width: f32,
}

pub fn shown_columns(shown: Vec<(PROPERTYKEY, f32)>) -> Vec<Col> {
    let mut cols: Vec<Col> = shown
        .into_iter()
        .filter_map(|(key, width)| {
            describe(&key).map(|c| Col {
                key,
                label: c.label,
                right: c.right,
                width,
            })
        })
        .collect();
    if !cols.iter().any(|c| c.key == PKEY_ItemNameDisplay) {
        if let Some(c) = describe(&PKEY_ItemNameDisplay) {
            cols.insert(
                0,
                Col {
                    key: PKEY_ItemNameDisplay,
                    label: c.label,
                    right: false,
                    width: 272.0,
                },
            );
        }
    }
    cols
}

pub fn cell_text(
    e: &ShellEntry,
    k: &PROPERTYKEY,
    read: Option<&[(PROPERTYKEY, String)]>,
) -> String {
    if *k == PKEY_ItemNameDisplay {
        e.name.clone()
    } else if *k == PKEY_DateModified {
        format::modified(e.modified)
    } else if *k == PKEY_DateCreated {
        format::modified(e.created)
    } else if *k == PKEY_ItemTypeText {
        e.type_name.clone()
    } else if *k == PKEY_Size {
        if e.is_dir || (!e.is_fs && crate::shell::archive::root(&e.path).is_none()) {
            String::new()
        } else {
            format::size_kb(e.size)
        }
    } else {
        read.and_then(|r| r.iter().find(|(key, _)| key == k))
            .map(|(_, t)| t.clone())
            .unwrap_or_default()
    }
}

pub fn needs_property(key: &PROPERTYKEY) -> bool {
    !known(key)
}

/// What the folder's view says about its columns: the shown ones with their widths (empty
/// for its type's defaults) and its type's list.
pub fn initial(
    saved: &[(PROPERTYKEY, u32)],
    type_columns: &[(PROPERTYKEY, bool, Option<u32>)],
) -> (Vec<(PROPERTYKEY, f32)>, Vec<PROPERTYKEY>) {
    let list: Vec<(PROPERTYKEY, bool, Option<f32>)> = if type_columns.is_empty() {
        GENERIC
            .iter()
            .map(|(k, shown)| (*k, *shown, None))
            .collect()
    } else {
        type_columns
            .iter()
            .map(|(k, shown, chars)| (*k, *shown, chars.map(|c| c as f32 * 8.0)))
            .collect()
    };
    let shown = if saved.is_empty() {
        list.iter()
            .filter(|(_, shown, _)| *shown)
            .map(|(k, _, w)| {
                (
                    *k,
                    w.or_else(|| default_width(k))
                        .or_else(|| describe(k).map(|c| c.width))
                        .unwrap_or(120.0),
                )
            })
            .collect()
    } else {
        saved.iter().map(|(k, w)| (*k, *w as f32)).collect()
    };
    (shown, list.into_iter().map(|(k, _, _)| k).collect())
}

pub fn width_for(key: &PROPERTYKEY, requested: Option<f32>) -> f32 {
    requested
        .or_else(|| default_width(key))
        .or_else(|| describe(key).map(|c| c.width))
        .unwrap_or(120.0)
}

pub fn menu_entries(keys: &[PROPERTYKEY], cols: &[Col]) -> Vec<(PROPERTYKEY, String, bool)> {
    let mut keys = keys.to_vec();
    for col in cols {
        if !keys.contains(&col.key) {
            keys.push(col.key);
        }
    }
    keys.iter()
        .filter_map(|key| {
            describe(key).map(|col| (*key, col.label, cols.iter().any(|c| c.key == *key)))
        })
        .collect()
}

pub fn toggled(shown: &[(PROPERTYKEY, f32)], key: PROPERTYKEY) -> Vec<(PROPERTYKEY, f32)> {
    let mut next = shown.to_vec();
    if let Some(i) = next.iter().position(|(k, _)| *k == key) {
        if key != PKEY_ItemNameDisplay {
            next.remove(i);
        }
    } else {
        next.push((key, width_for(&key, None)));
    }
    next
}

pub fn chosen(chosen: Vec<(PROPERTYKEY, Option<u32>)>) -> Vec<(PROPERTYKEY, f32)> {
    chosen
        .into_iter()
        .map(|(key, width)| (key, width_for(&key, width.map(|w| w as f32))))
        .collect()
}

pub enum HeaderCommand {
    None,
    Fit(Option<usize>),
    Choose,
    Toggle(PROPERTYKEY),
}

pub fn show_header_menu(
    hwnd: windows::Win32::Foundation::HWND,
    column: Option<usize>,
    point: windows::Win32::Foundation::POINT,
    entries: &[(PROPERTYKEY, String, bool)],
) -> windows::core::Result<HeaderCommand> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let menu = CreatePopupMenu()?;
        let result: windows::core::Result<HeaderCommand> = (|| {
            let append = |label: &str, id: usize, flags| {
                let label = crate::shell::wide(label);
                AppendMenuW(menu, flags, id, PCWSTR(label.as_ptr()))
            };
            if column.is_some() {
                append(
                    &crate::i18n::system_string("shell32.dll", 37200),
                    1,
                    MF_STRING,
                )?;
            }
            append(
                &crate::i18n::system_string("shell32.dll", 37201),
                2,
                MF_STRING,
            )?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            for (i, (key, label, shown)) in entries.iter().enumerate() {
                let mut flags = MF_STRING;
                if *shown {
                    flags |= MF_CHECKED;
                }
                if *key == PKEY_ItemNameDisplay {
                    flags |= MF_GRAYED;
                }
                append(label, 100 + i, flags)?;
            }
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            append(
                &crate::i18n::system_string("shell32.dll", 4249),
                3,
                MF_STRING,
            )?;
            let id = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                Some(0),
                hwnd,
                None,
            )
            .0 as usize;
            Ok(match id {
                1 => HeaderCommand::Fit(column),
                2 => HeaderCommand::Fit(None),
                3 => HeaderCommand::Choose,
                id if id >= 100 => entries
                    .get(id - 100)
                    .map_or(HeaderCommand::None, |(key, _, _)| {
                        HeaderCommand::Toggle(*key)
                    }),
                _ => HeaderCommand::None,
            })
        })();
        let destroyed = DestroyMenu(menu);
        let command = result?;
        destroyed?;
        Ok(command)
    }
}
