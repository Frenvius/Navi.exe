use super::{
    address::{rect, rounded},
    render::{Layer, Renderer},
    WINDOW,
};
use crate::app::{filter::Filter, sort::SortKey};
use std::{cell::RefCell, rc::Rc};
use windows::{
    core::{w, Result},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::{
            Direct2D::{Common::*, *},
            DirectComposition::IDCompositionTarget,
            Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED,
        },
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, SetFocus},
            WindowsAndMessaging::*,
        },
    },
};

#[derive(Clone)]
struct Waiting {
    owner: HWND,
    token: u64,
    path: String,
    key: SortKey,
    column: usize,
    anchor: (f32, f32),
}
struct Popup {
    request: Waiting,
    hwnd: HWND,
    renderer: Rc<Renderer>,
    layer: Layer,
    target: IDCompositionTarget,
    icon: Rc<ID2D1Bitmap1>,
    labels: Vec<String>,
    hot: usize,
    pressed: Option<(usize, Part)>,
    at: (i32, i32),
    size: (f32, f32),
    framed: bool,
}
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Check,
    Label,
}
thread_local! {
    static WAITING:RefCell<Option<Waiting>>=const {RefCell::new(None)};
    static POPUP:RefCell<Option<Popup>>=const {RefCell::new(None)};
    static TOKEN:std::cell::Cell<u64>=const {std::cell::Cell::new(0)};
}
fn error(message: String) -> windows::core::Error {
    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, message)
}

pub fn open(owner: HWND, column: usize, anchor: (f32, f32)) {
    close(owner);
    super::menu::close_owner(owner);
    super::tooltip::suppress(owner);
    let token = TOKEN.with(|token| {
        let next = token.get().wrapping_add(1);
        token.set(next);
        next
    });
    let request = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let folder = window.list.folder.as_ref()?;
        let key = SortKey::from_property(&folder.columns.get(column)?.key);
        Some(Waiting {
            owner,
            token,
            path: window.tabs.active().location.tab.path().to_owned(),
            key,
            column,
            anchor,
        })
    });
    let Some(request) = request else {
        return;
    };
    let result = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(Ok(()), |window| window.list.filter_menu(token, request.key))
    });
    if let Err(error) = result {
        eprintln!("native header filter: {error}");
        return;
    }
    WAITING.with(|slot| *slot.borrow_mut() = Some(request));
}

pub fn ready(owner: HWND) {
    let data = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|window| window.list.filter_menu.take())
    });
    let Some((token, key, labels)) = data else {
        return;
    };
    let waiting = WAITING.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|request| {
                request.owner == owner && request.token == token && request.key == key
            })
            .cloned()
    });
    let Some(request) = waiting else {
        return;
    };
    let current = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.tabs.active().location.tab.path() == request.path)
    });
    if !current {
        return;
    }
    WAITING.with(|slot| slot.borrow_mut().take());
    match create(request, labels) {
        Ok(popup) => {
            POPUP.with(|slot| *slot.borrow_mut() = Some(popup));
            super::chrome::set_focus(owner, super::chrome::Focus::List);
            unsafe {
                let _ = SetFocus(Some(owner));
                SetCapture(owner);
            }
            if let Err(error) = paint() {
                eprintln!("native header filter paint: {error}");
                close(owner);
            }
            super::invalidate_all(owner);
        }
        Err(error) => eprintln!("native header filter create: {error}"),
    }
}

