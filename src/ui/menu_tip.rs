use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::DirectComposition::IDCompositionTarget;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::address::rect;
use super::render::{Layer, Renderer};
use super::text::{Family, TextStyle};

#[derive(Clone, Copy)]
pub enum Placement {
    Above,
    Below,
    Pointer,
}

fn origin(pointer: POINT, placement: Placement, size: (f32, f32), scale: f32) -> (i32, i32) {
    let work = crate::frame::work_area(pointer.x, pointer.y);
    let width = (size.0 * scale).round() as i32;
    let height = (size.1 * scale).round() as i32;
    let x = match placement {
        Placement::Pointer => pointer.x + scale.round() as i32,
        _ => pointer.x - (size.0 * scale / 2.0).round() as i32,
    };
    let y = match placement {
        Placement::Above => pointer.y - ((20.0 + size.1) * scale).round() as i32,
        Placement::Below => pointer.y + (20.0 * scale).round() as i32,
        Placement::Pointer => pointer.y + (11.0 * scale).round() as i32,
    };
    (
        x.clamp(work.left + 1, (work.right - width).max(work.left + 1)),
        y.clamp(work.top, (work.bottom - height).max(work.top)),
    )
}

pub enum Content {
    Tooltip {
        text: String,
        pointer: POINT,
        placement: Placement,
    },
    Description {
        text: String,
        pointer: POINT,
    },
    Keys {
        badges: Vec<(f32, f32, String)>,
        size: (f32, f32),
        at: (i32, i32),
    },
}

pub struct Overlay {
    hwnd: HWND,
    layer: Layer,
    target: IDCompositionTarget,
    opacity: windows::Win32::Graphics::DirectComposition::IDCompositionEffectGroup,
    placement: Option<Placement>,
    size: (f32, f32),
    scale: f32,
}

impl Overlay {
    pub fn show(renderer: &Renderer, owner: HWND, content: Content) -> Result<Self> {
        let popup = Self::prepare(renderer, owner, content)?;
        popup.reveal(renderer)?;
        Ok(popup)
    }

