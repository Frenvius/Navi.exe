use std::sync::Arc;
use std::time::Instant;
use windows::core::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

use super::render::Canvas;
use super::text::{Family, TextEngine, TextStyle};
use super::theme::Theme;
use crate::app::pane_model::{Kind, Model, Selection};

const LOAD: usize = 80;
const SMALL: TextStyle = TextStyle::new(Family::Variable, 12.0, 400).with_rounded_advances();
const HEADING: TextStyle = TextStyle::new(Family::Variable, 12.0, 600).with_rounded_advances();
const TITLE: TextStyle = TextStyle::new(Family::Variable, 16.0, 600);
const INFO: TextStyle = TextStyle::new(Family::Variable, 14.0, 400)
    .with_ideal_advances()
    .with_floor_baseline();

#[derive(PartialEq)]
struct Key {
    kind: Kind,
    parent: String,
    item: Option<(String, u64, u64, u32)>,
    count: usize,
    selected: usize,
    enumerations: usize,
    width: u32,
    icons_only: bool,
    handlers: bool,
}

pub struct Pending {
    token: u64,
    kind: Kind,
    selection: Selection,
    width: u32,
    at: Instant,
}

#[derive(Default)]
pub struct State {
    pub kind: Option<Kind>,
    pub model: Option<Arc<Model>>,
    pub bounds: D2D_RECT_F,
    pub revision: u64,
    key: Option<Key>,
    token: u64,
    pending: Option<Pending>,
    timer: bool,
    lines: Vec<String>,
    pub hover: Option<Button>,
    pressed: Option<Button>,
    pub focus: Option<Button>,
    dragging: Option<(f32, f32)>,
    pub width: f32,
}

impl State {
    pub fn sync(&mut self, list: &mut super::list::List, size: (f32, f32), left: f32) -> bool {
        let Some(folder) = list.folder.as_mut() else {
            return false;
        };
        let pane_width = if self.width == 0.0 {
            302.0
        } else {
            self.width.clamp(203.0, (size.0 - left - 132.0).max(203.0))
        };
        if folder.pane_width != pane_width {
            Arc::make_mut(folder).pane_width = pane_width;
        }
        let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
        let kind = if crate::app::ids::is_settings(path)
            || crate::shell::browser::handles(path)
            || size.0 - left < 335.0
        {
            None
        } else if folder.panes.1 {
            Some(Kind::Preview)
        } else if folder.panes.0 {
            Some(Kind::Details)
        } else {
            None
        };
        let bounds = super::address::rect(
            size.0 - pane_width,
            super::details::top(),
            pane_width,
            (size.1
                - super::details::top()
                - if folder.settings.status_bar {
                    24.0
                } else {
                    0.0
                })
            .max(0.0),
        );
        let geometry_changed = self.bounds != bounds || self.kind != kind;
        self.bounds = bounds;
        self.kind = kind;
        let Some(kind) = kind else {
            self.key = None;
            self.model = None;
            self.pending = None;
            return geometry_changed;
        };
        let selected = &list.interaction.selection.selected;
        let entry = if selected.len() == 1 {
            folder
                .order
                .iter()
                .map(|&index| &folder.entries[index])
                .find(|entry| selected.contains(&entry.path))
        } else {
            None
        };
        let parent = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
        let key = Key {
            kind,
            parent: parent.into(),
            item: entry.map(|entry| (entry.path.clone(), entry.modified, entry.size, entry.icon)),
            count: if selected.is_empty() {
                folder.order.len()
            } else {
                selected.len()
            },
            selected: selected.len(),
            enumerations: list.enumerations,
            width: (bounds.right - bounds.left - 3.0).max(1.0) as u32,
            icons_only: folder.settings.icons_only,
            handlers: folder.settings.preview_handlers,
        };
        if self.key.as_ref() != Some(&key) {
            let selection = if let Some(entry) = entry {
                Selection::Item {
                    parent: parent.into(),
                    entry: entry.clone(),
                }
            } else if !selected.is_empty() {
                Selection::Multiple(selected.len())
            } else {
                Selection::Folder {
                    path: parent.into(),
                    name: folder
                        .crumbs
                        .last()
                        .map_or("", |crumb| crumb.name.as_str())
                        .into(),
                    icon: folder.crumbs.last().map_or(0, |crumb| crumb.icon),
                    count: folder.order.len(),
                }
            };
            self.token = self.token.wrapping_add(1);
            self.pending = Some(Pending {
                token: self.token,
                kind,
                selection,
                width: key.width,
                at: Instant::now(),
            });
            self.key = Some(key);
            self.model = None;
        }
        let mut changed = geometry_changed;
        if let Some((token, model)) = list.pane_model.take() {
            if token == self.token {
                self.lines = crate::shell::text::wrap_whole_words(
                    &model.name,
                    "Segoe UI Variable Text",
                    16.0,
                    true,
                    bounds.right
                        - bounds.left
                        - 3.0
                        - if model.icon.is_some() { 83.0 } else { 43.0 },
                );
                self.model = Some(model);
                self.revision = self.revision.wrapping_add(1);
                changed = true;
            }
        }
        changed
    }
}

