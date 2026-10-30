use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::navigation::Hit;
use super::{chrome::Focus, WINDOW};

const FADE: usize = 13;
const REPEAT: usize = 14;

#[cfg(debug_assertions)]
pub fn diagnostics() {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow().as_ref() {
            let state = &window.navigation;
            eprintln!("native-tree-state|{}",serde_json::json!({"manual":state.manual_follow,"following":state.follow_pending,"focus":state.focused,"active":state.active,"top":state.top,"selected":state.selected,"path":window.tabs.active().location.tab.path()}));
        }
    });
    let rows = WINDOW.with(|slot| slot.borrow().as_ref().map(|window| window.navigation.tree.rows.iter().map(|row| match window.navigation.tree.node(&row.reference) { Some(node) => serde_json::json!({"name":node.node.name,"id":node.node.id,"depth":row.depth}), None => serde_json::json!({"separator":true}) }).collect::<Vec<_>>()));
    if let Some(rows) = rows {
        eprintln!("native-tree|{}", serde_json::json!(rows));
    }
}

pub fn invalidate(hwnd: HWND) {
    super::invalidate(
        hwnd,
        super::render::Damage {
            navigation: true,
            ..super::render::Damage::NONE
        },
    );
}

pub fn focus_changed(hwnd: HWND) {
    let running = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let state = &mut window.navigation;
        state.focused = window.chrome.focus == Focus::Tree;
        state.active = window.title.active;
        let target = if state.pointer_inside || (state.focused && state.active) {
            1.0
        } else {
            0.0
        };
        let previous = state.fade.map(|fade| fade.2).unwrap_or(state.opacity);
        if target != previous {
            state.fade = Some((
                std::time::Instant::now(),
                state.opacity,
                target,
                if target > 0.0 { 200.0 } else { 1000.0 },
            ));
        }
        state.changed();
        state.fade.is_some()
    });
    if running {
        super::input::arm(hwnd, FADE, 16);
    }
    invalidate(hwnd);
}

pub fn ready(hwnd: HWND) {
    let Some(mut window) = WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    window.navigation.receive(&window.navigation_worker);
    let mut watched = window.navigation.pinned_parents.clone();
    watched.extend(window.navigation.tree.rows.iter().filter_map(|row| {
        window
            .navigation
            .tree
            .node(&row.reference)
            .filter(|node| node.expanded)
            .map(|node| node.node.id.clone())
    }));
    if let Err(error) = window._navigation_notifications.sync(hwnd, &watched) {
        eprintln!("native tree watches: {error}");
    }
    let resized = window.list.navigation_width != window.navigation.left();
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if resized {
        super::invalidate_all(hwnd);
    } else {
        invalidate(hwnd);
    }
}

pub fn refresh() {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.navigation.refresh(&window.navigation_worker);
        }
    });
}

pub fn notified(wparam: usize, lparam: isize) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::UI::Shell::{SHChangeNotification_Lock, SHChangeNotification_Unlock};
    unsafe {
        let mut pidls = std::ptr::null_mut();
        let mut event = 0;
        let locked = SHChangeNotification_Lock(
            HANDLE(wparam as _),
            lparam as u32,
            Some(&mut pidls),
            Some(&mut event),
        );
        if locked.is_invalid() {
            eprintln!("native tree change lock failed");
            return;
        }
        if !SHChangeNotification_Unlock(HANDLE(locked.0)).as_bool() {
            eprintln!("native tree change unlock failed");
        }
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.navigation.reload(&window.navigation_worker);
        }
    });
}

