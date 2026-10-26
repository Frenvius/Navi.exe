//! Windows 11 style menus: command bar dropdowns, context menus and address bar dropdowns.

use std::rc::Rc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use super::actions;
use super::sort::SortKey;
use super::{ids, App, VIEW_MODES};
use crate::i18n::{tr, trf};
use crate::shell::listing::{self, ShellEntry};
use crate::{MenuButton, MenuEntry, Shell};

#[derive(Clone)]
pub enum Action {
    None,
    Navigate(String),
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
    /// "Agrupar por" a key, or "(Nenhum)".
    GroupBy(Option<SortKey>),
    GroupDescending(bool),
    /// "Mais...": the Choose Details dialog.
    ChooseDetails,
    /// A group header's menu: open or collapse that group (by its first row), or all.
    SetGroupOpen(usize, bool),
    SetGroupsOpen(bool),
    NewFolder,
    NewTextFile,
    NewShortcut,
    NewItem(usize),
    /// A "Novo" entry for another folder than the current one (navigation pane).
    NewItemIn(String, usize),
    SelectAll,
    SelectNone,
    InvertSelection,
    ToggleCompact,
    ToggleNavPane,
    NavOption(crate::shell::nav_options::Option,bool),
    ToggleDetailsPane,
    TogglePreviewPane,
    Zip(Vec<String>),
    /// Rotate images a quarter turn, clockwise when true.
    Rotate(Vec<String>, bool),
    /// "Compactar para" a format, by extension (".7z", ".tar"), or the wizard ("").
    Archive(Vec<String>, &'static str),
    Wallpaper(Vec<String>),
    /// A URI or program, through ShellExecute.
    Launch(String),
    /// An app's context menu command (or one of its subcommands) on items.
    AppCommand(windows::core::GUID, Option<usize>, Vec<String>),
    Undo,
    FolderOptions,
    FolderOption(listing::FolderOption, bool),
    Cut,
    Copy,
    /// Copy items other than the selection (a navigation pane folder).
    CopyItems(Vec<String>),
    Paste,
    Rename,
    Share(Vec<String>),
    Delete,
    /// Text to the clipboard (the address bar's "Copiar Endereço").
    CopyText(String),
    /// The address field: 1 cut, 2 copy, 3 paste, 4 select all, 5 edit.
    AddressCommand(i32),
    /// Forget the locations typed into the address bar.
    ClearAddressHistory,
    /// An entry of the tab's history.
    History(usize),
    /// The tab menu's commands, on the tab right-clicked.
    CloseTab(usize),
    /// Search results' "Fechar pesquisa".
    CloseSearch,
    /// Home's "Filtro": all files (0), or Word, Excel, PowerPoint, pictures, videos, PDF.
    HomeFilter(u8),
    GalleryFilter(usize),
    GalleryManage,
    /// The Recycle Bin's "Restaurar todos os itens" (true) or "... selecionados".
    RestoreBin(bool),
    /// "Opções de pesquisa": a "keyword:value" for the query; the folder only (true) or its
    /// subfolders; toggling system files (0), zipped folders (1), file contents (2).
    SearchRefine(String, String),
    SearchCurrentFolderOnly(bool),
    SearchToggle(u8),
    CloseOtherTabs(usize),
    CloseTabsRight(usize),
    DuplicateTab(usize),
}

pub struct Item {
    pub entry: MenuEntry,
    pub action: Action,
    pub sub: Vec<Item>,
    /// A check or radio item, on or off.
    pub toggle: bool,
    /// Its description, shown as a tooltip when the pointer rests on it.
    pub tip: String,
}

pub struct OpenMenu {
    pub owner: String,
    pub items: Vec<Item>,
    pub buttons: Vec<(MenuButton, Action)>,
    pub sub_parent: Option<usize>,
}

fn item(label: &str, icon: Image, action: Action) -> Item {
    Item {
        entry: MenuEntry { label: label.into(), icon, ..Default::default() },
        action,
        sub: Vec::new(),
        toggle: false,
        tip: String::new(),
    }
}

fn separator() -> Item {
    Item { entry: MenuEntry { separator: true, ..Default::default() }, action: Action::None, sub: Vec::new(), toggle: false, tip: String::new() }
}

/// A command's description in Explorer's CommandStore ("Windows.SortAscending": "Classificar os
/// itens de modo que os valores mais baixos fiquem na parte superior."), which its menus show
/// as the item's tooltip.
fn command_tip(command: &str) -> String {
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
    /// The tooltip of Explorer's command it stands for.
    fn tip(mut self, command: &str) -> Item {
        self.tip = command_tip(command);
        self
    }

    fn shortcut(mut self, s: &str) -> Item {
        self.entry.shortcut = s.into();
        self
    }
    fn bullet(mut self, on: bool) -> Item {
        self.entry.bullet = on;
        self.toggle = true;
        self
    }
    fn checked(mut self, on: bool) -> Item {
        self.entry.checked = on;
        self.toggle = true;
        self
    }
    fn disabled(mut self, off: bool) -> Item {
        self.entry.disabled = off;
        self
    }
    /// A Segoe Fluent Icons glyph in the icon's place.
    fn glyph(mut self, g: &str) -> Item {
        self.entry.glyph = g.into();
        self
    }
    fn submenu(mut self, items: Vec<Item>) -> Item {
        self.entry.submenu = true;
        self.sub = items;
        self
    }
}

fn button(label: &str, icon: Image, action: Action) -> (MenuButton, Action) {
    (MenuButton { label: label.into(), icon, disabled: false, key_tip: access_key(&action, false).into() }, action)
}

/// CommandBarFlyout access keys measured through UIA in pt-BR. MenuFlyout (the
/// command bar's Sort/View/New) exposes no AccessKey and must not invent badges.
fn access_key(action: &Action, background: bool) -> &'static str {
    let pt = crate::i18n::language() == "pt-BR";
    match action {
        Action::Cut => if pt { "O" } else { "T" },
        Action::Copy => "C",
        Action::Rename => if pt { "R1" } else { "M" },
        Action::Share(_) => if pt { "C1" } else { "S" },
        Action::Delete => if pt { "X" } else { "D" },
        Action::OpenSelection => if pt { "A" } else { "O" },
        Action::CopyPath(_) => if pt { "I2" } else { "A" },
        Action::Properties(_) => "P",
        Action::Undo => if pt { "D" } else { "U" },
        Action::ShellMenu(..) => if pt { if background { "R" } else { "R2" } } else { "W" },
        Action::Pin(..) => if pt { "F" } else { "F" },
        Action::Launch(s) if s.contains("wt.exe") => "T",
        _ => "",
    }
}

fn svg(app: &App, name: &str) -> Image {
    app.images.borrow_mut().svg(name)
}

fn entries_model(items: &[Item]) -> ModelRc<MenuEntry> {
    let background = items.iter().any(|i| i.entry.label == tr("menu.view"));
    let entry = |i: &Item| {
        let mut key = access_key(&i.action, background);
        if let Some(first) = i.sub.first() {
            key = match first.action {
                Action::View(_) => "X",
                Action::SortKey(_) => "C",
                Action::GroupBy(_) => "A",
                Action::NewItem(_) | Action::NewFolder => "V",
                Action::OpenWithApp(..) => "I1",
                _ => key,
            };
        }
        MenuEntry { key_tip: if i.entry.key_tip.is_empty() { key.into() } else { i.entry.key_tip.clone() }, toggle_submenu: i.toggle && i.entry.submenu, ..i.entry.clone() }
    };
    ModelRc::from(Rc::new(VecModel::from(items.iter().map(entry).collect::<Vec<_>>())))
}

/// Opens a menu at a window point; `flip_x` is where it ends when it has to open leftward.
fn popup_data(items: &[Item], buttons: &[(MenuButton, Action)], buttons_bottom: bool, min_width: f32, owner: &str, sub: bool) -> crate::PopupData {
    // Context menu submenus are MenuFlyouts too.
    let dropdown = is_dropdown(owner) || sub;
    let toggles = items.iter().any(|i| i.toggle);
    let icons = items.iter().any(|i| i.entry.icon.size().width > 0 || !i.entry.glyph.is_empty());
    let toggle_column = dropdown && toggles && icons;
    // (icon, label, separator): MenuFlyouts, the "Ver mais" overflow and context menus.
    // Offsets from the window edge, which DWM's border covers.
    let (icon_x, label_x, separator_height) = if owner == "nav-options" {
        (16.0,43.0,3.0)
    } else if !icons && !toggles {
        // Text only: the labels take the icon column.
        (16.0, 16.0, if dropdown { 3.0 } else { 9.0 })
    } else if owner == "more" && !sub {
        (13.0, 39.0, 9.0)
    } else if toggle_column {
        (44.0, 72.0, 3.0)
    } else if dropdown {
        (16.0, 44.0, 3.0)
    } else {
        (16.0, 43.0, 9.0)
    };
    crate::PopupData {
        keyboard: false,
        cloud:false,
        nav_options: owner == "nav-options",
        key_tips: !dropdown,
        items: entries_model(items),
        buttons: ModelRc::from(Rc::new(VecModel::from(buttons.iter().map(|b| b.0.clone()).collect::<Vec<_>>()))),
        buttons_bottom,
        min_width,
        label_x,
        icon_x,
        separator_height,
        // Inside the border: MenuFlyouts 2px, context menus 3px, the overflow menu 4px.
        padding: if owner == "nav-options" {4.0} else if owner == "more" && !sub { 5.0 } else if dropdown { 3.0 } else { 4.0 },
        // Overflow uses full-height plates, not an extra gap above its first item.
        extra_top: 0.0,
        right_padding: if owner == "more" && !sub { 13.0 } else { 16.0 },
        shortcut_width: if matches!(owner, "drive" | "nav-drive" | "library") { 52.0 } else { 0.0 },
        // The address bar's lists scroll past 320px (borders included).
        max_height: if matches!(owner, "crumbs" | "crumbs-more") { 320.0 } else { 0.0 },
        // The crumbs that don't fit list in 40px rows, a pixel apart.
        // The tab's menu has 33px rows.
        row_height: if owner == "crumbs-more" { 40.0 } else if owner == "tab" { 33.0 } else { 32.0 },
        row_gap: if owner == "crumbs-more" { 1.0 } else { 0.0 },
    }
}