pub fn after_paint(hwnd: HWND) {
    let action = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let state = &mut slot.as_mut()?.panes;
        if state.pending.is_some() && !state.timer {
            state.timer = true;
            Some(true)
        } else if state.pending.is_none() && state.timer {
            state.timer = false;
            Some(false)
        } else {
            None
        }
    });
    sync_preview(hwnd);
    match action {
        Some(true) => super::input::arm(hwnd, LOAD, 60),
        Some(false) => super::input::stop(hwnd, LOAD),
        None => {}
    }
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id != LOAD {
        return false;
    }
    super::input::stop(hwnd, LOAD);
    let remaining = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        window.panes.timer = false;
        let pending = window.panes.pending.take()?;
        if pending.at.elapsed().as_millis() < 60 {
            let remaining = 60 - pending.at.elapsed().as_millis() as u32;
            window.panes.pending = Some(pending);
            window.panes.timer = true;
            return Some(remaining.max(1));
        }
        if let Err(error) = window.list.request_pane(
            pending.token,
            pending.kind,
            pending.selection,
            pending.width,
        ) {
            eprintln!("native pane request: {error}");
        }
        None
    });
    if let Some(remaining) = remaining {
        super::input::arm(hwnd, LOAD, remaining);
    }
    true
}

fn label(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    style: TextStyle,
    bounds: D2D_RECT_F,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    let value = text.elide(value, style, (bounds.right - bounds.left).max(1.0))?;
    let metrics = text.layout(&value, style, 4096.0)?;
    text.draw(
        canvas,
        &value,
        style,
        (
            bounds.left,
            bounds.top + (bounds.bottom - bounds.top - metrics.height) / 2.0,
        ),
        color,
    )
}

