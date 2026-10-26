//! UI callbacks and the file operations behind commands, menus and shortcuts.

use std::collections::HashSet;
use std::rc::Rc;

use slint::platform::Key;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::menus;
use super::sort::{Sort, SortKey};
use super::{ids, is, App, AfterLoad, VIEW_DETAILS, VIEW_LARGE, VIEW_MODES};
use crate::shell::listing::ShellEntry;
use crate::shell::{menu, nav, ops};
use crate::{AppWindow, Shell};

macro_rules! cb {
    ($app:expr, |$a:ident $(, $arg:pat_param)*| $body:expr) => {{
        let weak = Rc::downgrade($app);
        move |$($arg),*| match weak.upgrade() {
            Some($a) => $body,
            None => Default::default(),
        }
    }};
}

pub fn bind(app: &Rc<App>, ui: &AppWindow) {
    let shell = ui.global::<Shell>();
    shell.on_settings_open(cb!(app,|a|a.open_settings()));
    shell.on_settings_refresh(cb!(app,|a|a.refresh_settings()));
    shell.on_default_toggle(cb!(app,|a,on|a.set_default_manager(on)));
    shell.on_cloud_open(cb!(app,|a,x,y,keyboard|a.cloud_open(x,y,keyboard)));
    shell.on_native_view_focus(cb!(app,|a|a.focus_native_view()));
    shell.on_native_viewport(cb!(app,|a,x,y,w,h|a.native_viewport(x,y,w,h)));
    shell.on_network_settings(cb!(app,|a| {let _=crate::shell::ops::shell_execute(a.hwnd.get(),"ms-settings:network-advancedsettings",None);}));
    shell.on_search_hits(|text,query| ModelRc::new(VecModel::from(super::columns::search_hits(&text,&query))));

    shell.on_caption_layout(|end| crate::frame::set_tabs_end(end));
    shell.on_minimize(cb!(app, |a| a.window_command(windows::Win32::UI::WindowsAndMessaging::SC_MINIMIZE)));
    shell.on_toggle_maximize(cb!(app, |a| a.toggle_maximize()));
    shell.on_close(cb!(app, |a| a.window_command(windows::Win32::UI::WindowsAndMessaging::SC_CLOSE)));

    shell.on_tab_selected(cb!(app, |a, i| a.select_tab(i as usize)));
    shell.on_tab_reordered(cb!(app, |a, from, to| a.reorder_tab(from as usize, to as usize)));
    shell.on_tab_transfer(cb!(app, |a,index,kind,x,y,local| super::tab_transfer::pointer(&a,index,kind,x,y,local)));
    shell.on_tab_drop_layout(cb!(app, |a, left, top, pitch| {
        a.tab_drop_layout.set(super::drop::TabDropLayout { left, top, pitch });
    }));
    shell.on_tab_closed(cb!(app, |a, i| a.close_tab(i as usize)));
    shell.on_tab_added(cb!(app, |a| a.new_tab(&ids::start_location())));
    shell.on_tab_context(cb!(app, |a, i, x, y| menus::tab_menu(&a, i as usize, x, y)));

    shell.on_back(cb!(app, |a| a.back()));
    shell.on_forward(cb!(app, |a| a.forward()));
    shell.on_up(cb!(app, |a| a.up()));
    shell.on_refresh(cb!(app, |a| {if !a.refresh_native_view(){a.reload();}}));
    shell.on_navigate_crumb(cb!(app, |a, i| {
        let id = a.state.borrow().crumbs.get(i as usize).map(|c| c.id.clone());
        if let Some(id) = id {
            a.navigate(&id);
        }
    }));
    shell.on_crumb_menu(cb!(app, |a, i, x, y| menus::crumb_menu(&a, i, x, y)));
    shell.on_crumb_width(cb!(app, |a, i, w| {
        let changed = {
            let mut st = a.state.borrow_mut();
            let i = i as usize;
            if st.crumb_widths.len() <= i {
                st.crumb_widths.resize(i + 1, 0.0);
            }
            let changed = (st.crumb_widths[i] - w).abs() > 0.1;
            st.crumb_widths[i] = w;
            changed
        };
        if changed {
            a.fit_crumbs();
        }
    }));
    shell.on_crumb_spot(cb!(app, |a, i, x, y| {
        let mut st = a.state.borrow_mut();
        let i = i as usize;
        if st.crumb_spots.len() <= i {
            st.crumb_spots.resize(i + 1, (0.0, 0.0));
        }
        st.crumb_spots[i] = (x, y);
    }));
    shell.on_trail_width(cb!(app, |a, w| {
        a.state.borrow_mut().trail_width = w;
        a.fit_crumbs();
    }));
    // After the crumbs are back on screen: a "not found" box comes up over them.
    shell.on_address_commit(cb!(app, |_a, text| {
        let text = text.to_string();
        slint::Timer::single_shot(std::time::Duration::from_millis(30), move || super::with_app(|a| address_commit(a, &text)));
    }));
    // Editing the address: the typed locations at first, then what matches the text.
    shell.on_address_edit_started(cb!(app, |a, _text| set_suggestions(&a, crate::shell::address::typed_paths())));
    shell.on_address_edited(cb!(app, |a, text| set_suggestions(&a, crate::shell::address::suggestions(&text))));
    shell.on_history_menu(cb!(app, |a, back, x, y| menus::history_menu(&a, back, x, y)));
    shell.on_address_context(cb!(app, |a, x, y| menus::address_menu(&a, x, y)));
    shell.on_address_text_context(cb!(app, |a, x, y| menus::address_text_menu(&a, x, y)));
    shell.on_suggestion_chosen(cb!(app, |a, i| {
        let chosen = a.ui().global::<Shell>().get_suggestions().row_data(i as usize);
        if let Some(text) = chosen {
            address_commit(&a, &text);
        }
    }));
    shell.on_search_edited(cb!(app, |a, text| a.search_edited(&text)));

    shell.on_command(cb!(app, |a, id| command(&a, &id)));
    shell.on_focus_noted(cb!(app, |a, zone| a.note_focus(&zone)));
    shell.on_focus_step(cb!(app, |a, reverse, fast| a.step_focus(reverse,fast)));
    shell.on_focus_exit(cb!(app, |a| a.ui().invoke_focus_target("neutral".into())));
    shell.on_command_edge(cb!(app, |a, last| a.focus_command(last)));
    shell.on_open_menu(cb!(app, |a, id, x, y| menus::command_menu(&a, &id, x, y)));
    shell.on_command_step(cb!(app, |a, step| {
        let ui = a.ui();
        let s = ui.global::<Shell>();
        let commands = a.command_ids();
        let current = commands.iter().position(|&c| c == s.get_command_focus().as_str()).unwrap_or(0);
        let next = (current as i32 + step).clamp(0,commands.len() as i32-1) as usize;
        s.set_command_focus(commands[next].into());
    }));

    shell.on_nav_clicked(cb!(app, |a, row| a.nav_clicked(row as usize)));
    shell.on_nav_drag_start(cb!(app, |a,row|a.start_nav_drag(row as usize)));
    shell.on_nav_key(cb!(app, |a, e| a.nav_key(&e.text, e.modifiers.control)));
    shell.on_nav_toggled(cb!(app, |a, row| a.toggle_nav(row as usize)));
    shell.on_nav_middle(cb!(app, |a, row| {
        if let Some((node, _)) = a.nav_node(row as usize) {
            a.new_tab(&node.id);
        }
    }));
    shell.on_nav_context(cb!(app, |a, row, x, y| if row < 0 {menus::nav_options_menu(&a,x,y)} else {menus::nav_menu(&a, row as usize, x, y)}));
    shell.on_nav_lines(cb!(app, |a, n| a.nav_scroll_by(n)));
    shell.on_nav_pages(cb!(app, |a, n| a.nav_pages(n)));
    shell.on_nav_dragged(cb!(app, |a| a.nav_snap()));
    shell.on_sidebar_resized(cb!(app, |a, w| {
        let max = a.ui().window().size().to_logical(a.ui().window().scale_factor()).width - 200.0;
        let w = w.clamp(60.0, max.max(60.0));
        a.state.borrow_mut().nav_width = w;
        a.ui().global::<Shell>().set_sidebar_width(w);
    }));

    shell.on_header_filter(cb!(app, |a, column, x, y| a.open_filter(column as usize, x, y)));
    shell.on_filter_toggle(cb!(app, |a, index| a.filter_toggle(index as usize)));
    shell.on_filter_only(cb!(app, |a, index| a.filter_only(index as usize)));
    shell.on_filter_dismissed(cb!(app, |a| a.close_filter()));
    shell.on_sort_clicked(cb!(app, |a, column| {
        if !a.header_click_sorts() {
            return;
        }
        let current = a.state.borrow().sort;
        let Some(key) = a.state.borrow().cols.get(column as usize).map(|c| SortKey::from_property(&c.key)) else { return };
        let next = if current.key == key && !a.state.borrow().search_ranked {
            Sort { key, descending: !current.descending }
        } else {
            Sort { key, descending: key.descending_first() }
        };
        a.set_sort(next);
    }));
    shell.on_row_pointer(cb!(app, |a, row, e| {
        if e.kind == slint::private_unstable_api::re_exports::PointerEventKind::Down {
            a.hide_key_tip();
        }
        a.row_pointer(row as usize, e.button, e.kind, e.modifiers.control, e.modifiers.shift)
    }));
    shell.on_group_clicked(cb!(app, |a, row, ctrl| a.group_clicked(row as usize, ctrl)));
    shell.on_group_toggled(cb!(app, |a, row| a.toggle_group(row as usize)));
    shell.on_gallery_viewport(cb!(app, |a,w,h|a.gallery_viewport(w,h)));
    shell.on_gallery_scrub(cb!(app, |a,y,initial|a.gallery_scrub(y,initial)));
    shell.on_gallery_scroll(cb!(app, |a,delta|a.gallery_scroll(delta)));
    shell.on_gallery_date_at(cb!(app, |a,y|a.gallery_date_at(y)));
    shell.on_library_empty_toggled(cb!(app, |a, label| {
        { let mut st=a.state.borrow_mut(); if !st.collapsed.remove(label.as_str()) {st.collapsed.insert(label.to_string());} }
        a.rebuild_order(); a.sync_rows(); a.sync_view_state();
    }));
    shell.on_group_title(|label|label.split_once(super::GROUP_SUBTITLE).map_or(label.as_str(),|(title,_)|title).into());
    shell.on_group_subtitle(|label|label.split_once(super::GROUP_SUBTITLE).map_or("",|(_,subtitle)|subtitle).into());
    shell.on_item_hovered(cb!(app, |a, row| a.item_hovered(row as usize)));
    shell.on_row_hover(cb!(app, |a, row, over, x, y| a.row_hover(row as usize, over, x, y)));
    shell.on_underline_widths(|text,size,limit| ModelRc::from(Rc::new(VecModel::from(crate::shell::text::underline_widths_at(&text,size as i32,limit)))));
    shell.on_tile_label(|text,width| {
        let label=crate::shell::text::icon_label(&text,width as i32);
        let mut lines=label.full.lines().map(str::to_owned).collect::<Vec<_>>();
        if lines.len()>1 && text.get(lines[0].len()..).is_some_and(|tail| tail.starts_with(' ')) {lines[0].push(' ');}
        if lines.len()>2 {lines.truncate(2);lines[1].push('…');}
        lines.join("\n").into()
    });
    shell.on_row_check(cb!(app, |a, row| a.row_check(row as usize)));
    shell.on_check_all(cb!(app, |a, on| a.check_all(on)));
    shell.on_row_double_clicked(cb!(app, |a, row| {
        if a.state.borrow().click_settings.single_click { return; }
        let entry = {
            let st = a.state.borrow();
            st.order.get(row as usize).map(|&i| st.entries[i].clone())
        };
        if let Some(entry) = entry {
            open_entry(&a, &entry);
        }
    }));
    shell.on_background_pointer(cb!(app, |a, e| a.background_pointer(e.button, e.kind, e.modifiers.control, e.modifiers.shift)));
    shell.on_band_changed(cb!(app, |a, x, y, w, h, _, _| a.band(x, y, w, h)));
    shell.on_list_key(cb!(app, |a, e| {
        let before = a.state.borrow().focused.clone();
        let used = a.list_key(&e.text, e.modifiers.control, e.modifiers.shift, e.modifiers.alt);
        a.key_moved(before);
        used
    }));
    shell.on_list_context(cb!(app, |a, row, x, y, shift| menus::list_menu(&a, row, x, y, shift)));
    shell.on_key_menu(|row, x, y| {
        slint::Timer::single_shot(std::time::Duration::ZERO, move || super::with_app(|a| {
            a.ui().global::<Shell>().set_keyboard_menu(true);
            menus::list_menu(a, row, x, y, false);
        }));
    });
    shell.on_group_context(cb!(app, |a, row, x, y| menus::group_menu(&a, row as usize, x, y)));
    shell.on_rename_finished(cb!(app, |a, commit, text| finish_rename(&a, commit, &text)));
    shell.on_list_nudge(cb!(app, |a, px| a.nudge_list(px)));
    shell.on_list_viewport(cb!(app, |a, h| {
        a.state.borrow_mut().viewport = h;
        a.sync_list_window(false);
        // Just after a folder loaded with an item to show (Abrir local do arquivo, the folder
        // we came from), it stays in view as the list's height settles.
        let focused = {
            let st = a.state.borrow();
            st.reveal_until.is_some_and(|t| std::time::Instant::now() < t).then(|| st.focused.clone()).flatten()
        };
        if let Some(f) = focused {
            a.reveal(&f);
        }
    }));
    shell.on_list_scrolled(cb!(app, |a| {
        a.sync_list_window(false);
        a.sync_grid_window(false);
        a.load_fields();
        a.load_thumbnails();
    }));
    shell.on_list_lines(cb!(app, |a, n| a.list_lines(n)));
    shell.on_list_pages(cb!(app, |a, n| a.list_pages(n)));
    shell.on_grid_columns(cb!(app, |a, n| a.state.borrow_mut().columns = n.max(1) as usize));
    shell.on_drives_viewport(cb!(app, |a, w, h| {
        let weak=Rc::downgrade(&a);
        slint::Timer::single_shot(std::time::Duration::ZERO,move||{if let Some(a)=weak.upgrade(){a.drives_viewport(w,h);}});
    }));
    shell.on_drives_thumb(cb!(app, |a, value| a.drives_thumb(value)));
    shell.on_drag_start(cb!(app, |a| a.start_drag()));
    ui.global::<crate::PreviewInfo>().on_geometry(cb!(app, |a, x, y, w, h| a.set_preview_geometry(x, y, w, h)));
    let pane = ui.global::<crate::Pane>();
    pane.on_share(cb!(app, |a| share(&a, a.selected_paths())));
    pane.on_properties(cb!(app, |a| properties(&a, a.selected_paths())));
    shell.on_name_measured(cb!(app, |a, name, width, lines| a.name_measured(&name, width, lines)));
    shell.on_grid_column_scrolled(cb!(app, |a| a.column_scrolled()));
    shell.on_grid_scrolled(cb!(app, |a| {
        a.sync_column_bar();
        a.sync_grid_window(false);
        a.load_fields();
        a.load_thumbnails();
    }));
    shell.on_column_resized(cb!(app, |a, index, width| {
        // Name (-1) sets its width in the UI; it only marks the view changed.
        if index < 0 {
            a.remember_view(|_| {});
        } else {
            a.column_resized(index as usize, width);
        }
    }));
    shell.on_header_pointer(cb!(app, |a, column, phase, x| a.header_pointer(column as usize, phase, x)));
    shell.on_header_context(cb!(app, |a, column, x, y| a.header_menu(column, x, y)));
    shell.on_column_fit(cb!(app, |a, column| a.fit_columns(Some(column as usize))));
    shell.on_header_width(cb!(app, |a, row, width| a.set_header_width(row as usize, width)));
    shell.on_grid_viewport(cb!(app, |a, w, h| a.set_grid_viewport(w, h)));

    shell.on_menu_activated(cb!(app, |a, level, index| menus::activate(&a, level, index as usize)));
    shell.on_menu_button(cb!(app, |a, index| menus::activate_button(&a, index as usize)));
    shell.on_menu_hover(cb!(app, |a, level, index| menus::hover(&a, level, index)));
    shell.on_menu_dismissed(cb!(app, |a| menus::dismiss(&a)));

    ui.on_global_key(cb!(app, |a, e| global_key(&a, &e.text, e.modifiers.control, e.modifiers.shift, e.modifiers.alt)));
}

