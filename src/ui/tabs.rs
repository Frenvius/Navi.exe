use std::sync::Arc;
use windows::core::Result;
use windows::Win32::Foundation::{E_FAIL, HWND};

use super::list::{Folder, List};
use crate::app::tabs::{Close, Collection, Location};

#[derive(Clone)]
pub struct Saved {
    pub location: Location,
    pub name: String,
    pub folder: Option<Arc<Folder>>,
    pub icon: Option<crate::shell::icons::Rgba>,
    used: u64,
}

impl Saved {
    pub fn new(path: &str) -> Self {
        let mut location = Location::new(path);
        let name = if crate::app::ids::is_settings(path) {
            crate::i18n::tr("settings.title").into()
        } else {
            crate::shell::listing::display_name_of(path)
        };
        location.tab.title =
            crate::app::tabs::label(path, &name, &crate::shell::listing::settings());
        location.tab.icon = crate::shell::icons::file_icon(path);
        Self {
            location,
            name,
            folder: None,
            icon: None,
            used: 0,
        }
    }
}

pub type Tabs = Collection<Saved>;

fn error(value: String) -> windows::core::Error {
    windows::core::Error::new(E_FAIL, value)
}

pub(super) fn weight(folder: &Folder) -> usize {
    let entries = folder.entries.capacity()
        * std::mem::size_of::<crate::shell::listing::ShellEntry>()
        + folder
            .entries
            .iter()
            .map(|entry| {
                entry.name.capacity()
                    + entry.path.capacity()
                    + entry.type_name.capacity()
                    + entry.child.capacity()
                    + entry.absolute.capacity()
                    + entry.library_origin.as_ref().map_or(0, |origin| {
                        origin.path.capacity() + origin.name.capacity() + origin.parent.capacity()
                    })
            })
            .sum::<usize>();
    let layout = folder.layout.tops.capacity() * std::mem::size_of::<f32>()
        + folder.layout.folded.capacity()
        + folder.layout.headers.capacity() * std::mem::size_of::<crate::app::group::Header>()
        + folder
            .layout
            .headers
            .iter()
            .map(|header| header.label.capacity())
            .sum::<usize>();
    entries
        + layout
        + folder.grid.as_ref().map_or(0, |grid| grid.retained_bytes())
        + folder.gallery_filter.as_ref().map_or(0, String::capacity)
        + folder.order.capacity() * std::mem::size_of::<usize>()
        + folder.headers.capacity() * std::mem::size_of::<String>()
        + folder.headers.iter().map(String::capacity).sum::<usize>()
        + folder.crumbs.capacity() * std::mem::size_of::<crate::shell::listing::Crumb>()
        + folder
            .crumbs
            .iter()
            .map(|crumb| {
                crumb.name.capacity()
                    + crumb.id.capacity()
                    + crumb.nav_root.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
        + folder.icon.pixels.capacity()
        + folder.title.capacity()
        + folder.columns.capacity() * std::mem::size_of::<crate::app::columns::Col>()
        + folder
            .columns
            .iter()
            .map(|column| column.label.capacity())
            .sum::<usize>()
        + folder.view.columns.capacity()
            * std::mem::size_of::<(windows::Win32::Foundation::PROPERTYKEY, f32)>()
        + folder.view.menu_columns.capacity()
            * std::mem::size_of::<windows::Win32::Foundation::PROPERTYKEY>()
        + folder.view.extra_sorts.capacity() * std::mem::size_of::<crate::app::sort::Sort>()
        + folder.collapsed.capacity().next_power_of_two() * (std::mem::size_of::<String>() + 1)
        + folder.collapsed.iter().map(String::capacity).sum::<usize>()
        + std::mem::size_of::<Folder>()
        + std::mem::size_of::<crate::app::group::DetailsLayout>()
        + 5 * std::mem::size_of::<Vec<usize>>()
        + 12 * std::mem::size_of::<usize>()
}

pub fn cached_bytes(tabs: &Tabs) -> usize {
    tabs.entries
        .iter()
        .filter_map(|tab| tab.folder.as_deref())
        .map(weight)
        .sum()
}

fn trim(tabs: &mut Tabs) {
    const BUDGET: usize = 16 * 1024 * 1024;
    while cached_bytes(tabs) > BUDGET {
        let oldest = tabs
            .entries
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.folder.is_some())
            .min_by_key(|(_, tab)| tab.used)
            .map(|(index, _)| index);
        let Some(index) = oldest else { break };
        tabs.entries[index].folder = None;
    }
}

pub(super) fn remember(window: &mut super::Window) -> Result<()> {
    let current = window.tabs.active_mut();
    let location = &mut current.location;
    let path = location.tab.path();
    window.list.save(path).map_err(error)?;
    if let Some(folder) = &window.list.folder {
        location.views.insert(path.into(), folder.view.clone());
        current.folder = Some(folder.clone());
        current.icon = Some(folder.icon.clone());
        current.name = folder
            .crumbs
            .last()
            .map_or_else(|| current.name.clone(), |crumb| crumb.name.clone());
        location.tab.title = window.title.title.clone();
        location.tab.icon = folder.crumbs.last().map_or(0, |crumb| crumb.icon);
    } else {
        current.name = crate::shell::listing::display_name_of(path);
        location.tab.icon = crate::shell::icons::file_icon(path);
        current.icon = None;
    }
    location.visits[location.tab.index] = window.list.interaction.selection.clone();
    location.group_focus = window.list.interaction.group_focus.clone();
    location.scroll = (window.scroll.x, window.scroll.y);
    location.tree_pick = window.navigation.pick.clone();
    location.tree_top = window.navigation.top;
    window.tab_clock += 1;
    current.used = window.tab_clock;
    trim(&mut window.tabs);
    Ok(())
}

pub(super) fn show(window: &mut super::Window, hwnd: HWND) -> Result<()> {
    let options = window.search.options;
    let target = window.tabs.active_mut();
    let location = &target.location;
    let path = location.tab.path().to_string();
    let view = location.views.get(&path).cloned();
    let next = match target.folder.take() {
        Some(folder) => List::resume(&window.worker, hwnd, path, folder),
        None => List::search_start(&window.worker, hwnd, path, view, options),
    }
    .map_err(error)?;
    window.list = next;
    window
        .list
        .set_filter(location.tab.filter())
        .map_err(error)?;
    window.pending_visit = location.visits.get(location.tab.index).cloned();
    window.pending_scroll = Some(location.scroll);
    window.scroll = Default::default();
    window.scroll.x = location.scroll.0;
    window.scroll.y = location.scroll.1;
    window.navigation.pick = location.tree_pick.clone();
    window.navigation.top = location.tree_top.min(window.navigation.maximum());
    window.navigation.changed();
    window.chrome.focus = super::chrome::Focus::List;
    window.title.focus = super::titlebar::Target::None;
    window.list.interaction.has_focus = true;
    window.list.interaction.active = window.title.active;
    window.list.interaction.group_focus = location.group_focus.clone();
    window.started = std::time::Instant::now();
    window.first_list = false;
    window.reveal_path = None;
    Ok(())
}

fn change(hwnd: HWND, action: impl FnOnce(&mut super::Window) -> Result<()>) -> Result<()> {
    super::filter::close(hwnd);
    super::search::leaving(hwnd);
    super::address_input::blur(hwnd);
    super::header::cancel(hwnd);
    super::input::cancel(hwnd);
    let Some(mut window) = super::WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return Ok(());
    };
    window.title.cancelled_drag = false;
    let result = action(&mut window).and_then(|_| sync(&mut window, hwnd));
    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if result.is_ok() {
        super::invalidate_all(hwnd);
    }
    result
}

