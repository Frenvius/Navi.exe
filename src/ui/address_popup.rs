use super::address::{rect, Item, Target};
use std::cell::RefCell;
use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::IDCompositionTarget;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const LABEL: super::text::TextStyle =
    super::text::TextStyle::new(super::text::Family::Variable, 14.0, 400);

fn label_style(target: Target) -> super::text::TextStyle {
    if matches!(target, Target::Address | Target::More) {
        LABEL.with_ideal_advances()
    } else {
        LABEL
    }
}

struct Popup {
    hwnd: HWND,
    owner: HWND,
    target: Target,
    items: Vec<Item>,
    picked: Option<usize>,
    hover: Option<usize>,
    pressed: Option<usize>,
    offset: f32,
    width: f32,
    height: f32,
    row: f32,
    gap: f32,
    inside: bool,
    bar_hot: bool,
    drag: Option<(f32, f32)>,
    wheel: i32,
    chrome: Option<ID2D1Bitmap1>,
    layer: super::render::Layer,
    _target: IDCompositionTarget,
}

thread_local! {static POPUP:RefCell<Option<Popup>>=const {RefCell::new(None)};}

pub fn close() {
    let popup = POPUP.with(|slot| slot.borrow_mut().take());
    if let Some(popup) = popup {
        let _context = super::context::enter(popup.owner);
        let renderer = super::WINDOW.with(|slot| {
            slot.borrow().as_ref().and_then(|window| {
                window
                    .graphics
                    .as_ref()
                    .map(|(renderer, _)| renderer.clone())
            })
        });
        let released = (|| -> Result<()> {
            unsafe {
                popup.layer.surface.Trim(Some(&[]))?;
                popup._target.SetRoot(None)?;
                if let Some(renderer) = renderer {
                    renderer.composition.Commit()?;
                }
            }
            Ok(())
        })();
        if let Err(error) = released {
            eprintln!("address popup backing release: {error}");
        }
        if let Err(error) = unsafe { DestroyWindow(popup.hwnd) } {
            eprintln!("address popup close: {error}");
        }
        super::memory::checkpoint("address-popup-closed");
    }
}

pub fn close_owner(owner: HWND) {
    let owned = POPUP.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|popup| popup.owner == owner)
    });
    if owned {
        close();
    }
}

pub fn menu_open() -> bool {
    POPUP.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|popup| popup.target != Target::Address)
    })
}

pub fn refresh(owner: HWND) -> Result<()> {
    let state = POPUP.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|popup| popup.owner == owner)
            .map(|popup| {
                (
                    popup.target,
                    popup.items.clone(),
                    popup.picked,
                    popup.offset,
                )
            })
    });
    let Some((target, items, picked, offset)) = state else {
        return Ok(());
    };
    close();
    super::paint_window(owner)?;
    unsafe {
        windows::Win32::Graphics::Dwm::DwmFlush()?;
    }
    show(owner, target, items, picked)?;
    POPUP.with(|slot| {
        if let Some(popup) = slot.borrow_mut().as_mut() {
            popup.offset = offset.min(popup.maximum());
        }
    });
    Ok(())
}

pub fn key(owner: HWND, key: u32) -> bool {
    let state = POPUP.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|popup| {
            if popup.owner != owner || popup.target == Target::Address {
                return None;
            }
            let count = popup.items.len();
            match key {
                0x26 | 0x28 => {
                    popup.picked = Some(match popup.picked {
                        Some(index) => (index as isize + if key == 0x26 { -1 } else { 1 })
                            .rem_euclid(count as isize)
                            as usize,
                        None => {
                            if key == 0x26 {
                                count - 1
                            } else {
                                0
                            }
                        }
                    })
                }
                0x24 => popup.picked = Some(0),
                0x23 => popup.picked = Some(count - 1),
                0x0d | 0x20 => {
                    return popup.picked.map(|index| {
                        (popup.hwnd, Some((popup.target, popup.items[index].clone())))
                    })
                }
                0x1b => return Some((popup.hwnd, None)),
                _ => return None,
            }
            unsafe {
                let _ = InvalidateRect(Some(popup.hwnd), None, false);
            }
            Some((HWND::default(), None))
        })
    });
    let Some((hwnd, chosen)) = state else {
        return false;
    };
    if hwnd.is_invalid() {
        return true;
    }
    close();
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.address.open = None;
        }
    });
    super::address_input::invalidate(owner);
    if let Some((target, item)) = chosen {
        super::address_input::chosen(owner, target, item);
    }
    true
}

