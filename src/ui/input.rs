use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{interaction::Action, WINDOW};
use crate::app::selection::Modifiers;

const BAND_TIMER: usize = 7;
const HOVER_TIMER: usize = 8;

pub fn modifiers() -> Modifiers {
    unsafe {
        Modifiers {
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
        }
    }
}

pub(super) fn stop(hwnd: HWND, timer: usize) {
    let armed = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|w| w.input_timers.remove(&timer))
    });
    if armed {
        if let Err(error) = unsafe { KillTimer(Some(hwnd), timer) } {
            super::fail(error);
        }
    }
    publish_timers(hwnd);
}

pub(super) fn arm(hwnd: HWND, timer: usize, delay: u32) {
    if unsafe { SetTimer(Some(hwnd), timer, delay, None) } == 0 {
        super::fail(windows::core::Error::from_win32());
        return;
    }
    WINDOW.with(|slot| {
        if let Some(w) = slot.borrow_mut().as_mut() {
            w.input_timers.insert(timer);
        }
    });
    publish_timers(hwnd);
}

pub(super) fn publish_timers(hwnd: HWND) {
    let count = WINDOW.with(|slot| slot.borrow().as_ref().map_or(0, |w| w.input_timers.len()));
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            windows::core::w!("NativeInputTimers"),
            Some(windows::Win32::Foundation::HANDLE(count as _)),
        )
    } {
        super::fail(error);
    }
}

pub fn flush(hwnd: HWND) {
    super::chrome::sync(hwnd);
    let dirty = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|w| w.list.interaction.take_dirty())
    });
    if let Some(rows) = dirty {
        let checks = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|w| w.list.folder.as_ref())
                .is_some_and(|folder| folder.settings.check_boxes)
        });
        if checks {
            super::invalidate_header(hwnd);
        }
        super::invalidate_rows(hwnd, rows);
    }
}

pub fn moved(hwnd: HWND, point: (f32, f32)) {
    super::rename::moved(hwnd, point);
    let mods = modifiers();
    let (hover, pending, band) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((None, false, None), |w| {
            let Some(folder) = &w.list.folder else {
                return (None, false, None);
            };
            w.list.interaction.move_pointer(point, folder, &w.scroll);
            let delay = w.list.interaction.hover_delay(folder, mods);
            let band = w
                .scroll
                .view
                .and_then(|view| w.list.interaction.band_scroll(view, folder))
                .map(|(axis, _)| {
                    if axis == super::scroll::Axis::Horizontal {
                        crate::app::selection::LIST_BAND_INTERVAL_MS
                    } else {
                        crate::app::selection::BAND_INTERVAL_MS
                    }
                });
            (delay, w.list.interaction.hover_pending, band)
        })
    });
    if let Some(delay) = hover {
        arm(hwnd, HOVER_TIMER, delay);
    } else if !pending {
        stop(hwnd, HOVER_TIMER);
    }
    let running = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|w| w.input_timers.contains(&BAND_TIMER))
    });
    if let Some(delay) = band {
        if !running {
            arm(hwnd, BAND_TIMER, delay);
        }
    } else {
        stop(hwnd, BAND_TIMER);
    }
    flush(hwnd);
}

pub fn refresh_hover(hwnd: HWND) {
    let mods = modifiers();
    let (delay, pending) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((None, false), |w| {
            let Some(folder) = &w.list.folder else {
                return (None, false);
            };
            (
                w.list.interaction.hover_delay(folder, mods),
                w.list.interaction.hover_pending,
            )
        })
    });
    if let Some(delay) = delay {
        arm(hwnd, HOVER_TIMER, delay);
    } else if !pending {
        stop(hwnd, HOVER_TIMER);
    }
}