pub fn moved(hwnd: HWND, point: (f32, f32)) {
    let (changed, fade, resize, scroll) = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return (false, false, false, false);
        };
        let state = &mut window.navigation;
        state.pointer = Some(point);
        let hit = state.hit(point);
        let inside = hit.is_some();
        let fade = inside != state.pointer_inside;
        state.pointer_inside = inside;
        let changed = state.hover != hit;
        state.set_hover(hit);
        let mut resize = false;
        if let Some((start, width)) = state.split_drag {
            let space = window
                .scroll
                .view
                .map_or(1100.0, |view| view.x + view.width);
            let width = crate::app::navtree::pane_width(width + point.0 - start, space);
            resize = state.width != width;
            state.width = width;
        }
        let old = state.top;
        if let (Some((pointer, top)), Some(bar)) = (state.thumb_drag, state.bar()) {
            state.top = (top as f32
                + (point.1 - pointer) * state.maximum() as f32 / bar.travel().max(1.0))
            .round()
            .clamp(0.0, state.maximum() as f32) as usize;
        }
        if resize {
            state.changed();
        }
        (changed, fade, resize, old != state.top)
    });
    if fade {
        focus_changed(hwnd);
    }
    if resize {
        super::invalidate_all(hwnd);
    } else if changed || scroll {
        invalidate(hwnd);
    }
}

pub fn leave(hwnd: HWND) {
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let state = &mut window.navigation;
            let changed = state.pointer_inside || state.hover.is_some();
            state.pointer_inside = false;
            state.pointer = None;
            state.set_hover(None);
            changed
        })
    });
    if changed {
        focus_changed(hwnd);
    }
}

pub fn wheel(hwnd: HWND, point: (f32, f32), delta: i32) -> bool {
    let handled = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            if window.navigation.hit(point).is_none() {
                return false;
            }
            window.navigation.wheel(delta);
            true
        })
    });
    if handled {
        invalidate(hwnd);
    }
    handled
}

pub(super) fn toggle(hwnd: HWND, row: usize) {
    let navigate = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let state = &mut window.navigation;
        let node = state.tree.node(&state.tree.rows.get(row)?.reference)?;
        if !node.node.has_children {
            return None;
        }
        if node.expanded {
            state.collapse(row)
        } else {
            state.expand(row, &window.navigation_worker);
            None
        }
    });
    if let Some(path) = navigate {
        open(hwnd, path);
    }
    invalidate(hwnd);
}

fn open(hwnd: HWND, path: String) {
    if let Err(error) = super::navigate(hwnd, super::Navigation::Open(path)) {
        super::fail(error);
    }
}

fn bar_step(hwnd: HWND) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            let state = &mut window.navigation;
            state.set_hover(state.pointer.and_then(|point| state.hit(point)));
            if state.hover != state.pressed {
                return;
            }
            let step = match state.pressed {
                Some(Hit::Bar(super::scroll::Part::Start)) => -1,
                Some(Hit::Bar(super::scroll::Part::End)) => 1,
                Some(Hit::Bar(super::scroll::Part::Before)) => {
                    -(state.page().saturating_sub(1).max(1) as isize)
                }
                Some(Hit::Bar(super::scroll::Part::After)) => {
                    state.page().saturating_sub(1).max(1) as isize
                }
                _ => return,
            };
            state.scroll(step);
        }
    });
    invalidate(hwnd);
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    let hit = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.navigation.hit(point))
    });
    let Some(hit) = hit else { return false };
    if hit != Hit::Splitter && !matches!(hit, Hit::Row(_)) {
        super::address_input::blur(hwnd);
        super::chrome::set_focus(hwnd, Focus::Tree);
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            let state = &mut window.navigation;
            state.pressed = Some(hit);
            if hit == Hit::Splitter {
                state.split_drag = Some((point.0, state.width));
            }
            if hit == Hit::Bar(super::scroll::Part::Thumb) {
                state.thumb_drag = Some((point.1, state.top));
            }
            state.changed();
        }
    });
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
    }
    match hit {
        Hit::Expando(row) => toggle(hwnd, row),
        Hit::Bar(part) if part != super::scroll::Part::Thumb => {
            bar_step(hwnd);
            super::input::arm(hwnd, REPEAT, 500);
        }
        _ => {}
    }
    invalidate(hwnd);
    true
}

