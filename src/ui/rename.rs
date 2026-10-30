use std::cell::{Cell, RefCell};
use windows::core::{w, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy)]
enum Layout {
    Line {
        right: i32,
    },
    Tiles {
        left: i32,
        preferred: i32,
        right: i32,
    },
    Stacked,
}

struct Editor {
    owner: HWND,
    control: super::edit::Label,
    state: super::edit::State,
    serial: u64,
    path: String,
    paths: Vec<String>,
    original: String,
    multiline: bool,
    width: i32,
    layout: Layout,
    bounds: (i32, i32, i32, i32),
    origin: (f32, f32),
    image: Option<std::sync::Arc<crate::shell::icons::Rgba>>,
}
thread_local! {
    static EDITOR: RefCell<Option<Editor>> = const { RefCell::new(None) };
    static SERIAL: Cell<u64> = const { Cell::new(0) };
}

pub fn is_control(hwnd: HWND) -> bool {
    EDITOR.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|editor| editor.control.hwnd == hwnd)
    })
}

pub fn active(path: &str) -> bool {
    EDITOR.with(|slot| {
        slot.borrow().as_ref().is_some_and(|editor| {
            editor.owner == super::context::hwnd() && crate::app::ids::same(&editor.path, path)
        })
    })
}

pub fn selected() -> Option<usize> {
    EDITOR.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|editor| editor.owner == super::context::hwnd())
            .map(|editor| editor.paths.len())
    })
}

pub fn take_history(key: u32) -> Option<super::edit::Record> {
    EDITOR.with(|slot| slot.borrow_mut().as_mut()?.state.take_history(key))
}

pub fn can_undo() -> bool {
    EDITOR.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|editor| editor.state.can_undo())
    })
}