pub fn leave(hwnd: HWND) {
    WINDOW.with(|slot| {
        if let Some(w) = slot.borrow_mut().as_mut() {
            w.list.interaction.leave();
        }
    });
    stop(hwnd, HOVER_TIMER);
    flush(hwnd);
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    super::drop::prepare_list(hwnd, point);
    super::rename::pressed(hwnd, point);
    stop(hwnd, HOVER_TIMER);
    let mods = modifiers();
    let used = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|w| {
            w.list.pending_selection = None;
            if let Some(folder) = &w.list.folder {
                w.list.interaction.press(point, folder, &w.scroll, mods)
            } else {
                false
            }
        })
    });
    if used {
        super::chrome::list_focus(hwnd);
        unsafe {
            if let Err(error) = SetFocus(Some(hwnd)) {
                if GetFocus() != hwnd {
                    eprintln!("native focus: {error}");
                }
            }
            SetCapture(hwnd);
        }
    } else {
        WINDOW.with(|slot| {
            if let Some(w) = slot.borrow_mut().as_mut() {
                if w.list.interaction.has_focus {
                    w.list.interaction.has_focus = false;
                    w.list.interaction.mark_all();
                }
            }
        });
    }
    flush(hwnd);
    let action = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|w| w.list.interaction.group_action.take())
    });
    if let Some(action) = action {
        act(hwnd, action);
    }
    used
}

pub fn release(hwnd: HWND) -> bool {
    let mods = modifiers();
    let (used, action) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((false, None), |w| {
            if !w.list.interaction.has_press() {
                return (false, None);
            }
            (
                true,
                w.list
                    .folder
                    .as_ref()
                    .and_then(|folder| w.list.interaction.release(folder, &w.scroll, mods)),
            )
        })
    });
    stop(hwnd, BAND_TIMER);
    stop(hwnd, HOVER_TIMER);
    if used {
        if let Err(error) = super::release_capture(hwnd) {
            super::fail(error);
        }
    }
    flush(hwnd);
    if let Some(action) = action {
        act(hwnd, action);
    }
    super::rename::released(hwnd);
    used
}

pub fn cancel(hwnd: HWND) {
    stop(hwnd, BAND_TIMER);
    stop(hwnd, HOVER_TIMER);
    WINDOW.with(|slot| {
        if let Some(w) = slot.borrow_mut().as_mut() {
            if w.list.interaction.has_press() {
                w.list.interaction.cancel();
            }
        }
    });
    flush(hwnd);
}

pub fn double_click(hwnd: HWND, point: (f32, f32)) {
    let action = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|w| {
            let folder = w.list.folder.as_ref()?;
            if folder.settings.single_click {
                return None;
            }
            w.list.interaction.pointer = Some(point);
            let row = w.list.interaction.hit(w.scroll.view?, &w.scroll, folder)?;
            w.list.interaction.cues = false;
            w.list.interaction.open_row(folder, row)
        })
    });
    if let Some(action) = action {
        act(hwnd, action);
    }
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    let before = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.list.interaction.selection.focused.clone())
    });
    let mods = modifiers();
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) < 0 };
    let action = if alt && !mods.ctrl {
        match key {
            0x25 => Some(Action::Back),
            0x27 => Some(Action::Forward),
            0x26 => Some(Action::Up),
            _ => None,
        }
    } else {
        None
    };
    if let Some(action) = action {
        WINDOW.with(|slot| {
            if let Some(w) = slot.borrow_mut().as_mut() {
                w.list.interaction.cues = true;
            }
        });
        cancel(hwnd);
        act(hwnd, action);
        return true;
    }
    let used = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|w| w.list.interaction.has_focus)
    });
    if !used || alt {
        return false;
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.pending_selection = None;
        }
    });
    stop(hwnd, HOVER_TIMER);
    if key == 0x1b {
        WINDOW.with(|slot| {
            if let Some(w) = slot.borrow_mut().as_mut() {
                w.list.cut.clear();
                w.list.interaction.mark_all();
            }
        });
        flush(hwnd);
        return true;
    }
    if key == 0x71 {
        super::rename::focused(hwnd);
        return true;
    }
    if key == 0x74 {
        let search = WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                crate::app::ids::is_search(window.tabs.active().location.tab.path())
            })
        });
        if search {
            if let Err(error) = super::search::reload(hwnd) {
                super::fail(error);
            }
            return true;
        }
        let result = WINDOW.with(|slot| {
            slot.borrow_mut().as_mut().map_or(Ok(()), |w| {
                w.list.interaction.cues = true;
                w.list.refresh(&w.scroll)
            })
        });
        if let Err(error) = result {
            super::fail(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                error,
            ));
        }
        return true;
    }
    let action = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|w| {
            let folder = w.list.folder.as_ref()?;
            w.list.interaction.key(key, folder, &mut w.scroll, mods)
        })
    });
    super::invalidate_view(hwnd);
    flush(hwnd);
    if let Some(action) = action {
        act(hwnd, action);
    }
    super::tooltip::key_moved(hwnd, before);
    matches!(key, 0x21..=0x28 | 0x20 | 0x0d | 0x08) || (key == 0x41 && mods.ctrl)
}

