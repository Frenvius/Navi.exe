use super::address::{Reply, Request, Target};
use super::chrome::Focus;
use super::WINDOW;
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

const DRAG_TIMER: usize = 20;

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id != DRAG_TIMER {
        return false;
    }
    let drag = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| {
            window
                .address
                .drag
                .map(|(target, anchor)| (target, anchor, window.address.drag_x))
        })
    });
    if let Some((target, anchor, x)) = drag {
        select_to(hwnd, target, anchor, x);
    }
    true
}

fn stop_drag_timer(hwnd: HWND) {
    let armed = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|window| window.input_timers.remove(&DRAG_TIMER))
    });
    if armed {
        if let Err(error) = unsafe { KillTimer(Some(hwnd), DRAG_TIMER) } {
            super::fail(error);
        }
    }
    super::input::publish_timers(hwnd);
}

pub fn cursor() -> bool {
    let edit = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            window.address.drag.is_some()
                || window.address.hover == Some(Target::Search)
                || window.address.hover == Some(Target::Address) && window.address.editing
        })
    });
    if !edit {
        return false;
    }
    match unsafe { LoadCursorW(None, IDC_IBEAM) } {
        Ok(cursor) => {
            unsafe {
                SetCursor(Some(cursor));
            }
            true
        }
        Err(error) => {
            eprintln!("edit cursor: {error}");
            false
        }
    }
}

pub fn invalidate(hwnd: HWND) {
    super::DAMAGE.with(|damage| {
        damage.set(damage.get().merge(super::render::Damage {
            address: true,
            ..super::render::Damage::NONE
        }))
    });
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn moved(hwnd: HWND, point: (f32, f32)) {
    if super::address_popup::menu_open() {
        return;
    }
    let drag = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.address.drag)
    });
    if let Some((target, anchor)) = drag {
        let outside = WINDOW.with(|slot| {
            slot.borrow_mut().as_mut().is_some_and(|window| {
                window.address.drag_x = point.0;
                let field = if target == Target::Search {
                    window.address.search_edit.field
                } else {
                    window.address.address_edit.field
                };
                point.0 < field.left || point.0 >= field.right
            })
        });
        if outside {
            if unsafe { SetTimer(Some(hwnd), DRAG_TIMER, 45, None) } == 0 {
                super::fail(windows::core::Error::from_win32());
            } else {
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        window.input_timers.insert(DRAG_TIMER);
                    }
                });
                super::input::publish_timers(hwnd);
            }
        } else {
            stop_drag_timer(hwnd);
        }
        select_to(hwnd, target, anchor, point.0);
        return;
    }
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let hit = window.address.hit(point);
            if window.address.hover == hit {
                return false;
            }
            window.address.hover = hit;
            true
        })
    });
    if changed {
        invalidate(hwnd);
    }
}

pub fn leave(hwnd: HWND) {
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|window| window.address.hover.take().is_some())
    });
    if changed {
        invalidate(hwnd);
    }
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    if point.1 < 136.0 && super::address_popup::menu_open() {
        blur(hwnd);
        return true;
    }
    let hit = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.address.hit(point))
    });
    let Some(target) = hit else {
        blur(hwnd);
        return false;
    };
    if matches!(target, Target::Address | Target::Search) {
        let editing = WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                if target == Target::Address {
                    window.address.editing
                } else {
                    window.address.search_focus
                }
            })
        });
        if !editing {
            super::chrome::set_focus(hwnd, Focus::Address(target));
        }
        if target == Target::Search || editing {
            if let Some(anchor) = position(hwnd, target, point.0) {
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        window.address.drag = Some((target, anchor));
                    }
                });
                unsafe {
                    SetCapture(hwnd);
                }
            }
        }
        return true;
    }
    if !matches!(target, Target::AddressClear | Target::SearchClear) {
        blur(hwnd);
    }
    if !matches!(target, Target::AddressClear | Target::SearchClear) {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.address.focus = Some(target);
                window.address.cues = false;
                window.chrome.focus = Focus::Address(target);
                window.chrome.cues = false;
                window.list.header.focus = None;
                window.title.focus = super::titlebar::Target::None;
                window.list.interaction.has_focus = false;
                window.list.interaction.cues = false;
                window.list.interaction.mark_all();
            }
        });
        super::input::flush(hwnd);
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.address.pressed = Some(target);
        }
    });
    unsafe {
        if !matches!(target, Target::AddressClear | Target::SearchClear) {
            let _ = SetFocus(Some(hwnd));
        }
        SetCapture(hwnd);
    }
    invalidate(hwnd);
    true
}

