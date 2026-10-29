use super::render::Canvas;
use super::text::{TextEngine, CAPTION_GLYPH, TAB_GLYPH, TAB_TITLE};
use super::theme::Theme;
use crate::frame::Button;
use crate::frame::{CAPTION_H, CAPTION_W};
use windows::core::Result;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

pub const HEIGHT: f32 = 40.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    None,
    Tab(usize),
    TabClose(usize),
    Plus,
    Caption(Button),
}

pub struct TitleBar {
    pub busy: Option<f32>,
    pub title: String,
    pub icon: crate::shell::icons::Rgba,
    pub active: bool,
    pub maximized: bool,
    pub hover: Target,
    pub focus: Target,
    pub pressed: Target,
    pub items: Vec<Item>,
    pub current: usize,
    pub drag: Option<crate::app::tabs::Drag>,
    pub cancelled_drag: bool,
    pub external_ghost: bool,
    pub incoming: Option<usize>,
    pub file_gap: bool,
}

#[derive(Clone)]
pub struct Item {
    pub title: String,
    pub icon: crate::shell::icons::Rgba,
    pub settings: bool,
}

impl Item {
    pub fn new(title: String, icon: crate::shell::icons::Rgba) -> Self {
        Self {
            title,
            icon,
            settings: false,
        }
    }
}

pub struct Assets {
    icons: Vec<(Vec<u8>, std::rc::Rc<ID2D1Bitmap1>)>,
    tab: Option<(f32, ID2D1PathGeometry, ID2D1PathGeometry)>,
    focus_shape: Option<(f32, ID2D1PathGeometry, ID2D1PathGeometry)>,
}

struct SmallButton<'a> {
    target: Target,
    position: (f32, f32),
    glyph: &'a str,
    opacity: f32,
}

pub fn tabs_end(width: f32, count: usize) -> f32 {
    4.0 + count as f32 * (crate::app::tabs::width(width, count) - 8.0) + 43.0
}

impl TitleBar {
    pub fn relabel(&mut self, title: String, icon: crate::shell::icons::Rgba) {
        self.title = title.clone();
        self.icon = icon.clone();
        if let Some(item) = self.items.get_mut(self.current) {
            item.title = title;
            item.icon = icon;
        }
    }
    pub fn new(path: &str) -> Result<Self> {
        let name = crate::app::ids::search_parts(path).map_or_else(
            || {
                if crate::app::ids::is_settings(path) {
                    crate::i18n::tr("settings.title").into()
                } else {
                    crate::shell::listing::display_name_of(path)
                }
            },
            |(scope, _)| {
                crate::i18n::shell_string(34132, &crate::shell::listing::display_name_of(scope))
            },
        );
        let title = crate::app::tabs::label(path, &name, &crate::shell::listing::settings());
        let icon = if crate::app::ids::is_settings(path) {
            super::settings::icon()
        } else {
            crate::shell::icons::icon(
                if crate::app::ids::is_search(path) {
                    crate::shell::icons::find_icon()
                } else {
                    crate::shell::icons::file_icon(path)
                },
                16,
            )
            .map_err(|error| windows::core::Error::new(E_FAIL, error))?
        };
        let mut item = Item::new(title.clone(), icon.clone());
        item.settings = crate::app::ids::is_settings(path);
        Ok(Self {
            busy: None,
            items: vec![item],
            current: 0,
            drag: None,
            cancelled_drag: false,
            external_ghost: false,
            incoming: None,
            file_gap: false,
            title,
            icon,
            active: true,
            maximized: false,
            hover: Target::None,
            focus: Target::None,
            pressed: Target::None,
        })
    }

    fn slot(&self, index: usize) -> usize {
        index + usize::from(self.incoming.is_some_and(|slot| index >= slot))
    }

    fn count(&self) -> usize {
        self.items.len() + usize::from(self.incoming.is_some() || self.file_gap)
    }