pub fn release(hwnd: HWND, point: (f32, f32)) -> bool {
    let action = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let state = &mut window.navigation;
        let pressed = state.pressed.take()?;
        let width = state.split_drag.take().map(|_| state.width);
        state.thumb_drag = None;
        state.changed();
        let path = match (pressed, state.hit(point)) {
            (Hit::Row(row), Some(Hit::Row(released))) if row == released => {
                state.select(row, window.tabs.active().location.tab.path());
                state
                    .tree
                    .node(&state.tree.rows[row].reference)
                    .map(|node| node.node.id.clone())
            }
            _ => None,
        };
        Some((path, width))
    });
    let Some((path, width)) = action else {
        return false;
    };
    if path.is_some() {
        super::chrome::set_focus(hwnd, Focus::Tree);
    }

    cancel(hwnd);
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    if let Some(width) = width {
        if let Err(error) = crate::shell::nav_options::set_pane_width(width) {
            eprintln!("native pane width: {error}");
        }
    }
    if let Some(path) = path {
        open(hwnd, path);
    }
    invalidate(hwnd);
    true
}

pub fn cancel(hwnd: HWND) {
    let captured = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let state = &mut window.navigation;
            let captured = state.pressed.take().is_some()
                || state.split_drag.is_some()
                || state.thumb_drag.is_some();
            state.split_drag = None;
            state.thumb_drag = None;
            captured
        })
    });
    super::input::stop(hwnd, REPEAT);
    if captured {
        if let Err(error) = super::release_capture(hwnd) {
            super::fail(error);
        }
        invalidate(hwnd);
    }
}

pub fn timer(hwnd: HWND, timer: usize) -> bool {
    if timer == FADE {
        let running = WINDOW.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(window) = slot.as_mut() else {
                return false;
            };
            let state = &mut window.navigation;
            let Some((at, from, to, duration)) = state.fade else {
                return false;
            };
            let t = (at.elapsed().as_secs_f32() * 1000.0 / duration).min(1.0);
            state.opacity = from + (to - from) * t;
            if t >= 1.0 {
                state.fade = None;
            }
            state.changed();
            state.fade.is_some()
        });
        if !running {
            super::input::stop(hwnd, FADE);
        }
        invalidate(hwnd);
        return true;
    }
    if timer == REPEAT {
        super::input::arm(hwnd, REPEAT, 62);
        bar_step(hwnd);
        return true;
    }
    false
}

pub fn double_click(hwnd: HWND, point: (f32, f32)) -> bool {
    let hit = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.navigation.hit(point))
    });
    if let Some(Hit::Row(row)) = hit {
        toggle(hwnd, row);
        return true;
    }
    hit.is_some()
}

