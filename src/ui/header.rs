use super::{details::Viewport, scroll, WINDOW};
use crate::app::{columns, header::Drag, sort::SortKey};
use std::time::Instant;
use windows::core::Result;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;
use windows::Win32::UI::WindowsAndMessaging::*;

const ANIMATION_TIMER: usize = 9;

pub enum Press {
    Resize { column: usize, x: f32, width: f32 },
    Cell(Drag),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Look {
    Idle,
    Hover,
    Pressed,
    Focused,
}

#[derive(Default)]
pub struct State {
    pub hover: Option<usize>,
    pub focus: Option<usize>,
    pub pressed: Option<Press>,
    pub ghost: Option<(usize, f32, bool)>,
    pub dirty: bool,
    from: Vec<f32>,
    target: Vec<f32>,
    motion: Option<Instant>,
    timer_running: bool,
}

impl State {
    pub fn look(&self, column: usize) -> Look {
        if self.ghost.is_some() {
            return Look::Idle;
        }
        if super::filter::framed_column() == Some(column) || self.focus == Some(column) {
            return Look::Focused;
        }
        if self.hover != Some(column) {
            return Look::Idle;
        }
        if self
            .pressed
            .as_ref()
            .is_some_and(|p| matches!(p,Press::Cell(d) if d.column==column))
        {
            Look::Pressed
        } else {
            Look::Hover
        }
    }
    pub fn positions(&self, cols: &[columns::Col]) -> Vec<f32> {
        if let Some(start) = self.motion {
            let t = (start.elapsed().as_secs_f32() / 0.333).min(1.0);
            let mut low: f32 = 0.0;
            let mut high: f32 = 1.0;
            for _ in 0..16 {
                let u = (low + high) * 0.5;
                let x =
                    3.0 * (1.0 - u).powi(2) * u * 0.42 + 3.0 * (1.0 - u) * u * u * 0.58 + u * u * u;
                if x < t {
                    low = u
                } else {
                    high = u
                }
            }
            let u = (low + high) * 0.5;
            let amount = 3.0 * (1.0 - u) * u * u + u * u * u;
            return self
                .from
                .iter()
                .zip(&self.target)
                .map(|(a, b)| a + (b - a) * amount)
                .collect();
        }
        if !self.target.is_empty() {
            return self.target.clone();
        }
        let mut x = 0.0;
        cols.iter()
            .map(|c| {
                let left = x;
                x += c.width;
                left
            })
            .collect()
    }
}

fn content_x(view: Viewport, scroll: &scroll::State, point: (f32, f32)) -> Option<f32> {
    if !matches!(view.header, 25.0 | 57.0) {
        return None;
    }
    (point.0 >= view.x
        && point.0 < view.x + view.right
        && point.1 >= view.y + view.header - 25.0
        && point.1 < view.y + view.header)
        .then_some(point.0 - view.x + scroll.x)
}

fn cell(cols: &[columns::Col], x: f32) -> Option<usize> {
    let mut right = 0.0;
    cols.iter().position(|c| {
        right += c.width;
        x < right
    })
}

fn divider(cols: &[columns::Col], x: f32) -> Option<usize> {
    let mut right = 0.0;
    cols.iter().position(|c| {
        right += c.width;
        (x - right).abs() < 4.0
    })
}

fn error(result: std::result::Result<(), String>) {
    if let Err(error) = result {
        super::fail(windows::core::Error::new(
            windows::Win32::Foundation::E_FAIL,
            error,
        ));
    }
}

pub fn flush(hwnd: HWND) {
    let (header, layout, count) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((false, false, 0), |w| {
            let layout = std::mem::take(&mut w.list.layout_changed);
            (
                std::mem::take(&mut w.list.header.dirty) || layout,
                layout,
                w.list.folder.as_ref().map_or(0, |f| f.order.len()),
            )
        })
    });
    if header {
        super::invalidate_header(hwnd);
    }
    if layout {
        super::invalidate_rows(hwnd, 0..count);
        super::invalidate_view(hwnd);
    }
}

