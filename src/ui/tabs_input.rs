use super::titlebar::Target;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::*;

const REORDER: usize = 21;
const DROP: usize = 22;

pub fn key(hwnd: HWND, key: u32) -> bool {
    let modifiers = super::input::modifiers();
    if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0 } {
        return false;
    }
    if key == 0x1b && cancel(hwnd) {
        return true;
    }
    if key == 0x7a && !modifiers.ctrl && !modifiers.shift {
        cancel(hwnd);
        super::address_input::blur(hwnd);
        crate::frame::toggle_full_screen(hwnd);
        super::chrome::set_focus(hwnd, super::chrome::Focus::List);
        super::invalidate_all(hwnd);
        return true;
    }
    if modifiers.ctrl {
        let result = match key {
            0x54 if !modifiers.shift => {
                Some(super::tabs::open(hwnd, &crate::app::ids::start_location()))
            }
            0x57 | 0x73 if !modifiers.shift => {
                let index = super::WINDOW
                    .with(|slot| slot.borrow().as_ref().map(|window| window.tabs.current));
                index.map(|index| super::tabs::close(hwnd, index))
            }
            0x09 => {
                let index = super::WINDOW.with(|slot| {
                    slot.borrow().as_ref().map(|window| {
                        crate::app::tabs::cycle(
                            window.tabs.current,
                            window.tabs.entries.len(),
                            if modifiers.shift { -1 } else { 1 },
                        )
                    })
                });
                index.map(|index| super::tabs::select(hwnd, index))
            }
            0x31..=0x39 if !modifiers.shift => {
                Some(super::tabs::select(hwnd, (key - 0x31) as usize))
            }
            _ => None,
        };
        if let Some(result) = result {
            if let Err(error) = result {
                super::fail(error);
            }
            return true;
        }
    }
    if key == 0x4e && modifiers.ctrl && !modifiers.shift {
        let path = super::WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|window| window.tabs.active().location.tab.path().to_string())
        });
        if let Some(path) = path {
            if let Err(error) = super::create_window(&path, std::time::Instant::now()) {
                super::fail(error);
            }
        }
        return true;
    }
    let focus = super::WINDOW.with(|slot| slot.borrow().as_ref().map(|window| window.chrome.focus));
    if matches!(
        focus,
        Some(super::chrome::Focus::Tab | super::chrome::Focus::NewTab)
    ) {
        if matches!(key, 0x25 | 0x27) {
            let index = super::WINDOW.with(|slot| {
                slot.borrow().as_ref().map(|window| {
                    (window.tabs.current as isize + if key == 0x25 { -1 } else { 1 })
                        .clamp(0, window.tabs.entries.len() as isize - 1)
                        as usize
                })
            });
            if let Some(index) = index {
                if let Err(error) = super::tabs::select(hwnd, index) {
                    super::fail(error);
                }
            }
            super::chrome::set_focus(hwnd, super::chrome::Focus::Tab);
            return true;
        }
        if matches!(key, 0x0d | 0x20) {
            if focus == Some(super::chrome::Focus::NewTab) {
                if let Err(error) = super::tabs::open(hwnd, &crate::app::ids::start_location()) {
                    super::fail(error);
                }
            }
            return true;
        }
        if key == 0x5d || (key == 0x79 && modifiers.shift) {
            let index =
                super::WINDOW.with(|slot| slot.borrow().as_ref().map(|window| window.tabs.current));
            if let Some(index) = index {
                super::menu_actions::tab(
                    hwnd,
                    index,
                    (8.0 + index as f32 * pitch(hwnd), 40.0),
                    super::menu::Origin::Keyboard,
                );
            }
            return true;
        }
    }
    false
}