/// Command bar and address bar dropdowns (MenuFlyout) versus context menus (CommandBarFlyout).
fn is_dropdown(owner: &str) -> bool {
    matches!(owner, "new" | "sort" | "view" | "more" | "filter" | "collection" | "search-options" | "crumbs" | "crumbs-more" | "address" | "address-text" | "history" | "tab" | "nav-options")
}

/// Command bar dropdowns hug their content; context menus keep the 200px minimum.
fn min_width(owner: &str) -> f32 {
    match owner {
        "new" | "sort" | "view" | "more" | "filter" | "collection" | "search-options" | "crumbs" | "crumbs-more" | "address" | "history" => 0.0,
        // The address field's text menu, as measured (wider than its rows need).
        "address-text" => 230.0,
        // The tab's menu, as measured.
        "tab" | "nav-options" => 215.0,
        "drive" | "nav-drive" | "library" => 287.0,
        _ => 200.0,
    }
}

pub fn open(app: &Rc<App>, owner: &str, x: f32, y: f32, flip_x: f32, items: Vec<Item>, buttons: Vec<(MenuButton, Action)>) {
    let ui = app.ui();
    let shell = ui.global::<Shell>();
    let dropdown = is_dropdown(owner);
    // A context menu's command bar goes on the side the menu opens from (see show_menu_popup).
    let mut data = popup_data(&items, &buttons, false, min_width(owner), owner, false);
    // Filesystem item menus reserve the native 348px body even when five fixed
    // command buttons or the currently visible extension labels need less room.
    if matches!(owner,"item"|"nav") && items.iter().any(|i|matches!(i.action,Action::CopyPath(_))){data.min_width=data.min_width.max(348.0);}
    data.keyboard = shell.get_keyboard_menu();
    shell.set_keyboard_menu(false);
    // The anchor is the button's left (Novo), center (Classificar, Visualizar) or right edge.
    let align = match owner {
        "sort" | "view" | "search-options" | "filter" | "collection" | "crumbs-more" => 0.5,
        "more" => 1.0,
        _ => 0.0,
    };
    // One pixel less padding, the rows in the same place (not for menus at the pointer).
    let at_pointer = matches!(owner, "address" | "address-text" | "history" | "nav-options");
    let y = if data.padding < 4.0 && !at_pointer { y + 1.0 } else { y };
    // A context menu without its command bar (the background's, with nothing to paste) opens
    // 2px under the pointer (opening up, it ends at it); one with the bar, at it.
    let gap = if owner == "nav-options" || (!dropdown && buttons.is_empty() && matches!(owner, "background")) { 2.0 } else { 0.0 };
    shell.set_menu_owner(owner.into());
    shell.set_menu_open(true);
    *app.menu.borrow_mut() = Some(OpenMenu { owner: owner.to_string(), items, buttons, sub_parent: None });
    app.show_menu_popup(data, x, y, flip_x, align, crate::frame::flyout_border(dropdown), (!dropdown || owner == "nav-options").then_some(gap));
}

pub fn dismiss(app: &Rc<App>) {
    let cloud=app.menu.borrow().as_ref().is_some_and(|m|m.owner=="cloud-status");
    *app.menu.borrow_mut() = None;
    app.close_popups();
    let ui = app.ui();
    let shell = ui.global::<Shell>();
    shell.set_menu_open(false);
    shell.set_sub_open(false);
    shell.set_menu_owner(SharedString::new());
    if cloud {
        // Clicking the provider's TouchArea clears Slint's logical focus. Giving
        // Win32 activation back alone leaves every main-window shortcut inert.
        // Explorer returns to the control focused before the provider flyout.
        let zone=shell.get_focus_zone();
        if zone=="commands" || zone=="pane-command" {
            shell.set_command_focus_request(shell.get_command_focus_request()+1);
        } else {
            ui.invoke_focus_target(if zone.is_empty(){"neutral".into()}else{zone});
        }
    }
}

/// The pointer came onto a menu row: its tooltip shows if it rests there a second.
pub fn hover_tip(app: &Rc<App>, level: i32, index: i32) {
    app.hide_tip();
    let text = {
        let menu = app.menu.borrow();
        let Some(menu) = menu.as_ref() else { return };
        let row = if level == 0 {
            menu.items.get(index.max(0) as usize)
        } else {
            menu.sub_parent.and_then(|p| menu.items.get(p)).and_then(|parent| parent.sub.get(index.max(0) as usize))
        };
        row.filter(|_| index >= 0).map(|r| r.tip.clone()).unwrap_or_default()
    };
    if text.is_empty() {
        return;
    }
    let weak = Rc::downgrade(app);
    app.tip_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(1000), move || {
        if let Some(app) = weak.upgrade() {
            app.show_tip(&text);
        }
    });
}

pub fn hover(app: &Rc<App>, level: i32, index: i32) {
    if level != 0 {
        return;
    }
    let refresh={
        let menu=app.menu.borrow();
        menu.as_ref().and_then(|m|m.items.get(index as usize)).and_then(|it|match &it.action{
            Action::AppCommand(id,None,paths) if !it.sub.is_empty()=>Some((*id,paths.clone())),_=>None
        })
    };
    if let Some((id,paths))=refresh {
        if let Some(command)=crate::shell::extmenu::refreshed_sharing(&id,&paths){
            let fresh=extension_item(app,&command,&paths);
            if let Some(row)=app.menu.borrow_mut().as_mut().and_then(|m|m.items.get_mut(index as usize)){*row=fresh;}
        }
    }
    let sub = {
        let mut menu = app.menu.borrow_mut();
        let Some(menu) = menu.as_mut() else { return };
        match menu.items.get(index as usize) {
            Some(it) if !it.sub.is_empty() && !it.entry.disabled => {
                if menu.sub_parent == Some(index as usize) {
                    return;
                }
                menu.sub_parent = Some(index as usize);
                Some(popup_data(&it.sub, &[], false, 0.0, &menu.owner, true))
            }
            _ => {
                menu.sub_parent = None;
                None
            }
        }
    };
    match sub {
        Some(data) => app.show_sub_popup(data, index),
        None => app.close_sub_popup(),
    }
}

pub fn activate(app: &Rc<App>, level: i32, index: usize) {
    let action = {
        let menu = app.menu.borrow();
        let Some(menu) = menu.as_ref() else { return };
        let list = if level == 0 {
            &menu.items
        } else {
            match menu.sub_parent.and_then(|p| menu.items.get(p)) {
                Some(parent) => &parent.sub,
                None => return,
            }
        };
        match list.get(index) {
            Some(it) if !it.entry.disabled && !it.entry.separator => it.action.clone(),
            _ => return,
        }
    };
    dismiss(app);
    run(app, action);
}

pub fn activate_button(app: &Rc<App>, index: usize) {
    let action = app.menu.borrow().as_ref().and_then(|m| m.buttons.get(index).map(|b| b.1.clone()));
    dismiss(app);
    if let Some(action) = action {
        run(app, action);
    }
}