pub fn begin(owner: HWND, path: &str) -> Result<()> {
    cancel_pending(owner);
    finish(EditEnd::Commit);
    super::input::cancel(owner);
    super::tooltip::suppress(owner);
    super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return;
        };
        let Some(folder) = window.list.folder.as_ref() else {
            return;
        };
        let Some(row) = folder
            .order
            .iter()
            .position(|&index| crate::app::ids::same(&folder.entries[index].path, path))
        else {
            return;
        };
        if let Some(view) = window.scroll.view {
            window
                .list
                .interaction
                .reveal(row, folder, view, &mut window.scroll);
        }
    });
    let geometry = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let folder = window.list.folder.as_ref()?;
        let row = folder
            .order
            .iter()
            .position(|&index| crate::app::ids::same(&folder.entries[index].path, path))?;
        let entry = &folder.entries[folder.order[row]];
        if !entry.can_rename {
            return None;
        }
        let view = window.scroll.view?;
        let natural = crate::shell::columns::widths([entry.name.as_str()])[0] + 37.0;
        let (x, y, width, multiline) = if let Some(grid) = &folder.grid {
            let cell = grid.layout.cells[row];
            let left = view.x + cell[0] - window.scroll.x;
            let top = view.y + view.header + cell[1] - window.scroll.y;
            match grid.spec.kind {
                0 => (left, top + grid.spec.icon, cell[2], true),
                2 => {
                    let width = natural.min(cell[2]);
                    (
                        (left + 52.0 + grid.spec.label_indent).min(left + cell[2] - width),
                        top,
                        width,
                        true,
                    )
                }
                _ => {
                    let x = left + grid.spec.icon + 30.0 + grid.spec.label_indent;
                    (
                        x,
                        top + if grid.spec.kind == 3 { 7.0 } else { 0.0 },
                        natural.min(view.x + view.right - x - 3.0),
                        false,
                    )
                }
            }
        } else {
            let index = folder
                .columns
                .iter()
                .position(|column| column.key == PKEY_ItemNameDisplay)?;
            let preceding = folder.columns[..index]
                .iter()
                .map(|column| column.width)
                .sum::<f32>();
            let x = view.x + preceding - window.scroll.x
                + if index == 0 {
                    34.0 + if folder.settings.check_boxes {
                        15.0
                    } else {
                        0.0
                    }
                } else {
                    2.0
                };
            (
                x,
                view.y + view.header + view.row_top(folder, row) - window.scroll.y
                    + if folder.settings.compact { 1.0 } else { 3.0 },
                natural.min(view.x + view.right - x - 3.0),
                false,
            )
        };
        let width = width.max(1.0);
        let paths = folder
            .order
            .iter()
            .map(|&index| &folder.entries[index].path)
            .filter(|path| window.list.interaction.selection.selected.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        let parent = folder.crumbs.last()?.id.clone();
        let parent = if crate::app::ids::is_search(&parent) {
            std::path::Path::new(&entry.path)
                .parent()?
                .to_str()?
                .to_owned()
        } else {
            parent
        };
        let layout = folder.grid.as_ref().map_or(
            Layout::Line {
                right: ((view.x + view.right - 3.0)
                    * unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(owner) } as f32
                    / 96.0)
                    .round() as i32,
            },
            |grid| {
                let scale =
                    unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(owner) } as f32 / 96.0;
                let cell = grid.layout.cells[row];
                let left = view.x + cell[0] - window.scroll.x;
                match grid.spec.kind {
                    0 => Layout::Stacked,
                    2 => Layout::Tiles {
                        left: (left * scale).round() as i32,
                        preferred: ((left + 52.0 + grid.spec.label_indent) * scale).round() as i32,
                        right: ((left + cell[2]) * scale).round() as i32,
                    },
                    _ => Layout::Line {
                        right: ((view.x + view.right - 3.0) * scale).round() as i32,
                    },
                }
            },
        );
        Some((
            entry.name.clone(),
            entry.is_dir,
            parent,
            paths,
            (x, y, width),
            multiline,
            layout,
            (
                x - view.x + window.scroll.x,
                y - view.y - view.header + window.scroll.y,
            ),
        ))
    });
    let Some((original, directory, parent, paths, (x, y, width), multiline, layout, origin)) =
        geometry
    else {
        return Ok(());
    };
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(owner) } as f32 / 96.0;
    let lines = if multiline {
        crate::shell::text::icon_label(&original, (width - 6.0).max(1.0) as i32)
            .full
            .lines()
            .count()
            .max(1)
    } else {
        1
    };
    let bounds = (
        (x * scale).round() as i32,
        (y * scale).round() as i32,
        (width * scale).round() as i32,
        ((lines as f32 * 15.0 + 4.0) * scale).round() as i32,
    );
    let control = super::edit::Label::new(
        owner,
        (bounds.0 + 1, bounds.1 + 2, bounds.2, bounds.3),
        if multiline {
            super::edit::LabelLayout::Paragraph
        } else {
            super::edit::LabelLayout::Line
        },
        &parent,
    )?;
    let hwnd = control.hwnd;
    let end = if directory {
        original.encode_utf16().count()
    } else {
        original
            .rfind('.')
            .filter(|&dot| dot != 0)
            .map_or(original.encode_utf16().count(), |dot| {
                original[..dot].encode_utf16().count()
            })
    };
    EDITOR.with(|slot| {
        *slot.borrow_mut() = Some(Editor {
            owner,
            control,
            state: Default::default(),
            serial: 0,
            path: path.to_owned(),
            paths,
            original: original.clone(),
            multiline,
            width: bounds.2,
            layout,
            bounds,
            origin,
            image: None,
        })
    });
    if let Err(error) = super::edit::set_text(hwnd, &original, (0, end as u32)) {
        finish(EditEnd::Cancel);
        return Err(error);
    }
    unsafe {
        if let Err(error) = SetPropW(owner, w!("NativeRenameControl"), Some(HANDLE(hwnd.0))) {
            finish(EditEnd::Cancel);
            return Err(error);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(hwnd));
    }
    let mut snapshot = super::edit::snapshot(hwnd);
    snapshot.kind = super::edit::Kind::Programmatic;
    changed(snapshot);
    invalidate(owner);
    Ok(())
}