    pub fn hit(&self, width: f32, x: f32, y: f32) -> Target {
        if super::details::top() == 0.0 {
            return Target::None;
        }
        if x < 0.0 || x >= width || y < 0.0 || y >= HEIGHT {
            return Target::None;
        }
        if (0.0..CAPTION_H).contains(&y) {
            if x >= width - CAPTION_W[2] {
                return Target::Caption(Button::Close);
            }
            if x >= width - CAPTION_W[1..].iter().sum::<f32>() {
                return Target::Caption(Button::Max);
            }
            if x >= width - CAPTION_W.iter().sum::<f32>() {
                return Target::Caption(Button::Min);
            }
        }
        let tab = crate::app::tabs::width(width, self.count());
        let pitch = tab - 8.0;
        let plus = 15.0 + self.count() as f32 * pitch;
        if (13.0..37.0).contains(&y) && (plus..plus + 32.0).contains(&x) {
            return Target::Plus;
        }
        for index in 0..self.items.len() {
            let left = self.slot(index) as f32 * pitch;
            if (12.0..36.0).contains(&y) && (left + tab - 36.0..left + tab - 4.0).contains(&x) {
                return Target::TabClose(index);
            }
            if (8.0..40.0).contains(&y) && (left + 8.0..left + tab).contains(&x) {
                return Target::Tab(index);
            }
        }
        Target::None
    }

