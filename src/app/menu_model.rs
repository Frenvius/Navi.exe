use super::sort::{Sort, SortKey};
use super::{ids, VIEW_MODES};
use crate::i18n::{tr, trf};
use crate::shell::icons::Rgba;
use crate::shell::listing::{self, ShellEntry};
use std::sync::Arc;
use windows::Win32::Foundation::{HWND, PROPERTYKEY};

#[derive(Clone, Default)]
pub enum Icon {
    #[default]
    None,
    Command(&'static str),
    System(u32),
    Pixels(Arc<Rgba>),
}

#[derive(Clone, Default)]
pub struct Entry {
    pub label: String,
    pub icon: Icon,
    pub glyph: String,
    pub shortcut: String,
    pub key_tip: String,
    pub separator: bool,
    pub disabled: bool,
    pub bullet: bool,
    pub checked: bool,
    pub submenu: bool,
    pub toggle_submenu: bool,
}

#[derive(Clone)]
pub struct Item {
    pub entry: Entry,
    pub action: Action,
    pub sub: Vec<Item>,
    pub toggle: bool,
    pub tip: String,
}

#[derive(Clone, Default)]
pub struct Button {
    pub label: String,
    pub icon: Icon,
    pub disabled: bool,
    pub key_tip: String,
}

pub struct Menu {
    pub owner: &'static str,
    pub items: Vec<Item>,
    pub buttons: Vec<(Button, Action)>,
}

pub fn minimum_width<'a>(owner: &str, actions: impl IntoIterator<Item = &'a Action>) -> f32 {
    if matches!(owner, "item" | "nav")
        && actions
            .into_iter()
            .any(|action| matches!(action, Action::CopyPath(_)))
    {
        return if crate::i18n::language() == "pt-BR" {
            348.0
        } else {
            335.0
        };
    }
    match owner {
        "new" | "sort" | "view" | "more" | "filter" | "collection" | "search-options"
        | "crumbs" | "crumbs-more" | "address" | "history" => 0.0,
        "address-text" => 230.0,
        "search-text" => 163.0,
        "background" => 332.0,
        "tab" | "nav-options" => 215.0,
        "drive" | "nav-drive" | "library" => 287.0,
        _ => 200.0,
    }
}

#[derive(Clone)]
pub struct Context {
    pub hwnd: HWND,
    pub path: String,
    pub selected: Vec<ShellEntry>,
    pub cols: Vec<super::columns::Col>,
    pub menu_columns: Vec<PROPERTYKEY>,
    pub sort: Sort,
    pub group: Option<super::group::Group>,
    pub view_mode: i32,
    pub icon_size: u32,
    pub compact: bool,
    pub nav_pane: bool,
    pub details_pane: bool,
    pub preview_pane: bool,
    pub can_paste: bool,
    pub can_undo: bool,
    pub undo_label: Option<String>,
    pub close_search_in_bar: bool,
    pub new_items: Vec<crate::shell::newmenu::NewEntry>,
    pub editor: Option<HWND>,
}

fn pixels(image: &Rgba) -> Icon {
    Icon::Pixels(Arc::new(image.clone()))
}

pub fn new_items(app: &Context) -> Vec<Item> {
    if app.new_items.is_empty() {
        return vec![item(
            tr("context.new-folder"),
            Icon::Command("newfolder"),
            Action::NewFolder,
        )];
    }
    app.new_items
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let icon = match entry.verb.as_str() {
                "NewFolder" => crate::shell::icons::folder_icon(),
                "NewLink" => crate::shell::icons::icon_for_extension(".lnk"),
                extension => crate::shell::icons::icon_for_extension(extension),
            };
            item(
                &entry.label,
                entry
                    .icon
                    .as_ref()
                    .map(|icon| Icon::Pixels(icon.clone()))
                    .unwrap_or(Icon::System(icon)),
                Action::NewItem(index),
            )
        })
        .collect()
}

pub fn new_command_items(app: &Context) -> Vec<Item> {
    let mut items = new_items(app);
    for (item, entry) in items.iter_mut().zip(&app.new_items) {
        if entry.verb.starts_with('.') {
            item.entry.icon = Icon::System(crate::shell::icons::icon_for_extension(&entry.verb));
        } else if let Some(image) = crate::shell::newmenu::command_icon(&entry.verb) {
            item.entry.icon = Icon::Pixels(Arc::new(image));
        }
    }
    items
}

pub fn tab_items(index: usize, count: usize) -> Vec<Item> {
    vec![
        item(tr("tab.close"), Icon::None, Action::CloseTab(index))
            .glyph("\u{E711}")
            .shortcut("Ctrl+W"),
        item(
            tr("tab.close-others"),
            Icon::None,
            Action::CloseOtherTabs(index),
        )
        .disabled(count < 2),
        item(
            tr("tab.close-right"),
            Icon::None,
            Action::CloseTabsRight(index),
        )
        .disabled(index + 1 >= count),
        item(tr("tab.duplicate"), Icon::None, Action::DuplicateTab(index)).glyph("\u{F5ED}"),
    ]
}