impl App {
    pub fn window_command(&self, cmd: u32) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_SYSCOMMAND};
        unsafe {
            let _ = PostMessageW(Some(self.hwnd.get()), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
        }
    }

    pub fn toggle_maximize(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{IsZoomed, SC_MAXIMIZE, SC_RESTORE};
        let zoomed = unsafe { IsZoomed(self.hwnd.get()).as_bool() };
        self.window_command(if zoomed { SC_RESTORE } else { SC_MAXIMIZE });
    }

    pub fn new_window(&self) {
        open_in_new_window(vec![self.current_path()]);
    }
}

/// Shortcuts that work anywhere in the window.
pub(super) fn global_key(app: &Rc<App>, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    if is(text,Key::Escape) && !ctrl && !alt && app.cancel_search(){return true;}
    let lower = text.to_lowercase();
    if alt && !ctrl && is(text, Key::LeftArrow) {
        app.back();
    } else if alt && !ctrl && is(text, Key::RightArrow) {
        app.forward();
    } else if alt && !ctrl && is(text, Key::UpArrow) {
        app.up();
    } else if alt && !ctrl && lower == "p" {
        if shift { toggle_details_pane(app) } else { toggle_preview_pane(app) }
    } else if is(text, Key::F11) && !ctrl && !alt {
        let full = crate::frame::toggle_full_screen(app.hwnd.get());
        app.ui().global::<Shell>().set_full_screen(full);
    } else if is(text, Key::F5) || (ctrl && !alt && lower == "r") {
        if !app.refresh_native_view(){app.reload();}
    } else if ctrl && !alt && lower == "t" {
        app.new_tab(&ids::start_location());
    } else if ctrl && !alt && !shift && ('1'..='9').contains(&text.chars().next().unwrap_or(' ')) && text.chars().count() == 1 {
        // Ctrl+1..9 picks that tab, when there is one (9 isn't the last, as in browsers).
        app.select_tab(text.chars().next().unwrap() as usize - '1' as usize);
    } else if ctrl && !alt && !shift && lower == "n" {
        let path = app.state.borrow().path.clone();
        open_in_new_window(vec![path]);
    } else if ctrl && !alt && lower == "w" {
        let current = app.state.borrow().current;
        app.close_tab(current);
    } else if ctrl && is(text, Key::Tab) {
        app.cycle_tab(if shift { -1 } else { 1 });
    } else if ctrl && shift && !alt && lower == "e" {
        app.nav_expand_to_current();
    } else if ctrl && shift && !alt {
        // Ctrl+Shift+1..8 picks a view; the text is the shifted symbol on most layouts.
        let digit = match text {
            "1" | "!" => 1,
            "2" | "@" => 2,
            "3" | "#" => 3,
            "4" | "$" => 4,
            "5" | "%" => 5,
            "6" | "¨" | "^" => 6,
            "7" | "&" => 7,
            "8" | "*" => 8,
            _ => return false,
        };
        app.set_view(digit);
    } else {
        return false;
    }
    true
}

