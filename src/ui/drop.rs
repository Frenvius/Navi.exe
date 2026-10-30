use super::{navigation::Hit, WINDOW};
use crate::dnd::{Resolver, Spot, Target};
use std::sync::Arc;
use std::time::Instant;
use windows::core::{w, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Ole::DROPEFFECT;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const FINISHED: u32 = WM_APP + 0x70;
const EXPAND: usize = 60;
const SCROLL: usize = 61;
const FILE_TAB: usize = 62;

struct Candidate {
    origin: POINT,
    paths: Vec<String>,
    selected: Option<std::collections::HashSet<String>>,
}

#[derive(Default)]
pub struct State {
    candidate: Option<Candidate>,
    closing: bool,
    right: bool,
    worker: Option<std::thread::JoinHandle<()>>,
    files: Vec<String>,
    folders: bool,
    point: Option<(f32, f32)>,
    expand: Option<(crate::app::navtree::RowRef, Instant)>,
    scroll_repeating: bool,
    file_hover: Option<usize>,
    file_pending: Option<(usize, Instant)>,
    token: u64,
    picture: Option<(
        Arc<crate::shell::icons::Rgba>,
        Arc<crate::shell::icons::Rgba>,
    )>,
    caption: Option<(String, DROPEFFECT, POINT)>,
    preview: Option<super::drag_visual::Popup>,
    preview_key: String,
}

pub struct Adapter(pub HWND);

impl Resolver for Adapter {
    fn target_at(&self, client: POINT) -> Option<Target> {
        let _context = super::context::enter(self.0);
        let point = logical(self.0, client);
        hover_tab(self.0, point);
        let target = WINDOW.with(|slot| {
            slot.borrow_mut().as_mut().and_then(|window| {
                window.drag.point = Some(point);
                target(window, point)
            })
        });
        hold(self.0, target.as_ref().and_then(|target| target.spot));
        target
    }
    fn highlight(&self, spot: Option<Spot>) {
        let _context = super::context::enter(self.0);
        highlight(self.0, spot);
    }
    fn caption(&self, caption: Option<(String, DROPEFFECT, POINT)>) {
        let _context = super::context::enter(self.0);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.drag.caption = caption;
            }
        });
        feedback(self.0);
    }
    fn entered(&self, files: Vec<String>) {
        let _context = super::context::enter(self.0);
        super::tooltip::suppress(self.0);
        let request = WINDOW.with(|slot| {
            let mut slot = slot.borrow_mut();
            let window = slot.as_mut()?;
            window.drag.folders = !files.is_empty()
                && files.iter().all(|path| {
                    std::path::Path::new(path).is_dir() || crate::shell::ops::is_folder(path)
                });
            window.drag.files = files;
            window.title.hover = super::titlebar::Target::None;
            window.drag.token = window.drag.token.wrapping_add(1);
            window.drag.picture = None;
            window
                .drag
                .files
                .first()
                .cloned()
                .map(|path| (window.drag.token, path))
        });
        if let Some((token, path)) = request {
            WINDOW.with(|slot| {
                if let Some(window) = slot.borrow().as_ref() {
                    window
                        .address_worker
                        .send(super::address::Request::DragPicture(token, path));
                }
            });
        }
        publish(self.0);
    }
    fn tab_preview(&self, client: POINT) -> bool {
        let _context = super::context::enter(self.0);
        file_tab(logical(self.0, client)).is_some()
    }
    fn left(&self) {
        let _context = super::context::enter(self.0);
        clear(self.0);
    }
    fn dropped(&self, transfer: Option<crate::shell::ops::Transfer>) {
        let _context = super::context::enter(self.0);
        clear(self.0);
        if let Some(transfer) = transfer {
            for operation in crate::app::undo::transfers(transfer) {
                super::menu_actions::push(self.0, operation);
            }
        }
        refresh(self.0);
    }
}

fn logical(hwnd: HWND, point: POINT) -> (f32, f32) {
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    (point.x as f32 / scale, point.y as f32 / scale)
}

