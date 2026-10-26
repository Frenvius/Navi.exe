//! Application state and the glue between the shell backend and the Slint UI.

mod actions;
mod default_manager;
mod settings;
mod choose;
mod clipboard;
mod columns;
mod file_icons;
mod filter;
mod focus;
pub mod drop;
pub mod format;
mod grid;
mod drives;
pub mod group;
pub mod ids;
pub mod images;
mod menus;
mod popup;
mod scroll;
mod single_click;
mod search_stream;
mod network;
mod preferences;
pub mod gallery;
mod native_view;
mod cloud;
mod jumplist;
pub mod session;
pub mod entry;
pub mod navtree;
mod navpane;
pub mod sort;
mod tabs;
pub mod tab_transfer;
mod undo;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use slint::platform::{Key, PointerEventButton};
use slint::private_unstable_api::re_exports::PointerEventKind;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::Win32::Foundation::HWND;

use crate::i18n::tr;
use crate::shell::icons::Rgba;
use crate::shell::listing::{self, Crumb, ShellEntry};
use crate::shell::nav::{self, NavNode};
use crate::{AppWindow, Crumb as UiCrumb, Entry, Icons, ListSlot, NavItem, Shell, TabItem};
use images::Images;
use navtree::{NavTree, RowRef};
use sort::{Sort, SortKey};

pub const VIEW_DETAILS: i32 = 6;
pub const VIEW_LARGE: i32 = 2;

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

const ROW_PITCH: f32 = 21.0;
/// Without "Exibição compacta" rows are 28px apart, their highlight 24px tall.
const ROW_PITCH_ROOMY: f32 = 28.0;
const ROW_TOP: f32 = 6.0;
/// Grouped, the first header sits 2px below the column headers, and each header takes 32px.
const GROUPED_TOP: f32 = 2.0;
const GROUP_HEADER: f32 = 32.0;
const GROUP_FOLDED: f32 = 30.0;
const GROUP_SUBTITLE: char = '\u{1f}';
const ROW_LEFT: f32 = 14.0;
/// The details pane's title line as the real breaks it: "tres linhas de nome aqui para"
/// (214.8px in DirectWrite) fits, "workspace-projects-slint-win-" (216.5px) doesn't. Without
/// an icon it has the icon's 40px more ("txt datademodificação:hoje (0", 216.7px, fits).
const PANE_TITLE_WIDTH: f32 = 216.0;
/// The details pane's thumbnail size: its picture area is 299 wide (and 220 tall).
const PANE_THUMBNAIL: u32 = 299;
const PANE_ICON_ROOM: f32 = 40.0;
const TYPEAHEAD_RESET_MS: u128 = 1000;

#[derive(Clone)]
pub struct Tab {
    pub history: Vec<String>,
    /// The header filter of each history step (a filter is a step of its own).
    pub filters: Vec<Option<filter::Filter>>,
    pub libraries: Vec<Option<crate::shell::library::Context>>,
    pub index: usize,
    pub title: String,
    pub icon: u32,
    pub selection: TabSelection,
}

/// Selection belongs to a tab, not to whichever folder view was last painted.
#[derive(Clone, Default)]
pub struct TabSelection {
    pub path: String,
    pub selected: HashSet<String>,
    pub anchor: Option<String>,
    pub focused: Option<String>,
}

impl Tab {
    fn new(path: &str) -> Tab {
        Tab { history: vec![ids::canonical(path)], filters: vec![None], libraries: vec![None], index: 0, title: String::new(), icon: 0, selection: Default::default() }
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

    /// A new step after the current one, dropping those ahead.
    pub fn push(&mut self, path: String, filter: Option<filter::Filter>) {
        let library = self.library().filter(|c| c.contains(&path));
        self.history.truncate(self.index + 1);
        self.filters.resize(self.history.len(), None);
        self.libraries.resize(self.history.len(), None);
        self.history.push(path);
        self.filters.push(filter);
        self.libraries.push(library);
        self.index = self.history.len() - 1;
    }
}

/// What to do with a freshly listed folder (select a created item, start renaming it).
pub struct AfterLoad {
    pub select: String,
    pub rename: bool,
}

pub struct State {
    pub tabs: Vec<Tab>,
    pub current: usize,

    pub path: String,
    pub entries: Vec<ShellEntry>,
    /// Indices into `entries`, in display order (sorted and filtered).
    pub order: Vec<usize>,
    pub loaded: bool,
    pub error: String,
    pub crumbs: Vec<Crumb>,
    pub crumb_widths: Vec<f32>,
    pub trail_width: f32,

    pub selected: HashSet<String>,
    pub anchor: Option<String>,
    pub focused: Option<String>,
    pub pending_narrow: Option<usize>,
    pub band_base: HashSet<String>,
    pub band_ctrl: bool,
    pub typeahead: (Instant, String),

    pub sort: Sort,
    pub sort_extra: Vec<Sort>,
    /// The search's hidden Rank remains the primary key until the user chooses a sort.
    pub search_ranked: bool,
    pub group: Option<group::Group>,
    /// Per row (in display order): the header of the group it starts, or empty.
    pub headers: Vec<String>,
    pub library_origins: Vec<crate::shell::library::Origin>,
    pub library_empty: Vec<crate::EmptyLibraryGroup>,
    pub gallery: gallery::Gallery,
    /// Collapsed groups, by header, and the rows they hide.
    pub collapsed: HashSet<String>,
    pub folded: Vec<bool>,
    /// The group header with keyboard focus (by its first row).
    pub focused_group: Option<usize>,
    /// Per row: its top within the details list's content.
    pub row_tops: Vec<f32>,
    /// The details list's content height: down to its last row's highlight.
    pub list_extent: f32,
    /// The rows the details list draws (first, past the last).
    pub list_window: (usize, usize),
    /// Per row, the selection and ghosting its entry shows, to update only rows that change.
    pub row_marks: Vec<(bool, bool)>,
    /// What the user changed in this session, per folder, over the folder's saved view.
    pub views: std::collections::HashMap<String, FolderView>,
    /// Values of the on-demand sort key (`sort_texts_key`) by path.
    pub sort_texts: std::collections::HashMap<String, crate::shell::columns::SortValue>,
    /// Tiles and content: each item's property lines (by list and path): label, value, and
    /// how wide they show ("label: value").
    pub fields: std::collections::HashMap<(listing::PropList, String), Vec<Option<(String, String, f32)>>>,
    network_properties: HashMap<String,bool>,
    pub sort_texts_key: Option<SortKey>,
    /// The same for the grouping's key, when it isn't the sort's (`group_texts_key`).
    pub group_texts: std::collections::HashMap<String, crate::shell::columns::SortValue>,
    pub group_texts_key: Option<SortKey>,
    pub query: String,
    pub view_mode: i32,
    /// The folder's view changed in this visit: it's written to its bag when left.
    pub view_dirty: bool,
    /// The icon views' icon size (Ctrl+wheel steps it between the modes' own sizes).
    pub icon_size: u32,
    pub compact: bool,
    pub check_boxes: bool,
    pub cut: HashSet<String>,
    pub undo: Vec<undo::Undo>,
    /// Renames undone, for Ctrl+Y; a new operation clears it.
    pub redo: Vec<undo::Undo>,
    /// The details view's columns, Name first; the header menu's list (the folder type's);
    /// and the values read for the columns the listing doesn't have.
    pub cols: Vec<columns::Col>,
    pub menu_columns: Vec<windows::Win32::Foundation::PROPERTYKEY>,
    pub col_texts: columns::ColumnTexts,
    /// Ctrl+Shift+E is opening the tree a level at a time, waiting for children to load.
    pub nav_expanding: bool,
    pub nav_expand_current: bool,
    pub nav_options: crate::shell::nav_options::Options,
    pub nav_request: u64,
    pub can_paste: bool,
    pub click_settings: listing::ExplorerSettings,
    pub click_view_path: String,
    /// The window's "Opções de pesquisa" (subfolders or not, system files, contents).
    pub search_options: crate::shell::search::Options,
    search_name_hits: HashMap<String,crate::Hit>,
    /// The location's header filter (the tab's history step has it), and the filter open.
    pub filter: Option<filter::Filter>,
    pub filter_menu: Option<filter::FilterMenu>,
    /// Home's "Filtro" (0: all file types; see `home_kind`).
    pub home_filter: u8,
    pub renaming: Option<String>,
    pub after_load: Option<AfterLoad>,
    /// Until when a change of the list's height (its horizontal bar coming in once the new
    /// folder's columns are laid out) brings the focused item back into view.
    pub reveal_until: Option<std::time::Instant>,
    /// The location we just left; Explorer selects it when it is a child of the new one.
    pub came_from: Option<String>,

