use std::rc::Rc;
use std::sync::Arc;
use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::IDCompositionTarget;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::address::rect;
use super::render::{Layer, Renderer};
use super::text::{Family, TextEngine, TextStyle, TAB_GLYPH, TAB_TITLE};

#[derive(Clone, Copy, PartialEq)]
pub enum TabLook {
    Strip,
    Detached,
}

pub enum Frame {
    Files {
        picture: Option<Arc<crate::shell::icons::Rgba>>,
        caption: String,
        effect: windows::Win32::System::Ole::DROPEFFECT,
    },
    Tab {
        item: super::titlebar::Item,
        width: f32,
        look: TabLook,
    },
}

pub struct Popup {
    hwnd: HWND,
    renderer: Rc<Renderer>,
    target: IDCompositionTarget,
    layer: Layer,
    ink: Option<Layer>,
    size: (f32, f32),
    scale: f32,
}

impl Popup {
    pub fn new(
        owner: HWND,
        renderer: Rc<Renderer>,
        text: &mut TextEngine,
        frame: Frame,
    ) -> Result<Self> {
        unsafe {
            static REGISTER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if !*REGISTER.get_or_init(|| {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    lpszClassName: w!("SwExplorerDragPreview"),
                    ..Default::default()
                }) != 0
            }) {
                return Err(windows::core::Error::from_win32());
            }
            let scale = GetDpiForWindow(owner) as f32 / 96.0;
            let label = TextStyle::new(Family::Segoe, 14.0, 400);
            let size = match &frame {
                Frame::Files { caption, .. } => (
                    (text.layout(caption, label, 4096.0)?.advance.ceil() + 40.0).max(80.0),
                    80.0,
                ),
                Frame::Tab { width, .. } => (*width, 32.0),
            };
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP,
                w!("SwExplorerDragPreview"),
                w!(""),
                WS_POPUP,
                0,
                0,
                (size.0 * scale).round() as i32,
                (size.1 * scale).round() as i32,
                Some(owner),
                None,
                None,
                None,
            )?;
            let result = (|| {
                let mut layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                layer.resize((
                    (size.0 * scale).round().max(1.0) as u32,
                    (size.1 * scale).round().max(1.0) as u32,
                ))?;
                let target = renderer.composition.CreateTargetForHwnd(hwnd, true)?;
                target.SetRoot(&layer.visual)?;
                let theme = super::theme::Theme::system();
                renderer.update(
                    &mut layer,
                    None,
                    GetDpiForWindow(owner),
                    &Default::default(),
                    |canvas| {
                        match &frame {
                            Frame::Files {
                                picture, effect, ..
                            } => {
                                if let Some(picture) = picture {
                                    let image = canvas.shared_icon(picture)?;
                                    canvas.context.DrawBitmap(
                                        image.as_ref(),
                                        Some(&rect(
                                            ((size.0 - 80.0) / 2.0).ceil(),
                                            0.0,
                                            80.0,
                                            80.0,
                                        )),
                                        1.0,
                                        D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                                        None,
                                        None,
                                    );
                                }
                                let forbidden =
                                    *effect == windows::Win32::System::Ole::DROPEFFECT_NONE;
                                let (left, top, width, height) = if forbidden {
                                    ((size.0 - 32.0) / 2.0, 27.0, 32.0, 32.0)
                                } else {
                                    (0.0, 22.0, size.0, 37.0)
                                };
                                let bounds = rect(left, top, width, height);
                                canvas.brush.SetColor(&theme.caption_box);
                                let shape = D2D1_ROUNDED_RECT {
                                    rect: bounds,
                                    radiusX: if forbidden { 5.0 } else { 4.0 },
                                    radiusY: if forbidden { 5.0 } else { 4.0 },
                                };
                                canvas.context.FillRoundedRectangle(&shape, &canvas.brush);
                                canvas.brush.SetColor(&theme.caption_box_border);
                                canvas.context.DrawRoundedRectangle(
                                    &D2D1_ROUNDED_RECT {
                                        rect: rect(
                                            left + 0.5,
                                            top + 0.5,
                                            width - 1.0,
                                            height - 1.0,
                                        ),
                                        radiusX: if forbidden { 4.5 } else { 3.5 },
                                        radiusY: if forbidden { 4.5 } else { 3.5 },
                                    },
                                    &canvas.brush,
                                    1.0,
                                    None,
                                );
                            }
                            Frame::Tab { item, width, look } => {
                                let detached = *look == TabLook::Detached;
                                canvas.brush.SetColor(&theme.tab_drag);
                                canvas.context.FillRoundedRectangle(
                                    &D2D1_ROUNDED_RECT {
                                        rect: rect(9.0, 1.0, width - 10.0, 30.0),
                                        radiusX: 8.0,
                                        radiusY: 8.0,
                                    },
                                    &canvas.brush,
                                );
                                let icon = canvas.shared_icon(&item.icon)?;
                                canvas.context.DrawBitmap(
                                    icon.as_ref(),
                                    Some(&rect(
                                        15.0 + (if detached { 1.0 } else { 0.0 }),
                                        7.5,
                                        16.0,
                                        16.0,
                                    )),
                                    1.0,
                                    D2D1_INTERPOLATION_MODE_LINEAR,
                                    None,
                                    None,
                                );
                                let layout = text.layout(&item.title, TAB_TITLE, 4096.0)?;
                                canvas.context.PushAxisAlignedClip(
                                    &rect(41.0, 0.0, crate::app::tabs::title_room(*width), 32.0),
                                    D2D1_ANTIALIAS_MODE_ALIASED,
                                );
                                let drawn = text.draw(
                                    canvas,
                                    &item.title,
                                    TAB_TITLE,
                                    (
                                        41.0 + (if detached { 1.0 } else { 0.0 }),
                                        -1.0 + (if detached { 1.0 } else { 0.0 })
                                            + (32.0 - layout.height) / 2.0,
                                    ),
                                    &theme.text,
                                );
                                canvas.context.PopAxisAlignedClip();
                                drawn?;
                                let layout = text.layout("\u{e711}", TAB_GLYPH, 4096.0)?;
                                text.draw(
                                    canvas,
                                    "\u{e711}",
                                    TAB_GLYPH,
                                    (
                                        width - 35.0 - (if detached { 1.0 } else { 0.0 })
                                            + (32.0 - layout.width / scale) / 2.0,
                                        4.0 + (24.0 - layout.height) / 2.0,
                                    ),
                                    &theme.text,
                                )?;
                            }
                        }
                        Ok(())
                    },
                )?;
                let ink = if let Frame::Files {
                    caption, effect, ..
                } = &frame
                {
                    let mut ink = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                    ink.resize((
                        (size.0 * scale).round().max(1.0) as u32,
                        (size.1 * scale).round().max(1.0) as u32,
                    ))?;
                    renderer.update(
                        &mut ink,
                        None,
                        GetDpiForWindow(owner),
                        &Default::default(),
                        |canvas| {
                            let forbidden = *effect == windows::Win32::System::Ole::DROPEFFECT_NONE;
                            let (left, top, width, height) = if forbidden {
                                ((size.0 - 32.0) / 2.0, 27.0, 32.0, 32.0)
                            } else {
                                (0.0, 22.0, size.0, 37.0)
                            };
                            let glyph = if forbidden {
                                "\u{e733}"
                            } else if effect.0 & windows::Win32::System::Ole::DROPEFFECT_MOVE.0 != 0
                            {
                                "\u{e8ad}"
                            } else if effect.0 & windows::Win32::System::Ole::DROPEFFECT_COPY.0 != 0
                            {
                                "\u{e710}"
                            } else {
                                "\u{e71b}"
                            };
                            let style = TextStyle::new(Family::Icons, 14.0, 400);
                            let layout = text.layout(glyph, style, 4096.0)?;
                            let position = (
                                if forbidden {
                                    left + (width - layout.advance) / 2.0
                                } else {
                                    8.0 + (16.0 - layout.advance) / 2.0
                                },
                                top + (height - layout.height) / 2.0,
                            );
                            if forbidden {
                                text.draw(canvas, glyph, style, position, &theme.text)?;
                            } else {
                                let mut color = theme.text;
                                color.a *= 0.5;
                                canvas.context.SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_ADD);
                                let drawn = (|| -> Result<()> {
                                    for offset in [-1.0, 0.0] {
                                        text.draw(
                                            canvas,
                                            glyph,
                                            style,
                                            (position.0, position.1 + offset),
                                            &color,
                                        )?;
                                    }
                                    Ok(())
                                })();
                                canvas
                                    .context
                                    .SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_SOURCE_OVER);
                                drawn?;
                            }
                            if !forbidden {
                                let layout = text.layout(caption, label, 4096.0)?;
                                text.draw(
                                    canvas,
                                    caption,
                                    label,
                                    (31.0, top + (height - layout.height) / 2.0),
                                    &theme.text,
                                )?;
                            }
                            Ok(())
                        },
                    )?;
                    ink.visual
                        .SetOffsetX2(if caption.is_empty() { 0.0 } else { 0.4 * scale })?;
                    layer.visual.AddVisual(&ink.visual, true, None)?;
                    Some(ink)
                } else {
                    None
                };
                renderer.composition.Commit()?;
                Ok(Self {
                    hwnd,
                    renderer: renderer.clone(),
                    target,
                    layer,
                    ink,
                    size,
                    scale,
                })
            })();
            if result.is_err() {
                DestroyWindow(hwnd)?;
            }
            result
        }
    }

    pub fn move_to(&self, point: POINT) -> Result<()> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                point.x,
                point.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        }
    }

    pub fn files_at(&self, pointer: POINT) -> Result<()> {
        self.move_to(POINT {
            x: pointer.x - (self.size.0 * self.scale / 2.0).round() as i32,
            y: pointer.y - (70.0 * self.scale).round() as i32,
        })
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            if let Some(ink) = &self.ink {
                if let Err(error) = ink.surface.Trim(Some(&[])) {
                    eprintln!("drag ink cleanup: {error}");
                }
            }
            for result in [
                self.target.SetRoot(None),
                self.layer.surface.Trim(Some(&[])),
                DestroyWindow(self.hwnd),
                self.renderer.composition.Commit(),
            ] {
                if let Err(error) = result {
                    eprintln!("drag preview cleanup: {error}");
                }
            }
        }
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
            let _ = windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &mut paint);
            let _ = windows::Win32::Graphics::Gdi::EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