fn select_to(hwnd: HWND, target: Target, anchor: u32, x: f32) {
    let result = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map(|window| {
            let (editor, control) = if target == Target::Search {
                (&window.address.search_edit, window.editors.search)
            } else {
                (&window.address.address_edit, window.editors.address)
            };
            editor
                .hit(&mut window.text, x)
                .map(|position| (control, position))
        })
    });
    match result {
        Some(Ok((control, position))) => unsafe {
            SendMessageW(
                control,
                windows::Win32::UI::Controls::EM_SETSEL,
                Some(WPARAM(anchor as usize)),
                Some(LPARAM(position as isize)),
            );
            super::edit::notify(hwnd, control);
        },
        Some(Err(error)) => super::fail(error),
        _ => {}
    }
}

fn position(hwnd: HWND, target: Target, x: f32) -> Option<u32> {
    let extend = super::input::modifiers().shift;
    let result = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map(|window| {
            let (editor, control) = if target == Target::Search {
                (&window.address.search_edit, window.editors.search)
            } else {
                (&window.address.address_edit, window.editors.address)
            };
            editor.hit(&mut window.text, x).map(|position| {
                let anchor = if extend {
                    if editor.active == editor.start {
                        editor.end
                    } else {
                        editor.start
                    }
                } else {
                    position
                };
                (control, position, anchor)
            })
        })
    });
    match result {
        Some(Ok((control, position, anchor))) => unsafe {
            SendMessageW(
                control,
                windows::Win32::UI::Controls::EM_SETSEL,
                Some(WPARAM(anchor as usize)),
                Some(LPARAM(position as isize)),
            );
            super::edit::notify(hwnd, control);
            return Some(anchor);
        },
        Some(Err(error)) => super::fail(error),
        _ => {}
    }
    None
}

pub fn double_click(hwnd: HWND, point: (f32, f32)) -> bool {
    let result = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|window| {
            let target = window.address.hit(point)?;
            let (editor, control) = match target {
                Target::Address if window.address.editing => {
                    (&window.address.address_edit, window.editors.address)
                }
                Target::Search => (&window.address.search_edit, window.editors.search),
                _ => return None,
            };
            Some(editor.hit(&mut window.text, point.0).map(|position| {
                (
                    control,
                    position.min(editor.text.encode_utf16().count().saturating_sub(1) as u32),
                )
            }))
        })
    });
    let Some(result) = result else {
        return false;
    };
    match result {
        Ok((control, position)) => unsafe {
            let pixel = SendMessageW(
                control,
                windows::Win32::UI::Controls::EM_POSFROMCHAR,
                Some(WPARAM(position as usize)),
                None,
            );
            SendMessageW(
                control,
                WM_LBUTTONDBLCLK,
                Some(WPARAM(1)),
                Some(LPARAM(pixel.0)),
            );
            super::edit::notify(hwnd, control);
        },
        Err(error) => super::fail(error),
    }
    true
}

pub fn release(hwnd: HWND, point: (f32, f32)) -> bool {
    let dragging = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|window| window.address.drag.take().is_some())
    });
    if dragging {
        stop_drag_timer(hwnd);
        if let Err(error) = super::release_capture(hwnd) {
            super::fail(error);
        }
        return true;
    }
    let state = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|window| {
            let pressed = window.address.pressed.take()?;
            Some((pressed, window.address.hit(point) == Some(pressed)))
        })
    });
    let Some((target, active)) = state else {
        return false;
    };
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    invalidate(hwnd);
    if active {
        activate(hwnd, target);
    }
    true
}