fn run(app: &Rc<App>, action: Action) {
    match action {
        Action::GalleryFilter(index)=>app.gallery_filter(index),
        Action::GalleryManage=>{std::thread::spawn(||{if let Err(error)=crate::shell::extmenu::invoke_store_dialog("Windows.GalleryManageLocationsCommand"){eprintln!("Gallery locations: {error}");}});},
        Action::None => {}
        Action::CopyText(text) => super::clipboard::set_text(app.hwnd.get(), &text),
        Action::AddressCommand(c) => app.ui().global::<Shell>().set_address_command(c),
        Action::ClearAddressHistory => crate::shell::address::clear_typed_paths(),
        Action::History(index) => app.go_to_history(index),
        Action::CloseTab(index) => app.close_tab(index),
        Action::CloseSearch => actions::command(app, "close-search"),
        Action::HomeFilter(kind) => {
            app.state.borrow_mut().home_filter = kind;
            app.rebuild_order();
            app.sync_all();
        }
        Action::RestoreBin(all) => {
            let children: Vec<Vec<u8>> = if all {
                app.state.borrow().entries.iter().map(|e| e.child.clone()).collect()
            } else {
                app.selected_entries().iter().map(|e| e.child.clone()).collect()
            };
            // Restoring them all asks first, as Explorer does (a classic message box).
            if all && !crate::shell::ops::confirm(app.hwnd.get(), &crate::shell::listing::display_name_of(super::ids::RECYCLE_BIN), &crate::i18n::system_string("shell32.dll", 16917)) {
                return;
            }
            if let Err(e) = crate::shell::ops::restore_from_recycle_bin(app.hwnd.get(), &children) {
                eprintln!("restore from the Recycle Bin: {e}");
            }
            app.reload();
            // The bin's handler may finish after this: look again shortly.
            let weak = Rc::downgrade(app);
            slint::Timer::single_shot(std::time::Duration::from_millis(800), move || {
                if let Some(app) = weak.upgrade() {
                    app.reload();
                }
            });
        }
        Action::SearchRefine(keyword, value) => app.search_refine(&keyword, &value),
        Action::SearchCurrentFolderOnly(on) => app.set_search_options(|o| o.current_folder_only = on),
        Action::SearchToggle(which) => app.set_search_options(|o| match which {
            0 => o.system_files = !o.system_files,
            1 => o.zipped_folders = !o.zipped_folders,
            _ => o.file_contents = !o.file_contents,
        }),
        Action::CloseOtherTabs(index) => app.close_other_tabs(index),
        Action::CloseTabsRight(index) => app.close_tabs_right(index),
        Action::DuplicateTab(index) => app.duplicate_tab(index),
        Action::Navigate(id) => app.navigate(&id),
        Action::NewTab(paths) => actions::open_in_new_tab(app, paths),
        Action::NewWindow(paths) => actions::open_in_new_window(paths),
        Action::OpenSelection => actions::open_selection(app),
        Action::OpenWith(path) => actions::open_with(app, &path),
        Action::OpenWithApp(handler, paths) => {
            let _ = crate::shell::assoc::open_with(&handler, app.hwnd.get(), &paths);
        }
        Action::Pin(paths, on) => actions::set_pinned(app, paths, on),
        Action::PinSearch(id) => {
            if let Some((scope, query)) = ids::search_parts(&id) {
                let options = app.state.borrow().search_options;
                if let Ok(pidl) = crate::shell::search::folder(scope, query, options) {
                    let _ = crate::shell::nav::set_pinned_pidl(&pidl, true);
                    app.refresh_nav();
                }
            }
        }
        Action::CommandStore(name, paths) => {
            if let Err(e) = crate::shell::ops::command_store(app.hwnd.get(), name, &paths) { eprintln!("{name}: {e}"); }
        }
        Action::ShellVerb(name, paths) => {
            let folder = app.current_path();
            if let Err(e) = crate::shell::ops::invoke_verb(app.hwnd.get(), &folder, &paths, &name, false) { eprintln!("{name}: {e}"); }
        }
        Action::ToggleNav(row) => app.toggle_nav(row),
        Action::PinToStart(paths) => actions::pin_to_start(app, paths),
        Action::RunAsAdmin(paths) => actions::run_as_admin(app, paths),
        Action::OpenLocation(target) => actions::open_location(app, target),
        Action::CopyPath(paths) => actions::copy_as_path(app, paths),
        Action::Properties(paths) => actions::properties(app, paths),
        Action::ShellMenu(paths, x, y) => {
            // Let the XAML-style host finish hiding before entering TrackPopupMenu's
            // nested native loop; its queued focus/cancel messages otherwise dismiss it.
            let weak=Rc::downgrade(app);
            slint::Timer::single_shot(std::time::Duration::from_millis(30),move||{
                if let Some(app)=weak.upgrade(){actions::shell_menu(&app,paths,x,y,false);}
            });
        },
        Action::View(mode) => app.set_view(mode),
        Action::SortKey(key) => actions::set_sort_key(app, key),
        Action::SortDescending(d) => actions::set_sort_descending(app, d),
        Action::GroupBy(key) => actions::set_group(app, key),
        Action::GroupDescending(d) => actions::set_group_descending(app, d),
        Action::ChooseDetails => app.choose_details(),
        Action::NewFolder => actions::new_folder(app),
        Action::NewTextFile => actions::new_text_file(app),
        Action::NewShortcut => {
            let folder = app.state.borrow().path.clone();
            let _ = crate::shell::ops::new_shortcut(app.hwnd.get(), &folder);
        }
        Action::NewItem(i) => actions::new_item(app, i),
        Action::NewItemIn(folder, i) => actions::new_item_in(app, &folder, i),
        Action::SelectAll => actions::select_all(app),
        Action::SelectNone => actions::select_none(app),
        Action::InvertSelection => actions::invert_selection(app),
        Action::ToggleCompact => {
            let on = !app.state.borrow().compact;
            app.state.borrow_mut().compact = on;
            listing::set_option(listing::FolderOption::Compact, on);
            app.rebuild_order();
            app.sync_rows();
            app.sync_view_state();
        }
        Action::NavOption(option,on) => {
            let _=crate::shell::nav_options::set(option,on);
            app.refresh_nav();
        }
        Action::ToggleNavPane => actions::toggle_nav_pane(app),
        Action::ToggleDetailsPane => actions::toggle_details_pane(app),
        Action::TogglePreviewPane => actions::toggle_preview_pane(app),
        Action::Zip(paths) => actions::compress(app, paths),
        Action::Rotate(paths, clockwise) => actions::rotate(app, paths, clockwise),
        Action::Archive(paths, ext) => actions::compress_tar(app, paths, ext),
        Action::Wallpaper(paths) => actions::set_wallpaper(app, paths),
        Action::AppCommand(clsid, sub, paths) => {
            let _ = crate::shell::extmenu::invoke(&clsid, sub, &paths);
        }
        Action::Launch(target) => {
            let _ = crate::shell::ops::shell_execute(app.hwnd.get(), &target, None);
        }
        Action::Undo => super::undo::undo(app),
        Action::SetGroupOpen(row, open) => app.set_group_open(row, open),
        Action::SetGroupsOpen(open) => app.set_groups_open(open),
        Action::FolderOptions => {
            app.save_view();
            let (path,view)=app.view_snapshot();
            let ui=app.ui();
            let rect=(ui.get_view_width().round() as i32,ui.get_view_height().round() as i32);
            if let Err(e)=crate::shell::folder_options::show(app.hwnd.get(),path,view,rect,|changed|{
                super::post(move|a|{if changed {a.state.borrow_mut().views.clear();a.reload();}});
            }) {eprintln!("Folder Options: {e}");}
        }
        Action::FolderOption(option, on) => actions::set_folder_option(app, option, on),
        Action::Cut => actions::cut(app),
        Action::Copy => actions::copy(app),
        Action::CopyItems(paths) => actions::copy_items(app, paths),
        Action::Paste => actions::paste(app),
        Action::Rename => actions::start_rename(app),
        Action::Share(paths) => actions::share(app, paths),
        Action::Delete => actions::delete(app, false),
    }
}

// ------------------------------------------------------------------ menu contents

fn new_items(app: &App) -> Vec<Item> {
    new_items_in(app, None)
}

/// "Novo" for the current folder, or for `folder`.
fn new_items_in(app: &App, folder: Option<&str>) -> Vec<Item> {
    use crate::shell::icons::icon_for_extension;
    let location=folder.map(str::to_string).unwrap_or_else(||app.current_path());
    if let Ok(entries)=crate::shell::newmenu::entries(app.hwnd.get(),&location) {
        app.state.borrow_mut().new_items=entries;
    }
    let entries = app.state.borrow().new_items.clone();
    if entries.is_empty() && folder.is_none() {
        return vec![item(tr("context.new-folder"), svg(app, "newfolder"), Action::NewFolder)];
    }
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let icon = match e.verb.as_str() {
                "NewFolder" => crate::shell::icons::folder_icon(),
                "NewLink" => icon_for_extension(".lnk"),
                ext => icon_for_extension(ext),
            };
            let icon = app.images.borrow_mut().icon(icon, 16);
            let action = match folder {
                Some(f) => Action::NewItemIn(f.to_string(), i),
                None => Action::NewItem(i),
            };
            item(&e.label, icon, action)
        })
        .collect()
}

/// The folder type's columns that the command bar's "Classificar" offers, with their names.
fn type_columns(app: &App) -> Vec<(SortKey, String)> {
    let keys = app.state.borrow().menu_columns.clone();
    keys.iter().filter_map(|k| super::columns::label(k).map(|label| (SortKey::from_property(k), label))).collect()
}

/// The command bar's "Classificar" > "Agrupar por": all the folder type's columns, then
/// "(Nenhum)" once grouped.
fn group_items(app: &App) -> Vec<Item> {
    let group = app.state.borrow().group.map(|g| g.key);
    let mut items: Vec<Item> =
        type_columns(app).into_iter().map(|(k, label)| item(&label, Image::default(), Action::GroupBy(Some(k))).bullet(group == Some(k))).collect();
    if group.is_some() {
        items.push(item(tr("menu.group-none"), Image::default(), Action::GroupBy(None)));
    }
    items
}