pub fn moved(hwnd: HWND, point: (f32, f32)) {
    let mut animate = false;
    let mut result = Ok(());
    WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(w) = slot.as_mut() else { return };
        let Some(folder) = &w.list.folder else { return };
        let Some(view) = w.scroll.view else { return };
        let x = point.0 - view.x + w.scroll.x;
        let hover = content_x(view, &w.scroll, point).and_then(|x| cell(&folder.columns, x));
        let old = w.list.header.hover;
        w.list.header.hover = hover;
        if old != hover {
            w.list.header.dirty = true;
        }
        match w.list.header.pressed.as_mut() {
            Some(Press::Resize {
                column,
                x: origin,
                width,
            }) => {
                let (column, width) = (
                    *column,
                    crate::app::header::resize_width(*width, x - *origin),
                );
                result = w.list.widths(&[(column, width)]);
            }
            Some(Press::Cell(drag)) => {
                let before = drag.lefts();
                if let Some(position) = drag.moved(x) {
                    let target = drag.lefts();
                    let index = drag.column;
                    let start = w.list.header.positions(&folder.columns);
                    if before != target {
                        w.list.header.from = start;
                        w.list.header.target = target;
                        w.list.header.motion = Some(Instant::now());
                        w.list.header.timer_running = true;
                        animate = true;
                    }
                    w.list.header.ghost = Some((index, position.ghost, position.over));
                    w.list.header.dirty = true;
                }
            }
            None => {}
        }
    });
    error(result);
    if animate {
        if unsafe { SetTimer(Some(hwnd), ANIMATION_TIMER, 16, None) } == 0 {
            super::fail(windows::core::Error::from_win32());
        }
        WINDOW.with(|slot| {
            if let Some(w) = slot.borrow_mut().as_mut() {
                w.input_timers.insert(ANIMATION_TIMER);
            }
        });
        super::input::publish_timers(hwnd);
    }
    flush(hwnd);
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    let filter = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let folder = window.list.folder.as_ref()?;
        let view = window.scroll.view?;
        let x = content_x(view, &window.scroll, point)?;
        let column = cell(&folder.columns, x)?;
        let right = folder.columns[..=column]
            .iter()
            .map(|column| column.width)
            .sum::<f32>();
        (x >= right - 16.0 && x < right - 2.0).then_some((
            column,
            (
                view.x + right - 16.0 - window.scroll.x,
                view.y + view.header,
            ),
        ))
    });
    if let Some((column, anchor)) = filter {
        super::filter::open(hwnd, column, anchor);
        return true;
    }

    let (used, capture) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((false, false), |w| {
            let Some(folder) = &w.list.folder else {
                return (false, false);
            };
            let Some(view) = w.scroll.view else {
                return (false, false);
            };
            let Some(x) = content_x(view, &w.scroll, point) else {
                return (false, false);
            };
            if folder.settings.check_boxes && x >= 11.0 && x < 24.0 {
                if w.list.interaction.selection.selected.len() == folder.order.len() {
                    w.list.interaction.selection.selected.clear();
                } else {
                    w.list.interaction.selection.selected = folder
                        .order
                        .iter()
                        .map(|&index| folder.entries[index].path.clone())
                        .collect();
                }
                w.list.interaction.mark_all();
                w.list.header.dirty = true;
                return (true, false);
            }
            let pressed = if let Some(column) = divider(&folder.columns, x) {
                Some(Press::Resize {
                    column,
                    x,
                    width: folder.columns[column].width,
                })
            } else if let Some(column) = cell(&folder.columns, x) {
                let right: f32 = folder.columns[..=column].iter().map(|c| c.width).sum();
                if x >= right - 16.0 {
                    None
                } else {
                    Drag::new(column, folder.columns.iter().map(|c| c.width).collect(), x)
                        .map(Press::Cell)
                }
            } else {
                None
            };
            let capture = pressed.is_some();
            w.list.header.pressed = pressed;
            w.list.header.dirty = true;
            (true, capture)
        })
    });
    if capture {
        unsafe {
            SetCapture(hwnd);
        }
    }
    flush(hwnd);
    used
}