pub fn cancel(hwnd: HWND) {
    stop_drag_timer(hwnd);
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            window.address.drag = None;
            window.address.pressed.take().is_some()
        })
    });
    if changed {
        invalidate(hwnd);
    }
}

pub fn blur(hwnd: HWND) {
    stop_drag_timer(hwnd);
    let dragging = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|window| window.address.drag.take().is_some())
    });
    if dragging {
        if let Err(error) = super::release_capture(hwnd) {
            super::fail(error);
        }
    }
    super::address_popup::close();
    let editors = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|window| (window.editors.address, window.editors.search))
    });
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let changed = window.address.editing
                || window.address.search_focus
                || window.address.focus.is_some()
                || window.address.open.is_some();
            window.address.editing = false;
            window.address.search_focus = false;
            window.address.focus = None;
            window.address.cues = false;
            window.address.open = None;
            window.address.address_edit.focused = false;
            window.address.search_edit.focused = false;
            changed
        })
    });
    if let Some((address, search)) = editors {
        let focus = unsafe { GetFocus() };
        if focus == address || focus == search {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
        }
    }
    if changed {
        invalidate(hwnd);
    }
}

pub fn restore_focus() {
    let control = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| {
            if window.address.editing {
                Some(window.editors.address)
            } else if window.address.search_focus {
                Some(window.editors.search)
            } else {
                None
            }
        })
    });
    if let Some(control) = control {
        super::edit::focus(control);
    }
}

#[derive(Clone, Copy)]
pub enum Entry {
    Direct,
    Traversal,
}

pub fn focus_changed(hwnd: HWND, focus: Focus, entry: Entry) {
    let Focus::Address(target) = focus else {
        blur(hwnd);
        return;
    };
    super::address_popup::close();
    let operation = WINDOW
        .with(|slot| {
            slot.borrow_mut().as_mut().map(|window| {
                window.address.focus = Some(target);
                window.address.cues = true;
                window.address.editing = target == Target::Address;
                window.address.search_focus = target == Target::Search;
                window.address.address_edit.focused = target == Target::Address;
                window.address.search_edit.focused = target == Target::Search;
                match target {
                    Target::Address => {
                        window.address.automatic_history = matches!(entry, Entry::Direct);
                        let text = if window.address.path.is_empty()
                            || crate::app::ids::is_virtual(&window.address.path)
                        {
                            window.address.location.clone()
                        } else {
                            window.address.path.clone()
                        };
                        window.address.typed = text.clone();
                        window.address.address_edit.clear_history();
                        window.address.address_edit.text = text.clone();
                        window.address.generation += 1;
                        window.address_worker.send(Request::Suggestions(
                            window.address.generation,
                            String::new(),
                        ));
                        Some((window.editors.address, text))
                    }
                    Target::Search => Some((
                        window.editors.search,
                        window.address.search_edit.text.clone(),
                    )),
                    _ => None,
                }
            })
        })
        .flatten();
    if let Some((control, text)) = operation {
        if let Err(error) =
            super::edit::set_text(control, &text, (0, text.encode_utf16().count() as u32))
        {
            super::fail(error);
        }
        super::edit::focus(control);
        super::edit::notify(hwnd, control);
    } else {
        unsafe {
            let _ = SetFocus(Some(hwnd));
        }
    }
    invalidate(hwnd);
}