pub fn select(hwnd: HWND, index: usize) -> Result<()> {
    let valid = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| index < window.tabs.entries.len() && index != window.tabs.current)
    });
    if !valid {
        return Ok(());
    }
    let focus = super::WINDOW.with(|slot| slot.borrow().as_ref().map(|window| window.chrome.focus));
    change(hwnd, |window| {
        if index >= window.tabs.entries.len() || index == window.tabs.current {
            return Ok(());
        }
        remember(window)?;
        window.tabs.select(index);
        show(window, hwnd)
    })?;
    if let Some(focus @ (super::chrome::Focus::Tab | super::chrome::Focus::Status(_))) = focus {
        super::chrome::set_focus(hwnd, focus);
    }
    Ok(())
}

pub fn open(hwnd: HWND, path: &str) -> Result<()> {
    let full = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.tabs.entries.len() >= 128)
    });
    if full {
        eprintln!("native tab limit reached");
        return Ok(());
    }
    change(hwnd, |window| {
        remember(window)?;
        crate::app::tabs::add(
            &mut window.tabs.entries,
            &mut window.tabs.current,
            Saved::new(path),
            crate::app::tabs::Disposition::Foreground,
        );
        show(window, hwnd)
    })
}

pub fn import(hwnd: HWND, path: &str, slot: usize) -> Result<()> {
    change(hwnd, |window| {
        if window.tabs.entries.len() >= 128 {
            return Err(error("Native tab limit reached".into()));
        }
        remember(window)?;
        let from = window.tabs.entries.len();
        if !crate::app::tabs::add(
            &mut window.tabs.entries,
            &mut window.tabs.current,
            Saved::new(path),
            crate::app::tabs::Disposition::Foreground,
        ) {
            return Err(error("Native tab import rejected".into()));
        }
        window.tabs.reorder(from, slot.min(from));
        show(window, hwnd)
    })
}

pub fn open_background(hwnd: HWND, path: &str) -> Result<()> {
    edit_tabs(hwnd, |tabs| {
        if !crate::app::tabs::add(
            &mut tabs.entries,
            &mut tabs.current,
            Saved::new(path),
            crate::app::tabs::Disposition::Background,
        ) {
            eprintln!("native tab limit reached");
        }
    })
}