    pub fn prepare(renderer: &Renderer, owner: HWND, content: Content) -> Result<Self> {
        let (content, placement) = match content {
            Content::Tooltip {
                text,
                pointer,
                placement,
            } => (Content::Description { text, pointer }, Some(placement)),
            content => (content, None),
        };
        let style = TextStyle::new(
            if placement.is_some() || matches!(&content, Content::Keys { .. }) {
                Family::Variable
            } else {
                Family::Small
            },
            12.0,
            400,
        );
        let scale = unsafe { GetDpiForWindow(owner) } as f32 / 96.0;
        let (at, size, labels, keys) = super::WINDOW.with(|slot| {
            let mut slot = slot.borrow_mut();
            let text = &mut slot
                .as_mut()
                .ok_or_else(windows::core::Error::from_win32)?
                .text;
            match content {
                Content::Tooltip { .. } => unreachable!(),
                Content::Keys { badges, size, at } => Ok((
                    at,
                    size,
                    badges
                        .into_iter()
                        .map(|(x, y, label)| (x, y + 28.0, label))
                        .collect::<Vec<_>>(),
                    true,
                )),
                Content::Description {
                    text: value,
                    pointer,
                } => {
                    let mut lines = Vec::new();
                    let mut line = String::new();
                    for word in value.split_whitespace() {
                        let next = if line.is_empty() {
                            word.to_owned()
                        } else {
                            format!("{line} {word}")
                        };
                        if !line.is_empty() && text.layout(&next, style, 4096.0)?.advance > 301.6 {
                            lines.push(std::mem::take(&mut line));
                        }
                        if !line.is_empty() {
                            line.push(' ');
                        }
                        line.push_str(word);
                    }
                    if !line.is_empty() {
                        lines.push(line);
                    }
                    let mut width: f32 = 0.0;
                    for line in &lines {
                        width = width.max(text.layout(line, style, 4096.0)?.advance);
                    }
                    let size = (
                        (width.min(301.6) + if placement.is_some() { 18.0 } else { 16.4 }).ceil(),
                        16.0 + lines.len() as f32 * 16.0,
                    );
                    let work = crate::frame::work_area(pointer.x, pointer.y);
                    let at = if let Some(placement) = placement {
                        origin(pointer, placement, size, scale)
                    } else {
                        (
                            (pointer.x - ((10.0 + size.0 / 2.0) * scale).round() as i32).clamp(
                                work.left,
                                (work.right - (size.0 * scale).round() as i32).max(work.left),
                            ),
                            (pointer.y - ((21.0 + size.1) * scale).round() as i32).max(work.top),
                        )
                    };
                    Ok::<_, windows::core::Error>((
                        at,
                        size,
                        lines
                            .into_iter()
                            .enumerate()
                            .map(|(index, line)| {
                                (
                                    if placement.is_some() { 9.0 } else { 10.0 },
                                    if placement.is_some() { 7.0 } else { 8.0 }
                                        + index as f32 * 16.0,
                                    line,
                                )
                            })
                            .collect(),
                        false,
                    ))
                }
            }
        })?;
        unsafe {
            static REGISTER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if !*REGISTER.get_or_init(|| {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    lpszClassName: w!("SwMenuAdornment"),
                    ..Default::default()
                }) != 0
            }) {
                return Err(windows::core::Error::from_win32());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP,
                w!("SwMenuAdornment"),
                w!(""),
                WS_POPUP,
                at.0,
                at.1,
                (size.0 * scale).round() as i32,
                (size.1 * scale).round() as i32,
                Some(owner),
                None,
                None,
                None,
            )?;
            let made = (|| {
                if !keys {
                    use windows::Win32::Graphics::Dwm::*;
                    for (attribute, value) in [
                        (DWMWA_WINDOW_CORNER_PREFERENCE, 3u32),
                        (
                            DWMWA_BORDER_COLOR,
                            if placement.is_some() {
                                DWMWA_COLOR_NONE
                            } else {
                                crate::frame::flyout_border(true)
                            },
                        ),
                    ] {
                        DwmSetWindowAttribute(hwnd, attribute, &value as *const _ as _, 4)?;
                    }
                }
                let mut layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                layer.resize((
                    (size.0 * scale).round() as u32,
                    (size.1 * scale).round() as u32,
                ))?;
                let target = renderer.composition.CreateTargetForHwnd(hwnd, true)?;
                target.SetRoot(&layer.visual)?;
                let theme = super::theme::Theme::system();
                super::WINDOW.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let text = &mut slot
                        .as_mut()
                        .ok_or_else(windows::core::Error::from_win32)?
                        .text;
                    renderer.update(
                        &mut layer,
                        None,
                        GetDpiForWindow(owner),
                        &Default::default(),
                        |canvas| {
                            if !keys {
                                if placement.is_some() {
                                    super::details::fill(
                                        canvas,
                                        rect(0.0, 0.0, size.0, size.1),
                                        &theme.xaml_tip_border,
                                    );
                                    super::details::fill(
                                        canvas,
                                        rect(0.0, 1.0, size.0, size.1 - 2.0),
                                        &theme.xaml_tip,
                                    );
                                } else {
                                    super::details::fill(
                                        canvas,
                                        rect(0.0, 0.0, size.0, size.1),
                                        &theme.xaml_tip,
                                    );
                                }
                            }
                            for (x, y, value) in labels {
                                let width = text.layout(&value, style, 4096.0)?.advance;
                                if keys {
                                    let color =
                                        windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                                            r: if crate::shell::theme::dark() {
                                                118.0 / 255.0
                                            } else {
                                                242.0 / 255.0
                                            },
                                            g: if crate::shell::theme::dark() {
                                                118.0 / 255.0
                                            } else {
                                                242.0 / 255.0
                                            },
                                            b: if crate::shell::theme::dark() {
                                                118.0 / 255.0
                                            } else {
                                                242.0 / 255.0
                                            },
                                            a: 1.0,
                                        };
                                    super::details::fill(
                                        canvas,
                                        rect(x, y, width.ceil() + 8.0, 24.0),
                                        &color,
                                    );
                                    text.draw(
                                        canvas,
                                        &value,
                                        style,
                                        (x + 4.0, y + 4.0),
                                        &theme.text,
                                    )?;
                                } else {
                                    text.draw(canvas, &value, style, (x, y), &theme.text)?;
                                }
                            }
                            Ok(())
                        },
                    )
                })?;
                let opacity = renderer.composition.CreateEffectGroup()?;
                opacity.SetOpacity2(0.0)?;
                layer.visual.SetEffect(&opacity)?;
                renderer.composition.Commit()?;
                Ok(Self {
                    hwnd,
                    layer,
                    target,
                    opacity,
                    placement,
                    size,
                    scale,
                })
            })();
            if made.is_err() {
                DestroyWindow(hwnd)?;
            }
            made
        }
    }

    pub fn reveal(&self, renderer: &Renderer) -> Result<()> {
        unsafe {
            self.opacity.SetOpacity2(1.0)?;
            renderer.composition.Commit()?;
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            Ok(())
        }
    }

    pub fn reposition(&self, pointer: POINT) -> Result<()> {
        let Some(placement) = self.placement else {
            return Ok(());
        };
        let (x, y) = origin(pointer, placement, self.size, self.scale);
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
    }

    pub fn close(self, renderer: &Renderer) -> Result<()> {
        unsafe {
            let results = [
                self.target.SetRoot(None),
                self.layer.surface.Trim(Some(&[])),
                DestroyWindow(self.hwnd),
                renderer.composition.Commit(),
            ];
            let mut failure = None;
            for result in results {
                if let Err(error) = result {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
            failure.map_or(Ok(()), Err)
        }
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    if message == WM_MOUSEACTIVATE {
        return windows::Win32::Foundation::LRESULT(MA_NOACTIVATE as isize);
    }
    if message == WM_ERASEBKGND {
        return windows::Win32::Foundation::LRESULT(1);
    }
    if message == WM_PAINT {
        let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
        let _ = windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &mut paint);
        let _ = windows::Win32::Graphics::Gdi::EndPaint(hwnd, &paint);
        return windows::Win32::Foundation::LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}