fn create(request: Waiting, labels: Vec<String>) -> Result<Popup> {
    let renderer = super::context::renderer()?;
    let width = labels
        .iter()
        .map(|label| crate::shell::text::ui_width(label))
        .fold(0.0, f32::max)
        + 136.4;
    let size = (width.ceil(), labels.len() as f32 * 21.0 + 6.0);
    let scale = unsafe { GetDpiForWindow(request.owner) } as f32 / 96.0;
    let desired = crate::frame::client_to_screen(
        request.owner,
        (request.anchor.0 * scale).round() as i32,
        (request.anchor.1 * scale).round() as i32,
    );
    let work = crate::frame::work_area(desired.0, desired.1);
    let at = (
        desired
            .0
            .min(work.right - ((size.0 + 4.0) * scale).ceil() as i32)
            .max(work.left),
        desired
            .1
            .min(work.bottom - ((size.1 + 4.0) * scale).ceil() as i32)
            .max(work.top),
    );
    unsafe {
        static REGISTER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*REGISTER.get_or_init(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: w!("SwHeaderFilter"),
                ..Default::default()
            }) != 0
        }) {
            return Err(windows::core::Error::from_win32());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            w!("SwHeaderFilter"),
            w!(""),
            WS_POPUP,
            at.0,
            at.1,
            ((size.0 + 4.0) * scale).ceil() as i32,
            ((size.1 + 4.0) * scale).ceil() as i32,
            Some(request.owner),
            None,
            None,
            None,
        )?;
        let made = (|| {
            use windows::Win32::Graphics::Dwm::*;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &1u32 as *const _ as _,
                4,
            )?;
            let mut layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            layer.resize((
                ((size.0 + 4.0) * scale).ceil() as u32,
                ((size.1 + 4.0) * scale).ceil() as u32,
            ))?;
            let target = renderer.composition.CreateTargetForHwnd(hwnd, true)?;
            target.SetRoot(&layer.visual)?;
            let image = super::icons::small(crate::shell::icons::stack_icon()).map_err(error)?;
            let icon = renderer.canvas.shared_icon(&image)?;
            Ok(Popup {
                request,
                hwnd,
                renderer,
                layer,
                target,
                icon,
                labels,
                hot: 0,
                pressed: None,
                at,
                size,
                framed: true,
            })
        })();
        if made.is_err() {
            DestroyWindow(hwnd)?;
        }
        made
    }
}

fn paint() -> Result<()> {
    let popup = POPUP.with(|slot| slot.borrow_mut().take());
    let Some(mut popup) = popup else {
        return Ok(());
    };
    let checked = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.tabs.active().location.tab.filter())
    });
    let renderer = popup.renderer.clone();
    let result = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return Ok(());
        };
        let size = popup.size;
        let hot = popup.hot;
        let labels = &popup.labels;
        let icon = &popup.icon;
        let theme = super::theme::Theme::system();
        renderer.update(
            &mut popup.layer,
            None,
            unsafe { GetDpiForWindow(popup.hwnd) },
            &D2D1_COLOR_F::default(),
            |canvas| {
                for (index, alpha) in [0.44, 0.32, 0.16, 0.04].into_iter().enumerate() {
                    let color = D2D1_COLOR_F {
                        a: alpha,
                        ..Default::default()
                    };
                    super::details::fill(
                        canvas,
                        rect(
                            size.0 + index as f32,
                            8.0,
                            1.0,
                            (size.1 - 8.0 + index as f32).max(0.0),
                        ),
                        &color,
                    );
                    super::details::fill(
                        canvas,
                        rect(
                            8.0,
                            size.1 + index as f32,
                            (size.0 - 8.0 + index as f32).max(0.0),
                            1.0,
                        ),
                        &color,
                    );
                }
                super::details::fill(canvas, rect(0.0, 0.0, size.0, size.1), &theme.filter_border);
                super::details::fill(
                    canvas,
                    rect(1.0, 1.0, size.0 - 2.0, size.1 - 2.0),
                    &theme.filter_body,
                );
                for (index, label) in labels.iter().enumerate() {
                    let y = 3.0 + index as f32 * 21.0;
                    if index == hot {
                        if popup.framed {
                            rounded(
                                canvas,
                                rect(33.0, y, size.0 - 36.0, 21.0),
                                2.0,
                                &theme.filter_hot_border,
                            );
                            super::details::fill(
                                canvas,
                                rect(34.0, y + 1.0, size.0 - 38.0, 19.0),
                                &theme.filter_hot,
                            );
                        } else {
                            rounded(
                                canvas,
                                rect(33.0, y, size.0 - 36.0, 21.0),
                                2.0,
                                &theme.filter_selected,
                            );
                        }
                    }
                    let on = checked.as_ref().is_some_and(|filter| {
                        filter.key == popup.request.key && filter.labels.contains(label)
                    });
                    rounded(
                        canvas,
                        rect(9.0, y + 4.0, 13.0, 13.0),
                        3.0,
                        if on {
                            &theme.filter_checked
                        } else {
                            &theme.filter_check_border
                        },
                    );
                    if on {
                        unsafe {
                            canvas.brush.SetColor(&theme.filter_tick);
                            let mut tick = D2D1_TRIANGLE::default();
                            tick.point1.X = 12.5;
                            tick.point1.Y = y + 10.6;
                            tick.point2.X = 14.6;
                            tick.point2.Y = y + 12.7;
                            tick.point3.X = 19.0;
                            tick.point3.Y = y + 8.6;
                            canvas.context.DrawLine(
                                tick.point1,
                                tick.point2,
                                &canvas.brush,
                                0.75,
                                None,
                            );
                            canvas.context.DrawLine(
                                tick.point2,
                                tick.point3,
                                &canvas.brush,
                                0.75,
                                None,
                            );
                        }
                    } else {
                        rounded(
                            canvas,
                            rect(10.0, y + 5.0, 11.0, 11.0),
                            2.0,
                            &theme.filter_check,
                        );
                    }
                    unsafe {
                        canvas.context.DrawBitmap(
                            icon.as_ref(),
                            Some(&rect(39.0, y + 2.0, 16.0, 16.0)),
                            1.0,
                            D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                            None,
                            None,
                        );
                    }
                    super::details::draw_line(
                        canvas,
                        &mut window.text,
                        label,
                        super::text::DIRECTUI,
                        rect(61.0, y, size.0 - 65.0, 21.0),
                        0.0,
                        &theme.filter_text,
                    )?;
                }
                Ok(())
            },
        )
    });
    let result = result.and_then(|()| unsafe {
        let _ = ShowWindow(popup.hwnd, SW_SHOWNOACTIVATE);
        renderer.composition.Commit()
    });
    POPUP.with(|slot| *slot.borrow_mut() = Some(popup));
    result
}