pub fn show(owner: HWND, target: Target, items: Vec<Item>, picked: Option<usize>) -> Result<()> {
    close();
    if items.is_empty() {
        return Ok(());
    }
    let scale = unsafe { GetDpiForWindow(owner) } as f32 / 96.0;
    let origin = crate::frame::client_to_screen(owner, 0, (80.0 * scale).round() as i32);
    let work = crate::frame::work_area(origin.0, origin.1);
    let available = ((work.bottom - origin.1) as f32 / scale - 4.0).max(1.0);
    let geometry = super::WINDOW.with(|slot| -> Result<Option<(f32, f32, f32, f32, f32)>> {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return Ok(None);
        };
        let Some(bounds) = window.address.bounds(target) else {
            return Ok(None);
        };
        let row = if target == Target::Address {
            40.0
        } else if target == Target::More {
            41.0
        } else {
            32.0
        };
        let maximum = if target == Target::Address {
            374.0
        } else if target == Target::More {
            available
        } else {
            320.0
        };
        let width = if target == Target::Address {
            window.address.address_width
        } else {
            let mut width = 160.0f32;
            for item in &items {
                width = width.max(
                    window
                        .text
                        .layout(&item.label, label_style(target), 4096.0)?
                        .advance
                        .ceil()
                        + 32.0,
                );
            }
            width.min(420.0).min((window.address.width - 16.0).max(1.0))
        };
        let height = if target == Target::Address {
            (items.len() as f32 * row).min(maximum) + 6.0
        } else {
            (items.len() as f32 * row + if target == Target::More { 4.0 } else { 6.0 }).min(maximum)
        };
        let x = if target == Target::More {
            (bounds.left + bounds.right - width) / 2.0
        } else {
            bounds.left
        };
        let y = if target == Target::Address {
            79.0
        } else {
            bounds.bottom + 44.0
        };
        Ok(Some((x, y, width, height, row)))
    })?;
    let Some((x, y, width, height, row)) = geometry else {
        return Ok(());
    };
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("SwAddressPopup"),
            ..Default::default()
        };
        static REGISTER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*REGISTER.get_or_init(|| RegisterClassW(&class) != 0) {
            return Err(windows::core::Error::from_win32());
        }
        let (x, y) = crate::frame::client_to_screen(
            owner,
            (x * scale).round() as i32,
            (y * scale).round() as i32,
        );
        let margin = if target == Target::Address { 0.0 } else { 20.0 };
        let dx = if margin > 0.0 {
            (10.0 * scale).round() as i32
        } else {
            0
        };
        let dy = if margin > 0.0 {
            (2.0 * scale).round() as i32
        } else {
            0
        };
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            class.lpszClassName,
            w!(""),
            WS_POPUP,
            x - dx,
            y - dy,
            ((width + margin) * scale).round() as i32,
            ((height + margin) * scale).round() as i32,
            Some(owner),
            None,
            None,
            None,
        )?;
        let result = (|| {
            let light = super::WINDOW.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .is_some_and(|window| window.theme.content.r > 0.5)
            });
            let mut chrome = if margin > 0.0 {
                Some(crate::app::menu_chrome::pixels(
                    (width + 20.0).round() as i32,
                    (height + 20.0).round() as i32,
                    !light,
                    false,
                ))
            } else {
                None
            };
            if let Some(pixels) = &mut chrome {
                if let Some(backdrop) = crate::shell::backdrop::blurred(
                    x - dx,
                    y - dy,
                    ((width + margin) * scale).round() as usize,
                    ((height + margin) * scale).round() as usize,
                    scale,
                ) {
                    let physical_width = ((width + margin) * scale).round() as usize;
                    let physical_height = ((height + margin) * scale).round() as usize;
                    let logical_width = pixels.width as usize;
                    for (index, pixel) in pixels.pixels.chunks_exact_mut(4).enumerate() {
                        let alpha = pixel[3] as u32;
                        if alpha >= if light { 217 } else { 245 } {
                            let bx = (((index % logical_width) as f32 + 0.5) * scale - 0.5)
                                .round()
                                .max(0.0) as usize;
                            let by = (((index / logical_width) as f32 + 0.5) * scale - 0.5)
                                .round()
                                .max(0.0) as usize;
                            let source = (by.min(physical_height - 1) * physical_width
                                + bx.min(physical_width - 1))
                                * 3;
                            for channel in 0..3 {
                                pixel[channel] = ((pixel[channel] as u32 * alpha
                                    + backdrop[source + channel] as u32 * (255 - alpha)
                                    + 127)
                                    / 255
                                    + u32::from(!light))
                                .min(255) as u8;
                            }
                            pixel[3] = 255;
                        }
                    }
                }
            }
            let (mut layer, composition_target, chrome) = super::WINDOW.with(|slot| {
                let slot = slot.borrow();
                let window = slot.as_ref().ok_or_else(windows::core::Error::from_win32)?;
                let (renderer, _) = window
                    .graphics
                    .as_ref()
                    .ok_or_else(windows::core::Error::from_win32)?;
                let layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                let target = renderer.composition.CreateTargetForHwnd(hwnd, true)?;
                target.SetRoot(&layer.visual)?;
                let chrome = chrome
                    .as_ref()
                    .map(|source| renderer.canvas.system_icon(source))
                    .transpose()?;
                Ok::<_, windows::core::Error>((layer, target, chrome))
            })?;
            layer.resize((
                ((width + margin) * scale).round() as u32,
                ((height + margin) * scale).round() as u32,
            ))?;
            POPUP.with(|slot| {
                *slot.borrow_mut() = Some(Popup {
                    hwnd,
                    owner,
                    target,
                    items,
                    picked,
                    hover: None,
                    pressed: None,
                    offset: 0.0,
                    width,
                    height,
                    row,
                    gap: if target == Target::More { 1.0 } else { 0.0 },
                    inside: false,
                    bar_hot: false,
                    drag: None,
                    wheel: 0,
                    chrome,
                    layer,
                    _target: composition_target,
                })
            });
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
            Ok(())
        })();
        if result.is_err() {
            DestroyWindow(hwnd)?;
        }
        result
    }
}