pub fn focused(owner: HWND) {
    let path = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let folder = window.list.folder.as_ref()?;
        let selection = &window.list.interaction.selection;
        selection
            .focused
            .as_ref()
            .filter(|path| selection.selected.contains(*path))
            .cloned()
            .or_else(|| {
                folder
                    .order
                    .iter()
                    .map(|&index| &folder.entries[index].path)
                    .find(|path| selection.selected.contains(*path))
                    .cloned()
            })
    });
    if let Some(path) = path {
        if let Err(error) = begin(owner, &path) {
            eprintln!("native rename: {error}");
        }
    }
}

pub fn changed(snapshot: super::edit::Snapshot) {
    let control = EDITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let editor = slot.as_mut()?;
        editor.state.update(snapshot);
        editor.serial = SERIAL.with(|serial| {
            let value = serial.get().wrapping_add(1);
            serial.set(value);
            value
        });
        let scale =
            unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(editor.owner) } as f32 / 96.0;
        let natural = (crate::shell::columns::widths_at(
            [editor.state.text.as_str()],
            (12.0 * scale).round() as i32,
        )[0] + 37.0 * scale)
            .ceil() as i32;
        let (left, width) = match editor.layout {
            Layout::Stacked => (editor.bounds.0, editor.width),
            Layout::Line { right } => {
                (editor.bounds.0, natural.min(right - editor.bounds.0).max(1))
            }
            Layout::Tiles {
                left,
                preferred,
                right,
            } => {
                let width = natural.min(right - left).max(1);
                (preferred.min(right - width), width)
            }
        };
        editor.origin.0 += (left - editor.bounds.0) as f32 / scale;
        editor.bounds.0 = left;
        editor.bounds.2 = width;
        editor.width = width;
        Some((
            editor.owner,
            editor.control.hwnd,
            editor.bounds,
            editor.multiline,
        ))
    });
    let Some((owner, hwnd, bounds, multiline)) = control else {
        return;
    };
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    if let Err(error) = unsafe {
        SetWindowPos(
            hwnd,
            None,
            bounds.0 + 1,
            bounds.1 + 2,
            bounds.2,
            bounds.3,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } {
        eprintln!("rename width: {error}");
    }
    let lines = if multiline {
        unsafe {
            SendMessageW(
                hwnd,
                windows::Win32::UI::Controls::EM_GETLINECOUNT,
                None,
                None,
            )
            .0
            .max(1)
        }
    } else {
        1
    };
    let height = ((lines as f32 * 15.0 + 4.0) * scale).round() as i32;
    if height != bounds.3 {
        if let Err(error) = unsafe {
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                bounds.2,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        } {
            eprintln!("rename height: {error}");
        }
    }
    unsafe {
        SendMessageW(
            hwnd,
            windows::Win32::UI::Controls::EM_SCROLLCARET,
            None,
            None,
        );
    }
    EDITOR.with(|slot| {
        if let Some(editor) = slot.borrow_mut().as_mut() {
            editor.bounds.3 = height;
        }
    });
    let result = super::edit::raster_label(hwnd, bounds.2, height);
    match result {
        Ok(image) => EDITOR.with(|slot| {
            if let Some(editor) = slot.borrow_mut().as_mut() {
                editor.image = Some(std::sync::Arc::new(image));
            }
        }),
        Err(error) => eprintln!("rename raster: {error}"),
    }
    invalidate(owner);
}

pub fn image() -> Option<((f32, f32), u64, std::sync::Arc<crate::shell::icons::Rgba>)> {
    EDITOR.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|editor| editor.owner == super::context::hwnd())
            .and_then(|editor| Some((editor.origin, editor.serial, editor.image.clone()?)))
    })
}