pub fn edit_changed(hwnd: HWND, control: HWND, snapshot: super::edit::Snapshot) {
    let query_changed = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            window.editors.search == control && window.address.search_edit.text != snapshot.text
        })
    });
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            if control == window.editors.address {
                let changed = window.address.address_edit.update(snapshot);
                if changed && window.address.editing {
                    window.address.automatic_history = true;
                    window.address.typed = window.address.address_edit.text.clone();
                    window.address.suggestion = None;
                    window.address.generation += 1;
                    window.address_worker.send(Request::Suggestions(
                        window.address.generation,
                        window.address.typed.clone(),
                    ));
                }
                true
            } else if control == window.editors.search {
                window.address.search_edit.update(snapshot);
                true
            } else {
                false
            }
        })
    });
    if changed {
        invalidate(hwnd);
        let search = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.editors.search == control)
        });
        if search && query_changed {
            super::search::edited(hwnd);
        }
    }
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    if super::address_popup::key(hwnd, key) {
        return true;
    }
    let modifiers = super::input::modifiers();
    let alt = unsafe { GetKeyState(0x12) < 0 };
    if (modifiers.ctrl && !alt && key == 0x4c)
        || (alt && !modifiers.ctrl && key == 0x44)
        || (key == 0x73 && !alt && !modifiers.ctrl)
    {
        super::chrome::set_focus(hwnd, Focus::Address(Target::Address));
        return true;
    }
    if !alt && (modifiers.ctrl && matches!(key, 0x45 | 0x46) || !modifiers.ctrl && key == 0x72) {
        super::chrome::set_focus(hwnd, Focus::Address(Target::Search));
        return true;
    }
    let focus = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.address.focus)
    });
    if alt {
        return false;
    }
    let Some(target) = focus else {
        return false;
    };
    if key == 0x74 {
        activate(hwnd, Target::Refresh);
        return true;
    }
    if matches!(key, 0x09 | 0x75) {
        return false;
    }
    if key == 0x1b {
        super::address_popup::close();
        return true;
    }
    if key == 0x0d || key == 0x20 || key == 0x28 {
        let cloud = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.address.cloud.is_some())
        });
        activate(
            hwnd,
            if target == Target::Root && cloud {
                Target::Cloud
            } else {
                target
            },
        );
        return true;
    }
    if matches!(key, 0x25 | 0x27)
        && matches!(
            target,
            Target::Root | Target::More | Target::Crumb(_) | Target::Chevron(_)
        )
    {
        let next = WINDOW.with(|slot| {
            slot.borrow().as_ref().map(|window| {
                let mut targets = vec![Target::Root];
                if window.address.hidden > 0 {
                    targets.push(Target::More);
                }
                targets.extend(
                    (window.address.hidden..window.address.crumbs.len()).map(Target::Crumb),
                );
                let index = targets.iter().position(|item| *item == target).unwrap_or(0);
                targets[(index as isize + if key == 0x25 { -1 } else { 1 })
                    .clamp(0, targets.len() as isize - 1) as usize]
            })
        });
        if let Some(next) = next {
            super::chrome::set_focus(hwnd, Focus::Address(next));
        }
        return true;
    }
    true
}

pub fn edit_key(hwnd: HWND, control: HWND, key: u32) -> bool {
    let composing = super::edit::composing(control);
    if composing && matches!(key, 0x0d | 0x1b | 0x26 | 0x28) {
        return false;
    }
    let address = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.editors.address == control)
    });
    if key == 0x74 && !composing {
        return self::key(hwnd, key);
    }
    if key == 0x1b {
        if !address {
            super::search::cancel(hwnd);
        }
        super::chrome::set_focus(
            hwnd,
            if address {
                Focus::Address(Target::Root)
            } else {
                Focus::List
            },
        );
        return true;
    }
    if key == 0x0d {
        if address {
            submit(hwnd);
        } else if let Err(error) = super::search::submit(hwnd, super::search::Submission::Explicit)
        {
            super::fail(error);
        }
        return true;
    }
    if address && matches!(key, 0x26 | 0x28) {
        pick(hwnd, if key == 0x26 { -1 } else { 1 });
        return true;
    }
    if matches!(key, 0x09 | 0x75) {
        super::chrome::key(hwnd, key);
        return true;
    }
    let modifiers = super::input::modifiers();
    let alt = unsafe { GetKeyState(0x12) < 0 };
    if (modifiers.ctrl && !alt && matches!(key, 0x4c | 0x45 | 0x46))
        || (alt && !modifiers.ctrl && key == 0x44)
        || (matches!(key, 0x72 | 0x73) && !alt && !modifiers.ctrl)
    {
        return self::key(hwnd, key);
    }
    if alt && !modifiers.ctrl && matches!(key, 0x25 | 0x27 | 0x26) {
        let navigation = match key {
            0x25 => super::Navigation::Back,
            0x27 => super::Navigation::Forward,
            _ => super::Navigation::Up,
        };
        if let Err(error) = super::navigate(hwnd, navigation) {
            super::fail(error);
        }
        return true;
    }
    false
}

