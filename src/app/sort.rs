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
        use windows::Win32::Storage::EnhancedStorage::{
            PKEY_DateCreated, PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_ItemTypeText, PKEY_Size,
        };
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
        use windows::Win32::Storage::EnhancedStorage::{
            PKEY_DateCreated, PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_ItemTypeText, PKEY_Size,
        };
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

pub fn folders_first(path: &str) -> bool {
    !super::ids::is_virtual(path)
        || super::ids::is_search(path)
        || super::ids::is_library(path)
        || crate::shell::archive::root(path).is_some()
}

pub fn order(
    entries: &[ShellEntry],
    indices: &mut [usize],
    sort: Sort,
    secondary: &[Sort],
    values: &HashMap<String, SortValue>,
    folders_first: bool,
    path_ties: bool,
) {
    let names: Vec<Vec<u16>> = entries.iter().map(|e| wide_key(&e.name)).collect();
    let paths = if path_ties {
        entries
            .iter()
            .map(|e| wide_key(&e.path))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let none = SortValue::default();
    let props: Vec<&SortValue> = if sort.key.property().is_some() {
        entries
            .iter()
            .map(|e| values.get(&e.path).unwrap_or(&none))
            .collect()
    } else {
        Vec::new()
    };
    let texts: Vec<Vec<u16>> = if sort.key == SortKey::Type {
        entries.iter().map(|e| wide_key(&e.type_name)).collect()
    } else {
        props.iter().map(|v| wide_key(&v.text)).collect()
    };
    indices.sort_by(|&ia, &ib| {
        let (a, b) = (&entries[ia], &entries[ib]);
        let by_name = || compare_wide(&names[ia], &names[ib]);
        if folders_first && sort.key == SortKey::Type && a.is_dir != b.is_dir {
            return if a.is_dir {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let ord = if folders_first && a.is_dir != b.is_dir {
            if a.is_dir {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        } else {
            match sort.key {
                SortKey::Name => by_name(),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Modified => compare_time_ms(a.modified, b.modified),
                SortKey::Created => compare_time_ms(a.created, b.created),
                SortKey::Type => compare_wide(&texts[ia], &texts[ib]),
                SortKey::Authors | SortKey::Tags | SortKey::Title | SortKey::Prop(_) => {
                    let (va, vb) = (props[ia], props[ib]);
                    va.is_empty()
                        .cmp(&vb.is_empty())
                        .then_with(|| match (va.number, vb.number) {
                            (Some(x), Some(y)) if va.date && vb.date => (x / 10_000_000.0)
                                .floor()
                                .partial_cmp(&(y / 10_000_000.0).floor())
                                .unwrap_or(Ordering::Equal),
                            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                            _ => compare_wide(&texts[ia], &texts[ib]),
                        })
                }
            }
        };
        let ord = if sort.descending { ord.reverse() } else { ord };
        ord.then_with(|| {
            for extra in secondary {
                let order = match extra.key {
                    SortKey::Name => by_name(),
                    SortKey::Modified => compare_time_ms(a.modified, b.modified),
                    SortKey::Created => compare_time_ms(a.created, b.created),
                    SortKey::Size => a.size.cmp(&b.size),
                    SortKey::Type => compare_names(&a.type_name, &b.type_name),
                    _ => Ordering::Equal,
                };
                if order != Ordering::Equal {
                    return if extra.descending {
                        order.reverse()
                    } else {
                        order
                    };
                }
            }
            Ordering::Equal
        })
        .then_with(|| {
            if folders_first && sort.key == SortKey::Type && sort.descending {
                by_name().reverse()
            } else {
                by_name()
            }
        })
        .then_with(|| {
            if path_ties {
                compare_wide(&paths[ia], &paths[ib])
            } else {
                Ordering::Equal
            }
        })
    });
}

fn compare_time_ms(a: u64, b: u64) -> Ordering {
    (a / 1000).cmp(&(b / 1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn type_sort_keeps_folders_first_and_reverses_name_ties() {
        let entries = [
            ("z-folder", "Folder", true),
            ("a-folder", "Folder", true),
            ("a-zulu", "Zulu", false),
            ("z-zulu", "Zulu", false),
            ("a-alpha", "Alpha", false),
            ("z-alpha", "Alpha", false),
        ]
        .into_iter()
        .map(|(name, type_name, is_dir)| ShellEntry {
            name: name.into(),
            path: name.into(),
            is_dir,
            is_fs: true,
            is_link: false,
            hidden: false,
            size: 0,
            modified: 0,
            created: 0,
            type_name: type_name.into(),
            icon: 0,
            icon_pending: false,
            file_attributes: 0,
            library_origin: None,
            photo: None,
            total: 0,
            free: 0,
            can_move: false,
            can_copy: false,
            can_rename: false,
            can_delete: false,
            child: Vec::new(),
            absolute: Vec::new(),
        })
        .collect::<Vec<_>>();
        let mut indices = (0..entries.len()).collect::<Vec<_>>();
        order(
            &entries,
            &mut indices,
            Sort {
                key: SortKey::Type,
                descending: true,
            },
            &[],
            &HashMap::new(),
            true,
            false,
        );
        assert_eq!(
            indices
                .iter()
                .map(|&i| entries[i].name.as_str())
                .collect::<Vec<_>>(),
            ["z-folder", "a-folder", "z-zulu", "a-zulu", "z-alpha", "a-alpha"]
        );
        order(
            &entries,
            &mut indices,
            Sort {
                key: SortKey::Type,
                descending: false,
            },
            &[],
            &HashMap::new(),
            true,
            false,
        );
        assert_eq!(
            indices
                .iter()
                .map(|&i| entries[i].name.as_str())
                .collect::<Vec<_>>(),
            ["a-folder", "z-folder", "a-alpha", "z-alpha", "a-zulu", "z-zulu"]
        );
    }
    #[test]
    fn date_sort_uses_listing_milliseconds_and_whole_second_name_ties() {
        let root = std::env::temp_dir().join(format!(
            "sw-header-date-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let base = 1_700_000_000_000;
        for (name, modified_ms) in [
            ("alpha.txt", 100_100),
            ("beta.txt", 100_900),
            ("gamma.txt", 101_000),
        ] {
            let file = std::fs::File::create(root.join(name)).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_millis(base + modified_ms),
            ))
            .unwrap();
        }
        let entries = crate::shell::listing::list(&root.to_string_lossy()).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries
            .iter()
            .all(|entry| (base + 100_000..base + 102_000).contains(&entry.modified)));
        let mut order = vec![0, 1, 2];
        super::order(
            &entries,
            &mut order,
            Sort {
                key: SortKey::Modified,
                descending: true,
            },
            &[],
            &HashMap::new(),
            true,
            false,
        );
        let names = order
            .iter()
            .map(|&i| {
                std::path::Path::new(&entries[i].path)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["gamma.txt", "alpha.txt", "beta.txt"]);
        for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
            std::fs::remove_file(root.join(name)).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }
}

pub fn updated_order(
    previous: &[String],
    entries: &[ShellEntry],
    renames: &[(String, String)],
) -> Vec<usize> {
    let mut remaining = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path.to_lowercase(), index))
        .collect::<HashMap<_, _>>();
    let renamed = renames
        .iter()
        .map(|(old, new)| (old.to_lowercase(), new.to_lowercase()))
        .collect::<HashMap<_, _>>();
    let mut order = previous
        .iter()
        .filter_map(|path| {
            let path = path.to_lowercase();
            remaining.remove(renamed.get(&path).unwrap_or(&path))
        })
        .collect::<Vec<_>>();
    order.extend(
        entries.iter().enumerate().filter_map(|(index, entry)| {
            remaining.remove(&entry.path.to_lowercase()).map(|_| index)
        }),
    );
    order
}

#[cfg(test)]
mod notification_tests {
    #[test]
    fn external_create_appends_and_rename_retains_its_slot_until_resort() {
        let root =
            std::env::temp_dir().join(format!("sw-native-live-order-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let paths = ["a.txt", "b.txt", "c.txt"].map(|name| root.join(name));
        for path in &paths {
            std::fs::write(path, b"dummy").unwrap();
        }
        let previous = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let renamed = root.join("z.txt");
        let added = root.join("aa.txt");
        std::fs::rename(&paths[1], &renamed).unwrap();
        std::fs::remove_file(&paths[0]).unwrap();
        std::fs::write(&added, b"new").unwrap();
        let entries = [&paths[2], &renamed, &added].map(|path| {
            crate::shell::listing::read_file(path.to_str().unwrap())
                .unwrap()
                .unwrap()
        });
        let order = super::updated_order(
            &previous,
            &entries,
            &[(previous[1].clone(), renamed.to_string_lossy().into_owned())],
        );
        assert_eq!(
            order
                .iter()
                .map(|&index| entries[index].name.as_str())
                .collect::<Vec<_>>(),
            vec!["z.txt", "c.txt", "aa.txt"]
        );
        for path in [&paths[2], &renamed, &added] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }
}