    pub fn draw(
        &mut self,
        assets: &mut Assets,
        canvas: &Canvas,
        text: &mut TextEngine,
        theme: &Theme,
        width: f32,
        scale: f32,
    ) -> Result<()> {
        let count = self.count();
        let tab = crate::app::tabs::width(width, count);
        let pitch = tab - 8.0;
        let plus = 15.0 + count as f32 * pitch;
        while assets.icons.len() > count {
            assets.icons.pop();
        }
        for (index, item) in self.items.iter_mut().enumerate() {
            if index == assets.icons.len() {
                assets
                    .icons
                    .push((item.icon.pixels.clone(), canvas.shared_icon(&item.icon)?));
            } else if assets.icons[index].0 != item.icon.pixels {
                assets.icons[index] = (item.icon.pixels.clone(), canvas.shared_icon(&item.icon)?);
            }
        }
        if assets.tab.as_ref().is_none_or(|(old, _, _)| *old != tab) {
            assets.tab = Some((
                tab,
                tab_geometry(canvas, tab, &[0.0])?,
                tab_geometry(canvas, tab, &[0.0, 1.0])?,
            ));
        }
        unsafe {
            let mut original = Default::default();
            canvas.context.GetTransform(&mut original);
            let mut moved = original;
            moved.M31 += self.slot(self.current) as f32 * pitch;
            canvas.context.SetTransform(&moved);
            canvas.brush.SetColor(&theme.layer);
            if !self.drag.is_some_and(|drag| drag.active) {
                canvas
                    .context
                    .FillGeometry(&assets.tab.as_ref().unwrap().1, &canvas.brush, None);
            }
            if theme.tab_border.a > 0.0 {
                canvas.brush.SetColor(&theme.tab_border);
                canvas
                    .context
                    .FillGeometry(&assets.tab.as_ref().unwrap().2, &canvas.brush, None);
            }
            canvas.context.SetTransform(&original);
            if theme.tab_border.a > 0.0 {
                canvas.brush.SetColor(&theme.tab_border);
                for (left, right) in [
                    (0.0, 4.0 + self.slot(self.current) as f32 * pitch),
                    (self.slot(self.current) as f32 * pitch + tab + 4.0, width),
                ] {
                    canvas.context.FillRectangle(
                        &D2D_RECT_F {
                            left,
                            top: 39.0,
                            right,
                            bottom: 40.0,
                        },
                        &canvas.brush,
                    );
                }
            }
        }
        for (index, item) in self.items.iter().enumerate() {
            let drag = self.drag.filter(|drag| drag.active);
            if drag.is_some_and(|drag| drag.from == index) {
                continue;
            }
            let slot = drag.map_or(self.slot(index), |drag| drag.position(index));
            let left = slot as f32 * pitch;
            if left + tab < 0.0 || left >= width - CAPTION_W.iter().sum::<f32>() {
                continue;
            }
            let selected = index == self.current;
            let under = drag.is_some_and(|drag| {
                drag.pointer.0 >= 8.0 + left && drag.pointer.0 < 8.0 + left + pitch
            });
            let opacity = if drag.is_some() && !under { 0.5 } else { 1.0 };
            let settling = self.cancelled_drag && !selected;
            let icon_y = 16.0
                + if under {
                    1.0
                } else if settling {
                    0.5
                } else {
                    0.0
                };
            let shift = if under { 5.0 } else { 0.0 } + if selected { 0.0 } else { 1.0 };
            unsafe {
                if under {
                    canvas.brush.SetColor(&theme.tab_drag_hover);
                    canvas.context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: left + 15.0,
                                top: 9.0,
                                right: left + tab - 7.0,
                                bottom: 39.0,
                            },
                            radiusX: 8.0,
                            radiusY: 8.0,
                        },
                        &canvas.brush,
                    );
                }
                if drag.is_none()
                    && !selected
                    && matches!(self.hover,Target::Tab(at)|Target::TabClose(at) if at==index)
                {
                    canvas.brush.SetColor(&theme.subtle_pressed);
                    canvas.context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: left + 8.0,
                                top: 8.0,
                                right: left + tab,
                                bottom: 48.0,
                            },
                            radiusX: 8.0,
                            radiusY: 8.0,
                        },
                        &canvas.brush,
                    );
                }
                if index > 0 && !selected && index != self.current + 1 {
                    canvas
                        .brush
                        .SetColor(&opacity_color(theme.tab_divider, opacity));
                    canvas.context.FillRectangle(
                        &D2D_RECT_F {
                            left: left + 4.0,
                            top: 16.0,
                            right: left + 5.0,
                            bottom: 32.0,
                        },
                        &canvas.brush,
                    );
                }
                canvas.context.DrawBitmap(
                    assets.icons[index].1.as_ref(),
                    Some(&D2D_RECT_F {
                        left: left + 16.0 + shift,
                        top: icon_y,
                        right: left + 32.0 + shift,
                        bottom: icon_y + 16.0,
                    }),
                    if index == self.current && self.busy.is_some() {
                        0.0
                    } else {
                        opacity
                    },
                    if settling {
                        D2D1_INTERPOLATION_MODE_LINEAR
                    } else {
                        D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR
                    },
                    None,
                    None,
                );
            }
            if item.settings {
                text.draw(
                    canvas,
                    "\u{e713}",
                    super::text::TextStyle::new(super::text::Family::Icons, 16.0, 400),
                    (left + 16.0 + shift, icon_y),
                    &opacity_color(theme.text, opacity),
                )?;
            }
            let style = if selected {
                TAB_TITLE
            } else {
                super::text::TAB_INACTIVE
            };
            let room = (crate::app::tabs::title_room(tab) - 2.0 * shift).max(0.0);
            if room > 0.0 {
                let layout = text.layout(&item.title, style, 4096.0)?;
                let label_left = left + 42.0 + shift;
                unsafe {
                    canvas.context.PushAxisAlignedClip(
                        &D2D_RECT_F {
                            left: label_left,
                            top: 8.0,
                            right: label_left + room,
                            bottom: 40.0,
                        },
                        D2D1_ANTIALIAS_MODE_ALIASED,
                    );
                }
                let drawn = text.draw(
                    canvas,
                    &item.title,
                    style,
                    (label_left, 8.0 + (32.0 - layout.height) / 2.0),
                    &opacity_color(
                        if selected {
                            theme.text
                        } else {
                            theme.text_secondary
                        },
                        opacity,
                    ),
                );
                unsafe {
                    canvas.context.PopAxisAlignedClip();
                }
                drawn?;
            }
            if index == self.current {
                if let Some(phase) = self.busy {
                    super::search::draw_ring(
                        canvas,
                        theme,
                        (left + 24.0 + shift, icon_y + 8.0),
                        phase,
                    )?;
                }
            }
            self.small_button(
                canvas,
                text,
                theme,
                SmallButton {
                    target: Target::TabClose(index),
                    position: (
                        left + tab - 36.0 - shift,
                        12.0 + if settling { 0.5 } else { 0.0 },
                    ),
                    glyph: "\u{e711}",
                    opacity,
                },
                scale,
            )?;
            if self.focus == Target::Tab(index) {
                if assets
                    .focus_shape
                    .as_ref()
                    .is_none_or(|(old, _, _)| *old != tab)
                {
                    assets.focus_shape = Some((
                        tab,
                        tab_focus_geometry(canvas, tab, &[0.0, 2.0])?,
                        tab_focus_geometry(canvas, tab, &[2.0, 3.0])?,
                    ));
                }
                unsafe {
                    let mut original = Default::default();
                    canvas.context.GetTransform(&mut original);
                    let mut moved = original;
                    moved.M31 += left;
                    canvas.context.SetTransform(&moved);
                    canvas.brush.SetColor(&theme.focus_outer);
                    canvas.context.FillGeometry(
                        &assets.focus_shape.as_ref().unwrap().1,
                        &canvas.brush,
                        None,
                    );
                    canvas.brush.SetColor(&theme.focus_inner);
                    canvas.context.FillGeometry(
                        &assets.focus_shape.as_ref().unwrap().2,
                        &canvas.brush,
                        None,
                    );
                    canvas.context.SetTransform(&original);
                }
            }
        }
        if self.current + 1 < count {
            unsafe {
                canvas.brush.SetColor(&theme.tab_divider);
                let left = count as f32 * pitch + 8.0;
                canvas.context.FillRectangle(
                    &D2D_RECT_F {
                        left,
                        top: 16.0,
                        right: left + 1.0,
                        bottom: 32.0,
                    },
                    &canvas.brush,
                );
            }
        }
        self.small_button(
            canvas,
            text,
            theme,
            SmallButton {
                target: Target::Plus,
                position: (plus, 13.0 + if self.cancelled_drag { 0.5 } else { 0.0 }),
                glyph: "\u{e710}",
                opacity: 1.0,
            },
            scale,
        )?;
        if self.focus == Target::Plus {
            super::chrome::focus_ring(
                canvas,
                D2D_RECT_F {
                    left: plus - 3.0,
                    top: 10.0,
                    right: plus + 35.0,
                    bottom: 40.0,
                },
                theme,
            );
        }
        for (button, x, w, glyph) in [
            (
                Button::Min,
                width - CAPTION_W.iter().sum::<f32>(),
                CAPTION_W[0],
                "\u{e921}",
            ),
            (
                Button::Max,
                width - CAPTION_W[1..].iter().sum::<f32>(),
                CAPTION_W[1],
                if self.maximized {
                    "\u{e923}"
                } else {
                    "\u{e922}"
                },
            ),
            (
                Button::Close,
                width - CAPTION_W[2],
                CAPTION_W[2],
                "\u{e8bb}",
            ),
        ] {
            let target = Target::Caption(button);
            let hovered = self.hover == target;
            let pressed = hovered && self.pressed == target;
            let fill = if button == Button::Close && hovered {
                if pressed {
                    theme.caption_close_pressed
                } else {
                    theme.caption_close_hover
                }
            } else if pressed {
                theme.subtle_pressed
            } else if hovered {
                theme.subtle_hover
            } else {
                D2D1_COLOR_F::default()
            };
            unsafe {
                canvas.brush.SetColor(&fill);
                canvas.context.FillRectangle(
                    &D2D_RECT_F {
                        left: x + 1.0,
                        top: 1.0,
                        right: (x + w + 1.0).min(width),
                        bottom: CAPTION_H + 1.0,
                    },
                    &canvas.brush,
                );
            }
            let color = if button == Button::Close && hovered {
                theme.caption_close_glyph
            } else if self.active {
                theme.caption_glyph
            } else {
                theme.caption_glyph_inactive
            };
            let layout = text.layout(glyph, CAPTION_GLYPH, 4096.0)?;
            let gx = ((w - layout.width / scale) / 2.0).round()
                - if button == Button::Close { 0.0 } else { 1.0 };
            text.draw(
                canvas,
                glyph,
                CAPTION_GLYPH,
                (
                    x + gx,
                    (CAPTION_H - layout.height) / 2.0
                        + if button == Button::Min { 1.5 } else { 0.0 },
                ),
                &color,
            )?;
        }
        if let Some(drag) = self
            .drag
            .filter(|drag| drag.active && !drag.dropping && !self.external_ghost)
        {
            let left = drag.from as f32 * pitch + drag.pointer.0 - drag.origin.0;
            let top = drag.pointer.1 - drag.origin.1;
            let item = &self.items[drag.from];
            unsafe {
                canvas.brush.SetColor(&theme.tab_drag);
                canvas.context.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: left + 13.0,
                            top: top + 9.0,
                            right: left + tab + 3.0,
                            bottom: top + 39.0,
                        },
                        radiusX: 8.0,
                        radiusY: 8.0,
                    },
                    &canvas.brush,
                );
                canvas.context.DrawBitmap(
                    assets.icons[drag.from].1.as_ref(),
                    Some(&D2D_RECT_F {
                        left: left + 19.0,
                        top: top + 15.5,
                        right: left + 35.0,
                        bottom: top + 31.5,
                    }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            }
            if item.settings {
                text.draw(
                    canvas,
                    "\u{e713}",
                    super::text::TextStyle::new(super::text::Family::Icons, 16.0, 400),
                    (left + 19.0, top + 15.5),
                    &theme.text,
                )?;
            }
            let layout = text.layout(&item.title, TAB_TITLE, 4096.0)?;
            unsafe {
                canvas.context.PushAxisAlignedClip(
                    &D2D_RECT_F {
                        left: left + 45.0,
                        top: top + 8.0,
                        right: left + 45.0 + crate::app::tabs::title_room(tab),
                        bottom: top + 40.0,
                    },
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
            }
            let drawn = text.draw(
                canvas,
                &item.title,
                TAB_TITLE,
                (left + 45.0, top + 7.0 + (32.0 - layout.height) / 2.0),
                &theme.text,
            );
            unsafe {
                canvas.context.PopAxisAlignedClip();
            }
            drawn?;
            self.small_button(
                canvas,
                text,
                theme,
                SmallButton {
                    target: Target::None,
                    position: (left + tab - 31.0, top + 12.0),
                    glyph: "\u{e711}",
                    opacity: 1.0,
                },
                scale,
            )?;
        }
        Ok(())
    }

    fn small_button(
        &self,
        canvas: &Canvas,
        text: &mut TextEngine,
        theme: &Theme,
        button: SmallButton<'_>,
        scale: f32,
    ) -> Result<()> {
        let SmallButton {
            target,
            position,
            glyph,
            opacity,
        } = button;
        let hovered = target != Target::None
            && self.hover == target
            && !self.drag.is_some_and(|drag| drag.active);
        let pressed = hovered && self.pressed == target;
        if hovered {
            unsafe {
                canvas.brush.SetColor(if pressed {
                    &theme.subtle_pressed
                } else {
                    &theme.subtle_hover
                });
                canvas.context.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: position.0,
                            top: position.1,
                            right: position.0 + 32.0,
                            bottom: position.1 + 24.0,
                        },
                        radiusX: 4.0,
                        radiusY: 4.0,
                    },
                    &canvas.brush,
                );
            }
        }
        let layout = text.layout(glyph, TAB_GLYPH, 4096.0)?;
        text.draw(
            canvas,
            glyph,
            TAB_GLYPH,
            (
                position.0 + (32.0 - layout.width / scale) / 2.0,
                position.1 + (24.0 - layout.height) / 2.0,
            ),
            &opacity_color(
                if pressed {
                    theme.text_secondary
                } else {
                    theme.text
                },
                opacity,
            ),
        )
    }
}