pub fn close(owner: HWND) {
    WAITING.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|request| request.owner == owner)
        {
            slot.borrow_mut().take();
        }
    });
    let popup = POPUP.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|popup| popup.request.owner == owner)
        {
            slot.borrow_mut().take()
        } else {
            None
        }
    });
    if let Some(popup) = popup {
        unsafe {
            if GetCapture() == owner {
                if let Err(error) = ReleaseCapture() {
                    eprintln!("native filter capture cleanup: {error}");
                }
            }
            if let Err(error) = popup.target.SetRoot(None) {
                eprintln!("native filter target cleanup: {error}");
            }
            if let Err(error) = popup.layer.surface.Trim(Some(&[])) {
                eprintln!("native filter backing cleanup: {error}");
            }
            if let Err(error) = DestroyWindow(popup.hwnd) {
                eprintln!("native filter window cleanup: {error}");
            }
            if let Err(error) = popup.renderer.composition.Commit() {
                eprintln!("native filter composition cleanup: {error}");
            }
        }
        super::invalidate_all(owner);
    }
}

pub fn framed_column() -> Option<usize> {
    POPUP.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|popup| popup.request.owner == super::context::hwnd() && popup.framed)
            .map(|popup| popup.request.column)
    })
}

fn apply(owner: HWND, key: SortKey, label: String, part: Part) -> Result<()> {
    let next = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        Some(match part {
            Part::Check => Filter::toggle(
                window.tabs.active().location.tab.filter().as_ref(),
                key,
                &label,
            ),
            Part::Label => Some(Filter {
                key,
                labels: vec![label],
            }),
        })
    });
    let Some(next) = next else {
        return Ok(());
    };
    let window = WINDOW.with(|slot| slot.borrow_mut().take());
    let Some(mut window) = window else {
        return Ok(());
    };
    let result = (|| {
        if window.tabs.active().location.tab.filter() == next {
            return Ok(());
        }
        super::tabs::remember(&mut window)?;
        let location = &mut window.tabs.active_mut().location;
        location.tab.push(location.tab.path().to_owned(), next);
        location.visits.truncate(location.tab.index);
        location.visits.push(Default::default());
        location.scroll = (0.0, 0.0);
        super::tabs::show(&mut window, owner)?;
        super::tabs::sync(&mut window, owner)
    })();
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    super::invalidate_all(owner);
    result
}