fn target(window: &super::Window, point: (f32, f32)) -> Option<Target> {
    if let Some(super::address::Target::Crumb(index) | super::address::Target::Chevron(index)) =
        window.address.hit(point)
    {
        let id = window.address.crumbs.get(index)?.id.clone();
        return Some(Target {
            folder: id.clone(),
            item: Some(id),
            spot: Some(Spot::Crumb(index)),
        });
    }
    let nav = &window.navigation;
    if nav.visible
        && point.0 >= 2.0
        && point.0 < 2.0 + nav.content_width()
        && point.1 >= super::details::top()
        && point.1 < super::details::top() + nav.height
    {
        let row = match nav.hit(point) {
            Some(Hit::Row(row) | Hit::Expando(row)) => Some(row),
            _ => None,
        };
        if window.drag.folders {
            if let Some(spot) = crate::app::drop::pin_spot(
                &nav.tree,
                row,
                point.1 - super::details::top(),
                nav.metrics(),
                -nav.offset(),
            ) {
                return Some(Target {
                    folder: crate::app::ids::QUICK_ACCESS.into(),
                    item: None,
                    spot: Some(spot),
                });
            }
        }
        let row = row?;
        let node = nav.tree.node(&nav.tree.rows.get(row)?.reference)?;
        return Some(Target {
            folder: node.node.id.clone(),
            item: Some(node.node.id.clone()),
            spot: Some(Spot::Nav(row)),
        });
    }
    let view = window.scroll.view?;
    if point.0 < view.x
        || point.0 >= view.x + view.right
        || point.1 < view.y + view.header
        || point.1 >= view.y + view.bottom
    {
        return None;
    }
    let folder = window.list.folder.as_ref()?;
    let row = if let Some(model) = &folder.grid {
        model.hit((
            point.0 - view.x + window.scroll.x,
            point.1 - view.y - view.header + window.scroll.y,
        ))
    } else {
        let x = point.0 - view.x + window.scroll.x;
        (x >= 14.0 && x < view.extent_x - 13.0)
            .then(|| {
                folder
                    .layout
                    .row_at(point.1 - view.y - view.header + window.scroll.y, view.pitch)
            })
            .flatten()
    };
    if let Some((row, entry)) = row
        .and_then(|row| {
            folder
                .order
                .get(row)
                .map(|&index| (row, &folder.entries[index]))
        })
        .filter(|(_, entry)| entry.is_dir || !entry.is_fs)
    {
        return Some(Target {
            folder: window.tabs.active().location.tab.path().into(),
            item: Some(entry.path.clone()),
            spot: Some(Spot::Row(row)),
        });
    }
    let folder = window.tabs.active().location.tab.path();
    (!folder.is_empty() && !crate::app::ids::is_virtual(folder)).then(|| Target {
        folder: folder.into(),
        item: None,
        spot: None,
    })
}

pub fn prepare_list(hwnd: HWND, point: (f32, f32)) {
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return;
        };
        window.drag.candidate = None;
        let Some(folder) = &window.list.folder else {
            return;
        };
        let Some(view) = window.scroll.view else {
            return;
        };
        let interaction = &mut window.list.interaction;
        interaction.pointer = Some(point);
        let Some(row) = interaction.hit(view, &window.scroll, folder) else {
            return;
        };
        if !interaction
            .selection
            .selected
            .contains(&folder.entries[folder.order[row]].path)
        {
            return;
        }
        let paths = folder
            .order
            .iter()
            .map(|&index| &folder.entries[index].path)
            .filter(|path| interaction.selection.selected.contains(*path))
            .cloned()
            .collect();
        window.drag.candidate = Some(Candidate {
            origin: POINT {
                x: (point.0 * scale).round() as i32,
                y: (point.1 * scale).round() as i32,
            },
            paths,
            selected: Some(interaction.selection.selected.clone()),
        });
    });
}

pub fn right_pressed(hwnd: HWND, point: (f32, f32)) -> bool {
    let nav = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| matches!(window.navigation.hit(point), Some(Hit::Row(_))))
    });
    let used = if nav {
        super::navigation_input::press(hwnd, point)
    } else {
        let hit = WINDOW.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(window) = slot.as_mut() else {
                return false;
            };
            window.list.interaction.pointer = Some(point);
            window
                .list
                .folder
                .as_ref()
                .zip(window.scroll.view)
                .is_some_and(|(folder, view)| {
                    window
                        .list
                        .interaction
                        .hit(view, &window.scroll, folder)
                        .is_some()
                })
        });
        hit && super::input::press(hwnd, point)
    };
    if used {
        pressed(hwnd, point);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.drag.right = true;
            }
        });
    }
    used
}