#[derive(Clone)]
pub enum Action {
    None,
    NewTab(Vec<String>),
    NewWindow(Vec<String>),
    OpenSelection,
    OpenWith(String),
    OpenWithApp(crate::shell::assoc::App, Vec<String>),
    Pin(Vec<String>, bool),
    PinSearch(String),
    CommandStore(&'static str, Vec<String>),
    ShellVerb(String, Vec<String>),
    ToggleNav(usize),
    PinToStart(Vec<String>),
    RunAsAdmin(Vec<String>),
    OpenLocation(String),
    CopyPath(Vec<String>),
    Properties(Vec<String>),
    ShellMenu(Vec<String>, f32, f32),
    View(i32),
    SortKey(SortKey),
    SortDescending(bool),
    GroupBy(Option<SortKey>),
    GroupDescending(bool),
    ChooseDetails,
    SetGroupOpen(usize, bool),
    SetGroupsOpen(bool),
    NewFolder,
    NewItem(usize),
    NewItemIn(String, usize),
    SelectAll,
    SelectNone,
    InvertSelection,
    ToggleCompact,
    ToggleNavPane,
    NavOption(crate::shell::nav_options::Option, bool),
    ToggleDetailsPane,
    TogglePreviewPane,
    Zip(Vec<String>),
    Rotate(Vec<String>, bool),
    Archive(Vec<String>, &'static str),
    Wallpaper(Vec<String>),
    Launch(String),
    AppCommand(windows::core::GUID, Option<usize>, Vec<String>),
    Undo,
    FolderOptions,
    FolderOption(listing::FolderOption),
    Cut,
    Copy,
    CopyItems(Vec<String>),
    Paste,
    Rename,
    Share,
    Delete,
    CopyText(String),
    AddressCommand(i32),
    ClearAddressHistory,
    History(usize),
    CloseTab(usize),
    CloseSearch,
    HomeFilter(u8),
    CloudCommand(u64, usize),
    GalleryFilter(usize),
    GalleryManage,
    RestoreBin(bool),
    SearchRefine(String, String),
    SearchCurrentFolderOnly(bool),
    SearchToggle(u8),
    CloseOtherTabs(usize),
    CloseTabsRight(usize),
    DuplicateTab(usize),
}

pub fn item(label: &str, icon: Icon, action: Action) -> Item {
    Item {
        entry: Entry {
            label: label.into(),
            icon,
            ..Default::default()
        },
        action,
        sub: Vec::new(),
        toggle: false,
        tip: String::new(),
    }
}

pub fn separator() -> Item {
    Item {
        entry: Entry {
            separator: true,
            ..Default::default()
        },
        action: Action::None,
        sub: Vec::new(),
        toggle: false,
        tip: String::new(),
    }
}
pub fn command_tip(command: &str) -> String {
    thread_local! {
        static TIPS: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
    }
    if let Some(hit) = TIPS.with(|t| t.borrow().get(command).cloned()) {
        return hit;
    }
    let text = crate::i18n::command_description(command).unwrap_or_default();
    TIPS.with(|t| t.borrow_mut().insert(command.to_string(), text.clone()));
    text
}

impl Item {
    pub fn tip(mut self, command: &str) -> Item {
        self.tip = command_tip(command);
        self
    }

    pub fn shortcut(mut self, s: &str) -> Item {
        self.entry.shortcut = s.into();
        self
    }
    pub fn bullet(mut self, on: bool) -> Item {
        self.entry.bullet = on;
        self.toggle = true;
        self
    }
    pub fn checked(mut self, on: bool) -> Item {
        self.entry.checked = on;
        self.toggle = true;
        self
    }
    pub fn disabled(mut self, off: bool) -> Item {
        self.entry.disabled = off;
        self
    }
    pub fn glyph(mut self, g: &str) -> Item {
        self.entry.glyph = g.into();
        self
    }
    pub fn submenu(mut self, items: Vec<Item>) -> Item {
        self.entry.submenu = true;
        self.sub = items;
        self
    }
}

pub fn button(label: &str, icon: Icon, action: Action) -> (Button, Action) {
    (
        Button {
            label: label.into(),
            icon,
            disabled: false,
            key_tip: access_key(&action, false).into(),
        },
        action,
    )
}
pub fn access_key(action: &Action, background: bool) -> &'static str {
    let pt = crate::i18n::language() == "pt-BR";
    match action {
        Action::Cut => {
            if pt {
                "O"
            } else {
                "T"
            }
        }
        Action::Copy => "C",
        Action::Rename => {
            if pt {
                "R1"
            } else {
                "M"
            }
        }
        Action::Share => {
            if pt {
                "C1"
            } else {
                "S"
            }
        }
        Action::Delete => {
            if pt {
                "X"
            } else {
                "D"
            }
        }
        Action::OpenSelection => {
            if pt {
                "A"
            } else {
                "O"
            }
        }
        Action::CopyPath(_) => {
            if pt {
                "I2"
            } else {
                "A"
            }
        }
        Action::Properties(_) => "P",
        Action::Undo => {
            if pt {
                "D"
            } else {
                "U"
            }
        }
        Action::ShellMenu(..) => {
            if pt {
                if background {
                    "R"
                } else {
                    "R2"
                }
            } else {
                "W"
            }
        }
        Action::Pin(..) => {
            if pt {
                "F"
            } else {
                "F"
            }
        }
        Action::Launch(s) if s.contains("wt.exe") => "T",
        _ => "",
    }
}

pub fn submenu_key(action: &Action) -> Option<&'static str> {
    match action {
        Action::View(_) => Some("X"),
        Action::SortKey(_) => Some("C"),
        Action::GroupBy(_) => Some("A"),
        Action::NewItem(_) | Action::NewFolder => Some("V"),
        Action::OpenWithApp(..) => Some("I1"),
        _ => None,
    }
}

pub fn finalize(items: &mut [Item], owner: &str) {
    for item in items {
        item.entry.toggle_submenu = item.toggle && item.entry.submenu;
        if item.entry.key_tip.is_empty() {
            item.entry.key_tip = item
                .sub
                .first()
                .and_then(|first| submenu_key(&first.action))
                .unwrap_or_else(|| access_key(&item.action, owner == "background"))
                .to_owned();
        }
        finalize(&mut item.sub, owner);
    }
}

fn type_columns(app: &Context) -> Vec<(SortKey, String)> {
    let keys = app.menu_columns.clone();
    keys.iter()
        .filter_map(|k| super::columns::label(k).map(|label| (SortKey::from_property(k), label)))
        .collect()
}

fn group_items(app: &Context) -> Vec<Item> {
    let group = app.group.map(|g| g.key);
    let mut items: Vec<Item> = type_columns(app)
        .into_iter()
        .map(|(k, label)| {
            item(&label, Icon::None, Action::GroupBy(Some(k))).bullet(group == Some(k))
        })
        .collect();
    if group.is_some() {
        items.push(item(
            tr("menu.group-none"),
            Icon::None,
            Action::GroupBy(None),
        ));
    }
    items
}

fn shown_columns(app: &Context) -> Vec<(SortKey, String)> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_DateModified;
    let st = app;
    st.cols
        .iter()
        .map(|c| {
            (
                SortKey::from_property(&c.key),
                if c.key == PKEY_DateModified {
                    tr("column.modified.group").to_string()
                } else {
                    c.label.clone()
                },
            )
        })
        .collect()
}

fn context_sort_items(app: &Context) -> Vec<Item> {
    let sort = app.sort;
    let mut items: Vec<Item> = shown_columns(app)
        .into_iter()
        .map(|(k, label)| item(&label, Icon::None, Action::SortKey(k)).bullet(sort.key == k))
        .collect();
    items.extend([
        separator(),
        item(
            tr("menu.ascending"),
            Icon::None,
            Action::SortDescending(false),
        )
        .bullet(!sort.descending)
        .tip("Windows.SortAscending"),
        item(
            tr("menu.descending"),
            Icon::None,
            Action::SortDescending(true),
        )
        .bullet(sort.descending)
        .tip("Windows.SortDescending"),
    ]);
    items
}

fn context_group_items(app: &Context) -> Vec<Item> {
    let group = app.group;
    let mut items: Vec<Item> = shown_columns(app)
        .into_iter()
        .map(|(k, label)| {
            item(&label, Icon::None, Action::GroupBy(Some(k)))
                .bullet(group.map(|g| g.key) == Some(k))
        })
        .collect();
    if group.is_some() {
        items.push(item(
            tr("menu.group-none"),
            Icon::None,
            Action::GroupBy(None),
        ));
    }
    let descending = group.map(|g| g.descending);
    items.extend([
        separator(),
        item(
            tr("menu.ascending"),
            Icon::None,
            Action::GroupDescending(false),
        )
        .bullet(descending == Some(false))
        .disabled(group.is_none())
        .tip("Windows.SortGroupsAscending"),
        item(
            tr("menu.descending"),
            Icon::None,
            Action::GroupDescending(true),
        )
        .bullet(descending == Some(true))
        .disabled(group.is_none())
        .tip("Windows.SortGroupsDescending"),
        separator(),
        item(tr("menu.more-columns"), Icon::None, Action::ChooseDetails),
    ]);
    items
}