pub fn cursor() -> bool {
    let resizing = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            window.navigation.hover == Some(Hit::Splitter) || window.navigation.split_drag.is_some()
        })
    });
    if !resizing {
        return false;
    }
    match unsafe { LoadCursorW(None, IDC_SIZEWE) } {
        Ok(cursor) => unsafe {
            SetCursor(Some(cursor));
        },
        Err(error) => super::fail(error),
    }
    true
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    let mods = super::input::modifiers();
    if key == 0x45 && mods.ctrl && mods.shift {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.navigation.manual_follow = true;
                window.navigation.follow_pending = true;
            }
        });
        invalidate(hwnd);
        return true;
    }
    let focused = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.chrome.focus == Focus::Tree)
    });
    if !focused
        || matches!(key, 0x09 | 0x74 | 0x75 | 0x12)
        || unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0 }
    {
        return false;
    }
    if key == 0x5d || key == 0x79 && mods.shift {
        let target = WINDOW.with(|slot| {
            slot.borrow().as_ref().map(|window| {
                (
                    window.navigation.selected,
                    (
                        24.0,
                        window.navigation.selected.map_or(160.0, |row| {
                            window.navigation.tree.rows.get(row).map_or(160.0, |_| {
                                136.0
                                    + window.navigation.metrics().1
                                    + row as f32 * window.navigation.metrics().0
                                    - window.navigation.offset()
                                    + window.navigation.metrics().0 / 2.0
                            })
                        }),
                    ),
                )
            })
        });
        if let Some((row, point)) = target {
            super::menu_actions::navigation(hwnd, row, point, super::menu::Origin::Keyboard);
        }
        return true;
    }
    let (target, toggle_row, open_path) = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return (None, None, None);
        };
        let state = &mut window.navigation;
        let tree = &state.tree;
        let Some(current) = state
            .selected
            .or_else(|| tree.step(0, 1).or(Some(0)))
            .filter(|row| tree.rows.get(*row).is_some())
        else {
            return (None, None, None);
        };
        let node = tree.node(&tree.rows[current].reference);
        let mut target = None;
        let mut toggle = None;
        let mut path = None;
        match key {
            0x28 => target = tree.step(current, 1),
            0x26 => target = tree.step(current, -1),
            0x24 => {
                target = tree
                    .rows
                    .iter()
                    .position(|row| tree.node(&row.reference).is_some())
            }
            0x23 => target = tree.step(tree.rows.len(), -1),
            0x21 | 0x22 => {
                target =
                    Some(tree.page_target(current, if key == 0x21 { -1 } else { 1 }, state.page()));
            }
            0x27 => {
                if node.is_some_and(|node| node.expanded) {
                    target = tree
                        .step(current, 1)
                        .filter(|row| tree.rows[*row].depth > tree.rows[current].depth);
                } else {
                    toggle = Some(current);
                }
            }
            0x25 => {
                if node.is_some_and(|node| node.expanded) {
                    toggle = Some(current);
                } else {
                    target = tree.parent(current);
                }
            }
            0x08 => target = tree.parent(current),
            0x6b => {
                if node.is_some_and(|node| !node.expanded) {
                    toggle = Some(current);
                }
            }
            0x6d => {
                if node.is_some_and(|node| node.expanded) {
                    toggle = Some(current);
                }
            }
            0x6a => {
                state.expand_all(current, &window.navigation_worker);
            }
            0x1b => {
                state.expanding_all.clear();
                state.follow_pending = false;
                state.manual_follow = false;
            }
            0x0d => path = node.map(|node| node.node.id.clone()),
            _ => {}
        }
        if let Some(target) = target {
            state.select(target, window.tabs.active().location.tab.path());
            state.reveal(target, target);
        }
        (target, toggle, path)
    });
    if let Some(row) = toggle_row {
        toggle(hwnd, row);
    }
    if let Some(path) = open_path {
        open(hwnd, path);
    }
    if target.is_some() || toggle_row.is_some() || matches!(key, 0x6a | 0x0d) {
        invalidate(hwnd);
    }
    true
}

pub fn character(hwnd: HWND, value: u32) -> bool {
    if matches!(value, 43 | 45 | 42) {
        return WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.chrome.focus == Focus::Tree)
        });
    }
    let handled = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        if window.chrome.focus != Focus::Tree {
            return false;
        }
        let Some(character) = char::from_u32(value).filter(|character| !character.is_control())
        else {
            return true;
        };
        let state = &mut window.navigation;
        if state.typeahead.0.elapsed().as_millis() > 1000 {
            state.typeahead.1.clear();
        }
        state.typeahead.0 = std::time::Instant::now();
        state.typeahead.1.extend(character.to_lowercase());
        let current = state.selected.unwrap_or(0);
        let found = state.tree.typeahead(current, &state.typeahead.1);
        if let Some(row) = found {
            state.select(row, window.tabs.active().location.tab.path());
            state.reveal(row, row);
        }
        true
    });
    if handled {
        invalidate(hwnd);
    }
    handled
}

pub fn context(hwnd: HWND, point: (f32, f32)) -> bool {
    let hit = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.navigation.hit(point))
    });
    let Some(hit) = hit else {
        return false;
    };
    if matches!(hit, Hit::Splitter | Hit::Bar(_)) {
        return true;
    }
    let row = match hit {
        Hit::Row(row) | Hit::Expando(row) => Some(row),
        _ => None,
    };
    super::menu_actions::navigation(hwnd, row, point, super::menu::Origin::Pointer);
    true
}
