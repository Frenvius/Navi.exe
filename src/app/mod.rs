pub(crate) mod address;
pub(crate) mod choose;
pub(crate) mod clipboard;
pub mod columns;
pub(crate) mod default_manager;
pub mod drives_layout;
pub mod drop;
pub mod entry;
pub(crate) mod filter;
pub(crate) mod focus;
pub mod format;
pub mod gallery;
pub mod grid_layout;
pub mod group;
pub mod header;
pub(crate) mod home;
pub mod ids;
pub(crate) mod launch_preferences;
pub(crate) mod library_view;
pub(crate) mod menu_chrome;
pub(crate) mod menu_model;
pub mod navtree;
pub(crate) mod pane_model;
pub(crate) mod rename;
pub mod scroll;
pub(crate) mod search_query;
pub mod selection;
pub mod session;
pub mod sort;
pub mod tab_transfer;
pub(crate) mod tabs;
pub(crate) mod undo;
pub mod view;
pub(crate) mod zoxide;
pub(crate) mod zoxide_install;

pub(crate) mod commands;
use sort::Sort;

pub const VIEW_DETAILS: i32 = 6;

/// Explorer's view modes: (Ctrl+Shift+key, label key, icon, mode).
pub const VIEW_MODES: [(&str, &str, &str, i32); 8] = [
    ("1", "view.extra-large", "windows.iconsize.extralarge", 1),
    ("2", "view.large", "windows.iconsize.large", 2),
    ("3", "view.medium", "windows.iconsize.medium", 3),
    ("4", "view.small", "windows.iconsize.smallicon", 4),
    ("5", "view.list", "windows.iconsize.list", 5),
    ("6", "view.details", "windows.iconsize.details", 6),
    ("7", "view.tiles", "windows.iconsize.tile", 7),
    ("8", "view.content", "windows.iconsize.content", 8),
];

pub(crate) const ROW_PITCH: f32 = 21.0;
pub(crate) const ROW_PITCH_ROOMY: f32 = 28.0;
pub(crate) const ROW_TOP: f32 = 6.0;
/// Grouped, the first header sits 2px below the column headers, and each header takes 32px.
const GROUPED_TOP: f32 = 2.0;
const GROUP_HEADER: f32 = 32.0;
const GROUP_FOLDED: f32 = 30.0;
pub(crate) const GROUP_SUBTITLE: char = '\u{1f}';
const TYPEAHEAD_RESET_MS: u128 = 1000;
#[derive(Clone)]
pub struct Tab {
    pub history: Vec<String>,
    pub filters: Vec<Option<filter::Filter>>,
    pub libraries: Vec<Option<crate::shell::library::Context>>,
    pub index: usize,
    pub title: String,
    pub icon: u32,
}

impl Tab {
    pub fn new(path: &str) -> Tab {
        Tab {
            history: vec![ids::canonical(path)],
            filters: vec![None],
            libraries: vec![None],
            index: 0,
            title: String::new(),
            icon: 0,
        }
    }

    pub fn path(&self) -> &str {
        &self.history[self.index]
    }

    pub fn filter(&self) -> Option<filter::Filter> {
        self.filters.get(self.index).cloned().flatten()
    }

    pub fn library(&self) -> Option<crate::shell::library::Context> {
        self.libraries.get(self.index).cloned().flatten()
    }

    pub fn push(&mut self, path: String, filter: Option<filter::Filter>) {
        let library = self.library().filter(|c| c.contains(&path)).or_else(|| {
            let current = self.path();
            if !crate::shell::library::is_library(current) {
                return None;
            }
            crate::shell::library::origin_for(&path, &crate::shell::library::origins(current))
                .filter(|origin| !ids::same(&origin.path, &path))
                .map(|origin| crate::shell::library::Context {
                    library: current.to_owned(),
                    origin: origin.path,
                })
        });
        self.history.truncate(self.index + 1);
        self.filters.resize(self.history.len(), None);
        self.libraries.resize(self.history.len(), None);
        self.history.push(path);
        self.filters.push(filter);
        self.libraries.push(library);
        self.index = self.history.len() - 1;
    }
}

/// A folder's view as changed in this session (unset parts keep the saved view's).
#[derive(Clone, Default)]
pub struct FolderView {
    pub view_mode: Option<i32>,
    pub icon_size: Option<u32>,
    pub sort: Option<Sort>,
    pub group: Option<Option<group::Group>>,
    pub columns: Option<Vec<(windows::Win32::Foundation::PROPERTYKEY, f32)>>,
}