fn sort_items(app: &Context) -> Vec<Item> {
    let sort = app.sort;
    let key =
        |k: SortKey, label: &str| item(label, Icon::None, Action::SortKey(k)).bullet(sort.key == k);
    let mut columns = type_columns(app);
    columns.truncate(8);
    let more = columns.split_off(columns.len().min(3));
    let more_open = sort.key.key() == windows::Win32::Storage::EnhancedStorage::PKEY_Search_Rank
        || more.iter().any(|(k, _)| *k == sort.key);
    let mut items: Vec<Item> = columns.iter().map(|(k, label)| key(*k, label)).collect();
    if !more.is_empty() {
        items.push(
            item(tr("menu.more"), Icon::None, Action::None)
                .bullet(more_open)
                .submenu(more.iter().map(|(k, label)| key(*k, label)).collect()),
        );
    }
    items.extend([
        separator(),
        item(
            tr("menu.ascending"),
            Icon::None,
            Action::SortDescending(false),
        )
        .bullet(!sort.descending)
        .tip("Windows.SortAscending"),
        item(
            tr("menu.descending"),
            Icon::None,
            Action::SortDescending(true),
        )
        .bullet(sort.descending)
        .tip("Windows.SortDescending"),
    ]);
    items
}

pub fn sort_menu_items(app: &Context) -> Vec<Item> {
    let mut items = sort_items(app);
    items.extend([
        separator(),
        item(tr("menu.group-by"), Icon::None, Action::None).submenu(group_items(app)),
    ]);
    items
}

pub fn view_items(app: &Context, with_show: bool) -> Vec<Item> {
    let (view, compact, nav_pane, details_pane, preview_pane) = {
        let st = app;
        (
            if ids::same(&st.path, ids::GALLERY) {
                match st.icon_size {
                    0..=31 => 4,
                    32..=95 => 3,
                    96..=255 => 2,
                    _ => 1,
                }
            } else {
                st.view_mode
            },
            st.compact,
            st.nav_pane,
            st.details_pane,
            st.preview_pane,
        )
    };
    let mut items: Vec<Item> = VIEW_MODES
        .iter()
        .filter(|(_, _, _, mode)| !ids::same(&app.path.clone(), ids::GALLERY) || *mode <= 4)
        .map(|(k, label, icon, mode)| {
            let it =
                item(tr(label), Icon::Command(icon), Action::View(*mode)).bullet(view == *mode);
            if with_show {
                it
            } else {
                it.shortcut(&format!("Ctrl+Shift+{k}"))
            }
        })
        .collect();
    if with_show {
        use listing::FolderOption;
        let opts = listing::settings();
        items.push(separator());
        items.push(
            item(
                tr("menu.details-pane"),
                Icon::Command("windows.previewpane"),
                Action::ToggleDetailsPane,
            )
            .bullet(details_pane),
        );
        items.push(
            item(
                tr("menu.preview-pane"),
                Icon::Command("windows.readingpane"),
                Action::TogglePreviewPane,
            )
            .bullet(preview_pane),
        );
        items.push(separator());
        items.push(
            item(tr("menu.show"), Icon::None, Action::None).submenu(vec![
                item(
                    tr("menu.nav-pane"),
                    Icon::Command("windows.navpane"),
                    Action::ToggleNavPane,
                )
                .checked(nav_pane)
                .tip("Windows.navpane"),
                separator(),
                item(
                    tr("menu.compact"),
                    Icon::Command("windows.compactmode"),
                    Action::ToggleCompact,
                )
                .checked(compact)
                .tip("Windows.CompactMode"),
                separator(),
                item(
                    tr("menu.check-boxes"),
                    Icon::Command("windows.selectioncheckboxes"),
                    Action::FolderOption(FolderOption::CheckBoxes),
                )
                .checked(opts.check_boxes)
                .tip("Windows.SelectionCheckboxes"),
                item(
                    tr("menu.extensions"),
                    Icon::Command("windows.showfileextensions"),
                    Action::FolderOption(FolderOption::Extensions),
                )
                .checked(!opts.hide_extensions)
                .tip("Windows.ShowFileExtensions"),
                item(
                    tr("menu.hidden-items"),
                    Icon::Command("windows.showhiddenfiles"),
                    Action::FolderOption(FolderOption::HiddenItems),
                )
                .checked(opts.show_hidden)
                .tip("Windows.ShowHiddenFiles"),
            ]),
        );
    }
    items
}