    pub nav: NavTree,
    /// The tree row the user picked (clicked, or moved to with the keyboard).
    pub nav_pick: Option<navpane::NavPick>,
    pub nav_typeahead: (Instant, String),
    /// The tree row a drag has been held over, and since when (it opens after a while).
    pub drag_hold: Option<(RowRef, Instant)>,
    pub drag_scrolled: Option<Instant>,
    /// Each crumb's top-left in the window, as laid out.
    pub crumb_spots: Vec<(f32, f32)>,
    pub nav_pane: bool,
    /// The pane's width (the splitter moves it).
    pub nav_width: f32,
    pub details_pane: bool,
    pub preview_pane: bool,
    /// The pane the command bar button toggles: the last one shown.
    pub preview_kind: bool,
    pub viewport: f32,
    pub new_items: Vec<crate::shell::newmenu::NewEntry>,
    /// Paths present before a "Novo" item was created, to find and rename the new one.
    pub before_new: Option<HashSet<String>>,
    pub columns: usize,
    pub grid_viewport: (f32, f32),
    pub drives_rows: Vec<drives::NavRow>,
    pub drives_unit_starts: Vec<f32>,
    pub grid_columns: usize,
    pub grid_column_major: bool,
    pub label_lines: Vec<u8>,
    /// List view: each name's width, which sizes its column.
    pub label_widths: Vec<f32>,
    /// Grouped list: each group header's text width, by the row it starts.
    pub header_widths: Vec<f32>,
    /// Names already measured: wrapped labels per GDI pixel width, and widths
    /// on one line, kept across folders and views.
    pub name_lines: HashMap<u32, HashMap<String, Rc<crate::shell::text::IconLabel>>>,
    pub name_pending: HashSet<(u32, String)>,
    pub name_widths: HashMap<String, f32>,
    /// The rows handed to the UI to measure.
    pub measuring: HashSet<usize>,
    pub cells: Vec<[f32; 4]>,
    /// Row-major icon views: each row (and the gap under it) and each group header, in
    /// content order, which the scroll steps line up.
    pub grid_items: Vec<(f32, f32)>,
    /// Their content height: down to the last row.
    pub grid_extent: f32,
    /// The span (down, or across for the list) whose cells the icon views draw.
    pub grid_window: (f32, f32),
    pub request: u64,
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

pub struct App {
    pub ui: slint::Weak<AppWindow>,
    pub hwnd: std::cell::Cell<HWND>,
    pub state: RefCell<State>,
    pub images: RefCell<Images>,
    pub menu: RefCell<Option<menus::OpenMenu>>,
    pub popups: RefCell<popup::Popups>,
    entries: Rc<VecModel<Entry>>,
    list_rows: Rc<VecModel<ListSlot>>,
    gallery_tiles: Rc<VecModel<crate::GalleryTile>>,
    gallery_dates: Rc<VecModel<crate::GalleryDate>>,
    column_heads: Rc<VecModel<crate::ColumnHead>>,
    /// A header cell pressed, and the column drag it may become.
    header_drag: RefCell<Option<columns::HeaderDrag>>,
    grid_slots: Rc<VecModel<i32>>,
    tabs: Rc<VecModel<TabItem>>,
    crumbs: Rc<VecModel<UiCrumb>>,
    nav: Rc<VecModel<NavItem>>,
    reload_timer: slint::Timer,
    preferences_timer: slint::Timer,
    layout_timer: slint::Timer,
    /// Shows the keyboard focus's tip a while after it moved, and which row waits for its
    /// tip text.
    key_tip_timer: slint::Timer,
    key_tip: std::cell::Cell<Option<usize>>,
    pending_thumbs: RefCell<HashSet<(String, u32)>>,
    pending_icons: RefCell<HashSet<String>>,
    known_icons: RefCell<HashMap<String, (u64, u32)>>,
    search_timer: slint::Timer,
    search_pulse_at:std::cell::Cell<Option<Instant>>,
    search_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pending_fields: RefCell<HashSet<(u64,crate::shell::listing::PropList,String)>>,
    preview_timer: slint::Timer,
    gallery_timer: slint::Timer,
    cloud_timer:slint::Timer,
    jump_timer:slint::Timer,
    cloud_worker:RefCell<Option<std::sync::mpsc::Sender<crate::shell::cloud::Request>>>,
    cloud_snapshot:RefCell<crate::shell::cloud::Snapshot>,
    native_browser:RefCell<Option<crate::shell::browser::Browser>>,
    native_rect:std::cell::Cell<windows::Win32::Foundation::RECT>,
    hover_select_timer: slint::Timer,
    hover_anchor: std::cell::Cell<Option<(usize, f32, f32)>>,
    last_single_activation: RefCell<Option<(String, Instant)>>,
    /// A menu item's description: its window and the wait before it shows.
    pub tip_popup: RefCell<Option<crate::TipPopup>>,
    pub tip_timer: slint::Timer,
    tab_drop_layout: std::cell::Cell<drop::TabDropLayout>,
    tab_drop_hover: std::cell::Cell<Option<usize>>,
    tab_drop_pending: std::cell::Cell<Option<(usize, Instant)>>,
    drop_preview: RefCell<Option<crate::DropPreview>>,
    drag_folders:std::cell::Cell<bool>,
    /// The file being previewed and its handler.
    preview: RefCell<Option<(String, crate::shell::preview::Preview)>>,
    /// The preview pane's area, physical pixels in the window.
    pub preview_rect: std::cell::Cell<windows::Win32::Foundation::RECT>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Runs `f` with the application (UI thread only).
pub fn with_app(f: impl FnOnce(&Rc<App>)) {
    let app = APP.with(|a| a.borrow().clone());
    if let Some(app) = app {
        f(&app);
    }
}

pub fn dismiss_menu(app: &Rc<App>) {
    menus::dismiss(app);
}

/// Runs `f` on the UI thread with the application, from any thread.
pub fn post(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

/// Whether a key event's text is the given special key.
pub fn is(text: &str, k: Key) -> bool {
    SharedString::from(k).as_str() == text
}

impl App {
    pub fn new(ui: &AppWindow) -> Rc<App> {
        let app = Rc::new(App {
            ui: ui.as_weak(),
            hwnd: std::cell::Cell::new(HWND::default()),
            state: RefCell::new(State {
                tabs: vec![Tab::new(ids::HOME)],
                current: 0,
                path: String::new(),
                entries: Vec::new(),
                order: Vec::new(),
                loaded: false,
                error: String::new(),
                crumbs: Vec::new(),
                crumb_widths: Vec::new(),
                trail_width: 0.0,
                selected: HashSet::new(),
                anchor: None,
                focused: None,
                pending_narrow: None,
                band_base: HashSet::new(),
                band_ctrl: false,
                typeahead: (Instant::now(), String::new()),
                sort: Sort { key: SortKey::Name, descending: false },
                sort_extra: Vec::new(),
                search_ranked: false,
                group: None,
                headers: Vec::new(),
                library_origins: Vec::new(),
                library_empty: Vec::new(),
                gallery: Default::default(),
                collapsed: HashSet::new(),
                folded: Vec::new(),
                focused_group: None,
                row_tops: Vec::new(),
                list_extent: 0.0,
                list_window: (0, 0),
                row_marks: Vec::new(),
                views: Default::default(),
                sort_texts: Default::default(),
                fields: Default::default(),
                network_properties: Default::default(),
                sort_texts_key: None,
                view_dirty: false,
                group_texts: Default::default(),
                group_texts_key: None,
                query: String::new(),
                view_mode: VIEW_DETAILS,
                icon_size: 16,
                compact: listing::settings().compact,
                check_boxes: listing::settings().check_boxes,
                cut: HashSet::new(),
                undo: Vec::new(),
                redo: Vec::new(),
                cols: Vec::new(),
                menu_columns: Vec::new(),
                col_texts: Default::default(),
                nav_expanding: false,
                nav_expand_current: true,
                nav_options: Default::default(),
                nav_request: 0,
                can_paste: false,
                click_settings: listing::settings(),
                click_view_path: String::new(),
                search_options: Default::default(),
                search_name_hits:HashMap::new(),
                filter: None,
                filter_menu: None,
                home_filter: 0,
                renaming: None,
                after_load: None,
                reveal_until: None,
                came_from: None,
                nav: NavTree::default(),
                nav_pick: None,
                nav_typeahead: (Instant::now(), String::new()),
                drag_hold: None,
                drag_scrolled: None,
                crumb_spots: Vec::new(),
                nav_pane: true,
                nav_width: 196.0,
                details_pane: listing::panes().0,
                preview_pane: listing::panes().1 && !listing::panes().0,
                preview_kind: listing::panes().1 && !listing::panes().0,
                viewport: 400.0,
                new_items: Vec::new(),
                before_new: None,
                columns: 1,
                grid_viewport: (800.0, 500.0),
                drives_rows: Vec::new(),
                drives_unit_starts: Vec::new(),
                grid_columns: 1,
                grid_column_major: false,
                label_lines: Vec::new(),
                label_widths: Vec::new(),
                header_widths: Vec::new(),
                name_lines: HashMap::new(),
                name_pending: HashSet::new(),
                name_widths: HashMap::new(),
                measuring: HashSet::new(),
                grid_items: Vec::new(),
                grid_extent: 0.0,
                grid_window: (0.0, 0.0),
                cells: Vec::new(),
                request: 0,
            }),
            images: RefCell::new(Images::default()),
            menu: RefCell::new(None),
            popups: RefCell::new(popup::Popups::default()),
            entries: Rc::new(VecModel::default()),
            list_rows: Rc::new(VecModel::default()),
            gallery_tiles: Rc::new(VecModel::default()),
            gallery_dates: Rc::new(VecModel::default()),
            column_heads: Rc::new(VecModel::default()),
            header_drag: RefCell::new(None),
            grid_slots: Rc::new(VecModel::default()),
            tabs: Rc::new(VecModel::default()),
            crumbs: Rc::new(VecModel::default()),
            nav: Rc::new(VecModel::default()),
            reload_timer: slint::Timer::default(),
            preferences_timer: slint::Timer::default(),
            layout_timer: slint::Timer::default(),
            key_tip_timer: slint::Timer::default(),
            key_tip: std::cell::Cell::new(None),
            pending_thumbs: RefCell::new(HashSet::new()),
            pending_icons: RefCell::new(HashSet::new()),
            known_icons: RefCell::new(HashMap::new()),
            search_timer: slint::Timer::default(),
            search_pulse_at:std::cell::Cell::new(None),
            search_generation:std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pending_fields:RefCell::new(HashSet::new()),
            preview_timer: slint::Timer::default(),
            gallery_timer: slint::Timer::default(),
            cloud_timer:slint::Timer::default(),
            jump_timer:slint::Timer::default(),
            cloud_worker:RefCell::new(None),
            cloud_snapshot:RefCell::new(Default::default()),
            native_browser:RefCell::new(None),
            native_rect:std::cell::Cell::new(windows::Win32::Foundation::RECT{left:0,top:86,right:1102,bottom:720}),
            hover_select_timer: slint::Timer::default(),
            hover_anchor: std::cell::Cell::new(None),
            last_single_activation: RefCell::new(None),
            tip_popup: RefCell::new(None),
            tip_timer: slint::Timer::default(),
            tab_drop_layout: Default::default(),
            tab_drop_hover: Default::default(),
            tab_drop_pending: Default::default(),
            drop_preview: RefCell::new(None),
            drag_folders:std::cell::Cell::new(false),
            preview: RefCell::new(None),
            preview_rect: Default::default(),
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));

        let shell = ui.global::<Shell>();
        shell.set_entries(ModelRc::from(app.entries.clone()));
        shell.set_list_rows(ModelRc::from(app.list_rows.clone()));
        shell.set_gallery_tiles(ModelRc::from(app.gallery_tiles.clone()));
        shell.set_gallery_dates(ModelRc::from(app.gallery_dates.clone()));
        shell.set_columns(ModelRc::from(app.column_heads.clone()));
        shell.set_grid_slots(ModelRc::from(app.grid_slots.clone()));
        shell.set_tabs(ModelRc::from(app.tabs.clone()));
        shell.set_crumbs(ModelRc::from(app.crumbs.clone()));
        shell.set_nav(ModelRc::from(app.nav.clone()));
        app.load_icons(ui);
        actions::bind(&app, ui);
        app.watch_preferences();
        app.watch_cloud();
        app.watch_jump_list();
        app
    }

    pub fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window is alive")
    }
}

/// The tree's pin, as Explorer draws it (14 x 13 coverage, in #95a0a6).
#[rustfmt::skip]
const TREE_PIN: [u8; 14 * 13] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 31, 222, 177, 14, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 158, 255, 255, 208, 14, 0, 0, 0,
    0, 0, 0, 0, 0, 31, 255, 255, 255, 255, 208, 14, 0, 0,
    0, 0, 0, 14, 130, 239, 255, 255, 255, 255, 255, 191, 0, 0,
    0, 0, 111, 255, 255, 255, 255, 255, 255, 255, 255, 208, 0, 0,
    0, 0, 33, 224, 255, 255, 255, 255, 255, 241, 125, 16, 0, 0,
    0, 0, 0, 33, 241, 255, 255, 255, 158, 16, 0, 0, 0, 0,
    0, 0, 0, 128, 255, 241, 255, 255, 64, 0, 0, 0, 0, 0,
    0, 0, 128, 255, 128, 33, 224, 255, 0, 0, 0, 0, 0, 0,
    0, 64, 255, 128, 0, 0, 33, 97, 0, 0, 0, 0, 0, 0,
    0, 14, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

impl App {
    /// The navigation pane's plates and expando glyphs, as the theme draws them.
    fn load_tree_parts(icons: &Icons, dark_theme: bool) {
        use crate::shell::theme::*;
        let tile_plate = |bytes: &[u8]| {
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(bytes,8,8);
            let mut image = slint::Image::from_rgba8(buffer);
            // One flat pixel beyond the 2px curve keeps filtering from bleeding
            // corner colors into the stretched edges and center.
            image.set_nine_slice_edges(3,3,3,3);
            image
        };
        icons.set_tile_multi(tile_plate(if dark_theme {include_bytes!("../../ui/assets/tile-dark-multi.rgba")} else {include_bytes!("../../ui/assets/tile-light-multi.rgba")}));
        icons.set_tile_focused(tile_plate(if dark_theme {include_bytes!("../../ui/assets/tile-dark-focused.rgba")} else {include_bytes!("../../ui/assets/tile-light-focused.rgba")}));
        icons.set_tile_unfocused(tile_plate(if dark_theme {include_bytes!("../../ui/assets/tile-dark-unfocused.rgba")} else {include_bytes!("../../ui/assets/tile-light-unfocused.rgba")}));
        let image = |part: i32, state: i32, w: i32, h: i32, slice: bool| {
            let Some(rgba) = crate::shell::theme::part(tree(dark_theme), part, state, w, h) else { return slint::Image::default() };
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba.pixels, rgba.width, rgba.height);
            let mut image = slint::Image::from_rgba8_premultiplied(buffer);
            if slice {
                image.set_nine_slice_edges(2, 2, 2, 2);
            }
            image
        };
        icons.set_tree_hot(image(TVP_TREEITEM, TREIS_HOT, 8, 24, true));
        icons.set_tree_selected(image(TVP_TREEITEM, TREIS_SELECTEDNOTFOCUS, 8, 24, true));
        icons.set_tree_hot_selected(image(TVP_TREEITEM, TREIS_HOTSELECTED, 8, 24, true));
        icons.set_tree_drop(image(TVP_TREEITEM, TREIS_SELECTED, 8, 24, true));
        icons.set_tree_closed(image(TVP_GLYPH, GLPS_CLOSED, 16, 16, false));
        icons.set_tree_open(image(TVP_GLYPH, GLPS_OPENED, 16, 16, false));
        icons.set_tree_hot_closed(image(TVP_HOTGLYPH, GLPS_CLOSED, 16, 16, false));
        icons.set_tree_hot_open(image(TVP_HOTGLYPH, GLPS_OPENED, 16, 16, false));
        let pin: Vec<u8> = TREE_PIN.iter().flat_map(|&a| [0x95, 0xa0, 0xa6, a]).collect();
        icons.set_tree_pin(slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&pin, 14, 13)));
    }

    fn load_icons(&self, ui: &AppWindow) {
        crate::shell::extmenu::preload();
        self.load_theme_icons(ui);
    }

    /// Follows the system's light or dark theme: colors, command icons, the tree's parts.
    pub fn apply_theme(&self) {
        let ui = self.ui();
        self.load_theme_icons(&ui);
        self.sync_popup_theme();
        self.sync_view_state();
    }

    fn load_theme_icons(&self, ui: &AppWindow) {
        let dark = crate::shell::theme::dark();
        let system_color=|index| {
            let color=unsafe{windows::Win32::Graphics::Gdi::GetSysColor(index)};
            slint::Color::from_argb_u8(255,color as u8,(color>>8)as u8,(color>>16)as u8)
        };
        ui.global::<crate::Theme>().set_edit_selection(system_color(windows::Win32::Graphics::Gdi::COLOR_HIGHLIGHT));
        ui.global::<crate::Theme>().set_edit_selection_text(system_color(windows::Win32::Graphics::Gdi::COLOR_HIGHLIGHTTEXT));
        ui.global::<crate::Theme>().set_dark(dark);
        let icons = ui.global::<Icons>();
        let mut images = self.images.borrow_mut();
        images.set_dark(dark);
        icons.set_new_item(images.svg("windows.newitem"));
        icons.set_wallpaper(images.svg("windows.setdesktopwallpaper"));
        icons.set_new_folder(images.svg("newfolder"));
        icons.set_cut(images.svg("windows.cut"));
        icons.set_copy(images.svg("windows.copy"));
        icons.set_paste(images.svg("windows.paste"));
        icons.set_rename(images.svg("windows.rename"));
        icons.set_share(images.svg("Windows.ModernShare"));
        icons.set_delete(images.svg("windows.ribbondelete"));
        icons.set_sort(images.svg("SortBy"));
        icons.set_view(images.svg("View"));
        // Explorer's details icon is the one with lines, its preview icon the plain split.
        icons.set_details_pane(images.svg("windows.previewpane"));
        icons.set_preview_pane(images.svg("windows.readingpane"));
        icons.set_filter(images.svg("windows.ribbonfiltercommand"));
        icons.set_search_options(images.svg("SearchAdvancedOptions"));
        icons.set_recycle_empty(images.svg("windows.recyclebin.empty"));
        icons.set_extract_all(images.svg("windows.compressedfile.extract"));
        icons.set_close_search(images.svg("windows.searchclosetab"));
        icons.set_desktop(images.svg("addressbar.desktop"));
        icons.set_home(images.svg("addressbar.home"));
        icons.set_gallery(images.svg("addressbar.gallery"));
        icons.set_network(images.svg("addressbar.network"));
        Self::load_tree_parts(&icons, dark);
        ui.global::<crate::Theme>().set_accent(slint::Color::from_argb_encoded(0xFF00_0000 | nav::accent_fill(dark)));
    }

    // ---------------------------------------------------------------- navigation

    pub fn current_path(&self) -> String {
        let st = self.state.borrow();
        st.tabs[st.current].path().to_string()
    }

    pub fn navigate(self: &Rc<Self>, path: &str) {
        self.navigate_focused(path, true);
    }

    fn navigate_focused(self: &Rc<Self>, path: &str, focus_list: bool) {
        let path = &ids::canonical(path);
        let library = {
            let current = self.current_path();
            if crate::shell::library::is_library(&current) {
                crate::shell::library::origin_for(path, &crate::shell::library::origins(&current))
                    .filter(|o| !ids::same(&o.path, path))
                    .map(|o| crate::shell::library::Context { library: current, origin: o.path })
            } else { None }
        };
        {
            let mut st = self.state.borrow_mut();
            let cur = st.current;
            let tab = &mut st.tabs[cur];
            // The location shown again: a refresh, the keyboard focus going to the list.
            if tab.path() == path {
                drop(st);
                self.reload();
                if focus_list { self.ui().invoke_focus_list(); }
                return;
            }
            tab.push(path.to_string(), None);
            if library.is_some() { tab.libraries[tab.index] = library; }
        }
        self.open_tab_focused(false, focus_list);
    }

    pub fn back(self: &Rc<Self>) {
        let moved = {
            let mut st = self.state.borrow_mut();
            let cur = st.current;
            let tab = &mut st.tabs[cur];
            if tab.index > 0 {
                tab.index -= 1;
                true
            } else {
                false
            }
        };
        if moved {
            self.open_current();
        }
    }

    pub fn forward(self: &Rc<Self>) {
        let moved = {
            let mut st = self.state.borrow_mut();
            let cur = st.current;
            let tab = &mut st.tabs[cur];
            if tab.index + 1 < tab.history.len() {
                tab.index += 1;
                true
            } else {
                false
            }
        };
        if moved {
            self.open_current();
        }
    }

    /// Goes to an entry of the current tab's history (the Back / Forward menus).
    pub fn go_to_history(self: &Rc<Self>, index: usize) {
        let moved = {
            let mut st = self.state.borrow_mut();
            let cur = st.current;
            let tab = &mut st.tabs[cur];
            let ok = index < tab.history.len() && index != tab.index;
            if ok {
                tab.index = index;
            }
            ok
        };
        if moved {
            self.open_current();
        }
    }

    /// Above a top-level location (This PC, Home, a drive...) Explorer goes to the Desktop.
    pub fn up(self: &Rc<Self>) {
        let parent = {
            let st = self.state.borrow();
            let n = st.crumbs.len();
            if st.path.is_empty() {
                None
            } else if n > 1 {
                Some(st.crumbs[n - 2].id.clone())
            } else {
                Some(String::new())
            }
        };
        if ids::is_settings(&self.current_path()) { return; }
        if let Some(parent) = parent {
            self.navigate(&parent);
        }
    }

    pub fn new_tab(self: &Rc<Self>, path: &str) {
        if ids::is_settings(path) {
            let at={let s=self.state.borrow();s.tabs.iter().position(|t| ids::is_settings(t.path()))};
            if let Some(at)=at {self.select_tab(at);return;}
        }
        self.remember_tab_selection();
        {
            let mut st = self.state.borrow_mut();
            st.tabs.push(Tab::new(path));
            st.current = st.tabs.len() - 1;
        }
        self.open_current();
    }

    pub fn select_tab(self: &Rc<Self>, index: usize) {
        self.remember_tab_selection();
        {
            let mut st = self.state.borrow_mut();
            if index >= st.tabs.len() || index == st.current {
                return;
            }
            st.current = index;
        }
        self.open_tab(true);
    }

    pub fn close_tab(self: &Rc<Self>, index: usize) {
        self.remember_tab_selection();
        let reopen = {
            let mut st = self.state.borrow_mut();
            if st.tabs.len() == 1 {
                drop(st);
                let _ = self.ui().hide();
                let _ = slint::quit_event_loop();
                return;
            }
            if index >= st.tabs.len() {
                return;
            }
            st.tabs.remove(index);
            let was_current = index == st.current;
            if index < st.current || st.current >= st.tabs.len() {
                st.current = st.current.saturating_sub(1).min(st.tabs.len() - 1);
            }
            was_current
        };
        if reopen {
            self.open_tab(true);
        } else {
            self.sync_tabs();
        }
    }

    /// "Fechar outras guias": keeps only the tab at `index`.
    pub fn close_other_tabs(self: &Rc<Self>, index: usize) {
        self.remember_tab_selection();
        let reopen = {
            let mut st = self.state.borrow_mut();
            if index >= st.tabs.len() {
                return;
            }
            let keep = st.tabs.swap_remove(index);
            let was_current = st.current == index;
            st.tabs = vec![keep];
            st.current = 0;
            !was_current
        };
        if reopen {
            self.open_tab(true);
        } else {
            self.sync_tabs();
        }
    }