fn submit(hwnd: HWND) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.address_worker.send(Request::Submit(
                window.address.address_edit.text.clone(),
                window.address.path.clone(),
                window.address.location.clone(),
            ));
        }
    });
    super::chrome::set_focus(hwnd, Focus::Address(Target::Root));
}

fn pick(hwnd: HWND, step: isize) {
    let pick = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|window| {
            let address = &mut window.address;
            if address.suggestions.is_empty() {
                return None;
            }
            let next = (address.suggestion.map_or(-1, |i| i as isize) + step + 1)
                .rem_euclid(address.suggestions.len() as isize + 1)
                - 1;
            address.suggestion = (next >= 0).then_some(next as usize);
            let text = address
                .suggestion
                .map(|index| address.suggestions[index].clone())
                .unwrap_or_else(|| address.typed.clone());
            if address.suggestion.is_none() {
                address.generation += 1;
                address.suggestions = address.typed_paths.clone();
            }
            Some((window.editors.address, text))
        })
    });
    if let Some((control, text)) = pick {
        let length = text.encode_utf16().count() as u32;
        if let Err(error) = super::edit::set_text(control, &text, (length, length)) {
            super::fail(error);
        }
        super::edit::notify_preview(hwnd, control);
        show_suggestions(hwnd);
    }
}

pub fn activate(hwnd: HWND, target: Target) {
    let result: Result<()> = (|| {
        match target {
            Target::Settings => super::settings::open(hwnd)?,
            Target::Back => super::navigate(hwnd, super::Navigation::Back)?,
            Target::Forward => super::navigate(hwnd, super::Navigation::Forward)?,
            Target::Up => super::navigate(hwnd, super::Navigation::Up)?,
            Target::Refresh => {
                if super::settings::active() {
                    super::settings::refresh(hwnd);
                    return Ok(());
                }
                if super::hosted::refresh(hwnd) {
                    return Ok(());
                }
                let search = WINDOW.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|window| {
                        crate::app::ids::is_search(window.tabs.active().location.tab.path())
                    })
                });
                if search {
                    super::search::reload(hwnd)?;
                    return Ok(());
                }
                let result = WINDOW.with(|slot| {
                    slot.borrow_mut()
                        .as_mut()
                        .map_or(Ok(()), |window| window.list.refresh(&window.scroll))
                });
                result.map_err(|error| {
                    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                })?;
                super::invalidate_view(hwnd);
            }
            Target::Crumb(index) => {
                let path = WINDOW.with(|slot| {
                    slot.borrow().as_ref().and_then(|window| {
                        window
                            .address
                            .crumbs
                            .get(index)
                            .map(|crumb| crumb.id.clone())
                    })
                });
                if let Some(path) = path {
                    super::navigate(hwnd, super::Navigation::Open(path))?;
                }
            }
            Target::AddressClear | Target::SearchClear => {
                let control = WINDOW.with(|slot| {
                    slot.borrow().as_ref().map(|window| {
                        if target == Target::AddressClear {
                            window.editors.address
                        } else {
                            window.editors.search
                        }
                    })
                });
                if let Some(control) = control {
                    super::edit::set_text(control, "", (0, 0))?;
                    super::edit::focus(control);
                    super::edit::notify(hwnd, control);
                    if target == Target::SearchClear {
                        super::search::submit(hwnd, super::search::Submission::Typing)?;
                    }
                }
            }
            Target::More => {
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        window.address.open = Some(target);
                    }
                });
                let items = WINDOW.with(|slot| {
                    slot.borrow().as_ref().map(|window| {
                        window
                            .address
                            .crumbs
                            .iter()
                            .take(window.address.hidden)
                            .rev()
                            .map(|crumb| super::address::Item {
                                label: crumb.name.clone(),
                                path: crumb.id.clone(),
                            })
                            .collect()
                    })
                });
                if let Some(items) = items {
                    super::address_popup::show(hwnd, target, items, None)?;
                }
                invalidate(hwnd);
            }
            Target::Root | Target::Chevron(_) => {
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        let path = match target {
                            Target::Chevron(index) => window
                                .address
                                .crumbs
                                .get(index)
                                .map(|crumb| crumb.id.clone())
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        window.address.generation += 1;
                        window.address.open = Some(target);
                        window.address_worker.send(Request::Children(
                            window.address.generation,
                            target,
                            path,
                        ));
                    }
                });
                invalidate(hwnd);
            }
            Target::Address | Target::Search => {
                super::chrome::set_focus(hwnd, Focus::Address(target))
            }
            Target::Cloud => cloud_menu(hwnd)?,
        }
        Ok(())
    })();
    if let Err(error) = result {
        super::fail(error);
    }
}