pub fn more_items(app: &Context) -> Vec<Item> {
    let (selected, folder) = (app.selected.clone(), app.path.clone());
    let fs_selection = !selected.is_empty() && selected.iter().all(|e| e.is_fs);
    let fs_folder = !ids::is_virtual(&folder) && !folder.is_empty();
    let paths: Vec<String> = selected.iter().map(|e| e.path.clone()).collect();
    let mut items = Vec::new();
    let search = ids::is_search(&folder);
    let pc = ids::same(&folder, ids::THIS_PC);
    let library_folder = ids::is_library(&folder) || ids::same(&folder, ids::LIBRARIES);
    let library_selection =
        !selected.is_empty() && selected.iter().all(|e| ids::is_library(&e.path));
    let drive = if selected.len() == 1 && ids::is_drive_root(&selected[0].path) {
        Some(selected[0].path.clone())
    } else if selected.is_empty() && ids::is_drive_root(&folder) {
        Some(folder.clone())
    } else {
        None
    };
    if search && !app.close_search_in_bar {
        let close = crate::i18n::system_string("shell32.dll", 37565);
        items.extend([
            item(
                &close,
                Icon::Command("windows.searchclosetab"),
                Action::CloseSearch,
            )
            .tip("Windows.SearchCloseTab"),
            separator(),
        ]);
    }
    let recycle = ids::same(&folder, ids::RECYCLE_BIN);
    if recycle {
        let shell32 = |id: u32| crate::i18n::system_string("shell32.dll", id);
        items.push(
            item(
                &shell32(31333),
                Icon::Command("windows.recyclebin.restoreall"),
                Action::RestoreBin(true),
            )
            .tip("Windows.RecycleBin.RestoreAll"),
        );
        if !selected.is_empty() {
            items.push(
                item(
                    &shell32(31335),
                    Icon::Command("windows.recyclebin.restoreitems"),
                    Action::RestoreBin(false),
                )
                .tip("Windows.RecycleBin.RestoreItems"),
            );
        }
        items.push(separator());
    }
    if fs_selection
        && selected
            .iter()
            .all(|e| !e.is_dir && crate::shell::preview::is_image(&e.path))
    {
        items.push(item(
            tr("menu.rotate-left"),
            Icon::Command("windows.rotate270"),
            Action::Rotate(paths.clone(), false),
        ));
        items.push(item(
            tr("menu.rotate-right"),
            Icon::Command("windows.rotate90"),
            Action::Rotate(paths.clone(), true),
        ));
        items.push(separator());
    }
    items.push(
        item(tr("menu.undo"), Icon::Command("windows.undo"), Action::Undo).disabled(!app.can_undo),
    );
    if let Some(drive) = &drive {
        let native = crate::shell::menu::commands(app.hwnd, ids::THIS_PC, &[drive.clone()]);
        let kind = unsafe {
            windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(
                crate::shell::wide(drive).as_ptr(),
            ))
        };
        let first = items.len();
        items.push(separator());
        for (resource, icon, command) in [
            (37494, "windows.cleanup", "Windows.CleanUp"),
            (37490, "windows.defragment", "Windows.Defragment"),
            (37488, "windows.diskformat", "Windows.DiskFormat"),
        ] {
            if (command == "Windows.DiskFormat" && native.iter().any(|c| c.verb == "format"))
                || (command != "Windows.DiskFormat" && matches!(kind, 2 | 3))
            {
                items.push(
                    item(
                        &crate::i18n::system_string("shell32.dll", resource),
                        Icon::Command(icon),
                        Action::CommandStore(command, vec![drive.clone()]),
                    )
                    .tip(command),
                );
            }
        }
        for c in native
            .iter()
            .filter(|c| c.verb == "eject" || c.verb == "disconnect")
        {
            let icon = if c.verb == "eject" {
                "windows.eject"
            } else {
                "windows.disconnectnetworkdrive"
            };
            items.push(
                item(
                    &c.title,
                    Icon::Command(icon),
                    Action::ShellVerb(c.verb.clone(), vec![drive.clone()]),
                )
                .disabled(!c.enabled),
            );
        }
        if items.len() == first + 1 {
            items.pop();
        }
    }
    if pc {
        items.push(separator());
        for (resource, icon, command) in [
            (37600, "windows.addmediaserver", "Windows.AddMediaServer"),
            (
                31302,
                "windows.addnetworklocation",
                "Windows.AddNetworkLocation",
            ),
            (31300, "windows.mapnetworkdrive", "Windows.MapNetworkDrive"),
            (
                31304,
                "windows.disconnectnetworkdrive",
                "Windows.DisconnectNetworkDrive",
            ),
        ] {
            items.push(
                item(
                    &crate::i18n::system_string("shell32.dll", resource),
                    Icon::Command(icon),
                    Action::CommandStore(command, vec![]),
                )
                .tip(command),
            );
        }
    } else if !recycle {
        items.push(separator());
        if selected.is_empty() || library_selection {
            let action = if search {
                Action::PinSearch(folder.clone())
            } else {
                Action::Pin(
                    if library_selection {
                        paths.clone()
                    } else {
                        vec![folder.clone()]
                    },
                    true,
                )
            };
            items.push(
                item(
                    tr("menu.pin-home"),
                    Icon::Command("windows.pintohome"),
                    action,
                )
                .disabled(!fs_folder && !search && !library_folder && !library_selection)
                .tip("Windows.PinToHome"),
            );
        } else if drive.is_none() {
            items.push(
                item(
                    tr("menu.zip"),
                    Icon::Command("Windows.Zip.Action"),
                    Action::Zip(paths.clone()),
                )
                .disabled(!fs_selection)
                .tip("Windows.zip"),
            );
            items.push(
                item(
                    tr("menu.favorites"),
                    Icon::Command("windows.pintohomefile"),
                    Action::Pin(paths.clone(), true),
                )
                .disabled(!fs_selection)
                .tip("Windows.PinToHomeFile"),
            );
            items.push(
                item(
                    tr("menu.copy-path"),
                    Icon::Command("windows.copyaspath"),
                    Action::CopyPath(paths.clone()),
                )
                .disabled(!fs_selection)
                .tip("Windows.copyaspath"),
            );
        }
    }
    items.extend([
        separator(),
        item(
            tr("menu.select-all"),
            Icon::Command("windows.selectall"),
            Action::SelectAll,
        )
        .tip("Windows.selectall"),
        item(
            tr("menu.select-none"),
            Icon::Command("windows.selectnone"),
            Action::SelectNone,
        )
        .tip("Windows.selectnone"),
        item(
            tr("menu.invert-selection"),
            Icon::Command("windows.invertselection"),
            Action::InvertSelection,
        )
        .tip("Windows.invertselection"),
    ]);
    items.extend([
        separator(),
        item(
            tr("menu.properties"),
            Icon::Command("windows.properties"),
            Action::Properties(
                app.selected
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect(),
            ),
        )
        .disabled((search || ids::same(&folder, ids::LIBRARIES)) && selected.is_empty())
        .tip("Windows.properties"),
        item(
            tr("menu.options"),
            Icon::Command("windows.folderoptions"),
            Action::FolderOptions,
        )
        .tip("Windows.folderoptions"),
    ]);
    items
}

