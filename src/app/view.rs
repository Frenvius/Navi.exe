use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::PKEY_Search_Rank;

use super::{
    columns, grid_layout, group, ids,
    sort::{Sort, SortKey},
    FolderView, VIEW_DETAILS,
};
use crate::shell::viewstate::ViewState;

#[derive(Clone)]
pub struct ResolvedView {
    pub mode: i32,
    pub icon_size: u32,
    pub sort: Sort,
    pub extra_sorts: Vec<Sort>,
    pub search_ranked: bool,
    pub group: Option<group::Group>,
    pub columns: Vec<(PROPERTYKEY, f32)>,
    pub menu_columns: Vec<PROPERTYKEY>,
}

impl ResolvedView {
    pub fn snapshot(&self) -> ViewState {
        let (mode, size) = match self.mode {
            VIEW_DETAILS => (1, 16),
            5 => (4, 16),
            7 => (2, 48),
            8 => (5, 32),
            _ => (3, self.icon_size),
        };
        let mut sort = std::iter::once((self.sort.key.key(), self.sort.descending))
            .chain(self.extra_sorts.iter().map(|s| (s.key.key(), s.descending)))
            .collect::<Vec<_>>();
        if self.sort.key == SortKey::Type && self.extra_sorts.is_empty() {
            sort.push((SortKey::Name.key(), self.sort.descending));
        }
        ViewState {
            logical_mode: Some(mode),
            icon_size: Some(size),
            sort,
            group: Some(self.group.map(|g| (g.key.key(), g.descending))),
            columns: self
                .columns
                .iter()
                .map(|(key, width)| (*key, width.round() as u32))
                .collect(),
            ..Default::default()
        }
    }
}