fn pitch(hwnd: HWND) -> f32 {
    let mut bounds = Default::default();
    if let Err(error) = unsafe { GetClientRect(hwnd, &mut bounds) } {
        super::fail(error);
        return 240.0;
    }
    let count = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(1, |window| window.tabs.entries.len())
    });
    crate::app::tabs::width(
        bounds.right as f32 * 96.0
            / unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32,
        count,
    ) - 8.0
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    let target = hit(hwnd, point);
    let Target::Tab(index) = target else {
        return false;
    };
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.title.cancelled_drag = false;
        }
    });
    if let Err(error) = super::tabs::select(hwnd, index) {
        super::fail(error);
        return true;
    }
    super::press_title(hwnd, target);
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.chrome.cues = false;
            window.title.focus = Target::None;
            window.title.drag = Some(crate::app::tabs::Drag::new(index, point));
        }
    });
    true
}

fn hit(hwnd: HWND, point: (f32, f32)) -> Target {
    let mut bounds = Default::default();
    if let Err(error) = unsafe { GetClientRect(hwnd, &mut bounds) } {
        super::fail(error);
        return Target::None;
    }
    let width = bounds.right as f32 * 96.0
        / unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32;
    super::WINDOW.with(|slot| {
        slot.borrow().as_ref().map_or(Target::None, |window| {
            window.title.hit(width, point.0, point.1)
        })
    })
}

pub fn moved(hwnd: HWND, point: (f32, f32)) -> bool {
    let pitch = pitch(hwnd);
    let state = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let drag = window.title.drag.as_mut()?;
        if drag.dropping {
            return Some((true, false, false));
        }
        drag.moved(point, pitch, window.tabs.entries.len());
        Some((
            drag.active,
            drag.slot != drag.shown,
            window.input_timers.contains(&REORDER),
        ))
    });
    let Some((active, pending, armed)) = state else {
        return false;
    };
    if !active {
        return false;
    }
    if pending && !armed {
        super::input::arm(hwnd, REORDER, 300);
    } else if !pending {
        super::input::stop(hwnd, REORDER);
    }
    if let Err(error) = super::tab_transfer::moved(hwnd, point, pitch) {
        super::fail(error);
    }
    super::invalidate_title(hwnd);
    true
}

pub fn release(hwnd: HWND) -> bool {
    if super::tab_transfer::release(hwnd) {
        return true;
    }
    let active = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let drag = window.title.drag.as_mut()?;
        window.title.pressed = Target::None;
        if drag.active {
            if drag.slot != drag.shown || !(8.0..40.0).contains(&drag.pointer.1) {
                drag.slot = drag.from;
            }
            drag.dropping = true;
        } else {
            window.title.drag = None;
        }
        Some(window.title.drag.is_some())
    });
    let Some(active) = active else { return false };
    super::input::stop(hwnd, REORDER);
    if active {
        super::input::arm(hwnd, DROP, 150);
    }
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    super::invalidate_title(hwnd);
    true
}

pub fn cancel(hwnd: HWND) -> bool {
    super::tab_transfer::cancel(hwnd);
    let used = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let Some(drag) = window.title.drag.take() else {
                return false;
            };
            window.title.cancelled_drag = drag.active;
            window.title.pressed = Target::None;
            true
        })
    });
    if used {
        super::input::stop(hwnd, REORDER);
        super::input::stop(hwnd, DROP);
        if let Err(error) = super::release_capture(hwnd) {
            super::fail(error);
        }
        super::invalidate_title(hwnd);
    }
    used
}

pub fn capture_changed(hwnd: HWND) {
    let dropping = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.title.drag.is_some_and(|drag| drag.dropping))
    });
    if !dropping {
        cancel(hwnd);
    }
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if !matches!(id, REORDER | DROP) {
        return false;
    }
    super::input::stop(hwnd, id);
    let reorder = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        if id == REORDER {
            if let Some(drag) = window.title.drag.as_mut() {
                drag.shown = drag.slot;
            }
            None
        } else {
            window.title.drag.take().map(|drag| (drag.from, drag.slot))
        }
    });
    if let Some((from, to)) = reorder {
        super::tabs::reorder(hwnd, from, to);
    }
    super::invalidate_title(hwnd);
    true
}