pub fn ready(hwnd: HWND) {
    let replies = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|window| window.address_worker.receive())
            .unwrap_or_default()
    });
    for reply in replies {
        match reply {
            Reply::DragPicture(token, picture) => super::drop::picture(hwnd, token, picture),
            Reply::Infotip(token, text) => super::tooltip::ready(hwnd, token, text),
            Reply::Cloud(snapshot, image) => {
                let changed = WINDOW.with(|slot| {
                    slot.borrow_mut().as_mut().is_some_and(|window| {
                        if window.tabs.active().location.tab.path() != snapshot.path {
                            return false;
                        }
                        window.address.cloud = (!snapshot.label.is_empty()).then_some(snapshot);
                        window.address.cloud_image = image;
                        true
                    })
                });
                if changed {
                    invalidate(hwnd);
                    super::menu::cloud_changed(hwnd);
                }
            }
            Reply::Suggestions(generation, typed_paths, items) => {
                let accepted = WINDOW.with(|slot| {
                    slot.borrow_mut().as_mut().is_some_and(|window| {
                        if window.address.generation != generation || !window.address.editing {
                            return false;
                        }
                        window.address.suggestions = items;
                        window.address.typed_paths = typed_paths;
                        window.address.suggestion = None;
                        true
                    })
                });
                if accepted
                    && WINDOW.with(|slot| {
                        slot.borrow()
                            .as_ref()
                            .is_some_and(|window| window.address.automatic_history)
                    })
                {
                    show_suggestions(hwnd);
                }
            }
            Reply::Children(generation, target, items) => {
                let accepted = WINDOW.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|window| {
                        window.address.generation == generation
                            && window.address.open == Some(target)
                    })
                });
                if !accepted {
                    continue;
                }
                match items {
                    Ok(items) => {
                        if let Err(error) = super::address_popup::show(hwnd, target, items, None) {
                            super::fail(error);
                        }
                    }
                    Err(error) => eprintln!("address children: {error}"),
                }
            }
            Reply::Submitted(text, path, submission) => {
                if let Some(submission) = submission {
                    if let Some(target) =
                        crate::app::address::execute(hwnd, submission, &path, &text)
                    {
                        if let Err(error) = super::navigate(hwnd, super::Navigation::Open(target)) {
                            super::fail(error);
                        }
                    }
                }
            }
        }
    }
}

fn cloud_menu(hwnd: HWND) -> Result<()> {
    super::cloud::show(hwnd)
}

fn show_suggestions(hwnd: HWND) {
    let items = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|window| window.address.editing)
            .map(|window| {
                (
                    window
                        .address
                        .suggestions
                        .iter()
                        .map(|text| super::address::Item {
                            label: text.clone(),
                            path: text.clone(),
                        })
                        .collect::<Vec<_>>(),
                    window.address.suggestion,
                )
            })
    });
    if let Some((items, picked)) = items {
        if items.is_empty() {
            super::address_popup::close();
        } else if let Err(error) = super::address_popup::show(hwnd, Target::Address, items, picked)
        {
            super::fail(error);
        }
    }
}