pub fn command(app: &Rc<App>, id: &str) {
    match id {
        "cut" => cut(app),
        "copy" => copy(app),
        "paste" => paste(app),
        // The compressed folder's own wizard ("Extrair Pastas Compactadas"), its "extract" verb.
        "extract-all" => {
            let path=app.state.borrow().path.clone();
            let paths=if let Some(root)=crate::shell::archive::root(&path){vec![root.to_string()]}else{app.selected_paths()};
            verb(app, "extract", &paths, false);
        }
        // Back to the folder searched.
        "close-search" => {
            let scope = ids::search_parts(&app.state.borrow().path).map(|(scope, _)| scope.to_string());
            if let Some(scope) = scope {
                app.navigate(&scope);
            }
        }
        "empty-bin" => {
            ops::empty_recycle_bin(app.hwnd.get());
            app.reload();
        }
        "rename" => start_rename(app),
        "share" => share(app, app.selected_paths()),
        "delete" => delete(app, false),
        "wallpaper" => set_wallpaper(app, app.selected_paths()),
        "details-pane" => {
            if app.state.borrow().preview_kind {
                toggle_preview_pane(app)
            } else {
                toggle_details_pane(app)
            }
        }
        "view-details" => app.set_view(VIEW_DETAILS),
        "view-large" => app.set_view(VIEW_LARGE),
        _ => {}
    }
}