pub fn character(hwnd: HWND, value: u32) {
    if modifiers().ctrl
        || !WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.list.interaction.has_focus)
        })
    {
        return;
    }
    if let Some(ch) = char::from_u32(value) {
        let before = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|window| window.list.interaction.selection.focused.clone())
        });
        WINDOW.with(|slot| {
            if let Some(w) = slot.borrow_mut().as_mut() {
                if let Some(folder) = &w.list.folder {
                    w.list.interaction.character(ch, folder, &mut w.scroll);
                }
            }
        });
        super::invalidate_view(hwnd);
        flush(hwnd);
        super::tooltip::key_moved(hwnd, before);
    }
}

pub fn cursor() -> bool {
    let hand = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|w| {
            w.list.interaction.hover.is_some()
                && w.list
                    .folder
                    .as_ref()
                    .is_some_and(|f| f.settings.single_click)
        })
    });
    if !hand {
        return false;
    }
    match unsafe { LoadCursorW(None, IDC_HAND) } {
        Ok(cursor) => {
            unsafe {
                SetCursor(Some(cursor));
            }
            true
        }
        Err(error) => {
            super::fail(error);
            false
        }
    }
}

pub fn timer(hwnd: HWND, timer: usize) -> bool {
    if timer != BAND_TIMER && timer != HOVER_TIMER {
        return false;
    }
    let running = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|w| w.input_timers.contains(&timer))
    });
    if !running {
        return true;
    }
    let mods = modifiers();
    if timer == HOVER_TIMER {
        stop(hwnd, HOVER_TIMER);
    }
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|w| {
            let Some(folder) = &w.list.folder else {
                return false;
            };
            if timer == HOVER_TIMER {
                if unsafe {
                    GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0
                        || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0
                } {
                    return false;
                }
                w.list.interaction.hover_select(folder, mods);
                return false;
            }
            let Some(view) = w.scroll.view else {
                return false;
            };
            let Some((axis, delta)) = w.list.interaction.band_scroll(view, folder) else {
                return false;
            };
            let old = (w.scroll.x, w.scroll.y);
            w.scroll.band_step(axis, delta);
            super::memory::band_tick(
                if axis == super::scroll::Axis::Horizontal {
                    old.0
                } else {
                    old.1
                },
                if axis == super::scroll::Axis::Horizontal {
                    w.scroll.x
                } else {
                    w.scroll.y
                },
            );
            if old != (w.scroll.x, w.scroll.y) {
                if let Some(point) = w.list.interaction.pointer {
                    w.list.interaction.move_pointer(point, folder, &w.scroll);
                }
            }
            old != (w.scroll.x, w.scroll.y)
        })
    });
    if changed {
        super::invalidate_view(hwnd);
    }
    if timer == BAND_TIMER && !changed {
        stop(hwnd, BAND_TIMER);
    }
    flush(hwnd);
    true
}