    /// "Fechar guias à direita": closes the tabs after `index`.
    pub fn close_tabs_right(self: &Rc<Self>, index: usize) {
        let reopen = {
            let mut st = self.state.borrow_mut();
            if index + 1 >= st.tabs.len() {
                return;
            }
            st.tabs.truncate(index + 1);
            let moved = st.current > index;
            if moved {
                st.current = index;
            }
            moved
        };
        if reopen {
            self.open_tab(true);
        } else {
            self.sync_tabs();
        }
    }

    /// "Duplicar guia": a copy of the tab, its history too, right after it and shown.
    pub fn duplicate_tab(self: &Rc<Self>, index: usize) {
        if self.state.borrow().tabs.get(index).is_some_and(|t| ids::is_settings(t.path())) { self.open_settings(); return; }
        self.remember_tab_selection();
        {
            let mut st = self.state.borrow_mut();
            let Some(copy) = st.tabs.get(index).cloned() else { return };
            st.tabs.insert(index + 1, copy);
            st.current = index + 1;
        }
        self.open_tab(true);
    }

    pub fn cycle_tab(self: &Rc<Self>, delta: i32) {
        let next = {
            let st = self.state.borrow();
            let n = st.tabs.len() as i32;
            ((st.current as i32 + delta).rem_euclid(n)) as usize
        };
        self.select_tab(next);
    }

    /// Shows the current tab's location (from scratch: listing, crumbs, watcher).
    pub fn open_current(self: &Rc<Self>) {
        self.open_tab(false);
    }

    fn remember_tab_selection(&self) {
        let mut st = self.state.borrow_mut();
        if !st.loaded { return; }
        let selection = TabSelection {
            path: st.path.clone(), selected: st.selected.clone(),
            anchor: st.anchor.clone(), focused: st.focused.clone(),
        };
        let current = st.current;
        st.tabs[current].selection = selection;
    }

    fn open_tab(self: &Rc<Self>, restore_selection: bool) {
        self.open_tab_focused(restore_selection, true);
    }

    fn open_tab_focused(self: &Rc<Self>, restore_selection: bool, focus_list: bool) {
        let path = self.current_path();
        let library_origins = if ids::is_library(&path) { crate::shell::library::origins(&path) } else { Vec::new() };
        self.save_view();
        // The column widths of the folder being left, for when it's shown again.
        let columns = self.shown_columns();
        if !columns.is_empty() {
            self.remember_view(|v| v.columns = Some(columns));
        }
        self.state.borrow_mut().view_dirty = false;
        let gallery_size=if ids::same(&path,ids::GALLERY){Some(crate::shell::viewstate::saved_icon_size(&path).unwrap_or(48))}else{None};
        {
            let mut st = self.state.borrow_mut();
            st.came_from = if restore_selection { None } else { Some(st.path.clone()).filter(|p| !p.is_empty()) };
            st.path = path.clone();
            st.gallery.offset = 0.0;
            st.gallery.rail_position=None;
            st.gallery.motion = None;
            st.gallery.retries.clear();
            if let Some(size)=gallery_size{st.gallery.set_size(size);}
            st.library_origins = library_origins;
            st.entries.clear();
            st.network_properties.clear();
            st.collapsed.clear();
            st.order.clear();
            st.loaded = false;
            st.nav_expanding = false;
            st.error.clear();
            st.selected.clear();
            st.anchor = None;
            st.focused = None;
            let saved = st.tabs[st.current].selection.clone();
            if restore_selection && ids::same(&saved.path, &path) {
                st.selected = saved.selected;
                st.anchor = saved.anchor;
                st.focused = saved.focused;
            }
            st.renaming = None;
            st.query.clear();
            st.filter = st.tabs[st.current].filter();
        }
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        match ids::search_parts(&path) {
            Some((_, query)) => shell.set_search_text(query.into()),
            None => shell.set_search_text(SharedString::new()),
        }
        shell.set_search_hit_stage(0);
        shell.set_network_notice(SharedString::new());
        shell.set_list_scroll_y(0.0);
        shell.set_grid_scroll_x(0.0);
        shell.set_list_scroll_x(0.0);
        self.entries.set_vec(Vec::new());
        self.sync_all();
        self.sync_native_view();
        // The tree scrolls its selection (the location, or its nearest shown ancestor) into view.
        if let Some(row) = self.nav_selected() {
            self.nav_reveal(row, row, false);
        }
        self.watch(&path);
        self.list(path, true);
        if focus_list { ui.invoke_focus_list(); }
    }

    pub fn reload(self: &Rc<Self>) {
        let (path, initial) = {
            let st = self.state.borrow();
            (st.path.clone(), !st.loaded || st.crumbs.is_empty())
        };
        // Namespace notifications can supersede the initial request (notably Network).
        // Its replacement still needs the address chain and saved view.
        self.list(path, initial);
    }

    fn list(self: &Rc<Self>, path: String, with_crumbs: bool) {
        let request = {
            let mut st = self.state.borrow_mut();
            st.request += 1;
            st.request
        };
        self.search_generation.store(request,std::sync::atomic::Ordering::Release);
        self.pending_fields.borrow_mut().clear();
        self.pending_icons.borrow_mut().clear();
        if ids::is_settings(&path) { self.list_settings(); return; }
        if ids::is_search(&path) {
            self.start_search_stream(request,path,with_crumbs);
            return;
        }
        if ids::same(&path,ids::NETWORK)||crate::shell::network::is_computer(&path){self.start_network(request,path,with_crumbs);return;}
        self.ui().global::<Shell>().set_search_busy(false);
        self.ui().global::<Shell>().set_search_indicator(false);
        let known: HashSet<u32> = HashSet::new();
        let search_options = self.state.borrow().search_options;
        let library = { let st = self.state.borrow(); st.tabs[st.current].library() };
        std::thread::spawn(move || {
            crate::shell::com_init();
            let t0 = Instant::now();
            let (result, crumbs) = match ids::search_parts(&path) {
                Some((scope, query)) => {
                    let result = crate::shell::search::search(scope, query, search_options);
                    let icon = result.as_ref().map(|(_, icon)| *icon).unwrap_or(0);
                    let result = result.map(|(entries, _)| entries);
                    let crumbs = if with_crumbs {
                        listing::crumbs(scope).ok().map(|mut chain| {
                            let name = chain.last().map(|c| c.name.clone()).unwrap_or_default();
                            let icon = if icon != 0 { icon } else { chain.last().map(|c| c.icon).unwrap_or(0) };
                            chain.push(Crumb { nav_root: None, id: path.clone(), name: crate::i18n::shell_string(34132, &name), icon, has_subfolders: false });
                            chain
                        })
                    } else {
                        None
                    };
                    (result, crumbs)
                }
                None if crate::shell::browser::handles(&path) => (Ok(Vec::new()),if with_crumbs{listing::crumbs(&path).ok()}else{None}),
                None => (listing::list(&path), if with_crumbs {
                    library.as_ref().and_then(|c| c.crumbs(&path)).or_else(|| listing::crumbs(&path).ok())
                } else { None }),
            };
            let view = if !with_crumbs || path.is_empty() {
                None
            } else if let Some((scope, _)) = ids::search_parts(&path) {
                Some(crate::shell::viewstate::read_search(scope))
            } else if ids::has_own_view(&path) || crate::shell::browser::handles(&path) {
                None
            } else {
                Some(crate::shell::viewstate::read(&path))
            };
            let t1 = t0.elapsed();
            let mut icons: Vec<(u32, Rgba)> = Vec::new();
            let mut seen = known;
            let wanted = result.iter().flatten().map(|e| e.icon).chain(crumbs.iter().flatten().map(|c| c.icon));
            for icon in wanted {
                if seen.insert(icon) {
                    if let Ok(rgba) = crate::shell::icons::icon(icon, 16) {
                        icons.push((icon, rgba));
                    }
                }
            }
            if std::env::var("DEBUG_TIMING").is_ok() {
                eprintln!("list {:?} crumbs+icons {:?} ({} icons)", t1, t0.elapsed() - t1, icons.len());
            }
            post(move |app| {
                let t = Instant::now();
                app.listed(request, path, result, crumbs, icons, view);
                if std::env::var("DEBUG_TIMING").is_ok() {
                    eprintln!("ui {:?}", t.elapsed());
                }
            });
        });
    }