/// The shown columns, in order, as the context menu names them (Date modified is shell32's
/// "Data da modificação" there, not the header's "Data de modificação").
fn shown_columns(app: &App) -> Vec<(SortKey, String)> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_DateModified;
    let st = app.state.borrow();
    st.cols
        .iter()
        .map(|c| (SortKey::from_property(&c.key), if c.key == PKEY_DateModified { tr("column.modified.group").to_string() } else { c.label.clone() }))
        .collect()
}

/// The context menu's "Classificar por": the shown columns, then the order.
fn context_sort_items(app: &App) -> Vec<Item> {
    let sort = app.state.borrow().sort;
    let mut items: Vec<Item> =
        shown_columns(app).into_iter().map(|(k, label)| item(&label, Image::default(), Action::SortKey(k)).bullet(sort.key == k)).collect();
    items.extend([
        separator(),
        item(tr("menu.ascending"), Image::default(), Action::SortDescending(false)).bullet(!sort.descending).tip("Windows.SortAscending"),
        item(tr("menu.descending"), Image::default(), Action::SortDescending(true)).bullet(sort.descending).tip("Windows.SortDescending"),
    ]);
    items
}

/// The context menu's "Agrupar por": the shown columns, "(Nenhum)" once grouped, the groups'
/// order and "Mais...".
fn context_group_items(app: &App) -> Vec<Item> {
    let group = app.state.borrow().group;
    let mut items: Vec<Item> = shown_columns(app)
        .into_iter()
        .map(|(k, label)| item(&label, Image::default(), Action::GroupBy(Some(k))).bullet(group.map(|g| g.key) == Some(k)))
        .collect();
    if group.is_some() {
        items.push(item(tr("menu.group-none"), Image::default(), Action::GroupBy(None)));
    }
    let descending = group.map(|g| g.descending);
    items.extend([
        separator(),
        item(tr("menu.ascending"), Image::default(), Action::GroupDescending(false)).bullet(descending == Some(false)).disabled(group.is_none()).tip("Windows.SortGroupsAscending"),
        item(tr("menu.descending"), Image::default(), Action::GroupDescending(true)).bullet(descending == Some(true)).disabled(group.is_none()).tip("Windows.SortGroupsDescending"),
        separator(),
        item(tr("menu.more-columns"), Image::default(), Action::ChooseDetails),
    ]);
    items
}

/// The folder type's first eight columns: three, then five under "Mais" (the shown ones
/// don't matter). A sort by any other column bullets none.
fn sort_items(app: &App) -> Vec<Item> {
    let sort = app.state.borrow().sort;
    let key = |k: SortKey, label: &str| item(label, Image::default(), Action::SortKey(k)).bullet(sort.key == k);
    let mut columns = type_columns(app);
    columns.truncate(8);
    let more = columns.split_off(columns.len().min(3));
    let more_open = more.iter().any(|(k, _)| *k == sort.key);
    let mut items: Vec<Item> = columns.iter().map(|(k, label)| key(*k, label)).collect();
    if !more.is_empty() {
        items.push(item(tr("menu.more"), Image::default(), Action::None).bullet(more_open).submenu(more.iter().map(|(k, label)| key(*k, label)).collect()));
    }
    items.extend([
        separator(),
        item(tr("menu.ascending"), Image::default(), Action::SortDescending(false)).bullet(!sort.descending),
        item(tr("menu.descending"), Image::default(), Action::SortDescending(true)).bullet(sort.descending),
    ]);
    items
}

/// The command bar's "Classificar": the sort, then "Agrupar por".
fn sort_menu_items(app: &App) -> Vec<Item> {
    let mut items = sort_items(app);
    items.extend([separator(), item(tr("menu.group-by"), Image::default(), Action::None).submenu(group_items(app))]);
    items
}

fn view_items(app: &App, with_show: bool) -> Vec<Item> {
    let (view, compact, nav_pane, details_pane, preview_pane) = {
        let st = app.state.borrow();
        (if ids::same(&st.path,ids::GALLERY){match st.gallery.icon_size{0..=31=>4,32..=95=>3,96..=255=>2,_=>1}}else{st.view_mode}, st.compact, st.nav_pane, st.details_pane, st.preview_pane)
    };
    let mut items: Vec<Item> = VIEW_MODES
        .iter()
        .filter(|(_,_,_,mode)|!ids::same(&app.current_path(),ids::GALLERY)||*mode<=4)
        .map(|(k, label, icon, mode)| {
            let it = item(tr(label), svg(app, icon), Action::View(*mode)).bullet(view == *mode);
            // The command bar's menu lists no shortcuts; the context menu's does.
            if with_show { it } else { it.shortcut(&format!("Ctrl+Shift+{k}")) }
        })
        .collect();
    if with_show {
        use listing::FolderOption;
        let opts = listing::settings();
        items.push(separator());
        items.push(item(tr("menu.details-pane"), svg(app, "windows.previewpane"), Action::ToggleDetailsPane).bullet(details_pane));
        items.push(item(tr("menu.preview-pane"), svg(app, "windows.readingpane"), Action::TogglePreviewPane).bullet(preview_pane));
        items.push(separator());
        items.push(item(tr("menu.show"), Image::default(), Action::None).submenu(vec![
            item(tr("menu.nav-pane"), svg(app, "windows.navpane"), Action::ToggleNavPane).checked(nav_pane).tip("Windows.navpane"),
            separator(),
            item(tr("menu.compact"), svg(app, "windows.compactmode"), Action::ToggleCompact).checked(compact).tip("Windows.CompactMode"),
            separator(),
            item(tr("menu.check-boxes"), svg(app, "windows.selectioncheckboxes"), Action::FolderOption(FolderOption::CheckBoxes, !opts.check_boxes))
                .checked(opts.check_boxes)
                .tip("Windows.SelectionCheckboxes"),
            item(tr("menu.extensions"), svg(app, "windows.showfileextensions"), Action::FolderOption(FolderOption::Extensions, opts.hide_extensions))
                .checked(!opts.hide_extensions)
                .tip("Windows.ShowFileExtensions"),
            item(tr("menu.hidden-items"), svg(app, "windows.showhiddenfiles"), Action::FolderOption(FolderOption::HiddenItems, !opts.show_hidden))
                .checked(opts.show_hidden)
                .tip("Windows.ShowHiddenFiles"),
        ]));
    }
    items
}