pub fn draw(
    state: &State,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    assets: &super::svg::Assets,
) -> Result<()> {
    let width = state.bounds.right - state.bounds.left;
    let height = state.bounds.bottom - state.bounds.top;
    for (x, color) in [
        (0.0, theme.splitter_outer),
        (1.0, theme.splitter_mid),
        (2.0, theme.splitter_outer),
    ] {
        super::details::fill(canvas, super::address::rect(x, 0.0, 1.0, height), &color);
    }
    let body = width - 3.0;
    if state.kind == Some(Kind::Preview) {
        super::details::fill(
            canvas,
            super::address::rect(3.0, 0.0, body, height),
            &theme.preview_pane,
        );
        if let Some(model) = &state.model {
            if let Some(image) = &model.picture {
                let bitmap = canvas.shared_icon(image)?;
                let x = (3.0 + (body - image.width as f32) / 2.0 + 5.0).floor();
                let y = ((height - image.height as f32) / 2.0 - 2.5).round();
                unsafe {
                    let shadow = canvas.context.CreateEffect(&CLSID_D2D1Shadow)?;
                    shadow.SetInput(0, &*bitmap, true);
                    shadow.SetValue(0, D2D1_PROPERTY_TYPE_FLOAT, &2.0f32.to_ne_bytes())?;
                    let color = [0.0f32, 0.0, 0.0, 0.4]
                        .into_iter()
                        .flat_map(f32::to_ne_bytes)
                        .collect::<Vec<_>>();
                    shadow.SetValue(1, D2D1_PROPERTY_TYPE_VECTOR4, &color)?;
                    let mut offset = D2D1_ELLIPSE::default().point;
                    offset.X = x + 2.0;
                    offset.Y = y + 2.0;
                    canvas.context.DrawImage(
                        &shadow.GetOutput()?,
                        Some(&offset),
                        None,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                    canvas.context.DrawBitmap(
                        &*bitmap,
                        Some(&super::address::rect(
                            x,
                            y,
                            image.width as f32 + 1.0,
                            image.height as f32 + 1.0,
                        )),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                }
            }
            let value = &model.preview_message;
            let metrics = text.layout(value, super::text::DIRECTUI, 4096.0)?;
            text.draw(
                canvas,
                value,
                super::text::DIRECTUI,
                (
                    3.0 + (body - metrics.advance) / 2.0,
                    (height - metrics.height) / 2.0 + 6.0,
                ),
                &theme.dui_text,
            )?;
        }
        return Ok(());
    }
    super::details::fill(
        canvas,
        super::address::rect(3.0, 0.0, body, 220.0),
        &theme.pane,
    );
    let Some(model) = &state.model else {
        return Ok(());
    };
    if let Some(image) = &model.picture {
        let bitmap = canvas.shared_icon(image)?;
        let (w, h, x, y) = if model.large_picture {
            let scale = (body / image.width as f32).min(220.0 / image.height as f32);
            let w = image.width as f32 * scale;
            let h = image.height as f32 * scale;
            (
                w.round(),
                h.round(),
                ((body - w) / 2.0).round(),
                ((220.0 - h) / 2.0).round(),
            )
        } else {
            (96.0, 96.0, ((body - 96.0) / 2.0).round(), 62.0)
        };
        unsafe {
            canvas.context.DrawBitmap(
                &*bitmap,
                Some(&super::address::rect(3.0 + x, y, w, h)),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
        }
    }
    let lines = state.lines.len().max(1);
    let shift = if lines <= 1 {
        0.0
    } else if model.icon.is_some() {
        (lines as f32 * 21.25 + 6.5 - 32.0).max(0.0).round()
    } else {
        ((lines - 1) as f32 * 21.25).round()
    };
    if let Some(image) = &model.icon {
        let bitmap = canvas.shared_icon(image)?;
        unsafe {
            canvas.context.DrawBitmap(
                &*bitmap,
                Some(&super::address::rect(
                    24.0,
                    244.0 + (shift / 2.0).round(),
                    28.0,
                    28.0,
                )),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
        }
    }
    for (index, line) in state.lines.iter().enumerate() {
        let x = 3.0
            + if model.icon.is_some() {
                61.0 - 1.0 / 3.0
            } else {
                20.2
            };
        let title_style = if model.icon.is_some() {
            TITLE
        } else {
            TITLE.with_ideal_advances()
        };
        if lines == 1 {
            label(
                canvas,
                text,
                line,
                title_style,
                super::address::rect(x, 241.0, body - x - 17.0, 32.0),
                &theme.text,
            )?;
        } else {
            let value = text.elide(line, TITLE, (body - x - 17.0).max(1.0))?;
            text.draw(
                canvas,
                &value,
                TITLE,
                (x, 246.75 + index as f32 * 21.25),
                &theme.text,
            )?;
        }
    }
    if !model.info.is_empty() {
        let lines = crate::shell::text::wrap_whole_words(
            &model.info,
            "Segoe UI Variable Text",
            14.0,
            false,
            (body - 97.0).max(1.0),
        );
        let pitch = 56.0 / 3.0;
        let top = 284.0 + shift;
        let rect = super::address::rect(24.0, top, body - 42.0, lines.len() as f32 * pitch + 26.0);
        super::address::rounded(canvas, rect, 4.0, &theme.card);
        unsafe {
            canvas.brush.SetColor(&theme.card_border);
            canvas.context.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: rect.left + 0.5,
                        top: rect.top + 0.5,
                        right: rect.right - 0.5,
                        bottom: rect.bottom - 0.5,
                    },
                    radiusX: 3.5,
                    radiusY: 3.5,
                },
                &canvas.brush,
                1.0,
                None,
            );
        }
        label(
            canvas,
            text,
            "\u{F167}",
            TextStyle::new(Family::Icons, 16.0, 400),
            super::address::rect(37.0, top + 1.0, 16.0, rect.bottom - top),
            &theme.text_secondary,
        )?;
        text.draw(
            canvas,
            &lines.join("\n"),
            INFO,
            (65.0, top + 14.0),
            &theme.text,
        )?;
    }
    if model.can_share {
        button(
            canvas,
            text,
            theme,
            assets,
            "Windows.ModernShare",
            crate::i18n::tr("cmd.share"),
            288.0 + shift,
            button_state(state, Button::Share),
        )?;
    }
    let details = if model.can_share { shift } else { shift - 65.0 };
    if !model.rows.is_empty() {
        text.draw(
            canvas,
            crate::i18n::tr("cmd.details"),
            HEADING,
            (24.0, 349.0 + details),
            &theme.text,
        )?;
    }
    for (index, row) in model.rows.iter().enumerate() {
        let top = 375.0 + details + index as f32 * 22.0;
        label(
            canvas,
            text,
            &row.label,
            SMALL,
            super::address::rect(24.0, top, ((body - 42.0) / 3.0).ceil(), 20.0),
            &theme.text_secondary,
        )?;
        let width = (body - 134.0).max(1.0);
        let value = text.elide(&row.value, SMALL, width)?;
        let metrics = text.layout(&value, SMALL, 4096.0)?;
        label(
            canvas,
            text,
            &value,
            SMALL,
            super::address::rect((body - 18.0 - metrics.advance).max(116.0), top, width, 20.0),
            &theme.text_secondary,
        )?;
    }
    if model.can_properties {
        button(
            canvas,
            text,
            theme,
            assets,
            "windows.properties",
            crate::i18n::tr("menu.properties"),
            390.0 + details + model.rows.len() as f32 * 22.0,
            button_state(state, Button::Properties),
        )?;
    }
    Ok(())
}

fn button(
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    assets: &super::svg::Assets,
    icon: &'static str,
    value: &str,
    y: f32,
    state: (bool, bool, bool),
) -> Result<()> {
    let width = 48.0 + text.layout(value, SMALL, 4096.0)?.advance.ceil();
    let bounds = super::address::rect(24.0, y, width, 29.0);
    let stops: Vec<_> = theme
        .control_elevation_border
        .iter()
        .map(|&(position, color)| D2D1_GRADIENT_STOP { position, color })
        .collect();
    unsafe {
        let target: &ID2D1RenderTarget = &canvas.context;
        let stops =
            target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let mut properties = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default();
        properties.startPoint.Y = y;
        properties.endPoint.Y = y + 29.0;
        let gradient = target.CreateLinearGradientBrush(&properties, None, &stops)?;
        canvas.context.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bounds,
                radiusX: 4.0,
                radiusY: 4.0,
            },
            &gradient,
        );
    }
    super::address::rounded(
        canvas,
        super::address::rect(25.0, y + 1.0, width - 2.0, 27.0),
        3.0,
        &if state.1 {
            theme.control_pressed
        } else if state.0 {
            theme.control_hover
        } else {
            theme.control
        },
    );
    assets.icon(canvas, icon, (36.0, y + 6.0), 1.0)?;
    label(
        canvas,
        text,
        value,
        SMALL,
        super::address::rect(60.0, y, width - 48.0, 29.0),
        &if state.1 {
            theme.text_secondary
        } else {
            theme.text
        },
    )?;
    if state.2 {
        super::chrome::focus_ring(
            canvas,
            super::address::rect(21.0, y - 3.0, width + 6.0, 35.0),
            theme,
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Share,
    Properties,
}

fn title_shift(state: &State, model: &Model) -> f32 {
    let lines = state.lines.len().max(1);
    if lines <= 1 {
        0.0
    } else if model.icon.is_some() {
        (lines as f32 * 21.25 + 6.5 - 32.0).max(0.0).round()
    } else {
        ((lines - 1) as f32 * 21.25).round()
    }
}

fn button_state(state: &State, button: Button) -> (bool, bool, bool) {
    (
        state.hover == Some(button),
        state.pressed == Some(button) && state.hover == Some(button),
        state.focus == Some(button),
    )
}

fn hit(state: &State, text: &mut TextEngine, point: (f32, f32)) -> Option<Button> {
    if state.kind != Some(Kind::Details) {
        return None;
    }
    let model = state.model.as_ref()?;
    let shift = title_shift(state, model);
    let x = point.0 - state.bounds.left;
    let y = point.1 - state.bounds.top;
    for (button, visible, label, top) in [
        (
            Button::Share,
            model.can_share,
            crate::i18n::tr("cmd.share"),
            288.0 + shift,
        ),
        (
            Button::Properties,
            model.can_properties,
            crate::i18n::tr("menu.properties"),
            390.0 + shift - if model.can_share { 0.0 } else { 65.0 }
                + model.rows.len() as f32 * 22.0,
        ),
    ] {
        if !visible {
            continue;
        }
        let width = match text.layout(label, SMALL, 4096.0) {
            Ok(value) => 48.0 + value.advance.ceil(),
            Err(error) => {
                eprintln!("pane button geometry: {error}");
                continue;
            }
        };
        if x >= 24.0 && x < 24.0 + width && y >= top && y < top + 29.0 {
            return Some(button);
        }
    }
    None
}

pub fn invalidate(hwnd: HWND) {
    super::invalidate(
        hwnd,
        super::render::Damage {
            pane: true,
            ..super::render::Damage::NONE
        },
    );
}

pub fn moved(hwnd: HWND, point: (f32, f32)) -> bool {
    let (changed, resized, dragging) = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return (false, false, false);
        };
        let state = &mut window.panes;
        let hover = hit(state, &mut window.text, point);
        let changed = state.hover != hover;
        state.hover = hover;
        let mut resized = false;
        if let Some((start, width)) = state.dragging {
            let next = (width + start - point.0).clamp(
                203.0,
                (window.address.width - window.navigation.left() - 132.0).max(203.0),
            );
            resized = next != state.width;
            state.width = next;
        }
        (changed, resized, state.dragging.is_some())
    });
    if resized {
        super::invalidate_all(hwnd);
    } else if changed {
        invalidate(hwnd);
    }
    dragging
}