pub fn release_right(hwnd: HWND) -> bool {
    let state = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        if !window.drag.right {
            return None;
        }
        window.drag.right = false;
        window.drag.candidate = None;
        Some(window.drag.worker.is_some())
    });
    let Some(active) = state else {
        return false;
    };
    super::input::cancel(hwnd);
    super::navigation_input::cancel(hwnd);
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    active
}

pub fn pressed(hwnd: HWND, point: (f32, f32)) {
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return;
        };
        if window.list.interaction.drag_eligible() && window.drag.candidate.is_some() {
            return;
        }
        window.drag.candidate = None;
        let paths = match window.navigation.hit(point) {
            Some(Hit::Row(row)) => window
                .navigation
                .tree
                .rows
                .get(row)
                .and_then(|row| window.navigation.tree.node(&row.reference))
                .filter(|node| node.node.is_fs || node.node.is_pinned)
                .map(|node| vec![node.node.id.clone()]),
            _ => window.list.folder.as_ref().and_then(|folder| {
                let interaction = &window.list.interaction;
                if !interaction.drag_eligible() {
                    return None;
                }
                Some(
                    folder
                        .order
                        .iter()
                        .map(|&index| &folder.entries[index].path)
                        .filter(|path| interaction.selection.selected.contains(*path))
                        .cloned()
                        .collect(),
                )
            }),
        };
        if let Some(paths) = paths.filter(|paths: &Vec<String>| !paths.is_empty()) {
            window.drag.candidate = Some(Candidate {
                origin: POINT {
                    x: (point.0 * scale).round() as i32,
                    y: (point.1 * scale).round() as i32,
                },
                paths,
                selected: None,
            });
        }
    });
}

pub fn moved(hwnd: HWND, point: (f32, f32)) -> bool {
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    let paths = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let candidate = window.drag.candidate.as_ref()?;
        let moved = unsafe {
            ((point.0 * scale).round() as i32 - candidate.origin.x).abs()
                >= GetSystemMetrics(SM_CXDRAG).max(1)
                || ((point.1 * scale).round() as i32 - candidate.origin.y).abs()
                    >= GetSystemMetrics(SM_CYDRAG).max(1)
        };
        if !moved || window.drag.worker.is_some() || crate::dnd::active_workers() != 0 {
            return None;
        }
        let candidate = window.drag.candidate.take().unwrap();
        if let Some(selected) = candidate.selected {
            window.list.interaction.selection.selected = selected;
            window.list.interaction.mark_all();
        }
        Some(candidate.paths)
    });
    let Some(paths) = paths else {
        return false;
    };
    super::rename::cancel_pending(hwnd);
    super::tooltip::suppress(hwnd);
    super::input::cancel(hwnd);
    super::input::leave(hwnd);
    super::navigation_input::cancel(hwnd);
    super::navigation_input::leave(hwnd);
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    let owner = hwnd.0 as usize;
    match crate::dnd::drag_apart(hwnd, paths, move |effect| {
        if let Err(error) = unsafe {
            PostMessageW(
                Some(HWND(owner as _)),
                FINISHED,
                WPARAM(effect.0 as usize),
                LPARAM(0),
            )
        } {
            eprintln!("drag completion: {error}");
        }
    }) {
        Ok(worker) => WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.drag.worker = Some(worker);
            }
        }),
        Err(error) => eprintln!("native start drag: {error}"),
    }
    publish(hwnd);
    true
}

pub fn active() -> bool {
    WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| !window.drag.files.is_empty() || window.drag.worker.is_some())
    })
}

pub fn released() {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.drag.candidate = None;
        }
    });
}