fn paint(hwnd: HWND) -> Result<()> {
    let popup = POPUP.with(|slot| slot.borrow_mut().take());
    let Some(mut popup) = popup else {
        return Ok(());
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if let Some(picked) = popup.picked {
        let top = picked as f32 * popup.row;
        if top < popup.offset {
            popup.offset = top;
        }
        if top + popup.row - popup.gap > popup.offset + popup.page() {
            popup.offset = top + popup.row - popup.gap - popup.page();
        }
    }
    let result = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return Ok(());
        };
        let Some((renderer, _)) = window.graphics.as_ref() else {
            return Ok(());
        };
        let theme = &window.theme;
        let thumb = (popup.inside && popup.maximum() > 0.0).then(|| popup.thumb());
        let padding = popup.padding();
        let page = popup.page();
        renderer.update(
            &mut popup.layer,
            None,
            dpi,
            &D2D1_COLOR_F::default(),
            |canvas| {
                let (dx, dy) = if let Some(chrome) = &popup.chrome {
                    unsafe {
                        canvas.context.DrawBitmap(
                            chrome,
                            None,
                            1.0,
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            None,
                            None,
                        );
                    }
                    if popup.target == Target::More {
                        super::address::rounded(
                            canvas,
                            rect(10.0, 2.0, popup.width, popup.height),
                            9.0,
                            &theme.flyout,
                        );
                        unsafe {
                            canvas.brush.SetColor(&theme.address_overflow_border);
                            canvas.context.DrawRoundedRectangle(
                                &D2D1_ROUNDED_RECT {
                                    rect: rect(10.5, 2.5, popup.width - 1.0, popup.height - 1.0),
                                    radiusX: 8.5,
                                    radiusY: 8.5,
                                },
                                &canvas.brush,
                                1.0,
                                None,
                            );
                        }
                    }
                    (10.0, 2.0)
                } else {
                    super::details::fill(
                        canvas,
                        rect(0.0, 0.0, popup.width, 1.0),
                        &theme.suggestions_border,
                    );
                    super::details::fill(
                        canvas,
                        rect(1.0, 1.0, popup.width - 2.0, popup.height - 9.0),
                        &theme.suggestions,
                    );
                    unsafe {
                        canvas.context.PushAxisAlignedClip(
                            &rect(0.0, popup.height - 8.0, popup.width, 8.0),
                            D2D1_ANTIALIAS_MODE_ALIASED,
                        );
                    }
                    super::address::rounded(
                        canvas,
                        rect(1.0, 1.0, popup.width - 2.0, popup.height - 2.0),
                        7.0,
                        &theme.suggestions,
                    );
                    unsafe {
                        canvas.context.PopAxisAlignedClip();
                    }
                    (0.0, 0.0)
                };
                let mut original = Default::default();
                unsafe {
                    canvas.context.GetTransform(&mut original);
                    let mut moved = original;
                    moved.M31 += dx;
                    moved.M32 += dy;
                    canvas.context.SetTransform(&moved);
                }
                unsafe {
                    canvas.context.PushAxisAlignedClip(
                        &rect(1.0, padding, popup.width - 2.0, page),
                        D2D1_ANTIALIAS_MODE_ALIASED,
                    );
                }
                let result = (|| {
                    let start = (popup.offset / popup.row).floor() as usize;
                    let end = (((popup.offset + page) / popup.row).ceil() as usize)
                        .min(popup.items.len());
                    for index in start..end {
                        let top = padding + index as f32 * popup.row - popup.offset;
                        if popup.hover == Some(index) || popup.picked == Some(index) {
                            super::address::rounded(
                                canvas,
                                rect(
                                    4.0,
                                    top + 2.0,
                                    popup.width - 8.0,
                                    popup.row
                                        - popup.gap
                                        - if popup.target == Target::More {
                                            5.0
                                        } else {
                                            4.0
                                        },
                                ),
                                4.0,
                                if popup.pressed == Some(index) {
                                    &theme.subtle_pressed
                                } else {
                                    &theme.subtle_hover
                                },
                            );
                        }
                        if popup.picked == Some(index) && popup.target == Target::Address {
                            super::address::rounded(
                                canvas,
                                rect(4.0, top + 12.0, 3.0, 16.0),
                                1.5,
                                &theme.accent,
                            );
                        }
                        let item = &popup.items[index];
                        let label = if popup.target == Target::Address {
                            item.label.clone()
                        } else {
                            window
                                .text
                                .elide(&item.label, label_style(popup.target), popup.width - 32.0)?
                                .into_owned()
                        };
                        unsafe {
                            canvas.context.PushAxisAlignedClip(
                                &rect(
                                    16.0,
                                    top,
                                    popup.width
                                        - if popup.target == Target::Address {
                                            30.0
                                        } else {
                                            32.0
                                        },
                                    popup.row - popup.gap,
                                ),
                                D2D1_ANTIALIAS_MODE_ALIASED,
                            );
                        }
                        let layout =
                            window
                                .text
                                .layout(&label, label_style(popup.target), 4096.0)?;
                        let y = top + (popup.row - popup.gap - layout.height) / 2.0;
                        window.text.draw(
                            canvas,
                            &label,
                            label_style(popup.target),
                            (
                                16.0,
                                if popup.target == Target::Address {
                                    y
                                } else {
                                    y.floor()
                                },
                            ),
                            &theme.text,
                        )?;
                        unsafe {
                            canvas.context.PopAxisAlignedClip();
                        }
                    }
                    Ok(())
                })();
                unsafe {
                    canvas.context.PopAxisAlignedClip();
                }
                if let Some((top, height, _, _)) = thumb {
                    let thickness = if popup.bar_hot || popup.drag.is_some() {
                        6.0
                    } else {
                        2.0
                    };
                    unsafe {
                        canvas.brush.SetColor(if popup.drag.is_some() {
                            &theme.xaml_thumb_drag
                        } else if popup.bar_hot {
                            &theme.xaml_thumb_hot
                        } else {
                            &theme.xaml_thumb
                        });
                        canvas.context.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: rect(
                                    popup.width - 6.0 - thickness / 2.0,
                                    top,
                                    thickness,
                                    height,
                                ),
                                radiusX: thickness / 2.0,
                                radiusY: 4.0,
                            },
                            &canvas.brush,
                        );
                    }
                }
                unsafe {
                    canvas.context.SetTransform(&original);
                }
                result
            },
        )?;
        unsafe { renderer.composition.Commit() }
    });
    POPUP.with(|slot| *slot.borrow_mut() = Some(popup));
    result
}