pub fn caret() -> Option<(
    u64,
    windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    crate::shell::icons::Rgba,
)> {
    let data = EDITOR.with(|slot| {
        let slot = slot.borrow();
        let editor = slot.as_ref()?;
        Some((
            editor.owner,
            editor.control.hwnd,
            editor.serial,
            editor.bounds,
            editor.image.clone()?,
        ))
    })?;
    let (owner, control, revision, bounds, image) = data;
    unsafe {
        if windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() != control
            || GetForegroundWindow() != owner
        {
            return None;
        }
        let mut point = windows::Win32::Foundation::POINT::default();
        if GetCaretPos(&mut point).is_err() {
            return None;
        }
        let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(owner) as f32 / 96.0;
        let x = (point.x + 1).clamp(0, image.width.saturating_sub(1) as i32);
        let y = point.y + 2;
        let height = (15.0 * scale).round().max(1.0) as u32;
        let mut pixels = Vec::with_capacity(height as usize * 4);
        for offset in 0..height {
            let row = (y + offset as i32).clamp(0, image.height.saturating_sub(1) as i32);
            let index = (row as usize * image.width as usize + x as usize) * 4;
            pixels.extend_from_slice(&[
                255 - image.pixels[index],
                255 - image.pixels[index + 1],
                255 - image.pixels[index + 2],
                255,
            ]);
        }
        let rect = super::address::rect(
            (bounds.0 + x) as f32 / scale,
            (bounds.1 + y) as f32 / scale - 40.0,
            1.0 / scale,
            height as f32 / scale,
        );
        Some((
            revision,
            rect,
            crate::shell::icons::Rgba {
                width: 1,
                height,
                pixels,
            },
        ))
    }
}

fn invalidate(owner: HWND) {
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.interaction.mark_all();
        }
    });
    super::input::flush(owner);
    super::invalidate_view(owner);
}

enum EditEnd {
    Commit,
    Blur,
    Cancel,
}

fn finish(end: EditEnd) {
    let editor = EDITOR.with(|slot| slot.borrow_mut().take());
    let Some(editor) = editor else {
        return;
    };
    let _context = super::context::enter(editor.owner);
    if matches!(end, EditEnd::Commit | EditEnd::Blur) {
        let snapshot = super::edit::snapshot(editor.control.hwnd);
        let value = snapshot.text.trim();
        if !value.is_empty() && (value != editor.original || editor.paths.len() > 1) {
            let result = if editor.paths.len() > 1
                && editor
                    .paths
                    .iter()
                    .any(|path| crate::app::ids::same(path, &editor.path))
            {
                crate::app::rename::many(editor.owner, &editor.path, &editor.paths, value)
            } else {
                match crate::shell::ops::rename(editor.owner, &editor.path, value) {
                    Ok(next) => crate::app::rename::Batch {
                        done: vec![(editor.path.clone(), next)],
                        errors: Vec::new(),
                    },
                    Err(error) => crate::app::rename::Batch {
                        done: Vec::new(),
                        errors: vec![error],
                    },
                }
            };
            for error in &result.errors {
                eprintln!("native rename: {error}");
            }
            if result.done.is_empty() && !result.errors.is_empty() && matches!(end, EditEnd::Commit)
            {
                let hwnd = editor.control.hwnd;
                EDITOR.with(|slot| *slot.borrow_mut() = Some(editor));
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
                return;
            }
            if !result.done.is_empty() {
                let paths = result.done.iter().map(|(_, path)| path.clone()).collect();
                super::menu_actions::push(
                    editor.owner,
                    crate::app::undo::Undo::Rename(result.done),
                );
                let result = super::WINDOW.with(|slot| {
                    slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                        window.list.refresh(&window.scroll)?;
                        window.list.select_created(paths, &mut window.scroll)
                    })
                });
                if let Err(error) = result {
                    eprintln!("native rename refresh: {error}");
                }
            }
        }
    }
    let owner = editor.owner;
    drop(editor);
    unsafe {
        if let Err(error) = RemovePropW(owner, w!("NativeRenameControl")) {
            eprintln!("native rename state: {error}");
        }
        if !matches!(end, EditEnd::Blur) && GetForegroundWindow() == owner {
            let _ = SetFocus(Some(owner));
        }
    }
    invalidate(owner);
}