pub fn finished(hwnd: HWND, effect: DROPEFFECT) {
    let worker = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|window| {
            window.drag.right = false;
            window.drag.worker.take()
        })
    });
    if let Some(worker) = worker {
        if worker.join().is_err() {
            eprintln!("drag worker panicked");
        }
    }
    super::input::cancel(hwnd);
    super::input::leave(hwnd);
    super::navigation_input::leave(hwnd);
    if effect != windows::Win32::System::Ole::DROPEFFECT_NONE {
        refresh(hwnd);
    }
    publish(hwnd);
    let closing = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.drag.closing)
    });
    if closing {
        if let Err(error) = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
            super::fail(error);
        }
    }
}

pub fn request_close() -> bool {
    let active = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            if window.drag.worker.is_some() {
                window.drag.closing = true;
                true
            } else {
                false
            }
        })
    });
    if active {
        crate::dnd::cancel_drag();
    }
    active
}

fn refresh(hwnd: HWND) {
    let result = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
            window.navigation.refresh(&window.navigation_worker);
            window.list.refresh(&window.scroll)
        })
    });
    if let Err(error) = result {
        eprintln!("drop refresh: {error}");
    }
    super::invalidate_view(hwnd);
    super::navigation_input::invalidate(hwnd);
}

fn highlight(hwnd: HWND, spot: Option<Spot>) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.interaction.drop_row = match spot {
                Some(Spot::Row(row)) => Some(row),
                _ => None,
            };
            window.list.interaction.mark_all();
            window.navigation.drop_row = match spot {
                Some(Spot::Nav(row) | Spot::Pin { row: Some(row), .. }) => Some(row),
                _ => None,
            };
            window.navigation.insert_y = match spot {
                Some(Spot::Pin { line, .. }) if line >= 0 => Some(line as f32),
                _ => None,
            };
            window.navigation.changed();
            window.navigation.dirty_rows = None;
            window.address.drop_crumb = match spot {
                Some(Spot::Crumb(index)) => Some(index),
                _ => None,
            };
        }
    });
    super::input::flush(hwnd);
    super::navigation_input::invalidate(hwnd);
    super::address_input::invalidate(hwnd);
}

fn hold(hwnd: HWND, spot: Option<Spot>) {
    let (expand, scroll) = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return (false, false);
        };
        let reference = match spot {
            Some(Spot::Nav(row)) => window
                .navigation
                .tree
                .rows
                .get(row)
                .filter(|row| {
                    window
                        .navigation
                        .tree
                        .node(&row.reference)
                        .is_some_and(|node| node.node.has_children && !node.expanded)
                })
                .map(|row| row.reference.clone()),
            _ => None,
        };
        let changed =
            window.drag.expand.as_ref().map(|(reference, _)| reference) != reference.as_ref();
        if changed {
            window.drag.expand = reference.map(|reference| (reference, Instant::now()));
        }
        (
            changed && window.drag.expand.is_some(),
            scroll_step(window).is_some(),
        )
    });
    if expand {
        super::input::arm(hwnd, EXPAND, 500);
    }
    if !WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.drag.expand.is_some())
    }) {
        super::input::stop(hwnd, EXPAND);
    }
    if scroll {
        if !WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.input_timers.contains(&SCROLL))
        }) {
            WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    window.drag.scroll_repeating = false;
                }
            });
            super::input::arm(hwnd, SCROLL, 100);
        }
    } else {
        super::input::stop(hwnd, SCROLL);
    }
}

fn scroll_step(window: &super::Window) -> Option<(bool, super::scroll::Axis, f32)> {
    let (x, y) = window.drag.point?;
    let nav = &window.navigation;
    if nav.visible
        && x < nav.left()
        && y >= super::details::top()
        && y < super::details::top() + nav.height
    {
        let y = y - super::details::top();
        let step = if y < 16.0 && nav.top > 0 {
            -1.0
        } else if y > nav.height - 16.0 && nav.top < nav.maximum() {
            1.0
        } else {
            return None;
        };
        return Some((true, super::scroll::Axis::Vertical, step));
    }
    let view = window.scroll.view?;
    if x < view.x
        || x >= view.x + view.right
        || y < view.y + view.header
        || y >= view.y + view.bottom
    {
        return None;
    }
    let column_major = window
        .list
        .folder
        .as_ref()
        .and_then(|folder| folder.grid.as_ref())
        .is_some_and(|grid| grid.spec.column_major);
    if column_major {
        let step = if x < view.x + 16.0 && window.scroll.x > 0.0 {
            -view.pitch
        } else if x > view.x + view.right - 16.0
            && window.scroll.x < (view.extent_x - view.right).max(0.0)
        {
            view.pitch
        } else {
            return None;
        };
        Some((false, super::scroll::Axis::Horizontal, step))
    } else {
        let step = if y < view.y + view.header + 16.0 && window.scroll.y > 0.0 {
            -view.pitch
        } else if y > view.y + view.bottom - 16.0
            && window.scroll.y < (view.extent_y - view.rows_height).max(0.0)
        {
            view.pitch
        } else {
            return None;
        };
        Some((false, super::scroll::Axis::Vertical, step))
    }
}