    fn listed(
        self: &Rc<Self>,
        request: u64,
        path: String,
        result: Result<Vec<ShellEntry>, String>,
        crumbs: Option<Vec<Crumb>>,
        icons: Vec<(u32, Rgba)>,
        view: Option<crate::shell::viewstate::ViewState>,
    ) {
        let navigated=crumbs.is_some();
        {
            let mut images = self.images.borrow_mut();
            for (icon, rgba) in &icons {
                images.insert(*icon, 16, rgba);
            }
        }
        if result.is_err() && !ids::is_virtual(&path) && !std::path::Path::new(&path).exists() {
            if request == self.state.borrow().request {
                let mut up = std::path::Path::new(&path).parent();
                while let Some(p) = up.filter(|p| !p.exists()) {
                    up = p.parent();
                }
                let target = up.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| ids::THIS_PC.to_string());
                self.navigate(&target);
            }
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            if request != st.request || path != st.path {
                return;
            }
            st.loaded = true;
            st.fields.clear();
            let settings = listing::settings();
            let old_click=st.click_settings;
            let refresh_font=st.click_view_path!=st.path || old_click.single_click!=settings.single_click;
            st.click_settings = settings;
            if !refresh_font { st.click_settings.underline_hover_only=old_click.underline_hover_only; }
            st.click_view_path=st.path.clone();
            st.check_boxes = settings.check_boxes;
            st.compact = settings.compact;
            match result {
                Ok(mut entries) => {
                    self.apply_known_icons(&mut entries);
                    st.entries = entries;
                    st.error.clear();
                }
                Err(e) => {
                    st.entries.clear();
                    st.error = e;
                }
            }
            let alive: HashSet<&str> = st.entries.iter().map(|e| e.path.as_str()).collect();
            let keep: HashSet<String> = st.selected.iter().filter(|p| alive.contains(p.as_str())).cloned().collect();
            st.selected = keep;
            if let Some(crumbs) = crumbs {
                st.crumbs = crumbs;
                let cur = st.current;
                if let Some(last) = st.crumbs.last().cloned() {
                    st.tabs[cur].title = match ids::search_parts(&st.path) {
                        Some((_, query)) => format!("{} - {}", query.trim(), last.name),
                        None if !ids::is_virtual(&st.path) && listing::settings().full_path => st.path.clone(),
                        None => last.name.clone(),
                    };
                    st.tabs[cur].icon = if ids::is_search(&st.path) { crate::shell::icons::find_icon() } else { last.icon };
                }
            }
            if let Some(before) = st.before_new.take() {
                if let Some(created) = st.entries.iter().find(|e| !before.contains(&e.path)).map(|e| e.path.clone()) {
                    st.undo.push(undo::Undo::Create(created.clone()));
                    st.redo.clear();
                    st.after_load = Some(AfterLoad { select: created, rename: true });
                } else {
                    st.before_new = Some(before);
                }
            }
            if st.after_load.is_none() {
                if let Some(from) = st.came_from.take() {
                    let lower = from.to_lowercase();
                    let leads_back = |path: &str| {
                        let p = path.to_lowercase();
                        ids::same(path, &from) || (!ids::is_virtual(path) && lower.starts_with(&p) && (p.ends_with('\\') || lower[p.len()..].starts_with('\\')))
                    };
                    if let Some(e) = st.entries.iter().find(|e| leads_back(&e.path)) {
                        let p = e.path.clone();
                        st.selected = HashSet::from([p.clone()]);
                        st.anchor = Some(p.clone());
                        st.focused = Some(p);
                    }
                }
            }
            if let Some(after) = st.after_load.take() {
                if let Some(actual) = st.entries.iter().find(|e| e.path == after.select || entry::same(&e.path, &after.select)).map(|e| e.path.clone()) {
                    st.selected = HashSet::from([actual.clone()]);
                    st.anchor = Some(actual.clone());
                    st.focused = Some(actual.clone());
                    if after.rename {
                        st.renaming = Some(actual);
                    }
                } else {
                    st.after_load = Some(after);
                }
            }
        }
        let search_pulse = view.is_some() && ids::is_search(&path) && !self.state.borrow().entries.is_empty();
        if let Some(view) = view {
            self.apply_folder_view(&path, view);
        }
        self.rebuild_order();
        self.load_sort_texts();
        self.sync_all();
        self.resolve_icons();
        let follow=navigated && self.state.borrow().nav_options.expand;
        let focus_ui=self.ui();let focus_shell=focus_ui.global::<Shell>();
        if navigated&&focus_shell.get_focus_zone()=="list"&&!focus_shell.get_menu_open(){focus_ui.invoke_focus_list();}
        if follow {self.nav_follow_current();}
        if search_pulse {
            self.start_search_pulse(request);
        }
        let focused = self.state.borrow().focused.clone();
        if let Some(focused) = focused {
            self.reveal(&focused);
            self.state.borrow_mut().reveal_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
        }
    }

    /// Shows a folder the way it was last left: the session's changes over the view saved
    /// by Explorer (or its folder type's defaults).
    fn apply_folder_view(&self, path: &str, saved: crate::shell::viewstate::ViewState) {
        let session = self.state.borrow().views.get(path).cloned().unwrap_or_default();
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
        // Search results sort by rank first, which only the index gives (ties elsewhere): the
        // next key decides then.
        let saved_sort = saved
            .sort
            .iter()
            .find(|(k, _)| *k != windows::Win32::Storage::EnhancedStorage::PKEY_Search_Rank)
            .map(|(k, d)| Sort { key: sort::SortKey::from_property(k), descending: *d });
        let saved_group = saved.group.map(|g| g.map(|(k, d)| group::Group { key: sort::SortKey::from_property(&k), descending: d }));
        columns::set_folder_labels(saved.labels.clone(), saved.right.clone());
        let (saved_columns, menu_columns) = columns::initial(&saved.columns, &saved.type_columns);
        let columns = session.columns.clone().unwrap_or(saved_columns);
        {
            let mut st = self.state.borrow_mut();
            st.view_mode = session.view_mode.or(saved_mode).unwrap_or(VIEW_DETAILS);
            let saved_size = saved.icon_size.filter(|_| saved.logical_mode == Some(3)).map(|s| s.clamp(16, 256));
            st.icon_size = session.icon_size.or(saved_size).unwrap_or_else(|| grid::mode_icon_size(st.view_mode));
            st.sort = session.sort.or(saved_sort).unwrap_or(Sort { key: SortKey::Name, descending: false });
            st.sort_extra=if ids::is_library(path) && session.sort.is_none() {
                saved.sort.iter().filter(|(key,_)|*key!=windows::Win32::Storage::EnhancedStorage::PKEY_Search_Rank).skip(1)
                    .map(|(key,descending)|Sort{key:SortKey::from_property(key),descending:*descending}).collect()
            }else{Vec::new()};
            st.search_ranked = ids::is_search(path) && session.sort.is_none()
                && saved.sort.first().is_some_and(|(key,_)|*key==windows::Win32::Storage::EnhancedStorage::PKEY_Search_Rank);
            st.group = session.group.or(saved_group).flatten();
        }
        {
            let mut st = self.state.borrow_mut();
            st.menu_columns = menu_columns;
            st.col_texts.clear();
        }
        self.set_columns(columns);
    }

    /// Remembers a change to the current folder's view for when it's shown again.
    pub fn remember_view(&self, change: impl FnOnce(&mut FolderView)) {
        let mut st = self.state.borrow_mut();
        let path = st.path.clone();
        let search = ids::is_search(&path);
        if path.is_empty() || (ids::has_own_view(&path) && !search) {
            return;
        }
        change(st.views.entry(path).or_default());
        // Search view changes live in this window's session, never in a filesystem bag.
        st.view_dirty |= !search;
    }

    /// Writes the current folder's view to its Explorer bag if it changed in this visit, as
    /// Explorer does when it leaves a folder.
    pub fn save_view(&self) {
        if !std::mem::take(&mut self.state.borrow_mut().view_dirty) {return;}
        if ids::is_settings(&self.current_path()) || crate::shell::browser::handles(&self.current_path()){return;}
        let (path,view)=self.view_snapshot();
        crate::shell::viewstate::write(&path, &view);
    }

    pub fn view_snapshot(&self) -> (String, crate::shell::viewstate::ViewState) {
        let columns = self.shown_columns();
        {
            let st = self.state.borrow();
            // FOLDERLOGICALVIEWMODE and the icon size Explorer writes with it.
            if ids::same(&st.path,ids::GALLERY){return(st.path.clone(),crate::shell::viewstate::ViewState{logical_mode:Some(3),icon_size:Some(st.gallery.icon_size),..Default::default()});}
            let (mode, size) = match st.view_mode {
                VIEW_DETAILS => (1, 16),
                5 => (4, 16),
                7 => (2, 48),
                8 => (5, 32),
                _ => (3, st.icon_size),
            };
            let view = crate::shell::viewstate::ViewState {
                logical_mode: Some(mode),
                icon_size: Some(size),
                sort: std::iter::once((st.sort.key.key(),st.sort.descending)).chain(st.sort_extra.iter().map(|s|(s.key.key(),s.descending))).collect(),
                group: Some(st.group.map(|g| (g.key.key(), g.descending))),
                columns: columns.iter().map(|(k, w)| (*k, w.round() as u32)).collect(),
                type_columns: Vec::new(),
                labels: Vec::new(),
                right: Vec::new(),
            };
            (st.path.clone(), view)
        }
    }

    /// The details list's row geometry: its top (within the content) for each row, past
    /// the group headers.
    fn layout_rows(st: &mut State) {
        st.library_empty.clear();
        if Self::library_location_group(st) {
            let pitch = Self::row_pitch(st);
            let mut y = GROUPED_TOP;
            st.row_tops = vec![0.0; st.order.len()];
            for (k, (label, start, end)) in Self::library_ranges(st).into_iter().enumerate() {
                if k > 0 { y += 13.0; }
                let folded = st.collapsed.contains(&label);
                if start == end {
                    st.library_empty.push(crate::EmptyLibraryGroup { label:label.into(), x:0.0, y, width:-1.0, folded });
                    y += if folded {48.0} else {82.0};
                } else {
                    y += Self::group_height(&label, folded);
                    for row in start..end { st.row_tops[row] = y; if !folded {y += pitch;} }
                }
            }
            st.list_extent = y + 1.0;
            return;
        }
        let grouped = !st.headers.is_empty();
        let pitch = Self::row_pitch(st);
        let library=st.headers.iter().any(|h|h.contains(GROUP_SUBTITLE));
        let mut y = if grouped { GROUPED_TOP } else { ROW_TOP };
        st.row_tops = (0..st.order.len())
            .map(|row| {
                let folded = st.folded.get(row).copied().unwrap_or(false);
                if let Some(header)=st.headers.get(row).filter(|h| !h.is_empty()) {
                    // A collapsed group is its header alone, 30px.
                    if library && row>0 {y+=13.0;}
                    y += Self::group_height(header,folded);
                }
                let top = y;
                if !folded {
                    y += pitch;
                }
                top
            })
            .collect();
        // The last row's highlight reaches past its pitch (by a pixel, compact).
        let last_shown = st.folded.last().is_some_and(|f| !f);
        st.list_extent = if last_shown { y - pitch + Self::row_height(st) } else { y };
        if st.order.is_empty() {
            st.list_extent = 0.0;
        }
    }

    fn group_height(label:&str,folded:bool)->f32 {
        (if folded {GROUP_FOLDED}else{GROUP_HEADER}) + if label.contains(GROUP_SUBTITLE){18.0}else{0.0}
    }

    fn library_location_group(st: &State) -> bool {
        !st.library_origins.is_empty() && st.group.is_some_and(|g|g.key.property().is_some_and(|key|crate::shell::library::is_location(&key)))
    }

    fn library_ranges(st: &State) -> Vec<(String,usize,usize)> {
        let mut origins:Vec<_>=st.library_origins.iter().collect();
        if st.group.is_some_and(|g|g.descending) {origins.reverse();}
        origins.into_iter().map(|origin| {
            let label=format!("{}{GROUP_SUBTITLE}{}",origin.name,origin.parent);
            let start=st.order.iter().position(|&i|st.entries[i].library_origin.as_ref().is_some_and(|o|ids::same(&o.path,&origin.path))).unwrap_or(0);
            let count=st.order.iter().filter(|&&i|st.entries[i].library_origin.as_ref().is_some_and(|o|ids::same(&o.path,&origin.path))).count();
            (label,start,start+count)
        }).collect()
    }

    pub fn row_pitch(st: &State) -> f32 {
        if st.compact { ROW_PITCH } else { ROW_PITCH_ROOMY }
    }

    /// The row highlight's height.
    fn row_height(st: &State) -> f32 {
        if st.compact { 22.0 } else { 24.0 }
    }

    pub fn row_top(st: &State, row: usize) -> f32 {
        st.row_tops.get(row).copied().unwrap_or(ROW_TOP + row as f32 * Self::row_pitch(st))
    }

    /// The details row at a content y, if any.
    pub fn row_at(st: &State, y: f32) -> Option<usize> {
        let row = st.row_tops.partition_point(|&top| top <= y).checked_sub(1)?;
        if st.folded.get(row).copied().unwrap_or(false) {
            return None;
        }
        (y < st.row_tops[row] + Self::row_pitch(st)).then_some(row)
    }

    /// The shell's "Novo" entries (the same for every file system folder), read once.
    pub fn load_new_items(self: &Rc<Self>) {
        std::thread::spawn(|| {
            let folder = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\".into());
            let items = crate::shell::newmenu::entries(HWND::default(), &folder).unwrap_or_default();
            post(move |app| app.state.borrow_mut().new_items = items);
        });
    }

    pub fn refresh_nav(self: &Rc<Self>) {
        let request={let mut st=self.state.borrow_mut();st.nav_request+=1;st.nav_request};
        std::thread::spawn(move || {
            crate::shell::com_init();
            let options=crate::shell::nav_options::read();
            let desktop=options.all.then(nav::desktop).flatten();
            let roots = nav::children("").unwrap_or_default();
            let pins = nav::children(ids::QUICK_ACCESS).unwrap_or_default();
            let mut icons: Vec<(u32, Rgba)> = Vec::new();
            let mut seen = HashSet::new();
            for n in roots.iter().chain(pins.iter()).chain(desktop.iter()) {
                if seen.insert(n.icon) {
                    if let Ok(rgba) = crate::shell::icons::icon(n.icon, 16) {
                        icons.push((n.icon, rgba));
                    }
                }
            }
            post(move |app| {
                if app.state.borrow().nav_request!=request {return}
                app.add_icons(&icons);
                let (mode_changed,follow)={
                    let mut st=app.state.borrow_mut();
                    let changed=st.nav_options.all!=options.all;
                    let follow=options.expand && (st.nav.rows.is_empty() || changed || !st.nav_options.expand);
                    st.nav_options=options;
                    if !options.expand && !st.nav_expand_current {st.nav_expanding=false;}
                    st.nav.set_roots(roots,pins,desktop);
                    (changed,follow)
                };
                if mode_changed {app.ui().global::<Shell>().set_nav_scroll_y(0.0);}
                app.sync_nav();
                if follow {app.nav_follow_current();}
            });
        });
    }

    fn add_icons(&self, icons: &[(u32, Rgba)]) {
        let mut images = self.images.borrow_mut();
        for (icon, rgba) in icons {
            images.insert(*icon, 16, rgba);
        }
    }

    pub fn toggle_nav(self: &Rc<Self>, row: usize) {
        self.toggle_nav_revealing(row, true);
    }

    /// A drag held over a closed tree row opens it, without scrolling the tree.
    pub fn expand_nav_quietly(self: &Rc<Self>, row: usize) {
        self.toggle_nav_revealing(row, false);
    }

    fn toggle_nav_revealing(self: &Rc<Self>, row: usize, reveal: bool) {
        let reference = {
            let st = self.state.borrow();
            match st.nav.rows.get(row) {
                Some(r) => r.reference.clone(),
                None => return,
            }
        };
        // Whether the tree's selection is under the row (collapsing takes it there).
        let holds_selection = self
            .nav_selected()
            .and_then(|r| self.state.borrow().nav.rows.get(r).map(|row| navtree::is_inside(&row.reference, &reference)))
            .unwrap_or(false);
        let load = {
            let mut st = self.state.borrow_mut();
            let Some(node) = st.nav.node_mut(&reference) else { return };
            if !node.node.has_children {
                return;
            }
            node.expanded = !node.expanded;
            let expanded = node.expanded;
            let load = expanded && node.children.is_none();
            let id = node.node.id.clone();
            st.nav.flatten();
            (load.then_some(id.clone()), expanded, id)
        };
        let (load, expanded, id) = load;
        // Collapsed over the selection, the tree selects the row and the view goes there.
        if !expanded && holds_selection {
            let at = self.state.borrow().path.clone();
            self.state.borrow_mut().nav_pick = Some(navpane::NavPick { reference: reference.clone(), id: id.clone(), at });
            self.navigate(&id);
            self.ui().invoke_focus_tree();
        }
        self.sync_nav();
        if reveal && expanded && load.is_none() {
            self.nav_reveal_children(&reference);
        }
        if let Some(id) = load {
            std::thread::spawn(move || {
                crate::shell::com_init();
                // The shell's CompareIDs order includes folders before archive streams.
                let kids = nav::children(&id).unwrap_or_default();
                let icons: Vec<(u32, Rgba)> = kids
                    .iter()
                    .map(|k| k.icon)
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .filter_map(|i| crate::shell::icons::icon(i, 16).ok().map(|r| (i, r)))
                    .collect();
                post(move |app| {
                    app.add_icons(&icons);
                    {
                        let mut st = app.state.borrow_mut();
                        if let Some(node) = st.nav.node_mut(&reference) {
                            if ids::same(&node.node.id, &id) {
                                if kids.is_empty() {
                                    node.node.has_children = false;
                                    node.expanded = false;
                                }
                                node.children = Some(kids.into_iter().map(|n| navtree::TreeNode { node: n, expanded: false, children: None }).collect());
                            }
                        }
                        st.nav.flatten();
                    }
                    app.sync_nav();
                    if reveal {
                        app.nav_reveal_children(&reference);
                    }
                    if app.state.borrow().nav_expanding {
                        app.nav_continue_expansion();
                    }
                });
            });
        }
    }

    pub fn nav_node(&self, row: usize) -> Option<(NavNode, bool)> {
        let st = self.state.borrow();
        let r = st.nav.rows.get(row)?;
        let pinned_group = matches!(&r.reference, RowRef::Node { section, .. } if *section == navtree::PINS);
        st.nav.node(&r.reference).map(|n| (n.node.clone(), pinned_group))
    }

    // ---------------------------------------------------------------- watcher

    fn watch(&self, path: &str) {
        if ids::is_settings(path) { crate::watch::watch(None); crate::watch::watch_shell(self.hwnd.get(),None); return; }
        let shell = ids::is_virtual(path) && !ids::is_search(path) && !path.is_empty();
        if crate::shell::library::is_library(path) {
            crate::watch::watch_many(path.to_string(), crate::shell::library::origins(path).into_iter().map(|o| o.path).collect());
        } else {
            crate::watch::watch(if ids::is_virtual(path) || path.is_empty() { None } else { Some(path.to_string()) });
        }
        crate::watch::watch_shell(self.hwnd.get(), shell.then_some(path));
    }

    pub fn fs_changed(self: &Rc<Self>, path: String) {
        if path != self.state.borrow().path {
            return;
        }
        let weak = Rc::downgrade(self);
        self.reload_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(150), move || {
            if let Some(app) = weak.upgrade() {
                app.reload();
            }
        });
    }

    // ---------------------------------------------------------------- folder view model

    pub fn rebuild_order(&self) {
        let mut st = self.state.borrow_mut();
        let needle = st.query.trim().to_lowercase();
        let today = group::Today::now();
        let home_filter = if ids::same(&st.path, ids::HOME) { st.home_filter } else { 0 };
        let mut order: Vec<usize> = (0..st.entries.len())
            .filter(|&i| needle.is_empty() || st.entries[i].name.to_lowercase().contains(&needle))
            .filter(|&i| home_filter == 0 || Self::home_kind(&st.entries[i], home_filter))
            .filter(|&i| st.filter.as_ref().is_none_or(|f| filter::passes(&st, f, &st.entries[i], &today)))
            .collect();
        let folders_first = !ids::is_virtual(&st.path) || ids::is_search(&st.path) || ids::is_library(&st.path) || crate::shell::archive::root(&st.path).is_some();
        sort::order(&st.entries, &mut order, st.sort, &st.sort_extra, &st.sort_texts, folders_first,ids::is_search(&st.path));
        if st.search_ranked { order.sort_by_key(|&i|!st.entries[i].is_dir); }
        let mut headers = Vec::new();
        if Self::drives_view(&st) {
            // Devices first, then network locations, each in display order.
            order.sort_by_key(|&i| if Self::is_device(&st.entries[i]) { 0 } else { 1 });
        } else if let Some(g) = st.group {
            // Groups in their order; the sort holds within each.
            let values = if g.key == st.sort.key { &st.sort_texts } else { &st.group_texts };
            // This PC groups its items by kind as "Dispositivos e unidades" and "Locais de rede".
            let this_pc = ids::same(&st.path, ids::THIS_PC) && g.key == SortKey::Type;
            let buckets: std::collections::HashMap<usize, group::Bucket> = order
                .iter()
                .map(|&i| {
                    let e = &st.entries[i];
                    let bucket = if this_pc {
                        let device = Self::is_device(e);
                        group::Bucket { rank: if device { 0 } else { 1 }, label: tr(if device { "tiles.drives" } else { "tiles.network" }).to_string() }
                    } else if g.key.property().is_some_and(|key|crate::shell::library::is_location(&key)) && e.library_origin.is_some() {
                        let origin=e.library_origin.as_ref().unwrap();
                        group::Bucket {rank:st.library_origins.iter().position(|o|ids::same(&o.path,&origin.path)).unwrap_or(0) as i32,label:format!("{}{GROUP_SUBTITLE}{}",origin.name,origin.parent)}
                    } else {
                        group::bucket(g.key, e, &today, values.get(&e.path))
                    };
                    (i, bucket)
                })
                .collect();
            order.sort_by(|a, b| {
                let ord = buckets[a].cmp(&buckets[b]);
                if g.descending { ord.reverse() } else { ord }
            });
            let mut last: Option<&group::Bucket> = None;
            for i in &order {
                let b = &buckets[i];
                headers.push(if last != Some(b) { b.label.clone() } else { String::new() });
                last = Some(b);
            }
        }
        st.order = order;
        if ids::same(&st.path,ids::GALLERY) {
            let mut order=std::mem::take(&mut st.order);
            order.sort_by(|&a,&b|st.entries[b].photo.unwrap_or_default().date.cmp(&st.entries[a].photo.unwrap_or_default().date).then_with(||st.entries[a].path.to_lowercase().cmp(&st.entries[b].path.to_lowercase())));
            if let Some(root)=st.gallery.filter.as_ref() {let root=root.to_lowercase();order.retain(|&i|{let path=st.entries[i].path.to_lowercase();path==root||path.strip_prefix(&root).is_some_and(|tail|tail.starts_with('\\'))});}
            st.order=order;
        }
        st.headers = headers;
        let mut fold = false;
        st.folded = (0..st.order.len())
            .map(|row| {
                if let Some(h) = st.headers.get(row).filter(|h| !h.is_empty()) {
                    fold = st.collapsed.contains(h);
                }
                fold
            })
            .collect();
        Self::layout_rows(&mut st);
    }

    fn entry_row(&self, st: &State, e: &ShellEntry, slot: i32) -> Entry {
        let drives = Self::drives_view(st);
        let details=Self::details_list(st);
        let icon = self.images.borrow_mut().icon(e.icon, 16);
        // This PC's folder shortcuts (Network Shortcuts) have no link overlay in the
        // native items view, unlike ordinary .lnk files in file-system folders.
        let big_icon = if drives { self.images.borrow_mut().icon(e.icon & 0x00ff_ffff, 48) } else { Default::default() };
        let (usage, low, meta) = if e.total > 0 {
            let used = (e.total - e.free.min(e.total)) as f32 / e.total as f32;
            // propsys.dll's "%1!ls! livre(s) de %2!ls!".
            let meta = crate::i18n::system_format("propsys.dll", 195, &[&format::short_size(e.free), &format::short_size(e.total)]);
            // The bar turns red past 90% full, in the shell's whole percent (System.PercentFull,
            // rounded: 90.9% is 91).
            let percent = ((e.total - e.free.min(e.total)) as u128 * 100 + e.total as u128 / 2) / e.total as u128;
            (used, percent > 90, meta)
        } else {
            (0.0, false, String::new())
        };
        Entry {
            name_style: if !st.click_settings.compressed_color {0}else if e.file_attributes & 0x4000 != 0 {2}else if e.file_attributes & 0x800 != 0 {1}else{0},
            folder: e.is_dir,
            name: e.name.as_str().into(),
            stacked_collapsed: e.name.as_str().into(),
            date: if details {format::modified(e.modified).into()} else {Default::default()},
            kind: e.type_name.as_str().into(),
            size: if !details || e.is_dir || !e.is_fs { SharedString::new() } else { format::size_kb(e.size).into() },
            cells: if details {Self::cells(st,e)} else {Default::default()},
            lead: if details {Self::lead(st,e)} else {Default::default()},
            kind_tile: if e.is_dir { SharedString::new() } else { e.type_name.as_str().into() },
            long_size: if !details || e.is_dir || !e.is_fs { SharedString::new() } else { format::short_size(e.size).into() },
            icon,
            selected: st.selected.contains(&e.path),
            ghost: e.hidden || st.cut.contains(&e.path),
            big_icon,
            group: if Self::is_device(e) { 0 } else { 1 },
            slot,
            usage,
            low,
            meta: meta.into(),
            hit: Self::hit(st, e),
            ..Default::default()
        }
    }

    /// Search results mark where the name matches the query, as Explorer does: the x and
    /// width of the match in the content view's 15px name (empty without a match).
    fn hit(st: &State, e: &ShellEntry) -> crate::Hit {
        if let Some(hit)=st.search_name_hits.get(&e.path){return hit.clone();}
        let Some((_, query)) = ids::search_parts(&st.path) else { return Default::default() };
        Self::name_hit(&e.name,query)
    }

    fn name_hit(original:&str,query:&str)->crate::Hit {
        let (name, query) = (original.to_lowercase(), query.trim().to_lowercase());
        let Some(at) = (!query.is_empty()).then(|| name.find(&query)).flatten() else { return Default::default() };
        // Lowercasing can change byte lengths; map back by characters.
        let start = name[..at].chars().count();
        let len = query.chars().count();
        let prefix: String = original.chars().take(start).collect();
        let hit: String = original.chars().skip(start).take(len).collect();
        let w = crate::shell::columns::widths_at([prefix.as_str(), hit.as_str()], 15);
        crate::Hit { x: w[0], width: w[1] }
    }

    /// This PC shows drives as tiles grouped into devices and network locations.
    /// Whether a file is of the kind Home's "Filtro" picked: 1 Word, 2 Excel, 3 PowerPoint,
    /// 4 pictures, 5 videos, 6 PDF (by what the system perceives the type as, for the last).
    fn home_kind(e: &ShellEntry, kind: u8) -> bool {
        if e.is_dir {
            return false;
        }
        let ext = std::path::Path::new(&e.path).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        let any = |list: &[&str]| list.contains(&ext.as_str());
        match kind {
            1 => any(&["doc", "docx", "docm", "dot", "dotx", "dotm", "rtf", "odt"]),
            2 => any(&["xls", "xlsx", "xlsm", "xlsb", "xlt", "xltx", "xltm", "csv", "ods"]),
            3 => any(&["ppt", "pptx", "pptm", "pps", "ppsx", "ppsm", "pot", "potx", "potm", "odp"]),
            4 | 5 => crate::shell::assoc::perceived(&ext) == if kind == 4 { "image" } else { "video" },
            6 => ext == "pdf",
            _ => true,
        }
    }

    /// This PC in Blocos: its drives as tiles, under their groups.
    pub fn drives_view(st: &State) -> bool {
        ids::same(&st.path, ids::THIS_PC) && st.view_mode == 7
    }

    fn is_device(e: &ShellEntry) -> bool {
        ids::is_drive_root(&e.path) || !e.is_fs
    }

    pub fn sync_entries(&self) {
        let st = self.state.borrow();
        let mut slots = [0i32; 2];
        let rows: Vec<Entry> = st
            .order
            .iter()
            .enumerate()
            .map(|(row, &i)| {
                let e = &st.entries[i];
                let g = if Self::is_device(e) { 0 } else { 1 };
                let slot = slots[g];
                slots[g] += 1;
                let mut entry = self.entry_row(&st, e, slot);
                entry.header = st.headers.get(row).cloned().unwrap_or_default().into();
                entry.folded = st.folded.get(row).copied().unwrap_or(false);
                entry.collapsed = entry.folded;
                entry
            })
            .collect();
        let marks = rows.iter().map(|r| (r.selected, r.ghost)).collect();
        self.entries.set_vec(rows);
        drop(st);
        self.state.borrow_mut().row_marks = marks;
        let st = self.state.borrow();
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        shell.set_drives_view(Self::drives_view(&st));
        shell.set_grouped(!st.headers.is_empty());
        shell.set_icons_view(Self::icons_spec(&st).is_some());
        shell.set_group_counts(ModelRc::from(Rc::new(VecModel::from(slots.to_vec()))));
        shell.set_list_extent(st.list_extent);
        shell.set_empty_library_groups(ModelRc::new(VecModel::from(st.library_empty.clone())));
        drop(st);
        self.layout_drives();
        self.sync_list_window(true);
    }

    /// Updates only the rows whose selection or ghosting changed.
    pub fn sync_selection(&self) {
        let mut st = self.state.borrow_mut();
        let changed: Vec<(usize, (bool, bool))> = st
            .order
            .iter()
            .enumerate()
            .filter_map(|(row, &i)| {
                let e = &st.entries[i];
                let mark = (st.selected.contains(&e.path), e.hidden || st.cut.contains(&e.path));
                (st.row_marks.get(row) != Some(&mark)).then_some((row, mark))
            })
            .collect();
        for &(row, mark) in &changed {
            if let Some(slot) = st.row_marks.get_mut(row) {
                *slot = mark;
            }
        }
        drop(st);
        for (row, (selected, ghost)) in changed {
            if let Some(mut data) = self.entries.row_data(row) {
                data.selected = selected;
                data.ghost = ghost;
                self.entries.set_row_data(row, data);
            }
        }
        self.sync_view_state();
        self.sync_drives_scrollbar();
        if ids::same(&self.current_path(),ids::GALLERY) {self.layout_gallery();}
    }

    /// Everything derived from the selection and location besides the rows themselves.
    pub fn sync_view_state(&self) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let st = self.state.borrow();
        let picked: Vec<&ShellEntry> = st.order.iter().map(|&i| &st.entries[i]).filter(|e| st.selected.contains(&e.path)).collect();
        // Read-only timing probe, +1 distinguishes an empty selection from no property.
        unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::SetPropW(self.hwnd.get(), windows::core::w!("SlintSelectionCount"),
            Some(windows::Win32::Foundation::HANDLE((picked.len()+1) as _)));
            let _ = windows::Win32::UI::WindowsAndMessaging::SetPropW(self.hwnd.get(), windows::core::w!("SlintSingleClick"),
                Some(windows::Win32::Foundation::HANDLE((st.click_settings.single_click as usize+1) as _))); }
        unsafe {let _=windows::Win32::UI::WindowsAndMessaging::SetPropW(self.hwnd.get(),windows::core::w!("SlintItemCount"),Some(windows::Win32::Foundation::HANDLE((st.order.len()+1) as _)));}
        let files: Vec<&&ShellEntry> = picked.iter().filter(|e| !e.is_dir).collect();

        shell.set_status_count(if st.loaded || !st.entries.is_empty() { format::count(st.order.len(), "count.item", "count.items").into() } else { SharedString::new() });
        shell.set_status_selection(if picked.is_empty() { SharedString::new() } else { format::count(picked.len(), "count.selected", "count.selected-many").into() });
        shell.set_status_size(if !files.is_empty() && files.len() == picked.len() {
            format::short_size(files.iter().map(|e| e.size).sum()).into()
        } else {
            SharedString::new()
        });

        shell.set_settings_view(ids::is_settings(&st.path));
        let fs_folder = (!ids::is_virtual(&st.path) || ids::is_library(&st.path)) && !st.path.is_empty();
        shell.set_fs_folder(fs_folder);
        shell.set_gallery_view(ids::same(&st.path,ids::GALLERY));
        shell.set_native_shell_view(crate::shell::browser::handles(&st.path));
        shell.set_can_new(fs_folder || ids::same(&st.path,ids::LIBRARIES));
        let fs_selection = !picked.is_empty() && picked.iter().all(|e| e.is_fs || crate::shell::archive::root(&e.path).is_some());
        shell.set_fs_selection(fs_selection);
        // Each command as the items allow it (a drive: copy and rename only).
        shell.set_can_cut(fs_selection && picked.iter().all(|e| e.can_move));
        shell.set_can_copy(fs_selection && picked.iter().all(|e| e.can_copy));
        shell.set_can_rename(fs_selection && picked.len() == 1 && picked[0].can_rename);
        shell.set_can_delete(fs_selection && picked.iter().all(|e| e.can_delete));
        // Zip files selected: the command bar offers "Extrair tudo".
        shell.set_extract_view(crate::shell::archive::root(&st.path).is_some() || (fs_selection && picked.iter().all(|e| e.path.to_ascii_lowercase().ends_with(".zip"))));
        shell.set_can_share(fs_selection && picked.iter().all(|e| !e.is_dir));
        shell.set_single_selection(picked.len() == 1);
        shell.set_wallpaper(picked.len() == 1 && picked[0].is_fs && !picked[0].is_dir && crate::shell::preview::is_image(&picked[0].path));
        shell.set_can_paste(st.can_paste && !ids::is_settings(&st.path));
        shell.set_check_boxes(st.check_boxes);
        shell.set_status_bar(st.click_settings.status_bar);
        shell.set_single_click(st.click_settings.single_click);
        shell.set_underline_hover_only(st.click_settings.underline_hover_only);
        shell.set_compact(st.compact);
        shell.set_all_selected(!st.order.is_empty() && picked.len() == st.order.len());
        shell.set_multi_selected(picked.len() > 1);

        let focused_row = st.focused.as_ref().and_then(|f| st.order.iter().position(|&i| &st.entries[i].path == f));
        // A focused group header shows the focus instead of its first row.
        shell.set_focused_row(if st.focused_group.is_some() { -1 } else { focused_row.map(|r| r as i32).unwrap_or(-1) });
        shell.set_focused_group(st.focused_group.map(|r| r as i32).unwrap_or(-1));
        let renaming_row = st.renaming.as_ref().and_then(|f| st.order.iter().position(|&i| &st.entries[i].path == f));
        if let Some(row) = renaming_row {
            let e = &st.entries[st.order[row]];
            shell.set_rename_seed(e.name.as_str().into());
            let end = if e.is_dir { e.name.len() } else { e.name.rfind('.').filter(|&d| d > 0).unwrap_or(e.name.len()) };
            shell.set_rename_select_end(end as i32);
        }
        shell.set_renaming_row(renaming_row.map(|r| r as i32).unwrap_or(-1));

        shell.set_empty_text(if !st.loaded {
            SharedString::new()
        } else if !st.query.is_empty() || ids::is_search(&st.path) {
            tr("empty.search").into()
        } else if ids::same(&st.path, ids::HOME) {
            tr("empty.home").into()
        } else {
            crate::i18n::shell_string(4159, "").into()
        });
        shell.set_error_text(st.error.as_str().into());
        shell.set_sort_descending(st.sort.descending);
        let home = ids::same(&st.path, ids::HOME);
        shell.set_home_view(home);
        shell.set_search_view(ids::is_search(&st.path));
        let recycle = ids::same(&st.path, ids::RECYCLE_BIN);
        shell.set_recycle_view(recycle);
        shell.set_recycle_has_items(recycle && !st.entries.is_empty());
        let mode = if ids::same(&st.path,ids::GALLERY){match st.gallery.icon_size{0..=31=>4,32..=95=>3,96..=255=>2,_=>1}}else if Self::drives_view(&st) || home { 7 } else { st.view_mode };
        shell.set_view_mode(mode);
        shell.set_details_pane(st.details_pane);
        shell.set_preview_pane(st.preview_pane);
        shell.set_preview_kind(st.preview_kind);
        // The handler loads after the selection settles, as Explorer's does.
        self.preview_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(60), || {
            with_app(|app| app.update_preview());
        });
        if st.details_pane {
            self.sync_pane(&st, &picked);
        }
        let view_icon = VIEW_MODES.iter().find(|m| m.3 == mode).map(|m| m.2).unwrap_or("View");
        shell.set_view_icon(self.images.borrow_mut().svg(view_icon));
    }

    /// The preview pane: the selected file's preview handler, or why there is none.
    fn update_preview(&self) {
        use crate::shell::preview;
        let (on, picked): (bool, Vec<ShellEntry>) = {
            let st = self.state.borrow();
            let picked = st.order.iter().map(|&i| &st.entries[i]).filter(|e| st.selected.contains(&e.path)).cloned().collect();
            (st.preview_pane, picked)
        };
        let ui = self.ui();
        let info = ui.global::<crate::PreviewInfo>();
        if !on {
            *self.preview.borrow_mut() = None;
            return;
        }
        let file = match picked.as_slice() {
            [e] if !e.is_dir && e.is_fs => Some(e.path.clone()),
            _ => None,
        };
        let Some(path) = file else {
            *self.preview.borrow_mut() = None;
            info.set_image(Default::default());
            info.set_message(tr(if picked.is_empty() { "preview.select" } else { "preview.none" }).into());
            return;
        };
        if preview::is_image(&path) {
            *self.preview.borrow_mut() = None;
            info.set_message(SharedString::new());
            let thumb = self.images.borrow().thumb(&path, 256);
            match thumb {
                Some(image) => info.set_image(image),
                None => {
                    info.set_image(Default::default());
                    self.request_thumb(&path, 256);
                }
            }
            return;
        }
        info.set_image(Default::default());
        if !self.state.borrow().click_settings.preview_handlers {
            *self.preview.borrow_mut()=None;
            info.set_message(tr("preview.none").into());
            return;
        }
        if self.preview.borrow().as_ref().is_some_and(|(p, _)| *p == path) {
            return;
        }
        *self.preview.borrow_mut() = None;
        info.set_message(SharedString::new());
        match preview::open(self.hwnd.get(), self.preview_rect.get(), &path) {
            Ok(handler) => *self.preview.borrow_mut() = Some((path, handler)),
            Err(_) => info.set_message(tr("preview.failed").into()),
        }
    }

    pub fn set_preview_geometry(&self, x: f32, y: f32, w: f32, h: f32) {
        let scale = self.ui().window().scale_factor();
        let px = |v: f32| (v * scale).round() as i32;
        let rect = windows::Win32::Foundation::RECT { left: px(x), top: px(y), right: px(x + w), bottom: px(y + h) };
        self.preview_rect.set(rect);
        if let Some((_, handler)) = self.preview.borrow().as_ref() {
            handler.set_rect(rect);
        }
    }

    /// The details pane: the selected item (or the folder itself when nothing is selected).
    fn sync_pane(&self, st: &State, picked: &[&ShellEntry]) {
        use crate::{DetailRow, Pane};
        let ui = self.ui();
        let pane = ui.global::<Pane>();
        pane.set_preview_large(false);
        let mut images = self.images.borrow_mut();
        let row = |label: &str, value: String| DetailRow { label: label.into(), value: value.into() };
        let mut rows = Vec::new();
        let mut thumb_wanted: Option<String> = None;
        let mut large_wanted: Option<String> = None;
        // The title wraps at words only; the UI measures the longest against the line.
        // The title wraps as the real's XAML does, by DirectWrite: between whole words, also
        // before a backslash and after a hyphen ("C:\Users\...\C--" / "workspace-projects-slint-"),
        // on the pane's 216px line (256 without an icon).
        let set_name = |name: &str, icon: bool| {
            let width = if icon { PANE_TITLE_WIDTH } else { PANE_TITLE_WIDTH + PANE_ICON_ROOM };
            let lines = crate::shell::text::wrap_whole_words(name, "Segoe UI Variable Text", 16.0, true, width);
            pane.set_name(lines.join("\n").into());
            pane.set_name_lines(lines.len() as i32);
        };
        match picked {
            [e] => {
                // A deleted item shows where it was deleted from ("C:\...\keytest\dummy_b.txt").
                let deleted_from = ids::same(&st.path, ids::RECYCLE_BIN)
                    .then(|| crate::shell::columns::key("System.Recycle.DeletedFrom"))
                    .flatten()
                    .and_then(|k| st.col_texts.get(&e.path)?.iter().find(|(c, _)| *c == k).map(|(_, t)| t.clone()))
                    .filter(|t| !t.is_empty());
                match deleted_from {
                    Some(from) => set_name(&format!("{}\\{}", from.trim_end_matches('\\'), e.name), !e.is_dir),
                    None => set_name(&e.name, !e.is_dir),
                }
                // A folder shows no icon before its name, nor the sharing button.
                // Without a shortcut's arrow, as the real.
                let plain = e.icon & 0x00FF_FFFF;
                pane.set_icon(if e.is_dir { Default::default() } else { images.icon(plain, 32) });
                // A file's thumbnail fills the area; without one (or for a folder) the 96px
                // picture shows.
                let large = (!e.is_dir && e.is_fs).then(|| images.thumb(&e.path, PANE_THUMBNAIL)).flatten();
                match large.filter(|i| i.size().width > 0) {
                    Some(image) => {
                        pane.set_preview(image);
                        pane.set_preview_large(true);
                    }
                    None => {
                        pane.set_preview(images.thumb(&e.path, 96).unwrap_or_else(|| images.icon(plain, 96)));
                        pane.set_preview_large(false);
                    }
                }
                if e.is_fs {
                    thumb_wanted = Some(e.path.clone());
                    if !e.is_dir && !images.has_thumb(&e.path, PANE_THUMBNAIL) {
                        large_wanted = Some(e.path.clone());
                    }
                }
                if ids::same(&st.path,ids::NETWORK)&&!e.is_fs {
                    if let Some(fields)=st.fields.get(&(listing::PropList::Pane,e.path.clone())){
                        rows.extend(fields.iter().flatten().map(|(label,value,_)|row(label,value.clone())));
                    }
                }else{rows.push(row(tr("column.type"), e.type_name.clone()));}
                let zip_member=crate::shell::archive::root(&e.path).is_some();
                if !e.is_dir && (e.is_fs || zip_member) {
                    rows.push(row(tr("column.size"), format::short_size(e.size)));
                }
                if e.is_fs || zip_member {
                    rows.push(row(tr("column.location"), listing::folder_path_display(&e.path).unwrap_or_else(|| ids::parent_of(&e.path))));
                }
                if e.modified > 0 {
                    rows.push(row(tr("column.modified"), format::modified(e.modified)));
                }
                if zip_member && !e.is_dir {
                    for key in ["System.ZipFolder.CompressedSize","System.ZipFolder.Ratio"] {
                        if let Some((label,value))=listing::property(&e.path,key){rows.push(row(&label,value));}
                    }
                }
                // Pictures add their dimensions ("3840 x 2400").
                let ext = std::path::Path::new(&e.path).extension().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                if e.is_fs && !e.is_dir && crate::shell::assoc::perceived(&ext) == "image" {
                    if let Some((label, value)) = listing::property(&e.path, "System.Image.Dimensions") {
                        rows.push(row(&label, value));
                    }
                }
                pane.set_can_share((e.is_fs || zip_member) && !e.is_dir);
            }
            [] => {
                let location = st.crumbs.last();
                // Search results go by the query: "txt (3 itens)".
                let name = match ids::search_parts(&st.path) {
                    Some((_, query)) => query.trim(),
                    None => location.map(|c| c.name.as_str()).unwrap_or(""),
                };
                set_name(&format!("{name} ({})", format::count(st.order.len(), "count.item", "count.items")), false);
                if crate::shell::archive::is_root(&st.path) {
                    let stem=std::path::Path::new(name).file_stem().map(|s|s.to_string_lossy().into_owned()).unwrap_or_else(||name.to_string());
                    set_name(&format!("{stem} ({})",format::count(st.order.len(),"count.item","count.items")),false);
                }
                // Search results show the stack of documents, not their magnifier.
                let icon = if ids::is_search(&st.path) { crate::shell::icons::stack_icon() } else { location.map(|c| c.icon).unwrap_or(0) };
                pane.set_icon(Default::default());
                pane.set_preview(images.thumb(&st.path, 96).unwrap_or_else(|| images.icon(icon, 96)));
                thumb_wanted = Some(st.path.clone());
                pane.set_can_share(false);
            }
            // Several items, files or folders: the system's stack of documents and the same
            // "select a single file" card as for none, no details nor buttons.
            many => {
                set_name(&format::count(many.len(), "count.selected", "count.selected-many"), false);
                pane.set_icon(Default::default());
                pane.set_preview(images.icon(crate::shell::icons::stack_icon(), 96));
                pane.set_can_share(false);
            }
        }
        let single = picked.len() == 1;
        pane.set_info(if single { SharedString::new() } else { tr("pane.select-one").into() });
        pane.set_can_properties(single && picked.first().and_then(|e|st.network_properties.get(&e.path)).copied().unwrap_or(true));
        pane.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
        pane.set_share_icon(images.svg("Windows.ModernShare"));
        pane.set_properties_icon(images.svg("windows.properties"));
        drop(images);
        if let Some(path) = thumb_wanted {
            self.request_thumb(&path, 96);
        }
        if let Some(path) = large_wanted {
            self.request_thumb(&path, PANE_THUMBNAIL);
        }
    }

    /// Loads one thumbnail off the UI thread, then refreshes what shows it (the details pane).
    pub fn request_thumb(&self, path: &str, size: u32) {
        if path.is_empty() || (ids::is_virtual(path) && !crate::shell::archive::is_root(path)) || self.images.borrow().has_thumb(path, size) {
            return;
        }
        if size==768 && self.state.borrow().gallery.retries.get(path).is_some_and(|(count,next)|*count>=8||Instant::now()<*next){return;}
        if !self.pending_thumbs.borrow_mut().insert((path.to_string(), size)) {
            return;
        }
        let path = path.to_string();
        let identity=if size==768{self.state.borrow().entries.iter().find(|e|e.path==path).map(|e|e.absolute.clone()).unwrap_or_default()}else{Vec::new()};
        std::thread::spawn(move || {
            crate::shell::com_init();
            // 96 is the details pane's picture, PANE_THUMBNAIL its thumbnail (none for an
            // icon), 256 the preview pane's picture.
            let icons_only=listing::settings().icons_only;
            let rgba = match size {
                96 => crate::shell::icons::pane_picture(&path, size),
                PANE_THUMBNAIL => crate::shell::icons::pane_thumbnail(&path, size, 220),
                768 => crate::shell::icons::gallery_picture(&path,&identity,size),
                _ => crate::shell::icons::thumbnail(&path, size),
            }
            .ok();
            post(move |app| {
                app.pending_thumbs.borrow_mut().remove(&(path.clone(), size));
                if size!=768 && listing::settings().icons_only != icons_only {return;}
                match rgba {
                    Some(rgba) => app.images.borrow_mut().insert_thumb(&path, size, images::to_image(&rgba)),
                    // Remembered as none, so it isn't asked again.
                    None if size == PANE_THUMBNAIL => app.images.borrow_mut().insert_thumb(&path, size, Default::default()),
                    None if size==768 => {
                        let delay={let mut st=app.state.borrow_mut();let count=st.gallery.retries.get(&path).map_or(1,|(count,_)|count+1);let delay=std::time::Duration::from_millis((300u64 << count.min(4)).min(5000));st.gallery.retries.insert(path.clone(),(count,Instant::now()+delay));delay};
                        slint::Timer::single_shot(delay,||with_app(|app|{if ids::same(&app.current_path(),ids::GALLERY){app.layout_gallery();}}));
                        return;
                    },
                    None => return,
                }
                app.sync_view_state();
                if ids::same(&app.current_path(),ids::GALLERY) {app.layout_gallery();}
            });
        });
    }

    pub fn sync_tabs(&self) {
        unsafe{use windows::Win32::UI::WindowsAndMessaging::SetPropW;use windows::core::w;use windows::Win32::Foundation::HANDLE;let st=self.state.borrow();let _=SetPropW(self.hwnd.get(),w!("SlintTabCount"),Some(HANDLE((st.tabs.len()+1) as _)));let _=SetPropW(self.hwnd.get(),w!("SlintCurrentTab"),Some(HANDLE((st.current+1) as _)));}
        let ui = self.ui();
        let st = self.state.borrow();
        let mut images = self.images.borrow_mut();
        // The titles' room, as the strip lays the tabs out (tabstrip.slint): 248px tabs that
        // share what the caption buttons leave, less the icon and the close button (75px).
        let scale = ui.window().scale_factor();
        let available = ui.window().size().width as f32 / scale - 5.0 - 35.0 - 48.0 - 136.0;
        let n = st.tabs.len().max(1) as f32;
        let tab_width = (((available - 8.0) / n).floor() + 8.0).clamp(88.0, 248.0);
        let room = tab_width - 75.0;
        let items: Vec<TabItem> = st
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| TabItem {
                settings: ids::is_settings(t.path()),
                title: if ids::is_settings(t.path()) { tr("settings.title").into() } else { t.title.as_str().into() },
                shown: fit_whole_glyphs(&t.title, room, if i == st.current { 500 } else { 400 }).into(),
                icon: if t.icon == 0 && t.title.is_empty() { Default::default() } else { images.icon(t.icon, 16) },
            })
            .collect();
        // Switching tabs while the button is down must keep the pressed Tab/TouchArea
        // alive: resetting the model here discards its mouse capture before a drag starts.
        if self.tabs.row_count() == items.len() {
            for (i, item) in items.into_iter().enumerate() {
                self.tabs.set_row_data(i, item);
            }
        } else {
            self.tabs.set_vec(items);
        }
        ui.global::<Shell>().set_current_tab(st.current as i32);
        // explorerframe's "%1 – Explorador de Arquivos", "%1 e mais uma guia - ...", "%1 e %2 mais
        // guias - ...".
        let title = st.tabs[st.current].title.clone();
        let others = st.tabs.len() - 1;
        let window_title = match others {
            _ if title.is_empty() => tr("app.title").to_string(),
            0 => crate::i18n::system_format("explorerframe.dll", 35104, &[&title]),
            1 => crate::i18n::system_format("explorerframe.dll", 35105, &[&title]),
            n => crate::i18n::system_format("explorerframe.dll", 35106, &[&title, &n.to_string()]),
        };
        ui.set_window_title(window_title.into());
    }

    pub fn sync_crumbs(&self) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let st = self.state.borrow();
        let tab = &st.tabs[st.current];
        shell.set_can_back(tab.index > 0);
        shell.set_can_forward(tab.index + 1 < tab.history.len());
        shell.set_can_up(!st.path.is_empty() && !ids::is_settings(&st.path));

        let mut images = self.images.borrow_mut();
        // The last crumb drops its chevron when it has no subfolders. Search results show only
        // theirs ("Resultados da Pesquisa em ...").
        let last = st.crumbs.len().saturating_sub(1);
        let first = if ids::is_search(&st.path) { last } else { 0 };
        let crumbs: Vec<UiCrumb> = st
            .crumbs
            .iter()
            .enumerate()
            .skip(first)
            .map(|(i, c)| UiCrumb {
                id: c.id.as_str().into(),
                name: c.name.as_str().into(),
                icon: images.icon(c.icon, 16),
                chevron: ids::crumb_has_children(&c.id) && (i < last || c.has_subfolders),
            })
            .collect();
        let root = st.crumbs.first();
        shell.set_root_kind(root.map(|c| ids::root_kind(&c.id)).unwrap_or(0));
        shell.set_root_icon(root.map(|c| images.icon(c.icon, 16)).unwrap_or_default());
        let location = st.crumbs.last().map(|c| c.name.clone()).unwrap_or_default();
        shell.set_location_name(location.as_str().into());
        shell.set_native_location_icon(st.crumbs.last().map(|c|images.icon(c.icon,16)).unwrap_or_default());
        let native_scope=if crate::shell::browser::is_control_panel(&st.path){crate::i18n::system_string("shell32.dll",4161)}else{location.clone()};
        shell.set_native_search_scope(if crate::shell::browser::is_control_panel(&st.path){crate::i18n::shell_string(13122,&native_scope).into()}else{crate::i18n::trf("nav.search",&[&native_scope]).into()});
        // The up button's tip names where it goes.
        let parent = st.crumbs.len().checked_sub(2).map(|i| st.crumbs[i].name.clone()).unwrap_or_default();
        shell.set_parent_name(parent.as_str().into());
        let name_at = |i: usize| tab.history.get(i).map(|id| if ids::is_settings(id) { tr("settings.title").into() } else { listing::display_name_of(id) }).unwrap_or_default();
        shell.set_back_name(if tab.index > 0 { name_at(tab.index - 1) } else { String::new() }.into());
        shell.set_forward_name(name_at(tab.index + 1).into());
        shell.set_address_text(if ids::is_virtual(&st.path) || st.path.is_empty() { location.as_str().into() } else { st.path.as_str().into() });
        drop(images);
        let changed = self.crumbs.row_count() != crumbs.len()
            || crumbs.iter().enumerate().any(|(i, c)| self.crumbs.row_data(i).map(|o| o.id != c.id || o.name != c.name).unwrap_or(true));
        if changed {
            drop(st);
            self.state.borrow_mut().crumb_widths.clear();
            self.crumbs.set_vec(crumbs);
            self.fit_crumbs();
        }
    }

    /// Ancestors collapse into "…" (root first) when the trail does not fit.
    pub fn fit_crumbs(&self) {
        let st = self.state.borrow();
        let n = st.crumbs.len();
        // Ancestors collapse into "…" from the root only when the trail does not fit.
        let mut hidden = 0;
        let width = |hidden: usize| -> f32 {
            let more = if hidden > 0 { 30.0 + 26.0 } else { 0.0 };
            more + (hidden..n).map(|i| st.crumb_widths.get(i).copied().unwrap_or(0.0)).sum::<f32>()
        };
        if st.trail_width > 0.0 {
            while hidden + 1 < n && width(hidden) > st.trail_width + 0.5 {
                hidden += 1;
            }
        }
        self.ui().global::<Shell>().set_hidden_crumbs(hidden as i32);
    }

    pub fn sync_nav(&self) {
        let ui = self.ui();
        let selected = self.nav_selected();
        let st = self.state.borrow();
        let mut images = self.images.borrow_mut();
        // The pane's list starts with a blank row and ends with two (see sidebar.slint).
        let pad = NavItem { pad: true, ..Default::default() };
        let nodes = st.nav.rows.iter().enumerate().map(|(i, row)| match st.nav.node(&row.reference) {
            None => NavItem { separator: true, ..Default::default() },
            Some(node) => NavItem {
                name: node.node.name.as_str().into(),
                icon: images.icon(node.node.icon, 16),
                depth: row.depth,
                expandable: node.node.has_children,
                expanded: node.expanded,
                pinned: matches!(&row.reference, RowRef::Node { section, .. } if *section == navtree::PINS) && node.node.is_pinned,
                separator: false,
                selected: selected == Some(i),
                pad: false,
            },
        });
        let rows: Vec<NavItem> = std::iter::once(pad.clone()).chain(nodes).chain([pad.clone(), pad]).collect();
        drop(images);
        if self.nav.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if self.nav.row_data(i).as_ref() != Some(&row) {
                    self.nav.set_row_data(i, row);
                }
            }
        } else {
            self.nav.set_vec(rows);
        }
        ui.global::<Shell>().set_sidebar_width(if st.nav_pane { st.nav_width } else { 0.0 });
        drop(st);
        self.nav_clamp();
    }

    /// Rebuilds the rows and, in icon views, their layout and thumbnails.
    pub fn sync_rows(self: &Rc<Self>) {
        {
            let mut st = self.state.borrow_mut();
            st.header_widths.clear();
        }
        self.sync_entries();
        if ids::same(&self.current_path(),ids::GALLERY) {self.layout_gallery();return;}
        self.load_column_texts();
        if Self::icons_spec(&self.state.borrow()).is_some() {
            self.relayout();
            self.load_thumbnails();
            self.load_fields();
        }
    }

    pub fn sync_all(self: &Rc<Self>) {
        self.sync_rows();
        self.sync_view_state();
        self.sync_tabs();
        self.sync_crumbs();
        self.sync_nav();
    }

    // ---------------------------------------------------------------- selection

    pub fn selected_entries(&self) -> Vec<ShellEntry> {
        let st = self.state.borrow();
        st.order.iter().map(|&i| &st.entries[i]).filter(|e| st.selected.contains(&e.path)).cloned().collect()
    }

    pub fn selected_paths(&self) -> Vec<String> {
        self.selected_entries().into_iter().map(|e| e.path).collect()
    }

    fn row_path(st: &State, row: usize) -> Option<String> {
        st.order.get(row).map(|&i| st.entries[i].path.clone())
    }

    fn row_of(st: &State, path: &str) -> Option<usize> {
        st.order.iter().position(|&i| st.entries[i].path == path)
    }

    pub fn set_selection(&self, selected: HashSet<String>, anchor: Option<String>, focused: Option<String>) {
        {
            let mut st = self.state.borrow_mut();
            st.selected = selected;
            st.anchor = anchor;
            st.focused = focused;
            st.focused_group = None;
        }
        self.sync_selection();
    }

    /// A row's check box: adds the item to the selection or takes it out.
    pub fn row_check(&self, row: usize) {
        let (selected, path) = {
            let st = self.state.borrow();
            let Some(path) = Self::row_path(&st, row) else { return };
            let mut selected = st.selected.clone();
            if !selected.remove(&path) {
                selected.insert(path.clone());
            }
            (selected, path)
        };
        self.set_selection(selected, Some(path.clone()), Some(path));
    }

    /// The header's check box: everything or nothing.
    pub fn check_all(&self, on: bool) {
        let all: HashSet<String> = if on {
            let st = self.state.borrow();
            st.order.iter().map(|&i| st.entries[i].path.clone()).collect()
        } else {
            HashSet::new()
        };
        let focused = self.state.borrow().focused.clone();
        self.set_selection(all, None, focused);
    }

    /// A group header clicked: selects its items (adding them with Ctrl), and the header
    /// takes the focus.
    pub fn group_clicked(&self, row: usize, ctrl: bool) {
        let (selected, first) = {
            let st = self.state.borrow();
            let end = (row + 1..st.order.len()).find(|&r| st.headers.get(r).is_some_and(|h| !h.is_empty())).unwrap_or(st.order.len());
            let rows: Vec<String> = (row..end).filter_map(|r| Self::row_path(&st, r)).collect();
            let mut selected = if ctrl { st.selected.clone() } else { HashSet::new() };
            selected.extend(rows.iter().cloned());
            (selected, rows.first().cloned())
        };
        self.set_selection(selected, first.clone(), first);
        self.state.borrow_mut().focused_group = Some(row);
        self.sync_selection();
    }

    /// Collapses or opens the group whose header is on `row`.
    pub fn set_group_open(self: &Rc<Self>, row: usize, open: bool) {
        {
            let mut st = self.state.borrow_mut();
            let Some(label) = st.headers.get(row).filter(|h| !h.is_empty()).cloned() else { return };
            if open == !st.collapsed.contains(&label) {
                return;
            }
            if open {
                st.collapsed.remove(&label);
            } else {
                st.collapsed.insert(label);
            }
        }
        self.rebuild_order();
        self.sync_rows();
        self.sync_view_state();
    }

    /// Opens or collapses every group.
    pub fn set_groups_open(self: &Rc<Self>, open: bool) {
        {
            let mut st = self.state.borrow_mut();
            if open {
                st.collapsed.clear();
            } else {
                let all: Vec<String> = st.headers.iter().filter(|h| !h.is_empty()).cloned().collect();
                st.collapsed.extend(all);
            }
        }
        self.rebuild_order();
        self.sync_rows();
        self.sync_view_state();
    }

    pub fn toggle_group(self: &Rc<Self>, row: usize) {
        let open = self.state.borrow().folded.get(row).copied().unwrap_or(false);
        self.set_group_open(row, open);
    }

    fn range(st: &State, a: usize, b: usize) -> HashSet<String> {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        (lo..=hi.min(st.order.len().saturating_sub(1))).filter_map(|r| Self::row_path(st, r)).collect()
    }

    pub fn row_pointer(self: &Rc<Self>, row: usize, button: PointerEventButton, kind: PointerEventKind, ctrl: bool, shift: bool) {
        if kind == PointerEventKind::Down { self.hover_select_timer.stop(); self.hover_anchor.set(None); }
        let activate = kind == PointerEventKind::Up && button == PointerEventButton::Left && !ctrl && !shift
            && self.state.borrow().click_settings.single_click;
        let entry = if activate { let st=self.state.borrow();st.order.get(row).map(|&i|st.entries[i].clone()) } else { None };
        let mut st = self.state.borrow_mut();
        let Some(path) = Self::row_path(&st, row) else { return };
        match (kind, button) {
            (PointerEventKind::Down, PointerEventButton::Right) => {
                if !st.selected.contains(&path) {
                    drop(st);
                    self.set_selection(HashSet::from([path.clone()]), Some(path.clone()), Some(path));
                }
            }
            (PointerEventKind::Down, PointerEventButton::Left) => {
                st.pending_narrow = None;
                if shift {
                    let from = st.anchor.as_ref().and_then(|a| Self::row_of(&st, a)).unwrap_or(0);
                    let mut next = Self::range(&st, from, row);
                    if ctrl {
                        next.extend(st.selected.iter().cloned());
                    }
                    let anchor = st.anchor.clone().or(Some(path.clone()));
                    drop(st);
                    self.set_selection(next, anchor, Some(path));
                } else if ctrl {
                    let mut next = st.selected.clone();
                    if !next.remove(&path) {
                        next.insert(path.clone());
                    }
                    drop(st);
                    self.set_selection(next, Some(path.clone()), Some(path));
                } else if !st.selected.contains(&path) {
                    drop(st);
                    self.set_selection(HashSet::from([path.clone()]), Some(path.clone()), Some(path));
                } else {
                    // Pressing on a selected item keeps the selection (for a drag); releasing narrows it.
                    st.pending_narrow = Some(row);
                    st.focused = Some(path);
                    drop(st);
                    self.sync_view_state();
                }
            }
            (PointerEventKind::Up, PointerEventButton::Left) => {
                if st.pending_narrow.take() == Some(row) {
                    drop(st);
                    self.set_selection(HashSet::from([path.clone()]), Some(path.clone()), Some(path));
                }
            }
            _ => { drop(st); }
        }
        if let Some(entry) = entry {
            let interval=unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as u128;
            if self.last_single_activation.borrow().as_ref().is_some_and(|(path,when)| path==&entry.path && when.elapsed().as_millis()<interval) { return; }
            *self.last_single_activation.borrow_mut()=Some((entry.path.clone(),Instant::now()));
            let weak=Rc::downgrade(self);
            slint::Timer::single_shot(std::time::Duration::ZERO,move||{
                if let Some(app)=weak.upgrade(){actions::open_entry(&app,&entry);}
            });
        }
    }

    pub fn background_pointer(&self, button: PointerEventButton, kind: PointerEventKind, ctrl: bool, shift: bool) {
        if kind != PointerEventKind::Down {
            return;
        }
        let base = {
            let st = self.state.borrow();
            if ctrl || shift { st.selected.clone() } else { HashSet::new() }
        };
        {
            let mut st = self.state.borrow_mut();
            st.band_base = base;
            st.band_ctrl = ctrl;
        }
        let focus = { let st=self.state.borrow(); ids::same(&st.path,ids::NETWORK).then(||st.focused.clone()).flatten() };
        match button {
            PointerEventButton::Right if !ctrl => self.set_selection(HashSet::new(), None, focus),
            PointerEventButton::Left if !(ctrl || shift) => self.set_selection(HashSet::new(), None, focus),
            _ => {}
        }
    }

    /// Rubber band selection, in list content coordinates.
    pub fn band(&self, x: f32, y: f32, w: f32, h: f32) {
        if Self::icons_spec(&self.state.borrow()).is_some() {
            let hits = self.band_cells(x, y, w, h);
            let (next, first, last) = {
                let st = self.state.borrow();
                let paths: Vec<String> = hits.iter().filter_map(|&r| Self::row_path(&st, r)).collect();
                let mut next = st.band_base.clone();
                for p in &paths {
                    if st.band_ctrl && st.band_base.contains(p) {
                        next.remove(p);
                    } else {
                        next.insert(p.clone());
                    }
                }
                let focused=paths.last().cloned().or_else(||ids::same(&st.path,ids::NETWORK).then(||st.focused.clone()).flatten());
                (next, paths.first().cloned(), focused)
            };
            self.set_selection(next, first, last);
            return;
        }
        let (next, first, last) = {
            let st = self.state.borrow();
            let ctrl = st.band_ctrl;
            let ui = self.ui();
            let shell = ui.global::<Shell>();
            let total = shell.get_col_name() + shell.get_columns_width();
            let (left, right) = (ROW_LEFT, ROW_LEFT + total - 18.0);
            let mut hit = Vec::new();
            if x + w > left && x < right {
                for row in 0..st.order.len() {
                    let top = Self::row_top(&st, row);
                    let bottom = top + Self::row_height(&st);
                    if bottom > y && top < y + h {
                        hit.push(row);
                    }
                }
            }
            let hit_paths: Vec<String> = hit.iter().filter_map(|&r| Self::row_path(&st, r)).collect();
            let mut next: HashSet<String> = st.band_base.clone();
            for p in &hit_paths {
                if ctrl && st.band_base.contains(p) {
                    next.remove(p);
                } else {
                    next.insert(p.clone());
                }
            }
            (next, hit_paths.first().cloned(), hit_paths.last().cloned())
        };
        self.set_selection(next, first, last);
    }

    /// Scrolls so the row of `path` is visible.
    pub fn reveal(&self, path: &str) {
        let row = {
            let st = self.state.borrow();
            Self::row_of(&st, path)
        };
        let Some(row) = row else { return };
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        // The list runs sideways: bring the cell's column into view.
        let across = {
            let st = self.state.borrow();
            (st.grid_column_major && Self::icons_spec(&st).is_some()).then(|| st.cells.get(row).map(|c| (c[0], c[0] + c[2], st.grid_viewport.0))).flatten()
        };
        if let Some((left, right, width)) = across {
            // Snapped to a column's left edge, like the wheel: the cell's own column, or the
            // first one from which it shows whole.
            let lefts = self.column_lefts();
            let scroll = -shell.get_grid_scroll_x();
            let column = lefts.iter().copied().rev().find(|&l| l <= left).unwrap_or(0.0);
            if left < scroll {
                shell.set_grid_scroll_x(-column);
            } else if right > scroll + width {
                let to = lefts.iter().copied().find(|&l| right <= l + width).unwrap_or(column);
                shell.set_grid_scroll_x(-to.min(column));
            }
            return;
        }
        self.reveal_list_row(row);
    }

    fn move_to(&self, row: isize, ctrl: bool, shift: bool) {
        let (sel, anchor, focus) = {
            let st = self.state.borrow();
            if st.order.is_empty() {
                return;
            }
            let row = row.clamp(0, st.order.len() as isize - 1) as usize;
            // Items of collapsed groups are skipped.
            let shown = |r: usize| !st.folded.get(r).copied().unwrap_or(false);
            let back = st.focused.as_ref().and_then(|f| Self::row_of(&st, f)).is_some_and(|c| row < c);
            let ahead = || (row..st.order.len()).find(|&r| shown(r));
            let behind = || (0..=row).rev().find(|&r| shown(r));
            let Some(row) = (if back { behind().or_else(ahead) } else { ahead().or_else(behind) }) else { return };
            let target = Self::row_path(&st, row).unwrap();
            if shift {
                let from = st.anchor.as_ref().and_then(|a| Self::row_of(&st, a)).unwrap_or(0);
                (Self::range(&st, from, row), st.anchor.clone().or(Some(target.clone())), target)
            } else if ctrl {
                (st.selected.clone(), st.anchor.clone(), target)
            } else {
                (HashSet::from([target.clone()]), Some(target.clone()), target)
            }
        };
        self.set_selection(sel, anchor, Some(focus.clone()));
        self.reveal(&focus);
    }

    /// The icon cell in the row below (or above) `row`, nearest its column.
    fn cell_beside(st: &State, row: usize, down: bool) -> Option<usize> {
        let here = *st.cells.get(row)?;
        let next_y = st
            .cells
            .iter()
            .map(|c| c[1])
            .filter(|&y| if down { y > here[1] } else { y < here[1] })
            .fold(None, |best: Option<f32>, y| Some(match best {
                Some(b) if (down && b <= y) || (!down && b >= y) => b,
                _ => y,
            }))?;
        st.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c[1] == next_y)
            .min_by(|(_, a), (_, b)| (a[0] - here[0]).abs().total_cmp(&(b[0] - here[0]).abs()))
            .map(|(i, _)| i)
    }

    /// Keys handled by the folder view. Returns whether the key was consumed.
    pub fn list_key(self: &Rc<Self>, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        // Hover selection takes list focus; let the window handle its Alt navigation.
        if alt && !ctrl && (is(text,Key::LeftArrow) || is(text,Key::RightArrow) || is(text,Key::UpArrow)) { return actions::global_key(self,text,ctrl,shift,alt); }
        let printable=text.chars().count()==1 && !ctrl && !alt && text.chars().next().is_some_and(|c|!c.is_control() && (c as u32)<0xF700);
        let type_search=self.state.borrow().click_settings.type_search;
        if printable && type_search {self.ui().invoke_type_search(text.into());return true;}
        if self.drives_key(text, ctrl, shift, alt) {return true;}
        // On a focused group header, left collapses the group and right opens it again.
        let header = self.state.borrow().focused_group;
        if let Some(row) = header {
            if is(text, Key::LeftArrow) || is(text, Key::RightArrow) {
                self.set_group_open(row, is(text, Key::RightArrow));
                return true;
            }
        }
        let current = {
            let st = self.state.borrow();
            st.focused.as_ref().and_then(|f| Self::row_of(&st, f)).map(|r| r as isize).unwrap_or(-1)
        };
        let page = {
            let st = self.state.borrow();
            ((st.viewport / Self::row_pitch(&st)).floor() as isize - 1).max(1)
        };
        let (grid, cols) = {
            let st = self.state.borrow();
            if Self::drives_view(&st) {
                (true, st.columns.max(1) as isize)
            } else if Self::icons_spec(&st).is_some() {
                (true, st.grid_columns.max(1) as isize)
            } else {
                (false, 1)
            }
        };
        // List view runs top to bottom: up/down step one item, left/right a column.
        let column_major = grid && self.state.borrow().grid_column_major;
        // Icon rows page by their own heights.
        let rows = grid && !column_major && !Self::drives_view(&self.state.borrow());
        let (cols, step) = if column_major { (1, cols) } else { (cols, 1) };
        let len = self.state.borrow().order.len() as isize;
        let lower = text.to_lowercase();
        let c = !alt && ctrl;
        // Grouped icon rows end early: step to the cell above or below instead.
        let grouped_grid = grid && !column_major && !self.state.borrow().headers.is_empty();
        if grouped_grid && current >= 0 && (is(text, Key::DownArrow) || is(text, Key::UpArrow)) {
            let target = Self::cell_beside(&self.state.borrow(), current as usize, is(text, Key::DownArrow));
            self.move_to(target.map(|t| t as isize).unwrap_or(current), ctrl, shift);
        } else if is(text, Key::DownArrow) {
            self.move_to(if current < 0 { 0 } else { current + cols }, ctrl, shift);
        } else if is(text, Key::UpArrow) {
            self.move_to(if current < 0 { 0 } else { current - cols }, ctrl, shift);
        } else if grid && is(text, Key::RightArrow) {
            self.move_to(current + step, ctrl, shift);
        } else if grid && is(text, Key::LeftArrow) {
            self.move_to((current - step).max(0), ctrl, shift);
        } else if is(text, Key::Home) {
            self.move_to(0, ctrl, shift);
        } else if is(text, Key::End) {
            self.move_to(len - 1, ctrl, shift);
        } else if is(text, Key::PageDown) {
            let to = if !grid { self.list_page_target(current, true) } else if rows { self.grid_page_target(current, true) } else if column_major { self.column_page_target(current, true) } else { current + page };
            self.move_to(to, ctrl, shift);
        } else if is(text, Key::PageUp) {
            let to = if !grid { self.list_page_target(current, false) } else if rows { self.grid_page_target(current, false) } else if column_major { self.column_page_target(current, false) } else { current - page };
            self.move_to(to, ctrl, shift);
        } else if text == " " && c {
            let focused = self.state.borrow().focused.clone();
            if let Some(f) = focused {
                let mut sel = self.state.borrow().selected.clone();
                if !sel.remove(&f) {
                    sel.insert(f.clone());
                }
                self.set_selection(sel, Some(f.clone()), Some(f));
            }
        } else if is(text, Key::Return) && alt {
            actions::properties(self, self.selected_paths());
        } else if is(text, Key::Return) {
            actions::open_selection(self);
        } else if is(text, Key::Backspace) {
            self.back();
        } else if is(text, Key::Delete) {
            actions::delete(self, shift);
        } else if c && !shift && lower == "d" {
            actions::delete(self, false);
        } else if (is(text, Key::F10) && shift && !ctrl && !alt) || is(text, Key::Menu) {
            self.key_menu();
        } else if is(text, Key::F2) {
            actions::start_rename(self);
        } else if c && lower == "a" {
            actions::select_all(self);
        } else if c && lower == "z" {
            undo::undo(self);
        } else if c && !shift && lower == "y" {
            undo::redo(self);
        } else if c && shift && lower == "c" {
            actions::copy_as_path(self, self.selected_paths());
        } else if c && lower == "c" {
            actions::copy(self);
        } else if c && lower == "x" {
            actions::cut(self);
        } else if c && lower == "v" {
            actions::paste(self);
        } else if c && shift && lower == "n" {
            actions::new_folder(self);
        } else if text.chars().count() == 1 && !ctrl && !alt && !text.chars().next().unwrap().is_control() && (text.chars().next().unwrap() as u32) < 0xF700 {
            self.typeahead(text, current);
        } else {
            return false;
        }
        true
    }

    /// Shift+F10 or the menu key: the selection's menu at the focused item (or the first
    /// selected one), brought into view; with nothing selected, the folder's.
    fn key_menu(self: &Rc<Self>) {
        let target = {
            let st = self.state.borrow();
            let focused = st.focused.clone().filter(|f| st.selected.contains(f));
            focused.or_else(|| st.order.iter().map(|&i| &st.entries[i].path).find(|p| st.selected.contains(*p)).cloned())
        };
        let row = match target {
            Some(path) => {
                self.reveal(&path);
                Self::row_of(&self.state.borrow(), &path).map(|r| r as i32).unwrap_or(-2)
            }
            None => -2,
        };
        self.ui().global::<Shell>().set_key_menu_row(row);
    }

    fn typeahead(&self, text: &str, current: isize) {
        let target = {
            let mut st = self.state.borrow_mut();
            let now = Instant::now();
            if now.duration_since(st.typeahead.0).as_millis() > TYPEAHEAD_RESET_MS {
                st.typeahead.1.clear();
            }
            st.typeahead.0 = now;
            st.typeahead.1.push_str(text);
            let prefix = st.typeahead.1.to_lowercase();
            let n = st.order.len();
            if n == 0 {
                return;
            }
            let start = if st.typeahead.1.chars().count() == 1 { (current + 1) as usize } else { current.max(0) as usize };
            (0..n).map(|k| (start + k) % n).find(|&r| st.entries[st.order[r]].name.to_lowercase().starts_with(&prefix))
        };
        if let Some(row) = target {
            self.move_to(row as isize, false, false);
        }
    }

    pub fn set_sort(self: &Rc<Self>, sort: Sort) {
        { let mut st=self.state.borrow_mut();st.sort=sort;st.sort_extra.clear();st.search_ranked=false; }
        self.remember_view(|v| v.sort = Some(sort));
        self.rebuild_order();
        self.load_sort_texts();
        self.sync_rows();
        self.sync_view_state();
        self.sync_column_heads();
    }

    /// Authors, tags, title and the other columns' values come from each file's property
    /// handler: when sorting or grouping by one, read the missing values off the UI thread and
    /// sort again.
    pub fn load_sort_texts(self: &Rc<Self>) {
        let (sort, group) = {
            let st = self.state.borrow();
            (st.sort.key, st.group.map(|g| g.key))
        };
        self.load_values(sort, false);
        // Grouping by the sort's key uses its values.
        if let Some(group) = group.filter(|g| *g != sort) {
            self.load_values(group, true);
        }
    }

    /// Reads the missing values of `key` (the sort's, or the grouping's when `for_group`).
    fn load_values(self: &Rc<Self>, key: SortKey, for_group: bool) {
        let Some(property) = key.property() else { return };
        let (paths, folder, children): (Vec<String>, String, std::collections::HashMap<String, Vec<u8>>) = {
            let mut st = self.state.borrow_mut();
            let st = &mut *st;
            let (values, values_key) = if for_group { (&mut st.group_texts, &mut st.group_texts_key) } else { (&mut st.sort_texts, &mut st.sort_texts_key) };
            if *values_key != Some(key) {
                values.clear();
                *values_key = Some(key);
            }
            let children = st.entries.iter().filter(|e| !e.child.is_empty()).map(|e| (e.path.clone(), e.child.clone())).collect();
            (st.entries.iter().filter(|e| !values.contains_key(&e.path)).map(|e| e.path.clone()).collect(), st.path.clone(), children)
        };
        if paths.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            let items = crate::shell::columns::Items::new(&folder, children);
            let texts: Vec<(String, crate::shell::columns::SortValue)> = paths
                .into_iter()
                .map(|p| {
                    let value = crate::shell::columns::sort_value(&items, &p, &property);
                    (p, value)
                })
                .collect();
            post(move |app| {
                {
                    let mut st = app.state.borrow_mut();
                    let st = &mut *st;
                    let (values, values_key) = if for_group { (&mut st.group_texts, st.group_texts_key) } else { (&mut st.sort_texts, st.sort_texts_key) };
                    if values_key != Some(key) {
                        return;
                    }
                    values.extend(texts);
                }
                app.rebuild_order();
                app.sync_rows();
            });
        });
    }

    /// "Opções de pesquisa" > Data da modificação, Tipo, Tamanho: adds "keyword:value" to the
    /// query and searches at once. As Explorer does, when the keyword is already there the
    /// value goes in right after it ("espécie:folderdocument").
    pub fn search_refine(self: &Rc<Self>, keyword: &str, value: &str) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let text = shell.get_search_text().to_string();
        let tag = format!("{keyword}:");
        let query = match text.find(&tag) {
            Some(at) => format!("{}{value}{}", &text[..at + tag.len()], &text[at + tag.len()..]),
            None if text.trim().is_empty() => format!("{tag}{value}"),
            None => format!("{} {tag}{value}", text.trim_end()),
        };
        shell.set_search_text(query.as_str().into());
        self.search_now(&query);
    }

    /// Changes the window's search options and searches again.
    pub fn set_search_options(self: &Rc<Self>, change: impl FnOnce(&mut crate::shell::search::Options)) {
        let previous=self.state.borrow().search_options.zipped_folders;
        change(&mut self.state.borrow_mut().search_options);
        let zipped=self.state.borrow().search_options.zipped_folders;
        if zipped!=previous {
            if let Err(error)=crate::shell::search::set_archived_files(zipped) {
                self.state.borrow_mut().search_options.zipped_folders=previous;
                eprintln!("search archive preference: {error}");
                return;
            }
        }
        if ids::is_search(&self.state.borrow().path) {
            self.reload();
        }
    }

    /// Typing in the search box searches the folder (and its subfolders) after a pause, as a
    /// location of its own; clearing it goes back to the folder.
    /// The measured 26H2 Explorer has no history dropdown and leaves WordWheelQuery alone,
    /// even with a seeded legacy MRU. Do not write search history here (see the spec).
    pub fn search_edited(self: &Rc<Self>, text: &str) {
        if ids::is_settings(&self.current_path()) { return; }
        let query = text.trim().to_string();
        let weak = Rc::downgrade(self);
        self.search_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(500), move || {
            let Some(app) = weak.upgrade() else { return };
            app.search_now_focused(&query, false);
        });
    }

    /// Searches the folder for the query now (or goes back to it when the query is empty).
    fn search_now(self: &Rc<Self>, query: &str) {
        self.search_now_focused(query, true);
    }

    fn search_now_focused(self: &Rc<Self>, query: &str, focus_list: bool) {
        if ids::is_settings(&self.current_path()) { return; }
        let query = query.trim();
        let path = self.state.borrow().path.clone();
        let scope = ids::search_parts(&path).map(|(s, _)| s.to_string()).unwrap_or(path.clone());
        if query.is_empty() {
            if ids::is_search(&path) {
                self.navigate_focused(&scope, focus_list);
            }
            return;
        }
        let id = ids::search_id(&scope, query);
        // Retyping the existing query does not restart Explorer's crawl or discard
        // its selection; F5 and changed search options still explicitly reload it.
        if id==path {return;}
        if ids::is_search(&path) {
            // Refining the query keeps its view/columns/sort, rather than loading the
            // results type's Content defaults again. History entries retain their view.
            let view = self.state.borrow().views.get(&path).cloned();
            if let Some(view) = view { self.state.borrow_mut().views.insert(id.clone(),view); }
            // Refine in place rather than stacking one history entry per keystroke.
            {
                let mut st = self.state.borrow_mut();
                let cur = st.current;
                let tab = &mut st.tabs[cur];
                let index = tab.index;
                tab.history[index] = id;
            }
            self.open_tab_focused(false, focus_list);
        } else {
            self.navigate_focused(&id, focus_list);
        }
    }

    pub fn set_query(self: &Rc<Self>, query: &str) {
        self.state.borrow_mut().query = query.to_string();
        self.rebuild_order();
        self.sync_rows();
        self.sync_view_state();
    }

    pub fn set_view(self: &Rc<Self>, mode: i32) {
        if self.set_native_view(mode){return;}
        if ids::same(&self.current_path(),ids::GALLERY) {
            self.gallery_set_size(grid::mode_icon_size(mode));return;
        }
        self.set_view_sized(mode, grid::mode_icon_size(mode));
    }

    /// Ctrl+wheel: one step bigger (positive) or smaller through details, list, then
    /// every icon size Explorer stops at.
    /// Whether the wheel runs sideways here (the List view's columns).
    pub fn wheel_across(&self) -> bool {
        self.state.try_borrow().is_ok_and(|st| st.grid_column_major && Self::icons_spec(&st).is_some())
    }

    /// The List view's column lefts (the first one at 0: it comes back with the view's
    /// margin), when its columns overflow the view.
    fn column_lefts(&self) -> Vec<f32> {
        let st = self.state.borrow();
        if !st.grid_column_major || self.ui().global::<Shell>().get_grid_width() <= st.grid_viewport.0 {
            return Vec::new();
        }
        let mut lefts: Vec<f32> = st.cells.iter().map(|c| c[0]).collect();
        lefts.sort_by(f32::total_cmp);
        lefts.dedup();
        if let Some(first) = lefts.first_mut() {
            *first = 0.0;
        }
        lefts
    }

    /// The wheel in the List view: each notch brings the next column (or the one before) to
    /// the view's left edge, as far as the last column, past the end of the list.
    pub fn scroll_columns(&self, steps: i32) {
        let lefts = self.column_lefts();
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let max = lefts.last().copied().unwrap_or(0.0);
        let mut x = -shell.get_grid_scroll_x();
        for _ in 0..steps.unsigned_abs() {
            x = if steps < 0 {
                lefts.iter().copied().find(|&l| l > x + 0.5).unwrap_or(max)
            } else {
                lefts.iter().copied().rev().find(|&l| l < x - 0.5).unwrap_or(0.0)
            };
        }
        shell.set_grid_scroll_x(-x.clamp(0.0, max));
        self.rehover();
    }

    /// The List view's scrollbar moved (in columns): scrolls there, between columns in
    /// proportion.
    pub fn column_scrolled(&self) {
        let lefts = self.column_lefts();
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let at = (-shell.get_grid_column_value() / 48.0).max(0.0);
        let i = (at.floor() as usize).min(lefts.len().saturating_sub(1));
        let x = match (lefts.get(i), lefts.get(i + 1)) {
            (Some(&a), Some(&b)) => a + (b - a) * (at - i as f32),
            (Some(&a), None) => a,
            _ => 0.0,
        };
        shell.set_grid_scroll_x(-x);
    }

    /// Keeps the List view's scrollbar on the columns as the view scrolls (or lays out).
    pub fn sync_column_bar(&self) {
        let lefts = self.column_lefts();
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        shell.set_grid_column_count(lefts.len() as i32);
        let x = -shell.get_grid_scroll_x();
        let i = lefts.iter().rposition(|&l| l <= x + 0.01).unwrap_or(0);
        let at = match (lefts.get(i), lefts.get(i + 1)) {
            (Some(&a), Some(&b)) if b > a => i as f32 + ((x - a) / (b - a)).min(1.0),
            _ => i as f32,
        };
        let value = -at * 48.0;
        if (shell.get_grid_column_value() - value).abs() > 0.01 {
            shell.set_grid_column_value(value);
        }
    }

    pub fn zoom(self: &Rc<Self>, steps: i32) {
        let (mode, size) = {
            let st = self.state.borrow();
            if Self::drives_view(&st) || ids::same(&st.path, ids::HOME) {
                return;
            }
            (st.view_mode, st.icon_size)
        };
        let Some(at) = grid::zoom_index(mode, size) else { return };
        let to = (at as i32 + steps).clamp(0, grid::ZOOM_STEPS as i32 - 1) as usize;
        if to != at {
            let (mode, size) = grid::zoom_step(to);
            self.set_view_sized(mode, size);
        }
    }

    fn set_view_sized(self: &Rc<Self>, mode: i32, size: u32) {
        {
            let mut st = self.state.borrow_mut();
            st.view_mode = mode;
            st.icon_size = size;
        }
        // This PC leaves or enters its tiles: its groups are drawn the other way.
        if ids::same(&self.state.borrow().path, ids::THIS_PC) {
            self.rebuild_order();
        }
        self.remember_view(|v| {
            v.view_mode = Some(mode);
            v.icon_size = Some(size);
        });
        self.ui().global::<Shell>().set_list_scroll_y(0.0);
        self.ui().global::<Shell>().set_grid_scroll_x(0.0);
        self.sync_rows();
        self.sync_view_state();
        // The focused view may have been swapped for another one.
        self.ui().invoke_focus_list();
        let focused = self.state.borrow().focused.clone();
        if let Some(f) = focused {
            self.reveal(&f);
        }
    }
}

/// A tab title cut as the real cuts it: the characters whose glyphs fit whole in the room
/// (DirectWrite's widths, Segoe UI Variable Small 12px), no ellipsis ("txt - Resultados da
/// Pesquisa e" of "... em sorttest" in 173px).
fn fit_whole_glyphs(title: &str, room: f32, weight: i32) -> String {
    let width = |s: &str| crate::shell::text::width(s, "Segoe UI Variable Small", 12.0, weight);
    if width(title) <= room {
        return title.to_string();
    }
    let ends: Vec<usize> = title.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
    // The longest prefix that fits.
    let (mut lo, mut hi) = (0usize, ends.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if width(&title[..ends[mid - 1]]) <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 { String::new() } else { title[..ends[lo - 1]].to_string() }
}