pub fn leave(hwnd: HWND) {
    let changed = super::WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|window| window.panes.hover.take().is_some())
    });
    if changed {
        invalidate(hwnd);
    }
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    let result = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let state = &mut window.panes;
        if state.kind.is_none()
            || point.0 < state.bounds.left
            || point.0 >= state.bounds.right
            || point.1 < state.bounds.top
            || point.1 >= state.bounds.bottom
        {
            return None;
        }
        if point.0 < state.bounds.left + 3.0 {
            state.dragging = Some((point.0, state.bounds.right - state.bounds.left));
        }
        state.pressed = hit(state, &mut window.text, point);
        Some((state.dragging.is_some(), state.pressed))
    });
    let Some((dragging, button)) = result else {
        return false;
    };
    if let Some(button) = button {
        super::chrome::set_focus(hwnd, super::chrome::Focus::Pane(button));
        super::WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.chrome.cues = false;
                window.panes.focus = None;
            }
        });
    }
    unsafe {
        if let Err(error) = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd)) {
            eprintln!("pane focus: {error}");
        }
    }
    if dragging || button.is_some() {
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
        }
    }
    invalidate(hwnd);
    true
}

pub fn release(hwnd: HWND, point: (f32, f32)) -> bool {
    let result = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let state = &mut window.panes;
        let dragging = state.dragging.take().is_some();
        let pressed = state.pressed.take();
        let hover = hit(state, &mut window.text, point);
        Some((
            dragging || pressed.is_some(),
            pressed.filter(|&button| Some(button) == hover),
        ))
    });
    let Some((used, button)) = result else {
        return false;
    };
    if !used {
        return false;
    }
    if let Err(error) = super::release_capture(hwnd) {
        eprintln!("pane release: {error}");
    }
    invalidate(hwnd);
    if let Some(button) = button {
        activate(hwnd, button);
    }
    true
}