pub fn chosen(hwnd: HWND, target: Target, item: super::address::Item) {
    if target == Target::Address {
        let control =
            WINDOW.with(|slot| slot.borrow().as_ref().map(|window| window.editors.address));
        if let Some(control) = control {
            if let Err(error) = super::edit::set_text(control, &item.path, (0, 0)) {
                super::fail(error);
            }
            super::edit::notify_preview(hwnd, control);
            submit(hwnd);
        }
    } else if let Err(error) = super::navigate(hwnd, super::Navigation::Open(item.path)) {
        super::fail(error);
    }
}

pub fn context(hwnd: HWND, point: (f32, f32)) -> bool {
    let target = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.address.hit(point))
    });
    let Some(target) = target else {
        return false;
    };
    if matches!(target, Target::Back | Target::Forward) {
        return history(hwnd, target, point);
    }
    if matches!(target, Target::Address | Target::Search) {
        let focused = WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                if target == Target::Address {
                    window.address.editing
                } else {
                    window.address.search_focus
                }
            })
        });
        if !focused {
            super::chrome::set_focus(hwnd, Focus::Address(target));
        }
        if let Err(error) = text_menu(hwnd, target, point) {
            super::fail(error);
        }
        return true;
    }
    if matches!(
        target,
        Target::Address | Target::Crumb(_) | Target::Chevron(_) | Target::Root | Target::More
    ) {
        let address = WINDOW
            .with(|slot| {
                slot.borrow().as_ref().map(|window| {
                    if window.address.path.is_empty()
                        || crate::app::ids::is_virtual(&window.address.path)
                    {
                        window.address.location.clone()
                    } else {
                        window.address.path.clone()
                    }
                })
            })
            .unwrap_or_default();
        let menu = crate::app::menu_model::Menu {
            owner: "address",
            items: crate::app::menu_model::address_items(address),
            buttons: Vec::new(),
        };
        if let Err(error) = super::menu_actions::show(
            hwnd,
            menu,
            super::menu::Anchor::Point(point.0, point.1),
            super::menu::Origin::Pointer,
        ) {
            super::fail(error);
        }
        return true;
    }
    false
}

fn text_menu(hwnd: HWND, target: Target, point: (f32, f32)) -> Result<()> {
    let state = WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|window| {
            let (control, edit) = if target == Target::Address {
                (window.editors.address, &window.address.address_edit)
            } else {
                (window.editors.search, &window.address.search_edit)
            };
            (control, edit.start != edit.end, !edit.text.is_empty())
        })
    });
    let Some((control, selection, nonempty)) = state else {
        return Ok(());
    };
    let state = crate::app::menu_model::EditorState {
        selection,
        nonempty,
        paste: unsafe { windows::Win32::System::DataExchange::IsClipboardFormatAvailable(13) }
            .is_ok(),
    };
    let _ = control;
    let menu = crate::app::menu_model::Menu {
        owner: if target == Target::Search {
            "search-text"
        } else {
            "address-text"
        },
        items: crate::app::menu_model::address_text_items(state),
        buttons: Vec::new(),
    };
    super::menu_actions::show(
        hwnd,
        menu,
        super::menu::Anchor::Context(point.0, point.1),
        super::menu::Origin::Pointer,
    )
}

fn history(hwnd: HWND, target: Target, point: (f32, f32)) -> bool {
    let entries = WINDOW
        .with(|slot| {
            slot.borrow().as_ref().map(|window| {
                let indices: Vec<_> = if target == Target::Back {
                    (0..window.tabs.active().location.tab.index).rev().collect()
                } else {
                    (window.tabs.active().location.tab.index + 1
                        ..window.tabs.active().location.tab.history.len())
                        .collect()
                };
                indices
                    .into_iter()
                    .map(|index| {
                        (
                            index,
                            window.tabs.active().location.tab.history[index].clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default();
    if entries.is_empty() {
        return true;
    }
    let menu = crate::app::menu_model::Menu {
        owner: "history",
        items: crate::app::menu_model::history_items(entries),
        buttons: Vec::new(),
    };
    if let Err(error) = super::menu_actions::show(
        hwnd,
        menu,
        super::menu::Anchor::Point(point.0, point.1),
        super::menu::Origin::Pointer,
    ) {
        super::fail(error);
    }
    true
}