pub fn release(hwnd: HWND, point: (f32, f32)) -> bool {
    let started = Instant::now();
    let mut result = Ok(());
    let mut sorting = None;
    let used = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|w| {
            let Some(pressed) = w.list.header.pressed.take() else {
                return false;
            };
            if let Press::Cell(mut drag) = pressed {
                if let Some(order) = drag.finish() {
                    if let Some(folder) = &w.list.folder {
                        let cols = order
                            .iter()
                            .filter_map(|&i| folder.columns.get(i).cloned())
                            .collect();
                        result = w.list.set_columns(cols, &w.scroll);
                    }
                } else if let (Some(folder), Some(view)) = (&w.list.folder, w.scroll.view) {
                    if content_x(view, &w.scroll, point).and_then(|x| cell(&folder.columns, x))
                        == Some(drag.column)
                    {
                        sorting = Some((
                            (!folder.view.search_ranked).then_some(folder.view.sort),
                            SortKey::from_property(&folder.columns[drag.column].key),
                        ));
                        w.list.interaction.cues = false;
                        w.list.interaction.mark_all();
                    }
                }
            }
            w.list.header.ghost = None;
            w.list.header.motion = None;
            w.list.header.target.clear();
            w.list.header.dirty = true;
            true
        })
    });
    if let Some((current, key)) = sorting {
        let sort = crate::app::header::clicked(current, key);
        result = WINDOW.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map_or(Ok(()), |w| w.list.sort(sort, &w.scroll))
        });
    }
    if used {
        if let Err(error) = super::release_capture(hwnd) {
            super::fail(error);
        }
        stop_animation(hwnd);
    }
    error(result);
    flush(hwnd);
    if used {
        super::memory::work("header-input", started);
    }
    used
}

fn stop_animation(hwnd: HWND) {
    let running = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|w| {
            w.list.header.motion = None;
            w.input_timers.remove(&ANIMATION_TIMER);
            std::mem::take(&mut w.list.header.timer_running)
        })
    });
    if running {
        if let Err(error) = unsafe { KillTimer(Some(hwnd), ANIMATION_TIMER) } {
            super::fail(error);
        }
    }
    super::input::publish_timers(hwnd);
}

pub fn cancel(hwnd: HWND) {
    stop_animation(hwnd);
    WINDOW.with(|slot| {
        if let Some(w) = slot.borrow_mut().as_mut() {
            if w.list.header.pressed.take().is_some() || w.list.header.ghost.take().is_some() {
                w.list.header.dirty = true;
            }
            w.list.header.target.clear();
        }
    });
    flush(hwnd);
}

pub fn leave(hwnd: HWND) {
    WINDOW.with(|slot| {
        if let Some(w) = slot.borrow_mut().as_mut() {
            if w.list.header.hover.take().is_some() {
                w.list.header.dirty = true;
            }
        }
    });
    flush(hwnd);
}

pub fn timer(hwnd: HWND, timer: usize) -> bool {
    if timer != ANIMATION_TIMER {
        return false;
    }
    let done = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_none_or(|w| {
            w.list
                .header
                .motion
                .is_none_or(|start| start.elapsed().as_millis() >= 333)
        })
    });
    if done {
        stop_animation(hwnd);
    }
    super::invalidate_header(hwnd);
    true
}

pub fn fit(hwnd: HWND, index: Option<usize>) {
    let result = WINDOW.with(|slot| {
        slot.borrow().as_ref().map_or(Ok(()), |w| {
            let Some(folder) = &w.list.folder else {
                return Ok(());
            };
            let Some(view) = w.scroll.view else {
                return Ok(());
            };
            let indices = index.map_or_else(|| (0..folder.columns.len()).collect(), |i| vec![i]);
            w.list.fit(indices, view.visible(folder, w.scroll.y))
        })
    });
    error(result);
    flush(hwnd);
}