fn set_suggestions(app: &Rc<App>, list: Vec<String>) {
    let ui = app.ui();
    let shell = ui.global::<Shell>();
    shell.set_suggest_index(-1);
    shell.set_suggestions(ModelRc::from(Rc::new(VecModel::from(list.into_iter().map(SharedString::from).collect::<Vec<_>>()))));
}

fn address_commit(app: &Rc<App>, text: &str) {
    let text = text.trim().trim_matches('"').to_string();
    let (path, location) = {
        let st = app.state.borrow();
        (st.path.clone(), st.crumbs.last().map(|c| c.name.clone()).unwrap_or_default())
    };
    if text.is_empty() || text == path || text == location {
        return;
    }
    let mut expanded = expand_env(&text);
    // A name of the desktop's or This PC's ("Lixeira", "Downloads") goes there.
    if crate::shell::Pidl::parse(&expanded).is_err() {
        let named = [String::new(), ids::THIS_PC.to_string()].iter().find_map(|parent| {
            crate::shell::listing::subfolders(parent).ok()?.into_iter().find(|(name, _)| name.to_lowercase() == expanded.to_lowercase()).map(|(_, path)| path)
        });
        if let Some(path) = named {
            expanded = path;
        }
    }
    match crate::shell::listing::crumbs(&expanded) {
        Ok(chain) => {
            // A location typed in goes first among the typed ones, as written.
            crate::shell::address::push_typed_path(&text);
            if let Some(last) = chain.last() {
                let target = if last.id.is_empty() { String::new() } else { last.id.clone() };
                if crate::shell::ops::is_folder(&target) || target.is_empty() || ids::is_virtual(&target) {
                    app.navigate(&target);
                } else {
                    let _ = ops::open(app.hwnd.get(), &target);
                }
            }
        }
        Err(_) => {
            // Not a location: let the shell run it (commands, URLs), like Explorer's address
            // bar, which says so when there is nothing by that name either.
            if ops::shell_execute(app.hwnd.get(), &expanded, Some(&path)).is_err() {
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
                // shell32's "Windows can't find '%1'. Check the spelling and try again."
                let message = crate::shell::wide(&crate::i18n::system_format("shell32.dll", 12353, &[&text]));
                let title = crate::shell::wide(crate::i18n::tr("app.title"));
                unsafe {
                    MessageBoxW(
                        Some(app.hwnd.get()),
                        windows::core::PCWSTR(message.as_ptr()),
                        windows::core::PCWSTR(title.as_ptr()),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        }
    }
}

fn expand_env(text: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    let w = crate::shell::wide(text);
    let mut buf = vec![0u16; 4096];
    let n = unsafe { ExpandEnvironmentStringsW(windows::core::PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return text.to_string();
    }
    String::from_utf16_lossy(&buf[..n - 1])
}

pub fn open_entry(app: &Rc<App>, entry: &ShellEntry) {
    if entry.is_dir {
        if crate::shell::listing::settings().new_window {open_in_new_window(vec![entry.path.clone()]);}
        else {app.navigate(&entry.path);}
        return;
    }
    if let Ok(Some(target)) = ops::open(app.hwnd.get(), &entry.path) {
        if crate::shell::listing::settings().new_window {open_in_new_window(vec![target]);}
        else {app.navigate(&target);}
    }
}

pub fn open_selection(app: &Rc<App>) {
    let entries = app.selected_entries();
    if entries.len() == 1 && entries[0].is_dir {
        open_entry(app, &entries[0]);
        return;
    }
    for e in entries.iter().filter(|e| !e.is_dir) {
        open_entry(app, e);
    }
}

/// Another window of this app at each folder.
pub fn open_in_new_window(paths: Vec<String>) {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    for p in paths {
        if let Err(error)=std::process::Command::new(&exe).arg("--new-window").arg(&p).creation_flags(0x08000000).spawn(){eprintln!("New window at {p}: {error}");}
    }
}

pub fn pin_to_start(app: &Rc<App>, paths: Vec<String>) {
    verb(app, "pintostartscreen", &paths, false);
}

pub fn open_in_new_tab(app: &Rc<App>, paths: Vec<String>) {
    for p in paths {
        app.new_tab(&p);
    }
}

pub fn run_as_admin(app: &Rc<App>, paths: Vec<String>) {
    verb(app, "runas", &paths, false);
}

/// "Abrir local do arquivo": a result (or shortcut target) selected in its folder, in this tab.
pub fn open_location(app: &Rc<App>, target: String) {
    let Some(parent) = std::path::Path::new(&target).parent().map(|p| p.to_string_lossy().to_string()) else { return };
    app.state.borrow_mut().after_load = Some(AfterLoad { select: target, rename: false });
    app.navigate(&parent);
}

fn verb(app: &Rc<App>, name: &str, paths: &[String], shift: bool) -> bool {
    let folder = app.state.borrow().path.clone();
    ops::invoke_verb(app.hwnd.get(), &folder, paths, name, shift).is_ok()
}

pub fn refresh_paste_state(app: &Rc<App>) {
    let can = ops::can_paste();
    app.state.borrow_mut().can_paste = can;
    app.ui().global::<Shell>().set_can_paste(can);
}

pub fn cut(app: &Rc<App>) {
    let paths = app.selected_paths();
    if paths.is_empty() {
        return;
    }
    if verb(app, "cut", &paths, false) {
        app.state.borrow_mut().cut = paths.into_iter().collect();
        app.sync_selection();
        refresh_paste_state(app);
    }
}

pub fn copy(app: &Rc<App>) {
    copy_items(app, app.selected_paths());
}

pub fn copy_items(app: &Rc<App>, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    if verb(app, "copy", &paths, false) {
        app.state.borrow_mut().cut.clear();
        app.sync_selection();
        refresh_paste_state(app);
    }
}

pub fn paste(app: &Rc<App>) {
    // Files pasted into the folder: done at once, and what they became told apart, for
    // "Desfazer Mover" and "Desfazer Copiar".
    let dest = app.state.borrow().path.clone();
    let sources = ops::clipboard_files();
    let before = (!sources.is_empty()).then(|| ops::snapshot(&dest)).flatten();
    if verb(app, "paste", &[], false) {
        if let Some(before) = before {
            super::undo::record_transfer(app, ops::transfer(&sources, &dest, &before));
        }
        app.state.borrow_mut().cut.clear();
        refresh_paste_state(app);
        app.reload();
    }
}

pub fn delete(app: &Rc<App>, permanent: bool) {
    let paths = app.selected_paths();
    if paths.is_empty() {
        return;
    }
    verb(app, "delete", &paths, permanent);
    // To the Recycle Bin: "Desfazer Excluir" puts them back. (A cancelled delete leaves an
    // undo that finds nothing.)
    if !permanent && paths.iter().all(|p| !ids::is_virtual(p)) {
        super::undo::push(app, super::undo::Undo::Delete(paths.clone()));
    }
    app.reload();
}

pub fn properties(app: &Rc<App>, paths: Vec<String>) {
    if !paths.is_empty() {
        verb(app, "properties", &paths, false);
        return;
    }
    let folder = app.state.borrow().path.clone();
    let parent = ids::parent_of(&folder);
    let _ = ops::invoke_verb(app.hwnd.get(), &parent, &[folder], "properties", false);
}

pub fn share(app: &Rc<App>, paths: Vec<String>) {
    if !paths.is_empty() {
        verb(app, "Windows.ModernShare", &paths, false);
    }
}

pub fn open_with(app: &Rc<App>, path: &str) {
    let _ = ops::open_with(app.hwnd.get(), path);
}

pub fn copy_as_path(app: &Rc<App>, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let text = paths.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join("\r\n");
    super::clipboard::set_text(app.hwnd.get(), &text);
}

pub fn set_pinned(app: &Rc<App>, paths: Vec<String>, pinned: bool) {
    for p in &paths {
        let _ = nav::set_pinned(p, pinned);
    }
    app.refresh_nav();
}

pub fn start_rename(app: &Rc<App>) {
    let target = {
        let st = app.state.borrow();
        match &st.focused {
            Some(f) if st.selected.contains(f) => Some(f.clone()),
            _ => st.order.iter().map(|&i| &st.entries[i].path).find(|p| st.selected.contains(*p)).cloned(),
        }
    };
    if let Some(target) = target {
        app.state.borrow_mut().renaming = Some(target.clone());
        app.reveal(&target);
        app.sync_view_state();
    }
}

fn finish_rename(app: &Rc<App>, commit: bool, text: &str) {
    let Some(path) = app.state.borrow_mut().renaming.take() else { return };
    app.sync_view_state();
    let name = app.state.borrow().entries.iter().find(|e| e.path == path).map(|e| e.name.clone()).unwrap_or_default();
    let text = text.trim();
    let others: Vec<String> = {
        let st = app.state.borrow();
        st.order.iter().map(|&i| &st.entries[i].path).filter(|p| st.selected.contains(*p)).cloned().collect()
    };
    if commit && !text.is_empty() && others.len() > 1 && others.contains(&path) {
        rename_many(app, &path, &others, text);
    } else if commit && !text.is_empty() && text != name {
        if let Ok(next) = ops::rename(app.hwnd.get(), &path, text) {
            super::undo::push(app, super::undo::Undo::Rename(vec![(path.clone(), next.clone())]));
            app.state.borrow_mut().after_load = Some(AfterLoad { select: next, rename: false });
            app.reload();
        }
    }
    app.ui().invoke_focus_list();
}

/// F2 over several items: each takes the typed name (less the edited item's extension)
/// with its own extension and a number, counted per resulting name in view order:
/// "novo (1).txt", "novo (2).txt", "novo (1).md".
fn rename_many(app: &Rc<App>, edited: &str, paths: &[String], text: &str) {
    use std::collections::HashMap;
    let ext_of = |p: &str| {
        let path = std::path::Path::new(p);
        if path.is_dir() { String::new() } else { path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default() }
    };
    let edited_ext = ext_of(edited);
    let stem = if !edited_ext.is_empty() && text.to_lowercase().ends_with(&edited_ext.to_lowercase()) {
        &text[..text.len() - edited_ext.len()]
    } else {
        text
    };
    let mut counters: HashMap<String, u32> = HashMap::new();
    let mut done = Vec::new();
    for path in paths {
        let ext = ext_of(path);
        let parent = std::path::Path::new(path).parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let counter = counters.entry(ext.to_lowercase()).or_insert(0);
        // Skip numbers taken by items outside the selection.
        let name = loop {
            *counter += 1;
            let name = format!("{stem} ({counter}){ext}");
            let target = parent.join(&name);
            if !target.exists() || paths.iter().any(|p| std::path::Path::new(p) == target) {
                break name;
            }
        };
        if let Ok(next) = ops::rename(app.hwnd.get(), path, &name) {
            done.push((path.clone(), next));
        }
    }
    if let Some((_, first)) = done.first().cloned() {
        super::undo::push(app, super::undo::Undo::Rename(done));
        app.state.borrow_mut().after_load = Some(AfterLoad { select: first, rename: false });
        app.reload();
    }
}

fn create_and_rename(app: &Rc<App>, created: Result<String, String>) {
    if let Ok(path) = created {
        super::undo::push(app, super::undo::Undo::Create(path.clone()));
        app.state.borrow_mut().after_load = Some(AfterLoad { select: path, rename: true });
        app.reload();
    }
}

/// "Compactar para arquivo ZIP": the new archive is selected for renaming, like Explorer.
pub fn compress(app: &Rc<App>, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    create_and_rename(app, ops::compress_to_zip(app.hwnd.get(), &paths));
}

/// "Compactar para > Arquivo 7z / TAR / Opções adicionais": Explorer's own archive commands
/// (with their progress window); bsdtar off the UI thread if they can't be created.
pub fn compress_tar(app: &Rc<App>, paths: Vec<String>, ext: &'static str) {
    let _ = app;
    if paths.is_empty() {
        return;
    }
    let clsid = match ext {
        ".7z" => "{21DEF7E5-832F-44D1-9109-E1161753B062}",
        ".tar" => "{3DA1A190-B430-49F5-86BF-765D29F7B9F0}",
        _ => "{812EE37B-5065-4C14-B0D1-A2266454B0C1}",
    };
    if ops::explorer_command(clsid, &paths).is_ok() || ext.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let created = ops::compress_with_tar(&paths, ext);
        super::post(move |app| create_and_rename(app, created));
    });
}

pub fn set_wallpaper(app: &Rc<App>, paths: Vec<String>) {
    if !paths.is_empty() {
        verb(app, "setdesktopwallpaper", &paths, false);
    }
}

/// The image handler's own rotate verbs, as Explorer's commands use them.
pub fn rotate(app: &Rc<App>, paths: Vec<String>, clockwise: bool) {
    if paths.is_empty() {
        return;
    }
    verb(app, if clockwise { "rotate90" } else { "rotate270" }, &paths, false);
    {
        let mut images = app.images.borrow_mut();
        for p in &paths {
            images.forget_thumbs(p);
        }
    }
    app.reload();
}

pub fn new_folder(app: &Rc<App>) {
    let folder = app.state.borrow().path.clone();
    let folder = crate::shell::library::save_folder(&folder).unwrap_or(folder);
    if ids::is_virtual(&folder) || folder.is_empty() {
        return;
    }
    create_and_rename(app, ops::new_folder(&folder));
}

pub fn new_text_file(app: &Rc<App>) {
    let folder = app.state.borrow().path.clone();
    let folder = crate::shell::library::save_folder(&folder).unwrap_or(folder);
    if ids::is_virtual(&folder) || folder.is_empty() {
        return;
    }
    create_and_rename(app, ops::new_file(&folder, crate::i18n::tr("file.new-text"), ".txt"));
}

/// A "Novo" entry: the shell creates the item; the reload then selects it for renaming.
pub fn new_item(app: &Rc<App>, index: usize) {
    let folder = app.state.borrow().path.clone();
    let entry = app.state.borrow().new_items.get(index).cloned();
    let Some(entry) = entry else { return };
    if (ids::is_virtual(&folder) && !ids::is_library(&folder) && !ids::same(&folder,ids::LIBRARIES)) || folder.is_empty() {
        return;
    }
    let before: HashSet<String> = app.state.borrow().entries.iter().map(|e| e.path.clone()).collect();
    app.state.borrow_mut().before_new = Some(before);
    if crate::shell::newmenu::create(app.hwnd.get(), &folder, &entry.verb).is_ok() {
        app.reload();
    } else {
        app.state.borrow_mut().before_new = None;
    }
}

/// A "Novo" entry in another folder (the navigation pane's): made there, without renaming.
pub fn new_item_in(app: &Rc<App>, folder: &str, index: usize) {
    if folder == app.state.borrow().path {
        return new_item(app, index);
    }
    let entry = app.state.borrow().new_items.get(index).cloned();
    if let Some(entry) = entry {
        let _ = crate::shell::newmenu::create(app.hwnd.get(), folder, &entry.verb);
    }
}

pub fn select_all(app: &Rc<App>) {
    let all: HashSet<String> = {
        let st = app.state.borrow();
        st.order.iter().map(|&i| st.entries[i].path.clone()).collect()
    };
    let focused = app.state.borrow().focused.clone();
    app.set_selection(all, None, focused);
}

pub fn select_none(app: &Rc<App>) {
    app.set_selection(HashSet::new(), None, None);
}

pub fn invert_selection(app: &Rc<App>) {
    let next: HashSet<String> = {
        let st = app.state.borrow();
        st.order.iter().map(|&i| st.entries[i].path.clone()).filter(|p| !st.selected.contains(p)).collect()
    };
    app.set_selection(next, None, None);
}

/// The classic shell menu at a window point (logical pixels). "rename" starts in-place editing.
pub fn shell_menu(app: &Rc<App>, paths: Vec<String>, x: f32, y: f32, shift: bool) {
    let folder = app.state.borrow().path.clone();
    let scale = app.ui().window().scale_factor();
    let picked = menu::show(app.hwnd.get(), &folder, &paths, (x * scale) as i32, (y * scale) as i32, shift);
    match picked {
        Ok(Some(verb)) if verb == "rename" => start_rename(app),
        Ok(Some(_)) => {
            refresh_paste_state(app);
            app.reload();
        }
        _ => {}
    }
}

pub fn network_menu(app:&Rc<App>,paths:Vec<String>,x:f32,y:f32,shift:bool){
    let scale=app.ui().window().scale_factor();
    let folder=app.current_path();
    match menu::show_network(app.hwnd.get(),&folder,&paths,(x*scale) as i32,(y*scale) as i32,shift){
        Ok(Some(verb)) if verb=="open"=>open_selection(app),
        Ok(Some(verb)) if verb=="opennewtab"=>open_in_new_tab(app,paths),
        Ok(Some(verb)) if verb=="opennewwindow"=>open_in_new_window(paths),
        Ok(Some(_))=>{app.reload();app.refresh_nav();},
        _=>{}
    }
}

pub fn set_sort_key(app: &Rc<App>, key: SortKey) {
    let mut sort = app.state.borrow().sort;
    sort.key = key;
    app.set_sort(sort);
}

pub fn set_sort_descending(app: &Rc<App>, descending: bool) {
    let mut sort = app.state.borrow().sort;
    sort.descending = descending;
    app.set_sort(sort);
}

/// "Agrupar por": grouping by a column also sorts by it, in its first direction.
pub fn set_group(app: &Rc<App>, key: Option<SortKey>) {
    let group = key.map(super::group::Group::new);
    app.state.borrow_mut().group = group;
    app.remember_view(|v| v.group = Some(group));
    match key {
        Some(k) if app.state.borrow().sort.key != k => {
            app.set_sort(super::sort::Sort { key: k, descending: k.descending_first() });
        }
        _ => {
            app.rebuild_order();
            app.load_sort_texts();
            app.sync_rows();
        }
    }
}

pub fn set_group_descending(app: &Rc<App>, descending: bool) {
    let group = app.state.borrow().group.map(|g| super::group::Group { descending, ..g });
    app.state.borrow_mut().group = group;
    app.remember_view(|v| v.group = Some(group));
    app.rebuild_order();
    app.sync_rows();
}

/// The details and preview panes share the place right of the view: showing one hides the other.
pub fn toggle_details_pane(app: &Rc<App>) {
    {
        let mut st = app.state.borrow_mut();
        st.details_pane = !st.details_pane;
        st.preview_pane = false;
        st.preview_kind = false;
        crate::shell::listing::set_panes(st.details_pane, st.preview_pane);
    }
    app.sync_view_state();
}

pub fn toggle_preview_pane(app: &Rc<App>) {
    {
        let mut st = app.state.borrow_mut();
        st.preview_pane = !st.preview_pane;
        st.details_pane = false;
        st.preview_kind = true;
        crate::shell::listing::set_panes(st.details_pane, st.preview_pane);
    }
    app.sync_view_state();
}

/// Hidden items and extensions change what the folder lists: list it again.
pub fn set_folder_option(app: &Rc<App>, option: crate::shell::listing::FolderOption, on: bool) {
    crate::shell::listing::set_option(option, on);
    if option == crate::shell::listing::FolderOption::CheckBoxes {
        app.state.borrow_mut().check_boxes = on;
        app.sync_view_state();
    }
    app.reload();
}

pub fn toggle_nav_pane(app: &Rc<App>) {
    let on = !app.state.borrow().nav_pane;
    app.state.borrow_mut().nav_pane = on;
    app.sync_nav();
}

pub fn view_label(mode: i32) -> SharedString {
    VIEW_MODES.iter().find(|m| m.3 == mode).map(|m| crate::i18n::tr(m.1)).unwrap_or("").into()
}