pub fn cancel(hwnd: HWND) {
    let changed = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let dragging = window.panes.dragging.take().is_some();
            let pressed = window.panes.pressed.take().is_some();
            dragging || pressed
        })
    });
    if changed {
        invalidate(hwnd);
    }
}

pub fn cursor() -> bool {
    let resize = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            let state = &window.panes;
            if state.dragging.is_some() {
                return true;
            }
            let Some(point) = window.list.interaction.pointer else {
                return false;
            };
            state.kind.is_some()
                && point.0 >= state.bounds.left
                && point.0 < state.bounds.left + 3.0
                && point.1 >= state.bounds.top
                && point.1 < state.bounds.bottom
        })
    });
    if resize {
        unsafe {
            match windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
                None,
                windows::Win32::UI::WindowsAndMessaging::IDC_SIZEWE,
            ) {
                Ok(cursor) => {
                    windows::Win32::UI::WindowsAndMessaging::SetCursor(Some(cursor));
                }
                Err(error) => eprintln!("pane resize cursor: {error}"),
            }
        }
    }
    resize
}

pub fn activate(hwnd: HWND, button: Button) {
    let Some(context) = super::menu_actions::context(hwnd) else {
        return;
    };
    let paths: Vec<_> = context
        .selected
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let result = match button {
        Button::Share => super::menu_actions::file(
            hwnd,
            &context.path,
            &paths,
            crate::app::commands::FileAction::Share,
        ),
        Button::Properties => {
            crate::shell::ops::invoke_verb(hwnd, &context.path, &paths, "properties", false)
                .map_err(|error| {
                    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                })
        }
    };
    if let Err(error) = result {
        eprintln!("pane action: {error}");
    }
}