fn more_items(app: &App) -> Vec<Item> {
    let (selected, folder) = (app.selected_entries(), app.state.borrow().path.clone());
    let fs_selection = !selected.is_empty() && selected.iter().all(|e| e.is_fs);
    let fs_folder = !ids::is_virtual(&folder) && !folder.is_empty();
    let paths: Vec<String> = selected.iter().map(|e| e.path.clone()).collect();
    let mut items = Vec::new();
    let search = ids::is_search(&folder);
    let pc = ids::same(&folder, ids::THIS_PC);
    let library_folder = ids::is_library(&folder) || ids::same(&folder, ids::LIBRARIES);
    let library_selection = !selected.is_empty() && selected.iter().all(|e| ids::is_library(&e.path));
    let drive = if selected.len() == 1 && ids::is_drive_root(&selected[0].path) {
        Some(selected[0].path.clone())
    } else if selected.is_empty() && ids::is_drive_root(&folder) { Some(folder.clone()) } else { None };
    // Search results: "Fechar pesquisa" first when the command bar has no room for it.
    if search && !app.ui().global::<Shell>().get_close_search_in_bar() {
        let close = crate::i18n::system_string("shell32.dll", 37565);
        items.extend([item(&close, svg(app, "windows.searchclosetab"), Action::CloseSearch).tip("Windows.SearchCloseTab"), separator()]);
    }
    // The Recycle Bin: its restore commands first, and nothing to pin, zip or copy as path.
    let recycle = ids::same(&folder, ids::RECYCLE_BIN);
    if recycle {
        let shell32 = |id: u32| crate::i18n::system_string("shell32.dll", id);
        items.push(item(&shell32(31333), svg(app, "windows.recyclebin.restoreall"), Action::RestoreBin(true)).tip("Windows.RecycleBin.RestoreAll"));
        if !selected.is_empty() {
            items.push(item(&shell32(31335), svg(app, "windows.recyclebin.restoreitems"), Action::RestoreBin(false)).tip("Windows.RecycleBin.RestoreItems"));
        }
        items.push(separator());
    }
    if fs_selection && selected.iter().all(|e| !e.is_dir && crate::shell::preview::is_image(&e.path)) {
        items.push(item(tr("menu.rotate-left"), svg(app, "windows.rotate270"), Action::Rotate(paths.clone(), false)));
        items.push(item(tr("menu.rotate-right"), svg(app, "windows.rotate90"), Action::Rotate(paths.clone(), true)));
        items.push(separator());
    }
    items.push(item(tr("menu.undo"), svg(app, "windows.undo"), Action::Undo).disabled(!super::undo::can_undo(app)));
    if let Some(drive) = &drive {
        let native = crate::shell::menu::commands(app.hwnd.get(), ids::THIS_PC, &[drive.clone()]);
        let kind = unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(crate::shell::wide(drive).as_ptr())) };
        let first = items.len();
        items.push(separator());
        for (resource, icon, command) in [(37494, "windows.cleanup", "Windows.CleanUp"),
            (37490, "windows.defragment", "Windows.Defragment"), (37488, "windows.diskformat", "Windows.DiskFormat")] {
            if (command == "Windows.DiskFormat" && native.iter().any(|c| c.verb == "format")) ||
                (command != "Windows.DiskFormat" && matches!(kind, 2 | 3)) {
                items.push(item(&crate::i18n::system_string("shell32.dll", resource), svg(app, icon), Action::CommandStore(command, vec![drive.clone()])).tip(command));
            }
        }
        for c in native.iter().filter(|c| c.verb == "eject" || c.verb == "disconnect") {
            let icon = if c.verb == "eject" { "windows.eject" } else { "windows.disconnectnetworkdrive" };
            items.push(item(&c.title, svg(app, icon), Action::ShellVerb(c.verb.clone(), vec![drive.clone()])).disabled(!c.enabled));
        }
        if items.len() == first+1 { items.pop(); }
    }
    if pc {
        items.push(separator());
        for (resource, icon, command) in [(37600, "windows.addmediaserver", "Windows.AddMediaServer"),
            (31302, "windows.addnetworklocation", "Windows.AddNetworkLocation"),
            (31300, "windows.mapnetworkdrive", "Windows.MapNetworkDrive"),
            (31304, "windows.disconnectnetworkdrive", "Windows.DisconnectNetworkDrive")] {
            items.push(item(&crate::i18n::system_string("shell32.dll", resource), svg(app, icon), Action::CommandStore(command, vec![])).tip(command));
        }
    } else if !recycle {
        items.push(separator());
        if selected.is_empty() || library_selection {
            let action = if search { Action::PinSearch(folder.clone()) } else { Action::Pin(if library_selection { paths.clone() } else { vec![folder.clone()] }, true) };
            items.push(item(tr("menu.pin-home"), svg(app, "windows.pintohome"), action).disabled(!fs_folder && !search && !library_folder && !library_selection).tip("Windows.PinToHome"));
        } else if drive.is_none() {
            items.push(item(tr("menu.zip"), svg(app, "Windows.Zip.Action"), Action::Zip(paths.clone())).disabled(!fs_selection).tip("Windows.zip"));
            items.push(item(tr("menu.favorites"), svg(app, "windows.pintohomefile"), Action::Pin(paths.clone(), true)).disabled(!fs_selection).tip("Windows.PinToHomeFile"));
            items.push(item(tr("menu.copy-path"), svg(app, "windows.copyaspath"), Action::CopyPath(paths.clone())).disabled(!fs_selection).tip("Windows.copyaspath"));
        }
    }
    items.extend([separator(),
        item(tr("menu.select-all"), svg(app, "windows.selectall"), Action::SelectAll).tip("Windows.selectall"),
        item(tr("menu.select-none"), svg(app, "windows.selectnone"), Action::SelectNone).tip("Windows.selectnone"),
        item(tr("menu.invert-selection"), svg(app, "windows.invertselection"), Action::InvertSelection).tip("Windows.invertselection"),
    ]);
    items.extend([
        separator(),
        // Search results have no properties of their own: only the items'.
        item(tr("menu.properties"), svg(app, "windows.properties"), Action::Properties(app.selected_paths()))
            .disabled((search || ids::same(&folder, ids::LIBRARIES)) && selected.is_empty())
            .tip("Windows.properties"),
        item(tr("menu.options"), svg(app, "windows.folderoptions"), Action::FolderOptions).tip("Windows.folderoptions"),
    ]);
    items
}

pub fn command_menu(app: &Rc<App>, id: &str, x: f32, y: f32) {
    let items = match id {
        "new" => new_items(app),
        "sort" => sort_menu_items(app),
        "view" => view_items(app, true),
        "more" => more_items(app),
        "search-options" => search_options_items(app),
        "filter" => home_filter_items(app),
        "collection" => {
            let selected=app.state.borrow().gallery.filter_index;
            let labels=crate::shell::extmenu::store_subcommands("Windows.GalleryFilterCommand");
            let mut items=Vec::new();
            for(index,(label,icon))in labels.into_iter().enumerate(){if index==1{items.push(separator());}let icon=icon.as_ref().map(super::images::to_image).unwrap_or_default();items.push(item(&label,icon,Action::GalleryFilter(index)).bullet(index==selected));}
            items.push(separator());items.push(item(&crate::i18n::system_string("Windows.UI.FileExplorer.dll",51859),svg(app,"windows.gallerymanagelocationscommand"),Action::GalleryManage));
            items
        },
        _ => return,
    };
    open(app, id, x, y, x, items, Vec::new());
}

/// The menu for selected items, or for the folder background when `row` is -1.
pub fn list_menu(app: &Rc<App>, row: i32, x: f32, y: f32, shift: bool) {
    let targets: Vec<ShellEntry> = if row >= 0 { app.selected_entries() } else { Vec::new() };
    if !targets.is_empty() && targets.iter().all(|e|crate::shell::network::is_computer(&e.path)){
        let paths=targets.iter().map(|e|e.path.clone()).collect();let weak=Rc::downgrade(app);
        slint::Timer::single_shot(std::time::Duration::from_millis(30),move||{if let Some(app)=weak.upgrade(){actions::network_menu(&app,paths,x,y,shift);}});
        return;
    }
    if shift {
        actions::shell_menu(app, targets.iter().map(|e| e.path.clone()).collect(), x, y, true);
        return;
    }
    if targets.is_empty() {
        background_menu(app, x, y);
    } else {
        item_menu(app, &targets, Vec::new(), x, y);
    }
}

/// A group header right-clicked: its items are selected, and their menu leads with
/// collapsing the group (or opening it) and every group.
pub fn group_menu(app: &Rc<App>, row: usize, x: f32, y: f32) {
    app.group_clicked(row, false);
    let collapsed = app.state.borrow().folded.get(row).copied().unwrap_or(false);
    let first = if collapsed {
        item(tr("context.expand-group"), svg(app, "windows.expand"), Action::SetGroupOpen(row, true)).shortcut(tr("key.right"))
    } else {
        item(tr("context.collapse-group"), svg(app, "windows.collapse"), Action::SetGroupOpen(row, false)).shortcut(tr("key.left"))
    };
    let prefix = vec![
        first,
        item(tr("context.expand-all"), svg(app, "windows.expandall"), Action::SetGroupsOpen(true)),
        item(tr("context.collapse-all"), svg(app, "windows.collapseall"), Action::SetGroupsOpen(false)),
        separator(),
    ];
    let targets = app.selected_entries();
    item_menu(app, &targets, prefix, x, y);
}