fn file_tab(point: (f32, f32)) -> Option<usize> {
    WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        if window.drag.files.is_empty() {
            return None;
        }
        match window.title.hit(window.address.width, point.0, point.1) {
            super::titlebar::Target::Tab(index) | super::titlebar::Target::TabClose(index) => {
                Some(index)
            }
            _ => None,
        }
    })
}

fn hover_tab(hwnd: HWND, point: (f32, f32)) {
    let index = file_tab(point);
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        if window.drag.file_hover == index {
            return None;
        }
        window.drag.file_hover = index;
        window.title.file_gap = false;
        if let Some(index) = index {
            window.drag.file_pending = Some((index, Instant::now()));
            Some(true)
        } else {
            Some(false)
        }
    });
    let Some(arm) = changed else { return };
    if arm {
        super::input::arm(hwnd, FILE_TAB, 200);
    }
    super::invalidate_title(hwnd);
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    match id {
        EXPAND => {
            super::input::stop(hwnd, EXPAND);
            let remaining = WINDOW.with(|slot| {
                let slot = slot.borrow();
                let (_, since) = slot.as_ref()?.drag.expand.as_ref()?;
                std::time::Duration::from_millis(500).checked_sub(since.elapsed())
            });
            if let Some(remaining) = remaining {
                super::input::arm(hwnd, EXPAND, remaining.as_millis().max(1) as u32);
                return true;
            }
            WINDOW.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(window) = slot.as_mut() else {
                    return;
                };
                let Some((reference, _)) = window.drag.expand.take() else {
                    return;
                };
                let row = window
                    .navigation
                    .tree
                    .rows
                    .iter()
                    .position(|row| row.reference == reference);
                if let Some(row) = row.filter(|_| {
                    window
                        .navigation
                        .tree
                        .node(&reference)
                        .is_some_and(|node| node.node.has_children && !node.expanded)
                }) {
                    window.navigation.expand(row, &window.navigation_worker);
                }
            });
            super::navigation_input::invalidate(hwnd);
        }
        SCROLL => {
            let changed = WINDOW.with(|slot| {
                let mut slot = slot.borrow_mut();
                let window = slot.as_mut()?;
                let (nav, axis, step) = scroll_step(window)?;
                if nav {
                    window.navigation.scroll(step as isize);
                } else {
                    window.scroll.band_step(axis, step);
                }
                Some(nav)
            });
            if changed.is_some() {
                let first = WINDOW.with(|slot| {
                    slot.borrow_mut().as_mut().is_some_and(|window| {
                        !std::mem::replace(&mut window.drag.scroll_repeating, true)
                    })
                });
                if first {
                    super::input::arm(hwnd, SCROLL, 250);
                }
            }
            match changed {
                Some(true) => {
                    super::navigation_input::invalidate(hwnd);
                }
                Some(false) => {
                    super::invalidate_view(hwnd);
                }
                None => super::input::stop(hwnd, SCROLL),
            }
        }
        FILE_TAB => {
            super::input::stop(hwnd, FILE_TAB);
            let remaining = WINDOW.with(|slot| {
                let slot = slot.borrow();
                let (_, since) = slot.as_ref()?.drag.file_pending.as_ref()?;
                std::time::Duration::from_millis(200).checked_sub(since.elapsed())
            });
            if let Some(remaining) = remaining {
                super::input::arm(hwnd, FILE_TAB, remaining.as_millis().max(1) as u32);
                return true;
            }
            let index = WINDOW.with(|slot| {
                slot.borrow_mut()
                    .as_mut()
                    .and_then(|window| window.drag.file_pending.take())
            });
            if let Some((index, _)) = index {
                if let Err(error) = super::tabs::select(hwnd, index) {
                    super::fail(error);
                }
            }
            WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    window.title.file_gap =
                        !window.drag.files.is_empty() && window.drag.file_hover.is_some();
                }
            });
            super::invalidate_title(hwnd);
        }
        _ => return false,
    }
    publish(hwnd);
    true
}