impl Assets {
    pub fn new(canvas: &Canvas, source: &crate::shell::icons::Rgba) -> Result<Self> {
        let icon = canvas.shared_icon(source)?;
        Ok(Self {
            icons: vec![(source.pixels.clone(), icon)],
            tab: None,
            focus_shape: None,
        })
    }
}

fn tab_geometry(canvas: &Canvas, width: f32, insets: &[f32]) -> Result<ID2D1PathGeometry> {
    unsafe {
        let factory = canvas.context.GetFactory()?;
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
        for &inset in insets {
            let mut arc = D2D1_ARC_SEGMENT::default();
            arc.point.X = 8.0 + inset;
            arc.point.Y = 16.0;
            sink.BeginFigure(arc.point, D2D1_FIGURE_BEGIN_FILLED);
            for (x, y, radius, sweep) in [
                (
                    16.0,
                    8.0 + inset,
                    8.0 - inset,
                    D2D1_SWEEP_DIRECTION_CLOCKWISE,
                ),
                (
                    width - 8.0,
                    8.0 + inset,
                    0.0,
                    D2D1_SWEEP_DIRECTION_CLOCKWISE,
                ),
                (
                    width - inset,
                    16.0,
                    8.0 - inset,
                    D2D1_SWEEP_DIRECTION_CLOCKWISE,
                ),
                (width - inset, 36.0, 0.0, D2D1_SWEEP_DIRECTION_CLOCKWISE),
                (
                    width + 4.0,
                    40.0 + inset,
                    4.0 + inset,
                    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                ),
                (4.0, 40.0 + inset, 0.0, D2D1_SWEEP_DIRECTION_CLOCKWISE),
                (
                    8.0 + inset,
                    36.0,
                    4.0 + inset,
                    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                ),
            ] {
                arc.point.X = x;
                arc.point.Y = y;
                if radius == 0.0 {
                    sink.AddLine(arc.point);
                } else {
                    arc.size = D2D_SIZE_F {
                        width: radius,
                        height: radius,
                    };
                    arc.sweepDirection = sweep;
                    sink.AddArc(&arc);
                }
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        sink.Close()?;
        Ok(geometry)
    }
}

fn opacity_color(mut color: D2D1_COLOR_F, opacity: f32) -> D2D1_COLOR_F {
    color.a *= opacity;
    color
}

fn tab_focus_geometry(canvas: &Canvas, width: f32, insets: &[f32]) -> Result<ID2D1PathGeometry> {
    unsafe {
        let geometry = canvas.context.GetFactory()?.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
        for &inset in insets {
            let left = 7.0 + inset;
            let right = width + 1.0 - inset;
            let top = 8.0 + inset;
            let bottom = 39.0 - inset;
            let radius = 9.0 - inset;
            let mut arc = D2D1_ARC_SEGMENT::default();
            arc.point.X = left;
            arc.point.Y = bottom;
            sink.BeginFigure(arc.point, D2D1_FIGURE_BEGIN_FILLED);
            arc.point.Y = top + radius;
            sink.AddLine(arc.point);
            arc.point.X = left + radius;
            arc.point.Y = top;
            arc.size = D2D_SIZE_F {
                width: radius,
                height: radius,
            };
            arc.sweepDirection = D2D1_SWEEP_DIRECTION_CLOCKWISE;
            sink.AddArc(&arc);
            arc.point.X = right - radius;
            sink.AddLine(arc.point);
            arc.point.X = right;
            arc.point.Y = top + radius;
            sink.AddArc(&arc);
            arc.point.Y = bottom;
            sink.AddLine(arc.point);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        sink.Close()?;
        Ok(geometry)
    }
}