struct HostedPreview {
    token: u64,
    path: String,
    rect: windows::Win32::Foundation::RECT,
    preview: Option<crate::shell::preview::Preview>,
}

thread_local! {
    static PREVIEWS: std::cell::RefCell<std::collections::HashMap<isize, HostedPreview>> = Default::default();
    static PREVIEW_BUSY: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
}

fn sync_preview(hwnd: HWND) {
    let owner = hwnd.0 as isize;
    if PREVIEW_BUSY.with(|busy| !busy.borrow_mut().insert(owner)) {
        return;
    }
    let desired = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let state = &window.panes;
        if state.kind != Some(Kind::Preview) {
            return None;
        }
        let path = state.model.as_ref()?.preview_path.clone()?;
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
        Some((
            state.token,
            path,
            windows::Win32::Foundation::RECT {
                left: ((state.bounds.left + 3.0) * scale).round() as i32,
                top: (state.bounds.top * scale).round() as i32,
                right: (state.bounds.right * scale).round() as i32,
                bottom: (state.bounds.bottom * scale).round() as i32,
            },
        ))
    });
    let mut hosted = PREVIEWS.with(|previews| previews.borrow_mut().remove(&owner));
    if let Some((token, path, rect)) = desired {
        let same = hosted
            .as_ref()
            .is_some_and(|host| host.token == token && host.path == path);
        if same {
            if let Some(host) = &mut hosted {
                if host.rect != rect {
                    if let Some(preview) = &host.preview {
                        preview.set_rect(rect);
                    }
                    host.rect = rect;
                }
            }
        } else {
            drop(hosted.take());
            let preview = match crate::shell::preview::open(hwnd, rect, &path) {
                Ok(preview) => Some(preview),
                Err(error) => {
                    eprintln!("native preview {path}: {error}");
                    super::WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow_mut().as_mut() {
                            if window.panes.token == token {
                                if let Some(model) = &mut window.panes.model {
                                    Arc::make_mut(model).preview_message =
                                        crate::i18n::tr("preview.none").into();
                                }
                            }
                        }
                    });
                    invalidate(hwnd);
                    None
                }
            };
            hosted = Some(HostedPreview {
                token,
                path,
                rect,
                preview,
            });
        }
    } else {
        drop(hosted.take());
    }
    if unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)) }.as_bool() {
        if let Some(host) = hosted {
            PREVIEWS.with(|previews| previews.borrow_mut().insert(owner, host));
        }
    }
    PREVIEW_BUSY.with(|busy| busy.borrow_mut().remove(&owner));
}

pub fn close(hwnd: HWND) {
    let host = PREVIEWS.with(|previews| previews.borrow_mut().remove(&(hwnd.0 as isize)));
    drop(host);
}

pub fn contains(hwnd: HWND) -> bool {
    PREVIEWS.with(|previews| {
        previews.borrow().values().any(|host| {
            host.preview
                .as_ref()
                .is_some_and(|preview| preview.contains(hwnd))
        })
    })
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    if key != 0x50
        || !unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0 }
        || super::input::modifiers().ctrl
    {
        return false;
    }
    crate::app::commands::toggle_pane(if super::input::modifiers().shift {
        crate::app::commands::Pane::Details
    } else {
        crate::app::commands::Pane::Preview
    });
    if super::input::update_settings() {
        super::invalidate_all(hwnd);
    }
    true
}