fn item_menu(app: &Rc<App>, entries: &[ShellEntry], prefix: Vec<Item>, x: f32, y: f32) {
    let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
    if !paths.is_empty() && paths.iter().all(|p| ids::is_drive_root(p)) {
        drive_menu(app, &paths, prefix, x, y);
        return;
    }
    if !paths.is_empty() && paths.iter().all(|p| ids::is_library(p)) {
        library_menu(app, &paths, prefix, x, y);
        return;
    }
    let single = (entries.len() == 1).then(|| &entries[0]);
    let all_fs = entries.iter().all(|e| e.is_fs);
    let folders = entries.iter().all(|e| e.is_dir);

    let is_link = |e: &ShellEntry| e.path.to_lowercase().ends_with(".lnk");
    // A shortcut to a file is offered as that file: its apps, its "Executar como administrador".
    let target = single.filter(|e| !e.is_dir && e.is_fs).and_then(|e| if is_link(e) { crate::shell::ops::link_file(&e.path) } else { Some(e.path.clone()) });
    let target_file = target.clone().filter(|t| !std::path::Path::new(t).is_dir());
    // Programs and scripts open themselves: "Abrir" shows their own icon, and there's no
    // "Abrir com".
    let program = target_file.as_deref().is_some_and(|t| crate::shell::assoc::has_verb(t, "runas"));
    // "Abrir" shows the default app's icon; "Abrir com" lists the recommended apps.
    let apps = target_file.as_deref().filter(|_| !program).map(crate::shell::assoc::apps_for).unwrap_or_default();
    let app_image = |a: &crate::shell::assoc::App| a.icon.as_ref().map(super::images::to_image).unwrap_or_default();
    // Several items show the first one's app.
    let first_apps = if single.is_none() { entries.first().filter(|e| !e.is_dir && e.is_fs).map(|e| crate::shell::assoc::apps_for(&e.path)).unwrap_or_default() } else { Vec::new() };
    // Folders open in File Explorer, whose icon it shows.
    let open_icon = if folders && all_fs {
        app.images.borrow_mut().icon(crate::shell::icons::explorer_icon(), 16)
    } else if let Some(e) = single.filter(|_| program) {
        // The program's icon, without a shortcut's arrow.
        app.images.borrow_mut().icon(e.icon & 0x00FF_FFFF, 16)
    } else {
        apps.first().or(first_apps.first()).map(app_image).unwrap_or_default()
    };
    let mut items = prefix;
    items.push(item(tr("context.open"), open_icon, Action::OpenSelection).shortcut("Enter"));
    if folders {
        items.push(item(tr("context.open-tab"), svg(app, "windows.opennewtab"), Action::NewTab(paths.clone())));
        items.push(item(tr("context.open-window"), svg(app, "windows.opennewwindow"), Action::NewWindow(paths.clone())));
    }
    if let Some(t) = target_file.as_deref().filter(|_| !program) {
        // The submenu lists them by name; the default only gives "Abrir" its icon.
        let mut sorted: Vec<&crate::shell::assoc::App> = apps.iter().collect();
        sorted.sort_by_key(|a| a.name.to_lowercase());
        let mut sub: Vec<Item> = sorted
            .into_iter()
            .map(|a| item(&a.name, app_image(a), Action::OpenWithApp(a.clone(), paths.clone())))
            .collect();
        let ext = std::path::Path::new(t).extension().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        let store = crate::shell::assoc::store_icon().as_ref().map(super::images::to_image).unwrap_or_default();
        sub.push(item(tr("context.store"), store, Action::Launch(format!("ms-windows-store://assoc/?FileExt={ext}"))));
        sub.push(item(tr("context.other-app"), Image::default(), Action::OpenWith(t.to_string())));
        items.push(item(tr("context.open-with"), svg(app, "windows.openwith"), Action::None).submenu(sub));
    }
    if all_fs && !folders {
        for command in crate::shell::extmenu::sharing_commands(&paths) {
            items.push(extension_item(app,&command,&paths));
        }
    }
    if program {
        items.push(item(tr("context.run-as-admin"), svg(app, "RunAsAdmin"), Action::RunAsAdmin(paths.clone())));
    }
    // Search results add the containing-folder command. Shortcuts continue to reveal
    // their target, as in an ordinary folder (confirmed with a dummy .lnk).
    let location = if ids::is_search(&app.current_path()) {
        single.filter(|e| e.is_fs).map(|e| {
            if is_link(e) { target.clone().unwrap_or_else(||e.path.clone()) } else { e.path.clone() }
        })
    } else {
        target.clone().filter(|_| single.is_some_and(is_link))
    };
    if let Some(t) = location {
        let label=if single.is_some_and(|e|e.is_dir) { crate::i18n::system_string("shell32.dll",1040) } else { tr("context.open-location").to_string() };
        items.push(item(&label, svg(app, "windows.shortcut.opencontaining"), Action::OpenLocation(t)));
    }
    let images = all_fs && entries.iter().all(|e| !e.is_dir && crate::shell::preview::is_image(&e.path));
    if images {
        items.push(item(tr("context.background"), svg(app, "windows.setdesktopwallpaper"), Action::Wallpaper(paths.clone())));
        items.push(item(tr("menu.rotate-right"), svg(app, "windows.rotate90"), Action::Rotate(paths.clone(), true)));
        items.push(item(tr("menu.rotate-left"), svg(app, "windows.rotate270"), Action::Rotate(paths.clone(), false)));
    }
    if all_fs && folders {
        items.push(item(tr("menu.pin-home"), svg(app, "windows.pintohome"), Action::Pin(paths.clone(), true)));
    }
    // Start pins folders and programs (shortcuts to them too).
    let exe = |p: &str| p.to_lowercase().ends_with(".exe");
    let startable = |e: &ShellEntry| e.is_dir || exe(&e.path) || (is_link(e) && target.as_deref().is_some_and(exe));
    if let Some(e) = single.filter(|e| e.is_fs && startable(e)) {
        items.push(item(tr("context.pin-start"), svg(app, "PinToStartScreen"), Action::PinToStart(vec![e.path.clone()])));
    }
    if all_fs && !folders {
        items.push(item(tr("context.favorites"), svg(app, "windows.pintohomefile"), Action::Pin(paths.clone(), true)));
    }
    if all_fs {
        let formats = vec![
            item(tr("context.compress.zip"), Image::default(), Action::Zip(paths.clone())),
            item(tr("context.compress.7z"), Image::default(), Action::Archive(paths.clone(), ".7z")),
            item(tr("context.compress.tar"), Image::default(), Action::Archive(paths.clone(), ".tar")),
            item(tr("context.compress.options"), Image::default(), Action::Archive(paths.clone(), "")),
        ];
        items.push(item(tr("context.compress"), svg(app, "Windows.Zip.Action"), Action::None).submenu(formats));
    }
    if all_fs {
        items.push(item(tr("context.copy-as-path"), svg(app, "windows.copyaspath"), Action::CopyPath(paths.clone())).shortcut("Ctrl+Shift+C"));
    }
    items.push(item(tr("menu.properties"), svg(app, "windows.properties"), Action::Properties(paths.clone())).shortcut("Alt+Enter"));
    if all_fs {
        push_app_commands(app, &mut items, &paths, false);
    }
    items.push(separator());
    items.push(item(tr("context.more-options"), svg(app, "expandtoclassic"), Action::ShellMenu(paths.clone(), x, y)));

    let mut buttons = Vec::new();
    if all_fs {
        buttons.push(button(tr("context.cut"), svg(app, "windows.cut"), Action::Cut));
        buttons.push(button(tr("context.copy"), svg(app, "windows.copy"), Action::Copy));
        // Several items are renamed one after another; folders can't be shared.
        buttons.push(button(tr("menu.rename"), svg(app, "windows.rename"), Action::Rename));
        if !entries.iter().any(|e| e.is_dir) {
            buttons.push(button(tr("cmd.share"), svg(app, "Windows.ModernShare"), Action::Share(paths.clone())));
        }
        buttons.push(button(tr("context.delete"), svg(app, "windows.ribbondelete"), Action::Delete));
    }
    open(app, "item", x, y, x, items, buttons);
}

/// Libraries expose their native management/save-location controls in Properties on
/// this Explorer build; do not invent obsolete ribbon commands in the modern overflow.
fn library_menu(app: &Rc<App>, paths: &[String], mut items: Vec<Item>, x: f32, y: f32) {
    let icon = app.images.borrow_mut().icon(crate::shell::icons::explorer_icon(),16);
    items.push(item(tr("context.open"),icon,Action::OpenSelection).shortcut("Enter"));
    items.push(item(tr("context.open-tab"),svg(app,"windows.opennewtab"),Action::NewTab(paths.to_vec())));
    items.push(item(tr("context.open-window"),svg(app,"windows.opennewwindow"),Action::NewWindow(paths.to_vec())));
    items.push(item(tr("menu.pin-home"),svg(app,"windows.pintohome"),Action::Pin(paths.to_vec(),true)));
    items.push(item(tr("context.pin-start"),svg(app,"PinToStartScreen"),Action::PinToStart(paths.to_vec())));
    items.push(item(tr("menu.properties"),svg(app,"windows.properties"),Action::Properties(paths.to_vec())).shortcut("Alt+Enter"));
    items.push(separator());
    items.push(item(tr("context.more-options"),svg(app,"expandtoclassic"),Action::ShellMenu(paths.to_vec(),x,y)));
    let mut buttons=vec![button(tr("context.copy"),svg(app,"windows.copy"),Action::Copy)];
    if crate::shell::ops::can_paste(){buttons.push(button(tr("context.paste"),svg(app,"windows.paste"),Action::Paste));}
    buttons.push(button(tr("menu.rename"),svg(app,"windows.rename"),Action::Rename));
    buttons.push(button(tr("context.delete"),svg(app,"windows.ribbondelete"),Action::Delete));
    open(app,"library",x,y,x,items,buttons);
}

/// Drives have Copy/Rename, not the ordinary folder Cut/Delete/compress strip.
/// Querying IContextMenu only discovers capabilities; no device operation runs here.
fn drive_menu(app: &Rc<App>, paths: &[String], mut items: Vec<Item>, x: f32, y: f32) {
    let native = crate::shell::menu::commands(app.hwnd.get(), ids::THIS_PC, paths);
    let explorer = app.images.borrow_mut().icon(crate::shell::icons::explorer_icon(), 16);
    let mut open_row = item(tr("context.open"), explorer, Action::OpenSelection).shortcut("Enter");
    open_row.entry.key_tip = "A1".into();
    items.push(open_row);
    items.push(item(tr("context.open-tab"), svg(app, "windows.opennewtab"), Action::NewTab(paths.to_vec())));
    items.last_mut().unwrap().entry.key_tip = "G".into();
    items.push(item(tr("context.open-window"), svg(app, "windows.opennewwindow"), Action::NewWindow(paths.to_vec())));
    items.last_mut().unwrap().entry.key_tip = "E".into();
    items.extend(drive_shell_items(app, &native, paths));
    if let Some(c) = native.iter().find(|c| c.verb.eq_ignore_ascii_case("pintohome")) {
        let mut row = item(&c.title, svg(app, "windows.pintohome"), Action::Pin(paths.to_vec(), true)).disabled(!c.enabled);
        row.entry.key_tip = "A2".into();
        items.push(row);
    }
    if let Some(c) = native.iter().find(|c| c.verb.eq_ignore_ascii_case("PinToStartScreen")) {
        let mut row = item(&c.title, svg(app, "PinToStartScreen"), Action::PinToStart(paths.to_vec())).disabled(!c.enabled);
        row.entry.key_tip = "F".into();
        items.push(row);
    }
    items.push(item(tr("menu.properties"), svg(app, "windows.properties"), Action::Properties(paths.to_vec())).shortcut("Alt+Enter"));
    items.push(separator());
    items.push(item(tr("context.more-options"), svg(app, "expandtoclassic"), Action::ShellMenu(paths.to_vec(), x, y)));
    let mut buttons = vec![button(tr("context.copy"), svg(app, "windows.copy"), Action::Copy)];
    if crate::shell::ops::can_paste() {
        buttons.push(button(tr("context.paste"), svg(app, "windows.paste"), Action::Paste));
    }
    buttons.push(button(tr("menu.rename"), svg(app, "windows.rename"), Action::Rename));
    open(app, "drive", x, y, x, items, buttons);
}