pub fn context(hwnd: HWND, point: (f32, f32)) -> bool {
    let index = match hit(hwnd, point) {
        Target::Tab(index) | Target::TabClose(index) => index,
        _ => return false,
    };
    menu(hwnd, index, point);
    true
}

fn menu(hwnd: HWND, index: usize, point: (f32, f32)) {
    super::menu_actions::tab(hwnd, index, point, super::menu::Origin::Pointer);
}

pub fn middle(hwnd: HWND, point: (f32, f32)) -> bool {
    if let Target::Tab(index) | Target::TabClose(index) = hit(hwnd, point) {
        if let Err(error) = super::tabs::close(hwnd, index) {
            super::fail(error);
        }
        return true;
    }
    let path = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        if let Some(super::navigation::Hit::Row(row) | super::navigation::Hit::Expando(row)) =
            window.navigation.hit(point)
        {
            return window
                .navigation
                .tree
                .node(&window.navigation.tree.rows[row].reference)
                .map(|node| node.node.id.clone());
        }
        let folder = window.list.folder.as_ref()?;
        window.list.interaction.pointer = Some(point);
        let row = window
            .list
            .interaction
            .hit(window.scroll.view?, &window.scroll, folder)?;
        let entry = &folder.entries[folder.order[row]];
        entry.is_dir.then(|| entry.path.clone())
    });
    if let Some(path) = path {
        if let Err(error) = super::tabs::open_background(hwnd, &path) {
            super::fail(error);
        }
        return true;
    }
    false
}

pub fn item_menu(hwnd: HWND, parent: &str, paths: &[String], point: (f32, f32)) {
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    let result = crate::shell::menu::show_browser(
        hwnd,
        parent,
        paths,
        (point.0 * scale).round() as i32,
        (point.1 * scale).round() as i32,
        super::input::modifiers().shift,
    );
    match result {
        Ok(Some(verb)) if matches!(verb.as_str(), "opennewtab" | "opennewwindow") => {
            for path in paths {
                let result = if verb == "opennewtab" {
                    super::tabs::open_background(hwnd, path)
                } else {
                    super::create_window(path, std::time::Instant::now()).map(|_| ())
                };
                if let Err(error) = result {
                    super::fail(error);
                    break;
                }
            }
        }
        Ok(Some(verb)) if verb == "rename" => {
            if let Some(path) = paths.first() {
                if let Err(error) = super::rename::begin(hwnd, path) {
                    eprintln!("native classic rename: {error}");
                }
            }
        }
        Ok(_) => (),
        Err(error) => eprintln!("native item menu: {error}"),
    }
}

pub fn list_context(hwnd: HWND, point: (f32, f32)) -> bool {
    let menu = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let folder = window.list.folder.as_ref()?;
        window.list.interaction.pointer = Some(point);
        let row = window
            .list
            .interaction
            .hit(window.scroll.view?, &window.scroll, folder)?;
        let path = folder.entries[folder.order[row]].path.clone();
        if !window.list.interaction.selection.selected.contains(&path) {
            window.list.interaction.selection.selected.clear();
            window
                .list
                .interaction
                .selection
                .selected
                .insert(path.clone());
            window.list.interaction.selection.focused = Some(path.clone());
            window.list.interaction.selection.anchor = Some(path.clone());
        }
        window.list.interaction.cues = false;
        window.list.interaction.mark_all();
        Some((
            window.tabs.active().location.tab.path().to_string(),
            window
                .list
                .interaction
                .selection
                .selected
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        ))
    });
    let Some((parent, paths)) = menu else {
        return false;
    };
    super::input::flush(hwnd);
    if super::input::modifiers().shift {
        item_menu(hwnd, &parent, &paths, point);
    } else {
        super::menu_actions::list(hwnd, point, super::menu::Origin::Pointer);
    }
    true
}