pub fn double_click(hwnd: HWND, point: (f32, f32)) -> bool {
    let hit = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|w| {
            let folder = w.list.folder.as_ref()?;
            let view = w.scroll.view?;
            let x = content_x(view, &w.scroll, point)?;
            Some(divider(&folder.columns, x))
        })
    });
    match hit {
        Some(Some(index)) => {
            fit(hwnd, Some(index));
            true
        }
        Some(None) => press(hwnd, point),
        None => false,
    }
}

pub fn cursor(hwnd: HWND) -> bool {
    let mut point = POINT::default();
    if let Err(error) = unsafe { GetCursorPos(&mut point) } {
        super::fail(error);
        return false;
    }
    if !unsafe { windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point) }.as_bool() {
        super::fail(windows::core::Error::from_win32());
        return false;
    }
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    let resize = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|w| {
            if matches!(w.list.header.pressed, Some(Press::Resize { .. })) {
                return true;
            }
            let Some(folder) = &w.list.folder else {
                return false;
            };
            let Some(view) = w.scroll.view else {
                return false;
            };
            content_x(
                view,
                &w.scroll,
                (point.x as f32 / scale, point.y as f32 / scale),
            )
            .and_then(|x| divider(&folder.columns, x))
            .is_some()
        })
    });
    if !resize {
        return false;
    }
    match unsafe { LoadCursorW(None, IDC_SIZEWE) } {
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

pub fn key(hwnd: HWND, key: u32) -> bool {
    if key == 0x6b && super::input::modifiers().ctrl {
        fit(hwnd, None);
        true
    } else {
        false
    }
}

pub fn context(hwnd: HWND, point: (f32, f32)) -> bool {
    let menu = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|w| {
            let folder = w.list.folder.as_ref()?;
            let view = w.scroll.view?;
            let x = content_x(view, &w.scroll, point)?;
            Some((
                cell(&folder.columns, x),
                folder.view.menu_columns.clone(),
                folder.columns.clone(),
            ))
        })
    });
    let Some((column, keys, cols)) = menu else {
        return false;
    };
    let entries = columns::menu_entries(&keys, &cols);
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    let mut at = POINT {
        x: (point.0 * scale).round() as i32,
        y: (point.1 * scale).round() as i32,
    };
    let result = (|| -> Result<()> {
        if !unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at) }.as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        match columns::show_header_menu(hwnd, column, at, &entries)? {
            columns::HeaderCommand::Fit(index) => fit(hwnd, index),
            columns::HeaderCommand::Choose => choose(hwnd),
            columns::HeaderCommand::Toggle(key) => {
                let shown = WINDOW.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .and_then(|w| w.list.folder.as_ref().map(|f| f.view.columns.clone()))
                });
                if let Some(shown) = shown {
                    let cols = columns::shown_columns(columns::toggled(&shown, key));
                    let changed = WINDOW.with(|slot| {
                        slot.borrow_mut()
                            .as_mut()
                            .map_or(Ok(()), |w| w.list.set_columns(cols, &w.scroll))
                    });
                    error(changed);
                }
            }
            columns::HeaderCommand::None => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        super::fail(error);
    }
    flush(hwnd);
    true
}

pub(super) fn choose(hwnd: HWND) {
    let shown = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|w| w.list.folder.as_ref().map(|f| f.view.columns.clone()))
    });
    let Some(shown) = shown else { return };
    let Some(chosen) = crate::app::choose::choose(hwnd, &shown) else {
        return;
    };
    let shown = columns::chosen(chosen);
    let cols = columns::shown_columns(shown);
    let result = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or(Ok(()), |w| w.list.set_columns(cols, &w.scroll))
    });
    error(result);
    flush(hwnd);
}