fn drive_shell_items(app: &App, native: &[crate::shell::menu::Command], paths: &[String]) -> Vec<Item> {
    let mut items = Vec::new();
    for c in native {
        let (icon, key) = match c.verb.to_ascii_lowercase().as_str() {
            v if v.contains("-bde") => ("windows.bitlocker.manage", "K"),
            "format" => ("windows.diskformat", "T"),
            "eject" => ("windows.eject", "J"),
            "disconnect" => ("windows.disconnectnetworkdrive", "D"),
            _ => continue,
        };
        let mut row = item(&c.title, svg(app, icon), Action::ShellVerb(c.verb.clone(), paths.to_vec())).disabled(!c.enabled);
        row.entry.key_tip = key.into();
        items.push(row);
    }
    items
}

/// The installed apps' commands for the items, after a separator.
fn push_app_commands(app:&App, items: &mut Vec<Item>, paths: &[String], background: bool) {
    let commands = crate::shell::extmenu::commands(paths, background);
    if commands.is_empty() {
        return;
    }
    items.push(separator());
    for c in commands {
        items.push(extension_item(app,&c,paths));
    }
}

fn extension_item(app:&App,c:&crate::shell::extmenu::Command,paths:&[String])->Item {
    let icon=if c.subcommand.is_none() && c.clsid==windows::core::GUID::from_u128(0x2f788d0f_1317_441b_86d2_4725301bd49d){svg(app,"windows.sendtomyphone")}
        else if c.subcommand.is_none() && c.clsid==windows::core::GUID::from_u128(0x9ad244df_e071_4b65_b741_2a4c5f52bf32){svg(app,"Windows.ModernShare")}
        else{c.icon.as_ref().map(super::images::to_image).unwrap_or_default()};
    let mut row=item(&c.title,icon,Action::AppCommand(c.clsid,c.subcommand,paths.to_vec())).disabled(!c.enabled);
    row.entry.key_tip=c.access_key.clone().into();
    if !c.subs.is_empty(){row=row.submenu(c.subs.iter().filter(|s|!s.title.is_empty()).map(|s|extension_item(app,s,paths)).collect());}
    row
}

fn background_menu(app: &Rc<App>, x: f32, y: f32) {
    let (folder, can_paste) = {
        let st = app.state.borrow();
        (st.path.clone(), st.can_paste)
    };
    let fs = (!ids::is_virtual(&folder) || ids::is_library(&folder)) && !folder.is_empty();
    let mut items = vec![
        item(tr("menu.view"), svg(app, "View"), Action::None).submenu(view_items(app, false)),
        item(tr("menu.sort-by"), svg(app, "SortBy"), Action::None).submenu(context_sort_items(app)),
        item(tr("menu.group-by"), svg(app, "windows.groupbycolumn"), Action::None).submenu(context_group_items(app)),
        separator(),
    ];
    if let Some(label) = super::undo::label(app) {
        items.push(item(&trf("menu.undo-named", &[&label]), svg(app, "windows.undo"), Action::Undo).shortcut("Ctrl+Z"));
    }
    if fs || ids::same(&folder,ids::LIBRARIES) {
        items.push(item(tr("cmd.new"), svg(app, "windows.newitem"), Action::None).submenu(new_items(app)));
    }
    items.push(item(tr("menu.properties"), svg(app, "windows.properties"), Action::Properties(Vec::new())).shortcut("Alt+Enter"));
    if fs {
        push_app_commands(app, &mut items, &[folder.clone()], true);
    }
    items.push(separator());
    items.push(item(tr("context.more-options"), svg(app, "expandtoclassic"), Action::ShellMenu(Vec::new(), x, y)));
    let buttons = if can_paste && fs { vec![button(tr("context.paste"), svg(app, "windows.paste"), Action::Paste)] } else { Vec::new() };
    open(app, "background", x, y, x, items, buttons);
}

pub fn nav_options_menu(app:&Rc<App>,x:f32,y:f32) {
    use crate::shell::nav_options::{read,Option};
    let options=read();
    let items=vec![
        item(tr("nav.show-pc"),Image::default(),Action::NavOption(Option::ThisPc,!options.this_pc)).checked(options.this_pc),
        item(tr("nav.show-network"),Image::default(),Action::NavOption(Option::Network,!options.network)).checked(options.network),
        item(tr("nav.show-libraries"),Image::default(),Action::NavOption(Option::Libraries,!options.libraries)).checked(options.libraries),
        item(tr("nav.show-all"),Image::default(),Action::NavOption(Option::AllFolders,!options.all)).checked(options.all),
        item(tr("nav.expand-current"),Image::default(),Action::NavOption(Option::Expand,!options.expand)).checked(options.expand),
    ];
    open(app,"nav-options",x,y,x,items,Vec::new());
}

pub fn nav_menu(app: &Rc<App>, row: usize, x: f32, y: f32) {
    let Some((node, in_pins)) = app.nav_node(row) else { return };
    let pinned = in_pins && node.is_pinned;
    let ids_ = vec![node.id.clone()];
    let drive = ids::is_drive_root(&node.id);
    let mut items = vec![
        item(tr("context.open-tab"), svg(app, "windows.opennewtab"), Action::NewTab(ids_.clone())),
        item(tr("context.open-window"), svg(app, "windows.opennewwindow"), Action::NewWindow(ids_.clone())),
    ];
    if drive {
        let expanded = app.ui().global::<Shell>().get_nav().row_data(row).is_some_and(|n| n.expanded);
        items.insert(0, separator());
        items.insert(0, item(tr(if expanded {"context.collapse-group"} else {"context.expand-group"}), svg(app, if expanded {"windows.collapse"} else {"windows.expand"}), Action::ToggleNav(row)));
        let native = crate::shell::menu::commands(app.hwnd.get(), ids::THIS_PC, &ids_);
        items.extend(drive_shell_items(app, &native, &ids_));
    }
    if pinned {
        items.push(item(tr("menu.unpin-home"), svg(app, "windows.unpinfromhome"), Action::Pin(ids_.clone(), false)));
    } else {
        items.push(item(tr("menu.pin-home"), svg(app, "windows.pintohome"), Action::Pin(ids_.clone(), true)).disabled(!node.is_fs));
    }
    if drive {
        items.push(item(tr("context.pin-start"), svg(app, "PinToStartScreen"), Action::PinToStart(ids_.clone())));
    }
    if node.is_fs && !drive {
        items.push(item(tr("context.pin-start"), svg(app, "PinToStartScreen"), Action::PinToStart(ids_.clone())));
        let formats = vec![
            item(tr("context.compress.zip"), Image::default(), Action::Zip(ids_.clone())),
            item(tr("context.compress.7z"), Image::default(), Action::Archive(ids_.clone(), ".7z")),
            item(tr("context.compress.tar"), Image::default(), Action::Archive(ids_.clone(), ".tar")),
            item(tr("context.compress.options"), Image::default(), Action::Archive(ids_.clone(), "")),
        ];
        items.push(item(tr("context.compress"), svg(app, "Windows.Zip.Action"), Action::None).submenu(formats));
        items.push(item(tr("context.copy-as-path"), svg(app, "windows.copyaspath"), Action::CopyPath(ids_.clone())).shortcut("Ctrl+Shift+C"));
        let current = app.state.borrow().path == node.id;
        let new = new_items_in(app, (!current).then_some(node.id.as_str()));
        if !new.is_empty() {
            items.push(item(tr("cmd.new"), svg(app, "windows.newitem"), Action::None).submenu(new));
        }
    }
    items.push(item(tr("menu.properties"), svg(app, "windows.properties"), Action::Properties(ids_.clone())).shortcut("Alt+Enter"));
    if node.is_fs && !drive {
        push_app_commands(app, &mut items, &ids_, false);
    }
    items.push(separator());
    items.push(item(tr("context.more-options"), svg(app, "expandtoclassic"), Action::ShellMenu(ids_.clone(), x, y)));
    let buttons = if node.is_fs { vec![button(tr("context.copy"), svg(app, "windows.copy"), Action::CopyItems(ids_))] } else { Vec::new() };
    open(app, if drive { "nav-drive" } else { "nav" }, x, y, x, items, buttons);
}