pub fn owner_message(owner: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let ours = POPUP.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|popup| popup.request.owner == owner)
    });
    if !ours {
        return None;
    }
    if matches!(message, WM_CLOSE | WM_DESTROY | WM_CANCELMODE)
        || message == WM_ACTIVATE && wparam.0 as u32 & 0xffff == WA_INACTIVE
    {
        close(owner);
        return None;
    }
    if message == WM_CAPTURECHANGED && HWND(lparam.0 as _) != owner {
        close(owner);
        return None;
    }
    let mut action = None;
    let mut dismiss = false;
    let mut changed = false;
    let handled = POPUP.with(|slot| {
        let mut slot = slot.borrow_mut();
        let popup = slot.as_mut()?;
        if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            match wparam.0 as u32 {
                0x1b | 0x0d => dismiss = true,
                0x26 | 0x28 if !popup.labels.is_empty() => {
                    let count = popup.labels.len();
                    popup.hot = (popup.hot + if wparam.0 == 0x26 { count - 1 } else { 1 }) % count;
                    changed = true;
                }
                0x20 => {
                    if let Some(label) = popup.labels.get(popup.hot) {
                        action = Some((popup.request.key, label.clone(), Part::Check));
                        popup.framed = false;
                    }
                }
                _ => {}
            }
            return Some(LRESULT(0));
        }
        if !matches!(
            message,
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP
        ) {
            return None;
        }
        let scale = unsafe { GetDpiForWindow(owner) } as f32 / 96.0;
        let mut point = POINT {
            x: lparam.0 as i16 as i32,
            y: (lparam.0 >> 16) as i16 as i32,
        };
        if !unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(owner, &mut point) }.as_bool() {
            eprintln!("native filter pointer conversion failed");
            return Some(LRESULT(0));
        }
        let local = (
            (point.x - popup.at.0) as f32 / scale,
            (point.y - popup.at.1) as f32 / scale,
        );
        let hit = (local.0 >= 0.0
            && local.0 < popup.size.0
            && local.1 >= 3.0
            && local.1 < popup.size.1 - 3.0)
            .then(|| {
                (
                    ((local.1 - 3.0) / 21.0).floor() as usize,
                    if local.0 < 33.0 {
                        Part::Check
                    } else {
                        Part::Label
                    },
                )
            });
        match message {
            WM_MOUSEMOVE => {
                if let Some((row, _)) = hit {
                    if row != popup.hot {
                        popup.hot = row;
                        changed = true;
                    }
                }
            }
            WM_LBUTTONDOWN => {
                popup.pressed = hit;
                dismiss = hit.is_none();
            }
            WM_RBUTTONUP => dismiss = true,
            WM_LBUTTONUP => {
                if let Some((row, part)) = popup.pressed.take().filter(|press| Some(*press) == hit)
                {
                    if let Some(label) = popup.labels.get(row) {
                        action = Some((popup.request.key, label.clone(), part));
                        popup.framed = false;
                        dismiss = part == Part::Label;
                    }
                }
            }
            _ => {}
        }
        Some(LRESULT(0))
    });
    if dismiss {
        close(owner);
    }
    if let Some((key, label, part)) = action {
        if let Err(error) = apply(owner, key, label, part) {
            eprintln!("native filter apply: {error}");
        }
        changed = true;
    }
    if changed {
        if let Err(error) = paint() {
            eprintln!("native filter update: {error}");
            close(owner);
        }
        super::invalidate_all(owner);
    }
    handled
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    if message == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}