pub fn picture(
    hwnd: HWND,
    token: u64,
    picture: std::result::Result<(crate::shell::icons::Rgba, crate::shell::icons::Rgba), String>,
) {
    let accepted = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        if window.drag.token != token || window.drag.files.is_empty() {
            return false;
        }
        match picture {
            Ok((crumb, tab)) => window.drag.picture = Some((Arc::new(crumb), Arc::new(tab))),
            Err(error) => eprintln!("drag picture: {error}"),
        }
        window.drag.preview_key.clear();
        true
    });
    if accepted {
        feedback(hwnd);
    }
}

fn feedback(hwnd: HWND) {
    let Some(mut window) = WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let result = (|| -> Result<()> {
        let Some((caption, effect, client)) = window.drag.caption.clone() else {
            window.drag.preview.take();
            window.drag.preview_key.clear();
            return Ok(());
        };
        let key = format!(
            "{}:{}:{}:{}",
            window.drag.token,
            caption,
            effect.0,
            window.drag.picture.is_some()
        );
        if window.drag.preview_key != key {
            window.drag.preview.take();
            let renderer = window
                .graphics
                .as_ref()
                .ok_or_else(windows::core::Error::from_win32)?
                .0
                .clone();
            window.drag.preview = Some(super::drag_visual::Popup::new(
                hwnd,
                renderer,
                &mut window.text,
                super::drag_visual::Frame::Files {
                    picture: window.drag.picture.as_ref().map(|(crumb, tab)| {
                        if caption.is_empty() {
                            tab.clone()
                        } else {
                            crumb.clone()
                        }
                    }),
                    caption,
                    effect,
                },
            )?);
            window.drag.preview_key = key;
        }
        let mut point = client;
        if !unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut point) }.as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        if let Some(popup) = &window.drag.preview {
            popup.files_at(point)?;
        }
        Ok(())
    })();
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if let Err(error) = result {
        super::fail(error);
    }
    publish(hwnd);
}

fn clear(hwnd: HWND) {
    super::input::stop(hwnd, EXPAND);
    super::input::stop(hwnd, SCROLL);
    let preview = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        window.drag.files.clear();
        window.drag.folders = false;
        window.drag.point = None;
        window.drag.expand = None;
        window.drag.file_hover = None;
        window.drag.caption = None;
        window.drag.picture = None;
        window.drag.token = window.drag.token.wrapping_add(1);
        window.title.file_gap = false;
        window.drag.preview_key.clear();
        window.drag.preview.take()
    });
    drop(preview);
    highlight(hwnd, None);
    super::invalidate_title(hwnd);
    publish(hwnd);
}

pub fn close(hwnd: HWND) {
    super::input::stop(hwnd, FILE_TAB);
    clear(hwnd);
    if WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.drag.worker.is_some())
    }) {
        crate::dnd::cancel_drag();
    }
    if let Err(error) = crate::dnd::unregister(hwnd) {
        eprintln!("unregister native drop: {error}");
    }
}

pub fn publish(hwnd: HWND) {
    let values = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        Some((
            usize::from(window.drag.worker.is_some()),
            usize::from(window.drag.preview.is_some()),
            window.list.interaction.drop_row,
        ))
    });
    if let Some((workers, backings, row)) = values {
        for (name, count) in [
            (w!("NativeDragWorkers"), workers),
            (w!("NativeDragBackingCount"), backings),
            (w!("NativeDropRow"), row.map_or(0, |row| row + 1)),
        ] {
            if let Err(error) = unsafe { SetPropW(hwnd, name, Some(HANDLE(count as _))) } {
                super::fail(error);
            }
        }
    }
}