fn act(hwnd: HWND, action: Action) {
    let result: Result<()> = (|| {
        match action {
            Action::Open(paths) => {
                for path in paths {
                    let folder = crate::shell::ops::open(hwnd, &path).map_err(|error| {
                        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                    })?;
                    if let Some(path) = folder {
                        super::navigate(hwnd, super::Navigation::Open(path))?;
                        break;
                    }
                }
            }
            Action::Expand(label, expansion) => {
                let result = WINDOW.with(|slot| {
                    slot.borrow_mut()
                        .as_mut()
                        .map_or(Ok(()), |w| w.list.expand(&label, expansion))
                });
                result.map_err(|error| {
                    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                })?;
                super::invalidate_all(hwnd);
            }
            Action::Back => super::navigate(hwnd, super::Navigation::Back)?,
            Action::Forward => super::navigate(hwnd, super::Navigation::Forward)?,
            Action::Up => super::navigate(hwnd, super::Navigation::Up)?,
        }
        Ok(())
    })();
    if let Err(error) = result {
        super::fail(error);
    }
}

pub fn settings_changed(value: LPARAM) -> bool {
    value.0 != 0
        && unsafe {
            windows::core::PCWSTR(value.0 as _)
                .to_string()
                .is_ok_and(|name| name == "ShellState")
        }
}

pub fn update_settings() -> bool {
    let settings = crate::shell::listing::settings();
    let panes = crate::shell::listing::panes();
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(w) = slot.as_mut() else {
            return false;
        };
        let Some(folder) = w.list.folder.as_mut() else {
            return false;
        };
        if folder.settings == settings && folder.panes == panes {
            return false;
        }
        let reconstruct = folder.settings.compact != settings.compact
            || folder.settings.check_boxes != settings.check_boxes
            || folder.settings.compressed_color != settings.compressed_color;
        let refresh = folder.settings.show_hidden != settings.show_hidden
            || folder.settings.show_super_hidden != settings.show_super_hidden
            || folder.settings.hide_extensions != settings.hide_extensions;
        let folder = std::sync::Arc::make_mut(folder);
        folder.settings = settings;
        folder.panes = panes;
        folder.rebuild_layout();
        w.list.interaction.mark_all();
        if reconstruct {
            w.list.cut.clear();
            w.list.interaction.restore_view(folder);
            w.scroll.y = 0.0;
            if let Some(previous) = w.scroll.view {
                let view =
                    super::details::Viewport::new(previous.size, folder, w.list.navigation_width);
                w.scroll.layout(view, &folder.layout);
                if let Some(row) = w
                    .list
                    .interaction
                    .focus_row(folder)
                    .filter(|_| !w.list.interaction.selection.selected.is_empty())
                {
                    w.list.interaction.reveal(row, folder, view, &mut w.scroll);
                }
            }
        }
        if let Err(error) = w.list.sync_settings(settings, panes) {
            eprintln!("native options synchronization: {error}");
        }
        if refresh {
            if let Err(error) = w.list.refresh(&w.scroll) {
                eprintln!("native options refresh: {error}");
            }
        }
        true
    });
    if changed {
        if let Err(error) = super::tabs::refresh_titles(super::context::hwnd()) {
            super::fail(error);
        }
    }
    changed
}

pub fn network_notice(hwnd: HWND, point: (f32, f32)) -> bool {
    let hit = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            window
                .list
                .folder
                .as_ref()
                .is_some_and(|folder| folder.network_notice.is_some())
                && window.scroll.view.is_some_and(|view| {
                    point.0 >= view.x
                        && point.0 < view.x + view.right
                        && point.1 >= view.y
                        && point.1 < view.y + 32.0
                })
        })
    });
    if hit {
        if let Err(error) =
            crate::shell::ops::shell_execute(hwnd, "ms-settings:network-advancedsettings", None)
        {
            eprintln!("network settings: {error}");
        }
    }
    hit
}