struct Click {
    owner: HWND,
    point: (f32, f32),
    path: String,
}
thread_local! { static CLICK: RefCell<Option<Click>> = const { RefCell::new(None) }; }
const DELAY: usize = 52;

pub fn cancel_pending(owner: HWND) {
    CLICK.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|click| click.owner == owner)
        {
            slot.borrow_mut().take();
        }
    });
    super::input::stop(owner, DELAY);
}

pub fn pressed(owner: HWND, point: (f32, f32)) {
    cancel_pending(owner);
    let mods = super::input::modifiers();
    if mods.ctrl || mods.shift {
        return;
    }
    let path = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let folder = window.list.folder.as_ref()?;
        if folder.settings.single_click || window.list.interaction.selection.selected.len() != 1 {
            return None;
        }
        window.list.interaction.pointer = Some(point);
        let view = window.scroll.view?;
        let row = window.list.interaction.hit(view, &window.scroll, folder)?;
        let entry = &folder.entries[folder.order[row]];
        if !window
            .list
            .interaction
            .selection
            .selected
            .contains(&entry.path)
        {
            return None;
        }
        let local = (
            point.0 - view.x + window.scroll.x,
            point.1 - view.y - view.header + window.scroll.y,
        );
        let (left, top, right, bottom) = if let Some(grid) = &folder.grid {
            let cell = grid.layout.cells[row];
            match grid.spec.kind {
                0 => (
                    cell[0],
                    cell[1] + grid.spec.icon,
                    cell[0] + cell[2],
                    cell[1] + cell[3],
                ),
                2 => (
                    cell[0] + 57.0 + grid.spec.label_indent,
                    cell[1],
                    cell[0] + cell[2],
                    cell[1] + 32.0,
                ),
                _ => (
                    cell[0] + grid.spec.icon + 34.0 + grid.spec.label_indent,
                    cell[1],
                    cell[0] + cell[2],
                    cell[1] + cell[3],
                ),
            }
        } else {
            let index = folder
                .columns
                .iter()
                .position(|column| column.key == PKEY_ItemNameDisplay)?;
            let start = folder.columns[..index]
                .iter()
                .map(|column| column.width)
                .sum::<f32>();
            let left = start
                + if index == 0 {
                    38.0 + if folder.settings.check_boxes {
                        15.0
                    } else {
                        0.0
                    }
                } else {
                    6.0
                };
            (
                left,
                view.row_top(folder, row),
                (left + crate::shell::columns::widths([entry.name.as_str()])[0])
                    .min(start + folder.columns[index].width - 6.0),
                view.row_top(folder, row) + view.plate,
            )
        };
        (local.0 >= left && local.0 < right && local.1 >= top && local.1 < bottom)
            .then(|| entry.path.clone())
    });
    CLICK.with(|slot| *slot.borrow_mut() = path.map(|path| Click { owner, point, path }));
}

pub fn moved(owner: HWND, point: (f32, f32)) {
    let cancel = CLICK.with(|slot| {
        slot.borrow().as_ref().is_some_and(|click| {
            click.owner == owner
                && ((click.point.0 - point.0).abs() > 4.0 || (click.point.1 - point.1).abs() > 4.0)
        })
    });
    if cancel {
        cancel_pending(owner);
    }
}

pub fn released(owner: HWND) {
    let ready = CLICK.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|click| click.owner == owner)
    });
    if ready {
        super::input::arm(owner, DELAY, unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime()
        });
    }
}

pub fn blur() {
    if selected().is_some() {
        finish(EditEnd::Blur);
    }
}