pub fn close(hwnd: HWND, index: usize) -> Result<()> {
    let background = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| index < window.tabs.entries.len() && index != window.tabs.current)
    });
    if background {
        return edit_tabs(hwnd, |tabs| {
            tabs.close(index);
        });
    }
    let last = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            index < window.tabs.entries.len() && window.tabs.entries.len() == 1
        })
    });
    if last {
        return unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(hwnd),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                Default::default(),
                Default::default(),
            )
        };
    }
    change(hwnd, |window| {
        if index >= window.tabs.entries.len() {
            return Ok(());
        }
        remember(window)?;
        if window.tabs.close(index) == Close::Active {
            show(window, hwnd)?;
        } else {
            window.tabs.active_mut().folder = None;
        }
        Ok(())
    })
}

pub fn duplicate(hwnd: HWND, index: usize) -> Result<()> {
    change(hwnd, |window| {
        if index >= window.tabs.entries.len() || window.tabs.entries.len() >= 128 {
            return Ok(());
        }
        remember(window)?;
        crate::app::tabs::duplicate(&mut window.tabs.entries, &mut window.tabs.current, index);
        show(window, hwnd)
    })
}

pub fn reorder(hwnd: HWND, from: usize, to: usize) {
    if let Err(error) = edit_tabs(hwnd, |tabs| {
        tabs.reorder(from, to);
    }) {
        super::fail(error);
    }
}

fn edit_tabs(hwnd: HWND, action: impl FnOnce(&mut Tabs)) -> Result<()> {
    let Some(mut window) = super::WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return Ok(());
    };
    window.title.cancelled_drag = false;
    action(&mut window.tabs);
    let result = sync(&mut window, hwnd);
    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    super::invalidate_title(hwnd);
    result
}

pub fn sync(window: &mut super::Window, hwnd: HWND) -> Result<()> {
    super::launch::routes(hwnd, &window.tabs);
    if window.list.folder.is_some() && !window.list.search.busy() {
        super::jumplist::visit(hwnd, window.tabs.active().location.tab.path());
    }
    let mut items = Vec::with_capacity(window.tabs.entries.len());
    let settings = crate::shell::listing::settings();
    for saved in &mut window.tabs.entries {
        saved.location.tab.title =
            crate::app::tabs::label(saved.location.tab.path(), &saved.name, &settings);
        let icon = match &saved.icon {
            Some(icon) => icon.clone(),
            None => {
                let icon = if crate::app::ids::is_settings(saved.location.tab.path()) {
                    super::settings::icon()
                } else {
                    crate::shell::icons::icon(saved.location.tab.icon, 16).map_err(error)?
                };
                saved.icon = Some(icon.clone());
                icon
            }
        };
        let mut item = super::titlebar::Item::new(saved.location.tab.title.clone(), icon);
        item.settings = crate::app::ids::is_settings(saved.location.tab.path());
        items.push(item);
    }
    let active = &items[window.tabs.current];
    window.title.title = active.title.clone();
    window.title.icon = active.icon.clone();
    window.title.items = items;
    window.title.current = window.tabs.current;
    let caption = crate::shell::wide(&crate::app::tabs::caption(
        &window.title.title,
        window.tabs.entries.len(),
    ));
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::*;
        SetWindowTextW(hwnd, windows::core::PCWSTR(caption.as_ptr()))?;
        let mut bounds = Default::default();
        GetClientRect(hwnd, &mut bounds)?;
        let width =
            bounds.right as f32 * 96.0 / windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as f32;
        crate::frame::set_native_tabs_end(
            hwnd,
            super::titlebar::tabs_end(width, window.tabs.entries.len()),
        );
    }
    Ok(())
}

pub fn refresh_titles(hwnd: HWND) -> Result<()> {
    let Some(mut window) = super::WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return Ok(());
    };
    let result = sync(&mut window, hwnd);
    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    super::invalidate_title(hwnd);
    result
}

#[derive(Clone, Copy)]
pub enum CloseSet {
    Others,
    Right,
}

pub fn close_set(hwnd: HWND, index: usize, set: CloseSet) -> Result<()> {
    let background = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| match set {
            CloseSet::Others => index == window.tabs.current,
            CloseSet::Right => window.tabs.current <= index,
        })
    });
    if background {
        return edit_tabs(hwnd, |tabs| {
            match set {
                CloseSet::Others => {
                    crate::app::tabs::close_others(&mut tabs.entries, &mut tabs.current, index)
                }
                CloseSet::Right => {
                    crate::app::tabs::close_right(&mut tabs.entries, &mut tabs.current, index)
                }
            };
        });
    }
    change(hwnd, |window| {
        remember(window)?;
        let change = match set {
            CloseSet::Others => crate::app::tabs::close_others(
                &mut window.tabs.entries,
                &mut window.tabs.current,
                index,
            ),
            CloseSet::Right => crate::app::tabs::close_right(
                &mut window.tabs.entries,
                &mut window.tabs.current,
                index,
            ),
        };
        if change == Close::Active {
            show(window, hwnd)?;
        } else {
            window.tabs.active_mut().folder = None;
        }
        Ok(())
    })
}