pub fn resolve(path: &str, saved: &ViewState, session: &FolderView) -> ResolvedView {
    let saved_mode = saved.logical_mode.map(|m| match m {
        2 => 7,
        3 => match saved.icon_size.unwrap_or(96) {
            s if s >= 256 => 1,
            s if s >= 96 => 2,
            s if s >= 32 => 3,
            _ => 4,
        },
        4 => 5,
        5 => 8,
        _ => VIEW_DETAILS,
    });
    let mut mode = session.view_mode.or(saved_mode).unwrap_or(VIEW_DETAILS);
    let saved_size = saved
        .icon_size
        .filter(|_| saved.logical_mode == Some(3))
        .map(|s| s.clamp(16, 256));
    let mut icon_size = session
        .icon_size
        .or(saved_size)
        .unwrap_or_else(|| grid_layout::mode_icon_size(mode));
    if ids::same(path, ids::GALLERY) {
        icon_size = session
            .icon_size
            .or(saved.icon_size)
            .unwrap_or(48)
            .clamp(16, 256);
        mode = match icon_size {
            0..=31 => 4,
            32..=95 => 3,
            96..=255 => 2,
            _ => 1,
        };
    }
    let sorts: Vec<_> = saved
        .sort
        .iter()
        .filter(|(key, _)| *key != PKEY_Search_Rank)
        .map(|(key, descending)| Sort {
            key: SortKey::from_property(key),
            descending: *descending,
        })
        .collect();
    let sort = session.sort.or(sorts.first().copied()).unwrap_or(Sort {
        key: SortKey::Name,
        descending: false,
    });
    let extra_sorts =
        if (ids::is_library(path) || ids::same(path, ids::HOME)) && session.sort.is_none() {
            sorts.into_iter().skip(1).collect()
        } else {
            Vec::new()
        };
    let search_ranked = ids::is_search(path)
        && session.sort.is_none()
        && saved
            .sort
            .first()
            .is_some_and(|(key, _)| *key == PKEY_Search_Rank);
    let saved_group = saved.group.map(|g| {
        g.map(|(key, descending)| group::Group {
            key: SortKey::from_property(&key),
            descending,
        })
    });
    columns::set_folder_labels(saved.labels.clone(), saved.right.clone());
    let (columns, menu_columns) = columns::initial(&saved.columns, &saved.type_columns);
    ResolvedView {
        mode,
        icon_size,
        sort,
        extra_sorts,
        search_ranked,
        group: session.group.or(saved_group).flatten(),
        columns: session.columns.clone().unwrap_or(columns),
        menu_columns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn type_sort_snapshot_persists_name_direction_and_keeps_explicit_secondary() {
        let mut view = resolve("C:\\", &ViewState::default(), &FolderView::default());
        for descending in [false, true] {
            view.sort = Sort {
                key: SortKey::Type,
                descending,
            };
            assert_eq!(
                view.snapshot().sort,
                vec![
                    (SortKey::Type.key(), descending),
                    (SortKey::Name.key(), descending)
                ]
            );
        }
        view.extra_sorts = vec![Sort {
            key: SortKey::Name,
            descending: false,
        }];
        assert_eq!(
            view.snapshot().sort,
            vec![(SortKey::Type.key(), true), (SortKey::Name.key(), false)]
        );
        view.extra_sorts.clear();
        view.sort = Sort {
            key: SortKey::Modified,
            descending: true,
        };
        assert_eq!(view.snapshot().sort, vec![(SortKey::Modified.key(), true)]);
    }
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_Size,
    };

    #[test]
    fn rank_secondary_keys_and_session_override_keep_their_scope() {
        let saved = ViewState {
            sort: vec![
                (PKEY_Search_Rank, true),
                (PKEY_Size, true),
                (PKEY_ItemNameDisplay, false),
            ],
            group: Some(Some((PKEY_DateModified, true))),
            ..Default::default()
        };
        let session = FolderView::default();
        let search = resolve("search|C:\\|test", &saved, &session);
        assert!(search.search_ranked);
        assert_eq!(search.sort.key, SortKey::Size);
        assert!(search.extra_sorts.is_empty());
        let library = resolve("test.library-ms", &saved, &session);
        assert!(!library.search_ranked);
        assert_eq!(library.extra_sorts.len(), 1);
        assert_eq!(library.extra_sorts[0].key, SortKey::Name);
        let override_view = FolderView {
            sort: Some(Sort {
                key: SortKey::Modified,
                descending: false,
            }),
            group: Some(None),
            ..Default::default()
        };
        let overridden = resolve("search|C:\\|test", &saved, &override_view);
        assert!(!overridden.search_ranked);
        assert!(overridden.group.is_none());
        assert_eq!(overridden.sort.key, SortKey::Modified);
        assert!(resolve("test.library-ms", &saved, &override_view)
            .extra_sorts
            .is_empty());
    }

    #[test]
    fn mode_size_columns_and_zero_sort_preserve_saved_and_session_rules() {
        let saved = ViewState {
            logical_mode: Some(3),
            icon_size: Some(999),
            sort: vec![(PROPERTYKEY::default(), false)],
            columns: vec![(PKEY_ItemNameDisplay, 199), (PKEY_Size, 80)],
            ..Default::default()
        };
        let view = resolve("C:\\", &saved, &FolderView::default());
        assert_eq!((view.mode, view.icon_size), (1, 256));
        assert_eq!(view.sort.key, SortKey::Prop(PROPERTYKEY::default()));
        assert_eq!(view.columns[0], (PKEY_ItemNameDisplay, 199.0));
        let session = FolderView {
            view_mode: Some(4),
            icon_size: Some(24),
            columns: Some(vec![(PKEY_ItemNameDisplay, 300.0)]),
            ..Default::default()
        };
        let view = resolve("C:\\", &saved, &session);
        assert_eq!((view.mode, view.icon_size), (4, 24));
        assert_eq!(view.columns.len(), 1);
        assert_eq!(view.columns[0].1, 300.0);
        let default = resolve("C:\\", &ViewState::default(), &FolderView::default());
        assert_eq!(default.mode, VIEW_DETAILS);
        assert_eq!(default.sort.key, SortKey::Name);
    }
}