pub fn owner_message(owner: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if matches!(
        message,
        WM_LBUTTONDOWN
            | WM_LBUTTONDBLCLK
            | WM_NCLBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_KEYDOWN
            | WM_SYSKEYDOWN
            | WM_MOUSEWHEEL
            | WM_CANCELMODE
            | WM_DESTROY
    ) {
        cancel_pending(owner);
    }
    if message == WM_TIMER && wparam.0 == DELAY {
        super::input::stop(owner, DELAY);
        let click = CLICK.with(|slot| slot.borrow_mut().take());
        if let Some(click) = click {
            let valid = super::WINDOW.with(|slot| {
                slot.borrow().as_ref().is_some_and(|window| {
                    window.list.interaction.selection.selected.len() == 1
                        && window
                            .list
                            .interaction
                            .selection
                            .selected
                            .contains(&click.path)
                })
            });
            if valid {
                if let Err(error) = begin(owner, &click.path) {
                    eprintln!("native delayed rename: {error}");
                }
            }
        }
        return Some(LRESULT(0));
    }
    let data = EDITOR.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|editor| editor.owner == owner)
            .map(|editor| {
                (
                    editor.control.hwnd,
                    editor.bounds,
                    editor
                        .image
                        .as_ref()
                        .map_or(editor.bounds.3, |image| image.height as i32),
                )
            })
    });
    let Some((control, bounds, height)) = data else {
        return None;
    };
    if matches!(
        message,
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONUP
    ) {
        let point = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
        if point.0 >= bounds.0
            && point.0 < bounds.0 + bounds.2
            && point.1 >= bounds.1
            && point.1 < bounds.1 + height
        {
            let local = LPARAM(
                (((point.1 - bounds.1 - 2) as u32) << 16 | (point.0 - bounds.0 - 1) as u32 & 0xffff)
                    as isize,
            );
            return Some(unsafe { SendMessageW(control, message, Some(wparam), Some(local)) });
        }
        if message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN {
            finish(EditEnd::Blur);
            return None;
        }
    }
    if message == WM_MOUSEWHEEL
        || message == WM_NCLBUTTONDOWN
        || message == WM_ACTIVATE && wparam.0 as u32 & 0xffff == WA_INACTIVE
    {
        finish(EditEnd::Blur);
        return None;
    }
    if message == WM_SETCURSOR {
        let mut point = windows::Win32::Foundation::POINT::default();
        unsafe {
            if GetCursorPos(&mut point).is_ok()
                && ScreenToClient(owner, &mut point).as_bool()
                && point.x >= bounds.0
                && point.x < bounds.0 + bounds.2
                && point.y >= bounds.1
                && point.y < bounds.1 + height
            {
                if let Ok(cursor) = LoadCursorW(None, IDC_IBEAM) {
                    SetCursor(Some(cursor));
                    return Some(LRESULT(1));
                }
            }
        }
    }
    if message == WM_DESTROY {
        finish(EditEnd::Cancel);
    }
    if message == WM_CTLCOLOREDIT && lparam.0 == control.0 as isize {
        let dark = crate::shell::theme::dark();
        let background = if dark { 0x212121 } else { 0xffffff };
        let foreground = if dark { 0xffffff } else { 0 };
        unsafe {
            let dc = HDC(wparam.0 as _);
            SetTextColor(dc, windows::Win32::Foundation::COLORREF(foreground));
            SetBkColor(dc, windows::Win32::Foundation::COLORREF(background));
            SetDCBrushColor(dc, windows::Win32::Foundation::COLORREF(background));
            return Some(LRESULT(GetStockObject(DC_BRUSH).0 as isize));
        }
    }
    None
}

pub fn edit_message(hwnd: HWND, message: u32, wparam: WPARAM, _lparam: LPARAM) -> Option<LRESULT> {
    if !is_control(hwnd) {
        return None;
    }
    if message == WM_KEYDOWN && !super::edit::composing(hwnd) {
        match wparam.0 {
            0x0d => {
                finish(EditEnd::Commit);
                return Some(LRESULT(0));
            }
            0x1b => {
                finish(EditEnd::Cancel);
                return Some(LRESULT(0));
            }
            _ => {}
        }
    }
    if message == WM_KILLFOCUS {
        finish(EditEnd::Blur);
        return Some(LRESULT(0));
    }
    None
}