impl Popup {
    fn padding(&self) -> f32 {
        if self.target == Target::More {
            2.0
        } else {
            3.0
        }
    }
    fn page(&self) -> f32 {
        self.height
            - if self.target == Target::More {
                5.0
            } else {
                6.0
            }
    }
    fn maximum(&self) -> f32 {
        (self.items.len() as f32 * self.row - self.gap - self.page()).max(0.0)
    }
    fn thumb(&self) -> (f32, f32, f32, f32) {
        let page = self.page();
        let track = (page - 32.0).max(0.0);
        let height = (track * page / (self.items.len() as f32 * self.row - self.gap).max(1.0))
            .max(24.0)
            .min(track);
        let travel = (track - height).max(0.0);
        (
            19.0 + self.offset / self.maximum().max(1.0) * travel,
            (height - 6.0).max(0.0),
            travel,
            self.maximum(),
        )
    }
    fn point(&self, point: (f32, f32)) -> (f32, f32) {
        if self.target == Target::Address {
            point
        } else {
            (point.0 - 10.0, point.1 - 2.0)
        }
    }
    fn bar_at(&self, point: (f32, f32)) -> bool {
        let point = self.point(point);
        self.maximum() > 0.0
            && point.0 >= self.width - 13.0
            && point.0 < self.width - 1.0
            && point.1 >= 3.0
            && point.1 < self.height - 3.0
    }
}