pub fn item_menu(app: &Context, entries: &[ShellEntry], prefix: Vec<Item>, x: f32, y: f32) -> Menu {
    let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
    if !paths.is_empty() && paths.iter().all(|p| ids::is_drive_root(p)) {
        return drive_menu(app, &paths, prefix, x, y);
    }
    if !paths.is_empty() && paths.iter().all(|p| ids::is_library(p)) {
        return library_menu(&paths, prefix, x, y);
    }
    let single = (entries.len() == 1).then(|| &entries[0]);
    let all_fs = entries.iter().all(|e| e.is_fs);
    let folders = entries.iter().all(|e| e.is_dir);

    let is_link = |e: &ShellEntry| e.path.to_lowercase().ends_with(".lnk");
    let target = single.filter(|e| !e.is_dir && e.is_fs).and_then(|e| {
        if is_link(e) {
            crate::shell::ops::link_file(&e.path)
        } else {
            Some(e.path.clone())
        }
    });
    let target_file = target.clone().filter(|t| !std::path::Path::new(t).is_dir());
    let program = target_file
        .as_deref()
        .is_some_and(|t| crate::shell::assoc::has_verb(t, "runas"));
    let apps = target_file
        .as_deref()
        .filter(|_| !program)
        .map(crate::shell::assoc::apps_for)
        .unwrap_or_default();
    let app_image = |a: &crate::shell::assoc::App| a.icon.as_ref().map(pixels).unwrap_or_default();
    let first_apps = if single.is_none() {
        entries
            .first()
            .filter(|e| !e.is_dir && e.is_fs)
            .map(|e| crate::shell::assoc::apps_for(&e.path))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let open_icon = if folders && all_fs {
        Icon::System(crate::shell::icons::explorer_icon())
    } else if let Some(e) = single.filter(|_| program) {
        Icon::System(e.icon & 0x00FF_FFFF)
    } else {
        apps.first()
            .or(first_apps.first())
            .map(app_image)
            .unwrap_or_default()
    };
    let mut items = prefix;
    items.push(item(tr("context.open"), open_icon, Action::OpenSelection).shortcut("Enter"));
    if folders {
        items.push(item(
            tr("context.open-tab"),
            Icon::Command("windows.opennewtab"),
            Action::NewTab(paths.clone()),
        ));
        items.push(item(
            tr("context.open-window"),
            Icon::Command("windows.opennewwindow"),
            Action::NewWindow(paths.clone()),
        ));
    }
    if let Some(t) = target_file.as_deref().filter(|_| !program) {
        let mut sorted: Vec<&crate::shell::assoc::App> = apps.iter().collect();
        sorted.sort_by_key(|a| a.name.to_lowercase());
        let mut sub: Vec<Item> = sorted
            .into_iter()
            .map(|a| {
                item(
                    &a.name,
                    app_image(a),
                    Action::OpenWithApp(a.clone(), paths.clone()),
                )
            })
            .collect();
        let ext = std::path::Path::new(t)
            .extension()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_default();
        let store = crate::shell::assoc::store_icon()
            .as_ref()
            .map(pixels)
            .unwrap_or_default();
        sub.push(item(
            tr("context.store"),
            store,
            Action::Launch(format!("ms-windows-store://assoc/?FileExt={ext}")),
        ));
        sub.push(item(
            tr("context.other-app"),
            Icon::None,
            Action::OpenWith(t.to_string()),
        ));
        items.push(
            item(
                tr("context.open-with"),
                Icon::Command("windows.openwith"),
                Action::None,
            )
            .submenu(sub),
        );
    }
    if all_fs && !folders {
        for command in crate::shell::extmenu::sharing_commands(&paths) {
            items.push(extension_item(&command, &paths));
        }
    }
    if program {
        items.push(item(
            tr("context.run-as-admin"),
            Icon::Command("RunAsAdmin"),
            Action::RunAsAdmin(paths.clone()),
        ));
    }
    let location = if ids::is_search(&app.path.clone()) {
        single.filter(|e| e.is_fs).map(|e| {
            if is_link(e) {
                target.clone().unwrap_or_else(|| e.path.clone())
            } else {
                e.path.clone()
            }
        })
    } else {
        target.clone().filter(|_| single.is_some_and(is_link))
    };
    if let Some(t) = location {
        let label = if single.is_some_and(|e| e.is_dir) {
            crate::i18n::system_string("shell32.dll", 1040)
        } else {
            tr("context.open-location").to_string()
        };
        items.push(item(
            &label,
            Icon::Command("windows.shortcut.opencontaining"),
            Action::OpenLocation(t),
        ));
    }
    let images = all_fs
        && entries
            .iter()
            .all(|e| !e.is_dir && crate::shell::preview::is_image(&e.path));
    if images {
        items.push(item(
            tr("context.background"),
            Icon::Command("windows.setdesktopwallpaper"),
            Action::Wallpaper(paths.clone()),
        ));
        items.push(item(
            tr("menu.rotate-right"),
            Icon::Command("windows.rotate90"),
            Action::Rotate(paths.clone(), true),
        ));
        items.push(item(
            tr("menu.rotate-left"),
            Icon::Command("windows.rotate270"),
            Action::Rotate(paths.clone(), false),
        ));
    }
    if all_fs && folders {
        items.push(item(
            tr("menu.pin-home"),
            Icon::Command("windows.pintohome"),
            Action::Pin(paths.clone(), true),
        ));
    }
    let exe = |p: &str| p.to_lowercase().ends_with(".exe");
    let startable = |e: &ShellEntry| {
        e.is_dir || exe(&e.path) || (is_link(e) && target.as_deref().is_some_and(exe))
    };
    if let Some(e) = single.filter(|e| e.is_fs && startable(e)) {
        items.push(item(
            tr("context.pin-start"),
            Icon::Command("PinToStartScreen"),
            Action::PinToStart(vec![e.path.clone()]),
        ));
    }
    if all_fs && !folders {
        items.push(item(
            tr("context.favorites"),
            Icon::Command("windows.pintohomefile"),
            Action::Pin(paths.clone(), true),
        ));
    }
    if all_fs {
        let formats = vec![
            item(
                tr("context.compress.zip"),
                Icon::None,
                Action::Zip(paths.clone()),
            ),
            item(
                tr("context.compress.7z"),
                Icon::None,
                Action::Archive(paths.clone(), ".7z"),
            ),
            item(
                tr("context.compress.tar"),
                Icon::None,
                Action::Archive(paths.clone(), ".tar"),
            ),
            item(
                tr("context.compress.options"),
                Icon::None,
                Action::Archive(paths.clone(), ""),
            ),
        ];
        items.push(
            item(
                tr("context.compress"),
                Icon::Command("Windows.Zip.Action"),
                Action::None,
            )
            .submenu(formats),
        );
    }
    if all_fs {
        items.push(
            item(
                tr("context.copy-as-path"),
                Icon::Command("windows.copyaspath"),
                Action::CopyPath(paths.clone()),
            )
            .shortcut("Ctrl+Shift+C"),
        );
    }
    items.push(
        item(
            tr("menu.properties"),
            Icon::Command("windows.properties"),
            Action::Properties(paths.clone()),
        )
        .shortcut("Alt+Enter"),
    );
    if all_fs {
        push_app_commands(&mut items, &paths, false);
    }
    items.push(separator());
    items.push(item(
        tr("context.more-options"),
        Icon::Command("expandtoclassic"),
        Action::ShellMenu(paths.clone(), x, y),
    ));

    let mut buttons = Vec::new();
    if all_fs {
        buttons.push(button(
            tr("context.cut"),
            Icon::Command("windows.cut"),
            Action::Cut,
        ));
        buttons.push(button(
            tr("context.copy"),
            Icon::Command("windows.copy"),
            Action::Copy,
        ));
        buttons.push(button(
            tr("menu.rename"),
            Icon::Command("windows.rename"),
            Action::Rename,
        ));
        if !entries.iter().any(|e| e.is_dir) {
            buttons.push(button(
                tr("cmd.share"),
                Icon::Command("Windows.ModernShare"),
                Action::Share,
            ));
        }
        buttons.push(button(
            tr("context.delete"),
            Icon::Command("windows.ribbondelete"),
            Action::Delete,
        ));
    }
    Menu {
        owner: "item",
        items,
        buttons,
    }
}

pub fn library_menu(paths: &[String], mut items: Vec<Item>, x: f32, y: f32) -> Menu {
    let icon = Icon::System(crate::shell::icons::explorer_icon());
    items.push(item(tr("context.open"), icon, Action::OpenSelection).shortcut("Enter"));
    items.push(item(
        tr("context.open-tab"),
        Icon::Command("windows.opennewtab"),
        Action::NewTab(paths.to_vec()),
    ));
    items.push(item(
        tr("context.open-window"),
        Icon::Command("windows.opennewwindow"),
        Action::NewWindow(paths.to_vec()),
    ));
    items.push(item(
        tr("menu.pin-home"),
        Icon::Command("windows.pintohome"),
        Action::Pin(paths.to_vec(), true),
    ));
    items.push(item(
        tr("context.pin-start"),
        Icon::Command("PinToStartScreen"),
        Action::PinToStart(paths.to_vec()),
    ));
    items.push(
        item(
            tr("menu.properties"),
            Icon::Command("windows.properties"),
            Action::Properties(paths.to_vec()),
        )
        .shortcut("Alt+Enter"),
    );
    items.push(separator());
    items.push(item(
        tr("context.more-options"),
        Icon::Command("expandtoclassic"),
        Action::ShellMenu(paths.to_vec(), x, y),
    ));
    let mut buttons = vec![button(
        tr("context.copy"),
        Icon::Command("windows.copy"),
        Action::Copy,
    )];
    if crate::shell::ops::can_paste() {
        buttons.push(button(
            tr("context.paste"),
            Icon::Command("windows.paste"),
            Action::Paste,
        ));
    }
    buttons.push(button(
        tr("menu.rename"),
        Icon::Command("windows.rename"),
        Action::Rename,
    ));
    buttons.push(button(
        tr("context.delete"),
        Icon::Command("windows.ribbondelete"),
        Action::Delete,
    ));
    Menu {
        owner: "library",
        items,
        buttons,
    }
}

pub fn drive_menu(app: &Context, paths: &[String], mut items: Vec<Item>, x: f32, y: f32) -> Menu {
    let native = crate::shell::menu::commands(app.hwnd, ids::THIS_PC, paths);
    let explorer = Icon::System(crate::shell::icons::explorer_icon());
    let mut open_row = item(tr("context.open"), explorer, Action::OpenSelection).shortcut("Enter");
    open_row.entry.key_tip = "A1".into();
    items.push(open_row);
    items.push(item(
        tr("context.open-tab"),
        Icon::Command("windows.opennewtab"),
        Action::NewTab(paths.to_vec()),
    ));
    items.last_mut().unwrap().entry.key_tip = "G".into();
    items.push(item(
        tr("context.open-window"),
        Icon::Command("windows.opennewwindow"),
        Action::NewWindow(paths.to_vec()),
    ));
    items.last_mut().unwrap().entry.key_tip = "E".into();
    items.extend(drive_shell_items(&native, paths));
    if let Some(c) = native
        .iter()
        .find(|c| c.verb.eq_ignore_ascii_case("pintohome"))
    {
        let mut row = item(
            &c.title,
            Icon::Command("windows.pintohome"),
            Action::Pin(paths.to_vec(), true),
        )
        .disabled(!c.enabled);
        row.entry.key_tip = "A2".into();
        items.push(row);
    }
    if let Some(c) = native
        .iter()
        .find(|c| c.verb.eq_ignore_ascii_case("PinToStartScreen"))
    {
        let mut row = item(
            &c.title,
            Icon::Command("PinToStartScreen"),
            Action::PinToStart(paths.to_vec()),
        )
        .disabled(!c.enabled);
        row.entry.key_tip = "F".into();
        items.push(row);
    }
    items.push(
        item(
            tr("menu.properties"),
            Icon::Command("windows.properties"),
            Action::Properties(paths.to_vec()),
        )
        .shortcut("Alt+Enter"),
    );
    items.push(separator());
    items.push(item(
        tr("context.more-options"),
        Icon::Command("expandtoclassic"),
        Action::ShellMenu(paths.to_vec(), x, y),
    ));
    let mut buttons = vec![button(
        tr("context.copy"),
        Icon::Command("windows.copy"),
        Action::Copy,
    )];
    if crate::shell::ops::can_paste() {
        buttons.push(button(
            tr("context.paste"),
            Icon::Command("windows.paste"),
            Action::Paste,
        ));
    }
    buttons.push(button(
        tr("menu.rename"),
        Icon::Command("windows.rename"),
        Action::Rename,
    ));
    Menu {
        owner: "drive",
        items,
        buttons,
    }
}

pub fn drive_shell_items(native: &[crate::shell::menu::Command], paths: &[String]) -> Vec<Item> {
    let mut items = Vec::new();
    for c in native {
        let (icon, key) = match c.verb.to_ascii_lowercase().as_str() {
            v if v.contains("-bde") => ("windows.bitlocker.manage", "K"),
            "format" => ("windows.diskformat", "T"),
            "eject" => ("windows.eject", "J"),
            "disconnect" => ("windows.disconnectnetworkdrive", "D"),
            _ => continue,
        };
        let mut row = item(
            &c.title,
            Icon::Command(icon),
            Action::ShellVerb(c.verb.clone(), paths.to_vec()),
        )
        .disabled(!c.enabled);
        row.entry.key_tip = key.into();
        items.push(row);
    }
    items
}

pub fn push_app_commands(items: &mut Vec<Item>, paths: &[String], background: bool) {
    let commands = crate::shell::extmenu::commands(paths, background);
    if commands.is_empty() {
        return;
    }
    items.push(separator());
    for c in commands {
        items.push(extension_item(&c, paths));
    }
}

pub fn extension_item(c: &crate::shell::extmenu::Command, paths: &[String]) -> Item {
    let icon = if c.subcommand.is_none()
        && c.clsid == windows::core::GUID::from_u128(0x2f788d0f_1317_441b_86d2_4725301bd49d)
    {
        Icon::Command("windows.sendtomyphone")
    } else if c.subcommand.is_none()
        && c.clsid == windows::core::GUID::from_u128(0x9ad244df_e071_4b65_b741_2a4c5f52bf32)
    {
        Icon::Command("Windows.ModernShare")
    } else {
        c.icon.as_ref().map(pixels).unwrap_or_default()
    };
    let mut row = item(
        &c.title,
        icon,
        Action::AppCommand(c.clsid, c.subcommand, paths.to_vec()),
    )
    .disabled(!c.enabled);
    row.entry.key_tip = c.access_key.clone().into();
    if !c.subs.is_empty() {
        row = row.submenu(
            c.subs
                .iter()
                .filter(|s| !s.title.is_empty())
                .map(|s| extension_item(s, paths))
                .collect(),
        );
    }
    row
}

pub fn background_menu(app: &Context, x: f32, y: f32) -> Menu {
    let (folder, can_paste) = {
        let st = app;
        (st.path.clone(), st.can_paste)
    };
    let fs = (!ids::is_virtual(&folder) || ids::is_library(&folder)) && !folder.is_empty();
    let mut items = vec![
        item(tr("menu.view"), Icon::Command("View"), Action::None).submenu(view_items(app, false)),
        item(tr("menu.sort-by"), Icon::Command("SortBy"), Action::None)
            .submenu(context_sort_items(app)),
        item(
            tr("menu.group-by"),
            Icon::Command("windows.groupbycolumn"),
            Action::None,
        )
        .submenu(context_group_items(app)),
        separator(),
    ];
    if let Some(label) = app.undo_label.clone() {
        items.push(
            item(
                &trf("menu.undo-named", &[&label]),
                Icon::Command("windows.undo"),
                Action::Undo,
            )
            .shortcut("Ctrl+Z"),
        );
    }
    if fs || ids::same(&folder, ids::LIBRARIES) {
        items.push(
            item(
                tr("cmd.new"),
                Icon::Command("windows.newitem"),
                Action::None,
            )
            .submenu(new_items(app)),
        );
    }
    items.push(
        item(
            tr("menu.properties"),
            Icon::Command("windows.properties"),
            Action::Properties(Vec::new()),
        )
        .shortcut("Alt+Enter"),
    );
    if fs {
        push_app_commands(&mut items, &[folder.clone()], true);
    }
    items.push(separator());
    items.push(item(
        tr("context.more-options"),
        Icon::Command("expandtoclassic"),
        Action::ShellMenu(Vec::new(), x, y),
    ));
    let buttons = if can_paste && fs {
        vec![button(
            tr("context.paste"),
            Icon::Command("windows.paste"),
            Action::Paste,
        )]
    } else {
        Vec::new()
    };
    Menu {
        owner: "background",
        items,
        buttons,
    }
}

pub fn nav_options_menu() -> Menu {
    use crate::shell::nav_options::{read, Option};
    let options = read();
    let items = vec![
        item(
            tr("nav.show-pc"),
            Icon::None,
            Action::NavOption(Option::ThisPc, !options.this_pc),
        )
        .checked(options.this_pc),
        item(
            tr("nav.show-network"),
            Icon::None,
            Action::NavOption(Option::Network, !options.network),
        )
        .checked(options.network),
        item(
            tr("nav.show-libraries"),
            Icon::None,
            Action::NavOption(Option::Libraries, !options.libraries),
        )
        .checked(options.libraries),
        item(
            tr("nav.show-all"),
            Icon::None,
            Action::NavOption(Option::AllFolders, !options.all),
        )
        .checked(options.all),
        item(
            tr("nav.expand-current"),
            Icon::None,
            Action::NavOption(Option::Expand, !options.expand),
        )
        .checked(options.expand),
    ];
    Menu {
        owner: "nav-options",
        items,
        buttons: Vec::new(),
    }
}

pub fn nav_menu(app: &Context, target: NavTarget, x: f32, y: f32) -> Menu {
    let NavTarget {
        node,
        row,
        in_pins,
        expanded,
    } = target;
    let pinned = in_pins && node.is_pinned;
    let ids_ = vec![node.id.clone()];
    let drive = ids::is_drive_root(&node.id);
    let mut items = vec![
        item(
            tr("context.open-tab"),
            Icon::Command("windows.opennewtab"),
            Action::NewTab(ids_.clone()),
        ),
        item(
            tr("context.open-window"),
            Icon::Command("windows.opennewwindow"),
            Action::NewWindow(ids_.clone()),
        ),
    ];
    if drive {
        items.insert(0, separator());
        items.insert(
            0,
            item(
                tr(if expanded {
                    "context.collapse-group"
                } else {
                    "context.expand-group"
                }),
                Icon::Command(if expanded {
                    "windows.collapse"
                } else {
                    "windows.expand"
                }),
                Action::ToggleNav(row),
            ),
        );
        let native = crate::shell::menu::commands(app.hwnd, ids::THIS_PC, &ids_);
        items.extend(drive_shell_items(&native, &ids_));
    }
    if pinned {
        items.push(item(
            tr("menu.unpin-home"),
            Icon::Command("windows.unpinfromhome"),
            Action::Pin(ids_.clone(), false),
        ));
    } else {
        items.push(
            item(
                tr("menu.pin-home"),
                Icon::Command("windows.pintohome"),
                Action::Pin(ids_.clone(), true),
            )
            .disabled(!node.is_fs),
        );
    }
    if drive {
        items.push(item(
            tr("context.pin-start"),
            Icon::Command("PinToStartScreen"),
            Action::PinToStart(ids_.clone()),
        ));
    }
    if node.is_fs && !drive {
        items.push(item(
            tr("context.pin-start"),
            Icon::Command("PinToStartScreen"),
            Action::PinToStart(ids_.clone()),
        ));
        let formats = vec![
            item(
                tr("context.compress.zip"),
                Icon::None,
                Action::Zip(ids_.clone()),
            ),
            item(
                tr("context.compress.7z"),
                Icon::None,
                Action::Archive(ids_.clone(), ".7z"),
            ),
            item(
                tr("context.compress.tar"),
                Icon::None,
                Action::Archive(ids_.clone(), ".tar"),
            ),
            item(
                tr("context.compress.options"),
                Icon::None,
                Action::Archive(ids_.clone(), ""),
            ),
        ];
        items.push(
            item(
                tr("context.compress"),
                Icon::Command("Windows.Zip.Action"),
                Action::None,
            )
            .submenu(formats),
        );
        items.push(
            item(
                tr("context.copy-as-path"),
                Icon::Command("windows.copyaspath"),
                Action::CopyPath(ids_.clone()),
            )
            .shortcut("Ctrl+Shift+C"),
        );
        let new = new_items(app)
            .into_iter()
            .map(|mut item| {
                if !ids::same(&app.path, &node.id) {
                    if let Action::NewItem(index) = item.action {
                        item.action = Action::NewItemIn(node.id.clone(), index);
                    }
                }
                item
            })
            .collect::<Vec<_>>();
        if !new.is_empty() {
            items.push(
                item(
                    tr("cmd.new"),
                    Icon::Command("windows.newitem"),
                    Action::None,
                )
                .submenu(new),
            );
        }
    }
    items.push(
        item(
            tr("menu.properties"),
            Icon::Command("windows.properties"),
            Action::Properties(ids_.clone()),
        )
        .shortcut("Alt+Enter"),
    );
    if node.is_fs && !drive {
        push_app_commands(&mut items, &ids_, false);
    }
    items.push(separator());
    items.push(item(
        tr("context.more-options"),
        Icon::Command("expandtoclassic"),
        Action::ShellMenu(ids_.clone(), x, y),
    ));
    let buttons = if node.is_fs {
        vec![button(
            tr("context.copy"),
            Icon::Command("windows.copy"),
            Action::CopyItems(ids_),
        )]
    } else {
        Vec::new()
    };
    Menu {
        owner: if drive { "nav-drive" } else { "nav" },
        items,
        buttons,
    }
}

pub struct NavTarget {
    pub node: crate::shell::nav::NavNode,
    pub row: usize,
    pub in_pins: bool,
    pub expanded: bool,
}

pub fn history_items(entries: impl IntoIterator<Item = (usize, String)>) -> Vec<Item> {
    entries
        .into_iter()
        .map(|(index, path)| {
            item(
                &listing::display_name_of(&path),
                Icon::None,
                Action::History(index),
            )
        })
        .collect()
}

pub fn address_items(address: String) -> Vec<Item> {
    vec![
        item(
            tr("address.copy"),
            Icon::None,
            Action::CopyText(address.clone()),
        ),
        item(
            tr("address.copy-text"),
            Icon::None,
            Action::CopyText(address),
        ),
        item(tr("address.edit"), Icon::None, Action::AddressCommand(5)),
        item(
            tr("address.delete-history"),
            Icon::None,
            Action::ClearAddressHistory,
        ),
    ]
}

pub fn address_text_items(state: EditorState) -> Vec<Item> {
    let mut items = Vec::new();
    for (label, command, glyph, shortcut, offered) in [
        ("context.cut", 1, "\u{E8C6}", "Ctrl+X", state.selection),
        ("context.copy", 2, "\u{E8C8}", "Ctrl+C", state.selection),
        (
            "context.paste",
            3,
            "\u{E77F}",
            "Ctrl+V",
            state.paste || !state.nonempty,
        ),
        ("address.select-all", 4, "", "Ctrl+A", state.nonempty),
    ] {
        if offered {
            items.push(
                item(tr(label), Icon::None, Action::AddressCommand(command))
                    .glyph(glyph)
                    .shortcut(shortcut)
                    .disabled(command == 3 && !state.paste),
            );
        }
    }
    items
}

pub fn collection_items(selected: usize) -> Vec<Item> {
    let mut items = Vec::new();
    for (index, (label, icon)) in
        crate::shell::extmenu::store_subcommands("Windows.GalleryFilterCommand")
            .into_iter()
            .enumerate()
    {
        if index == 1 {
            items.push(separator());
        }
        items.push(
            item(
                &label,
                icon.as_ref().map(pixels).unwrap_or_default(),
                Action::GalleryFilter(index),
            )
            .bullet(index == selected),
        );
    }
    items.push(separator());
    items.push(item(
        &crate::i18n::system_string("Windows.UI.FileExplorer.dll", 51859),
        Icon::Command("windows.gallerymanagelocationscommand"),
        Action::GalleryManage,
    ));
    items
}

pub struct EditorState {
    pub selection: bool,
    pub nonempty: bool,
    pub paste: bool,
}

pub fn group_context_items(row: usize, expansion: super::group::Expansion) -> Vec<Item> {
    let collapsed = matches!(expansion, super::group::Expansion::Collapsed);
    let first = if collapsed {
        item(
            tr("context.expand-group"),
            Icon::Command("windows.expand"),
            Action::SetGroupOpen(row, true),
        )
        .shortcut(tr("key.right"))
    } else {
        item(
            tr("context.collapse-group"),
            Icon::Command("windows.collapse"),
            Action::SetGroupOpen(row, false),
        )
        .shortcut(tr("key.left"))
    };
    vec![
        first,
        item(
            tr("context.expand-all"),
            Icon::Command("windows.expandall"),
            Action::SetGroupsOpen(true),
        ),
        item(
            tr("context.collapse-all"),
            Icon::Command("windows.collapseall"),
            Action::SetGroupsOpen(false),
        ),
        separator(),
    ]
}

fn search_keyword(key: &windows::Win32::Foundation::PROPERTYKEY) -> String {
    crate::shell::columns::describe(key)
        .map(|c| c.label.to_lowercase().replace(' ', ""))
        .unwrap_or_default()
}

fn query_word(id: u32) -> String {
    crate::i18n::system_string("propsys.dll", id)
        .split('|')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn search_options_items(options: crate::shell::search::Options, query: &str) -> Vec<Item> {
    use windows::Win32::Storage::EnhancedStorage::{PKEY_DateModified, PKEY_Kind, PKEY_Size};
    let shell32 = |id: u32| crate::i18n::system_string("shell32.dll", id);
    let (date_key, kind_key, size_key) = (
        search_keyword(&PKEY_DateModified),
        search_keyword(&PKEY_Kind),
        search_keyword(&PKEY_Size),
    );
    let value = |label: &str, key: &str, value: String| {
        let on = query.contains(&format!("{key}:{value}"));
        item(
            label,
            Icon::None,
            Action::SearchRefine(key.to_string(), value),
        )
        .bullet(on)
    };
    let dates: Vec<Item> = [
        (106, 39260),
        (105, 39261),
        (500, 39265),
        (104, 39266),
        (501, 39270),
        (502, 39271),
        (503, 39275),
        (504, 39276),
    ]
    .iter()
    .map(|&(label, word)| {
        value(
            &crate::i18n::system_string("propsys.dll", label),
            &date_key,
            query_word(word),
        )
    })
    .collect();
    let kinds: Vec<Item> = crate::shell::search::enum_values("System.Kind")
        .into_iter()
        .map(|(kind, label)| value(&label, &kind_key, kind))
        .collect();
    let sizes: Vec<Item> = crate::shell::search::enum_values("System.Size")
        .into_iter()
        .enumerate()
        .map(|(i, (_, label))| value(&label, &size_key, query_word(39220 + i as u32)))
        .collect();
    let has = |key: &str| !key.is_empty() && query.contains(&format!("{key}:"));
    vec![
        item(
            &shell32(37539),
            Icon::Command("windows.searchoptiondeep"),
            Action::SearchCurrentFolderOnly(false),
        )
        .bullet(!options.current_folder_only)
        .tip("Windows.SearchOptionDeep"),
        item(
            &shell32(37541),
            Icon::Command("windows.searchoptionshallow"),
            Action::SearchCurrentFolderOnly(true),
        )
        .bullet(options.current_folder_only)
        .tip("Windows.SearchOptionShallow"),
        separator(),
        item(
            &shell32(37545),
            Icon::Command("windows.searchfilterdate"),
            Action::None,
        )
        .submenu(dates)
        .bullet(has(&date_key))
        .tip("Windows.SearchFilterDate"),
        item(
            &shell32(37547),
            Icon::Command("windows.searchfilterkind"),
            Action::None,
        )
        .submenu(kinds)
        .bullet(has(&kind_key))
        .tip("Windows.SearchFilterKind"),
        item(
            &shell32(37549),
            Icon::Command("windows.searchfiltersize"),
            Action::None,
        )
        .submenu(sizes)
        .bullet(has(&size_key))
        .tip("Windows.SearchFilterSize"),
        separator(),
        item(
            &shell32(37559),
            Icon::Command("windows.searchoptionsystem"),
            Action::SearchToggle(0),
        )
        .checked(options.system_files)
        .tip("Windows.SearchOptionSystem"),
        item(
            &shell32(37561),
            Icon::Command("windows.searchoptioncompressed"),
            Action::SearchToggle(1),
        )
        .checked(options.zipped_folders)
        .tip("Windows.SearchOptionCompressed"),
        item(
            &shell32(37557),
            Icon::Command("windows.searchoptioncontents"),
            Action::SearchToggle(2),
        )
        .checked(options.file_contents)
        .tip("Windows.SearchOptionContents"),
        separator(),
        item(
            &shell32(37535),
            Icon::Command("windows.changeindexedlocations"),
            Action::Launch("shell:::{87D66A43-7B11-4A28-9811-C86EE395ACF7}".into()),
        )
        .tip("Windows.ChangeIndexedLocations"),
    ]
}
