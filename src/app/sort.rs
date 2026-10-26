//! Explorer's ordering: folders first, then the shell's logical name comparison
//! (case-insensitive, digits by value, word sort ignoring hyphens and apostrophes).

use std::cmp::Ordering;
use std::collections::HashMap;

use windows::core::PCWSTR;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{PKEY_Author, PKEY_Keywords, PKEY_Title};
use windows::Win32::UI::Shell::StrCmpLogicalW;

use crate::shell::columns::SortValue;
use crate::shell::listing::ShellEntry;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SortKey {
    Name,
    Modified,
    Type,
    Size,
    Created,
    Authors,
    Tags,
    Title,
    /// Any other column, by its property's values.
    Prop(PROPERTYKEY),
}

impl SortKey {
    /// The key for a shell property (a saved view's sort or grouping column, a header's).
    pub fn from_property(key: &PROPERTYKEY) -> SortKey {
        use windows::Win32::Storage::EnhancedStorage::{PKEY_DateCreated, PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_ItemTypeText, PKEY_Size};
        [
            (PKEY_ItemNameDisplay, SortKey::Name),
            (PKEY_DateModified, SortKey::Modified),
            (PKEY_ItemTypeText, SortKey::Type),
            (PKEY_Size, SortKey::Size),
            (PKEY_DateCreated, SortKey::Created),
            (PKEY_Author, SortKey::Authors),
            (PKEY_Keywords, SortKey::Tags),
            (PKEY_Title, SortKey::Title),
        ]
        .into_iter()
        .find(|(k, _)| k == key)
        .map_or(SortKey::Prop(*key), |(_, s)| s)
    }

    /// Keys whose values come from each file's property handler, read on demand.
    pub fn property(self) -> Option<PROPERTYKEY> {
        match self {
            SortKey::Authors => Some(PKEY_Author),
            SortKey::Tags => Some(PKEY_Keywords),
            SortKey::Title => Some(PKEY_Title),
            SortKey::Prop(key) => Some(key),
            _ => None,
        }
    }

    /// The key's shell property.
    pub fn key(self) -> PROPERTYKEY {
        use windows::Win32::Storage::EnhancedStorage::{PKEY_DateCreated, PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_ItemTypeText, PKEY_Size};
        match self {
            SortKey::Name => PKEY_ItemNameDisplay,
            SortKey::Modified => PKEY_DateModified,
            SortKey::Type => PKEY_ItemTypeText,
            SortKey::Size => PKEY_Size,
            SortKey::Created => PKEY_DateCreated,
            other => other.property().unwrap_or(PKEY_ItemNameDisplay),
        }
    }

    /// The direction of a column's first click: descending for dates and sizes, as the
    /// property says.
    pub fn descending_first(self) -> bool {
        crate::shell::columns::describe(&self.key()).is_some_and(|c| c.descending)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

/// UTF-16, NUL-terminated copy for the comparison API.
pub fn wide_key(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn compare_wide(a: &[u16], b: &[u16]) -> Ordering {
    let r = unsafe { StrCmpLogicalW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr())) };
    r.cmp(&0)
}

pub fn compare_names(a: &str, b: &str) -> Ordering {
    compare_wide(&wide_key(a), &wide_key(b))
}

/// Returns the entry indices in display order; `values` holds the property values of the
/// on-demand keys by path. File system folders put their folders first; others, like the
/// Recycle Bin, sort them among the files.
pub fn order(entries: &[ShellEntry], indices: &mut [usize], sort: Sort, secondary: &[Sort], values: &HashMap<String, SortValue>, folders_first: bool, path_ties:bool) {
    let names: Vec<Vec<u16>> = entries.iter().map(|e| wide_key(&e.name)).collect();
    let paths=if path_ties {entries.iter().map(|e|wide_key(&e.path)).collect::<Vec<_>>()}else{Vec::new()};
    let none = SortValue::default();
    let props: Vec<&SortValue> =
        if sort.key.property().is_some() { entries.iter().map(|e| values.get(&e.path).unwrap_or(&none)).collect() } else { Vec::new() };
    let texts: Vec<Vec<u16>> = if sort.key == SortKey::Type {
        entries.iter().map(|e| wide_key(&e.type_name)).collect()
    } else {
        props.iter().map(|v| wide_key(&v.text)).collect()
    };
    indices.sort_by(|&ia, &ib| {
        let (a, b) = (&entries[ia], &entries[ib]);
        let by_name = || compare_wide(&names[ia], &names[ib]);
        let ord = if folders_first && a.is_dir != b.is_dir {
            if a.is_dir { Ordering::Less } else { Ordering::Greater }
        } else {
            match sort.key {
                SortKey::Name => by_name(),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Modified => a.modified.cmp(&b.modified),
                SortKey::Created => a.created.cmp(&b.created),
                SortKey::Type => compare_wide(&texts[ia], &texts[ib]),
                // Items without a value come after the ones with one; numbers and dates
                // compare as such.
                SortKey::Authors | SortKey::Tags | SortKey::Title | SortKey::Prop(_) => {
                    let (va, vb) = (props[ia], props[ib]);
                    va.is_empty().cmp(&vb.is_empty()).then_with(|| match (va.number, vb.number) {
                        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                        _ => compare_wide(&texts[ia], &texts[ib]),
                    })
                }
            }
        };
        // Descending turns the key (and folders first) around; ties still go by name A to Z.
        let ord = if sort.descending { ord.reverse() } else { ord };
        ord.then_with(||{
            for extra in secondary {
                let order=match extra.key {
                    SortKey::Name=>by_name(),SortKey::Modified=>a.modified.cmp(&b.modified),
                    SortKey::Created=>a.created.cmp(&b.created),SortKey::Size=>a.size.cmp(&b.size),
                    SortKey::Type=>compare_names(&a.type_name,&b.type_name),_=>Ordering::Equal,
                };
                if order!=Ordering::Equal {return if extra.descending {order.reverse()}else{order};}
            }
            Ordering::Equal
        }).then_with(by_name).then_with(||if path_ties {compare_wide(&paths[ia],&paths[ib])}else{Ordering::Equal})
    });
}