fn row_at(popup: &Popup, point: (f32, f32)) -> Option<usize> {
    if popup.bar_at(point) {
        return None;
    }
    let point = popup.point(point);
    if point.0 < 1.0
        || point.0 >= popup.width - 1.0
        || point.1 < popup.padding()
        || point.1 >= popup.padding() + popup.page()
    {
        return None;
    }
    let index = ((point.1 - popup.padding() + popup.offset) / popup.row).floor() as usize;
    (index < popup.items.len()).then_some(index)
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let _context = POPUP
        .with(|slot| slot.borrow().as_ref().map(|popup| popup.owner))
        .map(super::context::enter);
    let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
    let point = (
        lparam.0 as i16 as f32 / scale,
        (lparam.0 >> 16) as i16 as f32 / scale,
    );
    match message {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_NCHITTEST => return LRESULT(HTCLIENT as isize),
        WM_PAINT => {
            let mut state = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut state);
            let result = paint(hwnd);
            let _ = EndPaint(hwnd, &state);
            if let Err(error) = result {
                if super::render::device_lost(&error) {
                    let owner = POPUP.with(|slot| slot.borrow().as_ref().map(|popup| popup.owner));
                    close();
                    if let Some(owner) = owner {
                        super::WINDOW.with(|slot| {
                            if let Some(window) = slot.borrow_mut().as_mut() {
                                window.address.open = None;
                            }
                        });
                        super::invalidate_all(owner);
                    }
                }
                super::fail(error);
            }
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let changed = POPUP.with(|slot| {
                slot.borrow_mut().as_mut().is_some_and(|popup| {
                    let hot = popup.bar_at(point);
                    let hover = row_at(popup, point);
                    let changed = !popup.inside
                        || popup.hover != hover
                        || popup.bar_hot != hot
                        || popup.drag.is_some();
                    popup.inside = true;
                    popup.bar_hot = hot;
                    popup.hover = hover;
                    if let Some((y, offset)) = popup.drag {
                        let (_, _, travel, maximum) = popup.thumb();
                        popup.picked = None;
                        popup.offset = (offset
                            + (popup.point(point).1 - y) * maximum / travel.max(1.0))
                        .clamp(0.0, maximum);
                    }
                    changed
                })
            });
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                ..Default::default()
            };
            if let Err(error) = TrackMouseEvent(&mut tracking) {
                eprintln!("address popup mouse tracking: {error}");
            }
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            POPUP.with(|slot| {
                if let Some(popup) = slot.borrow_mut().as_mut() {
                    popup.inside = false;
                    popup.hover = None;
                    popup.bar_hot = false;
                }
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            POPUP.with(|slot| {
                if let Some(popup) = slot.borrow_mut().as_mut() {
                    popup.drag = None;
                    popup.pressed = None;
                }
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_LBUTTONDOWN => {
            let capture = POPUP.with(|slot| {
                slot.borrow_mut().as_mut().is_some_and(|popup| {
                    popup.pressed = row_at(popup, point);
                    if !popup.bar_at(point) {
                        return popup.pressed.is_some();
                    }
                    let y = popup.point(point).1;
                    let (top, height, _, maximum) = popup.thumb();
                    popup.picked = None;
                    if y >= top && y < top + height {
                        popup.drag = Some((y, popup.offset));
                    } else {
                        popup.offset = (popup.offset
                            + if y < top {
                                3.0 - popup.height
                            } else {
                                popup.height - 3.0
                            })
                        .clamp(0.0, maximum);
                    }
                    true
                })
            });
            if capture {
                SetCapture(hwnd);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let dragging = POPUP.with(|slot| {
                slot.borrow_mut()
                    .as_mut()
                    .is_some_and(|popup| popup.drag.take().is_some())
            });
            let chosen = POPUP.with(|slot| {
                slot.borrow().as_ref().and_then(|popup| {
                    let row = row_at(popup, point)?;
                    (popup.pressed == Some(row))
                        .then(|| (popup.owner, popup.target, popup.items[row].clone()))
                })
            });
            if GetCapture() == hwnd {
                if let Err(error) = ReleaseCapture() {
                    eprintln!("address popup capture release: {error}");
                }
            }
            if dragging {
                let _ = InvalidateRect(Some(hwnd), None, false);
                return LRESULT(0);
            }
            if let Some((owner, target, item)) = chosen {
                close();
                super::address_input::chosen(owner, target, item);
            }
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let mut cursor = windows::Win32::Foundation::POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            if !windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut cursor).as_bool() {
                eprintln!(
                    "address popup wheel coordinates: {}",
                    windows::core::Error::from_win32()
                );
            }
            let point = (cursor.x as f32 / scale, cursor.y as f32 / scale);
            POPUP.with(|slot| {
                if let Some(popup) = slot.borrow_mut().as_mut() {
                    popup.picked = None;
                    popup.wheel += (wparam.0 >> 16) as i16 as i32;
                    let notches = popup.wheel / 120;
                    popup.wheel %= 120;
                    let lines = crate::app::scroll::wheel_lines();
                    let step = if lines == u32::MAX {
                        popup.page()
                    } else {
                        (popup.page() * 0.05 * lines as f32).round()
                    };
                    popup.offset =
                        (popup.offset - notches as f32 * step).clamp(0.0, popup.maximum());
                    popup.hover = row_at(popup, point);
                }
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}