/// Address bar dropdowns: the children of a crumb (or of the Desktop for the root chevron),
/// or the collapsed ancestors behind "…" (`index` -2).
/// The address bar's dropdowns: a crumb's subfolders, the desktop's items (root chevron,
/// -1) or the crumbs that don't fit (-2). Text only, like Explorer's.
pub fn crumb_menu(app: &Rc<App>, index: i32, x: f32, y: f32) {
    app.ui().global::<Shell>().set_crumb_open(index);
    if index == -2 {
        let hidden = app.ui().global::<Shell>().get_hidden_crumbs().max(0) as usize;
        let crumbs = app.state.borrow().crumbs.clone();
        let items: Vec<Item> = crumbs.iter().take(hidden).rev().map(|c| item(&c.name, Image::default(), Action::Navigate(c.id.clone()))).collect();
        open(app, "crumbs-more", x, y, x, items, Vec::new());
        return;
    }
    let folder = if index < 0 { String::new() } else { app.state.borrow().crumbs.get(index as usize).map(|c| c.id.clone()).unwrap_or_default() };
    std::thread::spawn(move || {
        let Ok(dirs) = listing::subfolders(&folder) else { return };
        super::post(move |app| {
            let items: Vec<Item> = dirs.into_iter().map(|(name, path)| item(&name, Image::default(), Action::Navigate(path))).collect();
            open(app, "crumbs", x, y, x, items, Vec::new());
        });
    });
}

/// A crumb's right-click menu: the current location's address (as Explorer copies it, as
/// text either way), editing it, or forgetting the typed ones.
pub fn address_menu(app: &Rc<App>, x: f32, y: f32) {
    let address = app.ui().global::<Shell>().get_address_text().to_string();
    let items = vec![
        item(tr("address.copy"), Image::default(), Action::CopyText(address.clone())),
        item(tr("address.copy-text"), Image::default(), Action::CopyText(address)),
        item(tr("address.edit"), Image::default(), Action::AddressCommand(5)),
        item(tr("address.delete-history"), Image::default(), Action::ClearAddressHistory),
    ];
    open(app, "address", x, y, x, items, Vec::new());
}

/// The address field's text menu.
pub fn address_text_menu(app: &Rc<App>, x: f32, y: f32) {
    let items = vec![
        item(tr("context.cut"), svg(app, "windows.cut"), Action::AddressCommand(1)).shortcut("Ctrl+X"),
        item(tr("context.copy"), svg(app, "windows.copy"), Action::AddressCommand(2)).shortcut("Ctrl+C"),
        item(tr("context.paste"), svg(app, "windows.paste"), Action::AddressCommand(3)).shortcut("Ctrl+V"),
        item(tr("address.select-all"), Image::default(), Action::AddressCommand(4)).shortcut("Ctrl+A"),
    ];
    open(app, "address-text", x, y, x, items, Vec::new());
}

/// Home's "Filtro" (explorerframe's strings): the kinds of files its lists show.
fn home_filter_items(app: &Rc<App>) -> Vec<Item> {
    let current = app.state.borrow().home_filter;
    [50292u32, 50293, 50294, 50295, 50296, 50313, 50314]
        .iter()
        .enumerate()
        .map(|(i, &id)| item(&crate::i18n::system_string("explorerframe.dll", id), Image::default(), Action::HomeFilter(i as u8)).bullet(current == i as u8))
        .collect()
}

/// The search's keyword for a property: its display name in lower case without spaces
/// ("Data de modificação" is "datademodificação", Kind's "Espécie" "espécie").
fn search_keyword(key: &windows::Win32::Foundation::PROPERTYKEY) -> String {
    crate::shell::columns::describe(key).map(|c| c.label.to_lowercase().replace(' ', "")).unwrap_or_default()
}

/// The first of a propsys.dll query word's spellings ("mês passado|último mês").
fn query_word(id: u32) -> String {
    crate::i18n::system_string("propsys.dll", id).split('|').next().unwrap_or_default().trim().to_string()
}

/// "Opções de pesquisa" (search results): where to search, conditions to add to the query (a
/// bullet on those it has), what to include, and the indexing options.
fn search_options_items(app: &Rc<App>) -> Vec<Item> {
    use windows::Win32::Storage::EnhancedStorage::{PKEY_DateModified, PKEY_Kind, PKEY_Size};
    let shell32 = |id: u32| crate::i18n::system_string("shell32.dll", id);
    let options = app.state.borrow().search_options;
    let query = app.ui().global::<Shell>().get_search_text().to_string();
    let (date_key, kind_key, size_key) = (search_keyword(&PKEY_DateModified), search_keyword(&PKEY_Kind), search_keyword(&PKEY_Size));
    // The values are radio items (their labels in the bullet column's place).
    let value = |label: &str, key: &str, value: String| {
        let on = query.contains(&format!("{key}:{value}"));
        item(label, Image::default(), Action::SearchRefine(key.to_string(), value)).bullet(on)
    };
    // Hoje, Ontem, Esta semana, Semana passada, Este mês, Último mês, Este Ano, Último ano.
    let dates: Vec<Item> = [(106, 39260), (105, 39261), (500, 39265), (104, 39266), (501, 39270), (502, 39271), (503, 39275), (504, 39276)]
        .iter()
        .map(|&(label, word)| value(&crate::i18n::system_string("propsys.dll", label), &date_key, query_word(word)))
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
        item(&shell32(37539), svg(app, "windows.searchoptiondeep"), Action::SearchCurrentFolderOnly(false)).bullet(!options.current_folder_only).tip("Windows.SearchOptionDeep"),
        item(&shell32(37541), svg(app, "windows.searchoptionshallow"), Action::SearchCurrentFolderOnly(true)).bullet(options.current_folder_only).tip("Windows.SearchOptionShallow"),
        separator(),
        item(&shell32(37545), svg(app, "windows.searchfilterdate"), Action::None).submenu(dates).bullet(has(&date_key)).tip("Windows.SearchFilterDate"),
        item(&shell32(37547), svg(app, "windows.searchfilterkind"), Action::None).submenu(kinds).bullet(has(&kind_key)).tip("Windows.SearchFilterKind"),
        item(&shell32(37549), svg(app, "windows.searchfiltersize"), Action::None).submenu(sizes).bullet(has(&size_key)).tip("Windows.SearchFilterSize"),
        separator(),
        item(&shell32(37559), svg(app, "windows.searchoptionsystem"), Action::SearchToggle(0)).checked(options.system_files).tip("Windows.SearchOptionSystem"),
        item(&shell32(37561), svg(app, "windows.searchoptioncompressed"), Action::SearchToggle(1)).checked(options.zipped_folders).tip("Windows.SearchOptionCompressed"),
        item(&shell32(37557), svg(app, "windows.searchoptioncontents"), Action::SearchToggle(2)).checked(options.file_contents).tip("Windows.SearchOptionContents"),
        separator(),
        // The Indexing Options control panel.
        item(&shell32(37535), svg(app, "windows.changeindexedlocations"), Action::Launch("shell:::{87D66A43-7B11-4A28-9811-C86EE395ACF7}".into())).tip("Windows.ChangeIndexedLocations"),
    ]
}

/// A tab's right-click menu, as Explorer's: close it (Ctrl+W), the others, those to its
/// right, or duplicate it.
pub fn tab_menu(app: &Rc<App>, index: usize, x: f32, y: f32) {
    let count = app.state.borrow().tabs.len();
    let items = vec![
        item(tr("tab.close"), Image::default(), Action::CloseTab(index)).glyph("\u{E711}").shortcut("Ctrl+W"),
        item(tr("tab.close-others"), Image::default(), Action::CloseOtherTabs(index)).disabled(count < 2),
        item(tr("tab.close-right"), Image::default(), Action::CloseTabsRight(index)).disabled(index + 1 >= count),
        item(tr("tab.duplicate"), Image::default(), Action::DuplicateTab(index)).glyph("\u{F5ED}"),
    ];
    // Its border is on the pointer's row.
    open(app, "tab", x, y - 1.0, x, items, Vec::new());
}

/// Back's (or Forward's) right-click menu: the tab's history that way, nearest first.
pub fn history_menu(app: &Rc<App>, back: bool, x: f32, y: f32) {
    let entries: Vec<(usize, String)> = {
        let st = app.state.borrow();
        let tab = &st.tabs[st.current];
        let range: Vec<usize> = if back { (0..tab.index).rev().collect() } else { (tab.index + 1..tab.history.len()).collect() };
        range.into_iter().map(|i| (i, tab.history[i].clone())).collect()
    };
    if entries.is_empty() {
        return;
    }
    let items = entries
        .into_iter()
        .map(|(i, id)| item(&listing::display_name_of(&id), Image::default(), Action::History(i)))
        .collect();
    open(app, "history", x, y, x, items, Vec::new());
}
