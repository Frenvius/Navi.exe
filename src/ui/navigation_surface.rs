use std::collections::HashMap;
use windows::core::Result;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED,
};

use super::navigation::{Hit, State};
use super::render::{Canvas, Layer, Renderer};
use super::text::{TextEngine, DIRECTUI_KERNED as DIRECTUI};
use super::theme::Theme;

#[derive(Default)]
struct Assets {
    parts: Vec<ID2D1Bitmap1>,
    plates: Vec<crate::shell::icons::Rgba>,
    text: super::text::ClassicCache,
    icons: HashMap<u32, std::rc::Rc<ID2D1Bitmap1>>,
    pin: Option<ID2D1Bitmap1>,
    key: Option<(u32, u32, u32, bool)>,
}

impl Assets {
    fn prepare(&mut self, canvas: &Canvas, state: &State, theme: &Theme, dpi: u32) -> Result<()> {
        let key = (
            state.content_width().round() as u32,
            state.metrics().0 as u32,
            dpi,
            theme.content.r < 0.5,
        );
        if self.key != Some(key) {
            self.parts.clear();
            self.plates.clear();
            self.text = Default::default();
            for (part, part_state, dimensions) in [
                (1, 2, (state.content_width(), state.metrics().0)),
                (1, 5, (state.content_width(), state.metrics().0)),
                (1, 6, (state.content_width(), state.metrics().0)),
                (1, 3, (state.content_width(), state.metrics().0)),
                (2, 1, (16.0, 16.0)),
                (2, 2, (16.0, 16.0)),
                (4, 1, (16.0, 16.0)),
                (4, 2, (16.0, 16.0)),
            ] {
                let image = super::check::tree(part, part_state, dimensions, theme, dpi)?;
                self.parts.push(canvas.system_icon(&image)?);
                if part == 1 {
                    self.plates.push(image);
                }
            }
            self.key = Some(key);
        }
        if self.pin.is_none() {
            self.pin = Some(
                canvas.system_icon(&crate::shell::icons::Rgba {
                    width: 14,
                    height: 13,
                    pixels: crate::app::navtree::TREE_PIN
                        .iter()
                        .flat_map(|&alpha| [0x95, 0xa0, 0xa6, alpha])
                        .collect(),
                })?,
            );
        }
        self.icons
            .retain(|index, _| state.images.contains_key(index));
        for row in state.visible_rows() {
            let Some(node) = state.tree.node(&state.tree.rows[row].reference) else {
                continue;
            };
            if !self.icons.contains_key(&node.node.icon) {
                if let Some(image) = state.images.get(&node.node.icon) {
                    self.icons
                        .insert(node.node.icon, canvas.shared_icon(image)?);
                }
            }
        }
        Ok(())
    }
}

pub struct Surface {
    pub root: IDCompositionVisual,
    rows: Layer,
    bar: Layer,
    edges: Layer,
    left: IDCompositionVisual,
    valid: Option<RECT>,
    revision: usize,
    assets: Assets,
    bars: super::bars::Assets,
    pub rows_drawn: usize,
    rename: Option<(Layer, Option<u64>)>,
}

impl Surface {
    pub fn work(&self) -> (usize, usize) {
        (
            self.rows.presents + self.bar.presents + self.edges.presents,
            self.rows_drawn,
        )
    }

    pub fn new(renderer: &Renderer) -> Result<Self> {
        unsafe {
            let root = renderer.composition.CreateVisual()?;
            let rows = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let bar = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let edges = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let left = renderer.composition.CreateVisual()?;
            left.SetContent(&edges.surface)?;
            root.AddVisual(&rows.visual, true, None)?;
            root.AddVisual(&bar.visual, true, &rows.visual)?;
            root.AddVisual(&edges.visual, true, &bar.visual)?;
            root.AddVisual(&left, true, &edges.visual)?;
            Ok(Self {
                root,
                rows,
                bar,
                edges,
                left,
                valid: None,
                revision: 0,
                assets: Default::default(),
                bars: Default::default(),
                rows_drawn: 0,
                rename: None,
            })
        }
    }

    pub fn paint(
        &mut self,
        renderer: &Renderer,
        state: &State,
        text: &mut TextEngine,
        theme: &Theme,
        dpi: u32,
        damage: super::render::Damage,
    ) -> Result<()> {
        let scale = dpi as f32 / 96.0;
        unsafe {
            self.root.SetOffsetY2(super::details::top() * scale)?;
            self.root.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: state.left() * scale,
                bottom: state.height * scale,
            })?;
        }
        if !state.visible || state.height <= 0.0 || state.content_width() <= 0.0 {
            return Ok(());
        }
        self.assets.prepare(&renderer.canvas, state, theme, dpi)?;
        if let Some((origin, serial, image)) = super::rename::tree_image() {
            if self.rename.is_none() {
                let layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                unsafe {
                    self.root.AddVisual(&layer.visual, true, Some(&self.left))?;
                }
                self.rename = Some((layer, None));
            }
            let (layer, key) = self.rename.as_mut().unwrap();
            if *key != Some(serial) {
                layer.resize((image.width, image.height))?;
                renderer.update(layer, None, dpi, &theme.content, |canvas| {
                    let bitmap = canvas.system_icon(&image)?;
                    unsafe {
                        canvas.context.DrawBitmap(
                            &bitmap,
                            Some(&super::address::rect(
                                0.0,
                                0.0,
                                image.width as f32 / scale,
                                image.height as f32 / scale,
                            )),
                            1.0,
                            D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                            None,
                            None,
                        );
                    }
                    Ok(())
                })?;
                *key = Some(serial);
            }
            unsafe {
                layer.visual.SetOffsetX2(origin.0 * scale)?;
                layer.visual.SetOffsetY2(origin.1 * scale)?;
                layer.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: ((state.content_width() + 2.0 - origin.0) * scale).max(0.0),
                    bottom: image.height as f32,
                })?;
            }
        } else if let Some((layer, _)) = self.rename.take() {
            unsafe {
                self.root.RemoveVisual(&layer.visual)?;
                layer.surface.Trim(Some(&[]))?;
            }
        }
        let resized = self.rows.resize((
            (state.content_width() * scale).round().max(1.0) as u32,
            (((state.tree.rows.len() + 2) as f32 * state.metrics().0 + state.metrics().1)
                .max(state.height)
                * scale)
                .ceil()
                .max(1.0) as u32,
        ))?;
        if resized || damage.all || self.revision != state.revision {
            self.valid = None;
        }
        let visible = RECT {
            left: 0,
            top: (state.offset() * scale).round() as i32,
            right: self.rows.size.0 as i32,
            bottom: (((state.offset() + state.height) * scale).round() as i32)
                .min(self.rows.size.1 as i32),
        };
        self.rows_drawn = 0;
        let previous = self.valid;
        let mut updates = super::list_surface::exposed(visible, previous);
        if let (Some((first, last)), Some(previous)) = (state.dirty_rows, previous) {
            let rect = RECT {
                left: visible.left.max(previous.left),
                right: visible.right.min(previous.right),
                top: ((state.metrics().1 + first as f32 * state.metrics().0) * scale)
                    .floor()
                    .max(visible.top as f32)
                    .max(previous.top as f32) as i32,
                bottom: ((state.metrics().1 + last as f32 * state.metrics().0) * scale)
                    .ceil()
                    .min(visible.bottom as f32)
                    .min(previous.bottom as f32) as i32,
            };
            if rect.bottom > rect.top && rect.right > rect.left {
                updates.push(rect);
            }
        }
        for rect in updates {
            renderer.update(&mut self.rows, Some(rect), dpi, &theme.content, |canvas| {
                draw_rows(
                    canvas,
                    state,
                    &mut self.assets,
                    text,
                    theme,
                    rect,
                    scale,
                    &mut self.rows_drawn,
                )
            })?;
        }
        self.valid = Some(visible);
        self.revision = state.revision;
        unsafe {
            self.rows.visual.SetOffsetX2(2.0 * scale)?;
            self.rows.visual.SetOffsetY2(-state.offset() * scale)?;
            self.rows.surface.Trim(Some(&[RECT {
                left: 0,
                top: visible.top,
                right: visible.right,
                bottom: visible.bottom,
            }]))?;
        }
        if let Some(bar) = state.bar() {
            self.bar.resize((
                (17.0 * scale).round() as u32,
                (state.height * scale).round().max(1.0) as u32,
            ))?;
            let mut input = super::scroll::State::default();
            input.hover = if let Some(Hit::Bar(part)) = state.hover {
                Some(super::scroll::Hit {
                    axis: super::scroll::Axis::Vertical,
                    part,
                })
            } else {
                None
            };
            input.pressed = if let Some(Hit::Bar(part)) = state.pressed {
                Some(super::scroll::Hit {
                    axis: super::scroll::Axis::Vertical,
                    part,
                })
            } else {
                None
            };
            renderer.update(&mut self.bar, None, dpi, &theme.track, |canvas| {
                super::bars::draw(canvas, &mut self.bars, theme, &input, bar)
            })?;
            unsafe {
                self.bar
                    .visual
                    .SetOffsetX2((2.0 + state.width - 17.0) * scale)?;
                self.bar.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: 17.0 * scale,
                    bottom: state.height * scale,
                })?;
            }
        } else {
            unsafe {
                self.bar.visual.SetClip2(&D2D_RECT_F::default())?;
            }
        }
        let edges_resized = self.edges.resize((
            (6.0 * scale).round().max(1.0) as u32,
            (state.height * scale).round().max(1.0) as u32,
        ))?;
        if edges_resized || self.edges.presents == 0 || damage.all {
            renderer.update(&mut self.edges, None, dpi, &theme.content, |canvas| {
                super::details::fill(
                    canvas,
                    D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: 2.0,
                        bottom: state.height,
                    },
                    &theme.pane_edge,
                );
                for (index, color) in [
                    &theme.pane_edge,
                    &theme.splitter_outer,
                    &theme.splitter_mid,
                    &theme.splitter_outer,
                ]
                .into_iter()
                .enumerate()
                {
                    let x = 2.0 + index as f32;
                    super::details::fill(
                        canvas,
                        D2D_RECT_F {
                            left: x,
                            top: 0.0,
                            right: x + 1.0,
                            bottom: state.height,
                        },
                        color,
                    );
                }
                Ok(())
            })?;
        }
        unsafe {
            self.edges
                .visual
                .SetOffsetX2((state.left() - 6.0) * scale)?;
            self.edges.visual.SetClip2(&D2D_RECT_F {
                left: 2.0 * scale,
                top: 0.0,
                right: 6.0 * scale,
                bottom: state.height * scale,
            })?;
            self.left.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: 2.0 * scale,
                bottom: state.height * scale,
            })?;
        }
        Ok(())
    }
}

fn draw_rows(
    canvas: &Canvas,
    state: &State,
    assets: &mut Assets,
    text: &mut TextEngine,
    theme: &Theme,
    dirty: RECT,
    scale: f32,
    count: &mut usize,
) -> Result<()> {
    let (pitch, margin) = state.metrics();
    let first = (((dirty.top as f32 / scale - margin).max(0.0) / pitch).floor() as usize)
        .min(state.tree.rows.len());
    let last = (((dirty.bottom as f32 / scale - margin).max(0.0) / pitch).ceil() as usize)
        .min(state.tree.rows.len());
    for index in first..last {
        let row = &state.tree.rows[index];
        let y = margin + pitch * index as f32;
        let Some(node) = state.tree.node(&row.reference) else {
            super::details::fill(
                canvas,
                D2D_RECT_F {
                    left: 8.0,
                    top: y + pitch / 2.0,
                    right: state.content_width() - 8.0,
                    bottom: y + pitch / 2.0 + 1.0,
                },
                &theme.separator,
            );
            continue;
        };
        *count += 1;
        let hover = matches!(state.hover, Some(Hit::Row(row)|Hit::Expando(row)) if row == index);
        let selected = state.selected == Some(index);
        let plate = if state.drop_row == Some(index) {
            Some(3)
        } else if selected {
            Some(if matches!(state.pressed, Some(Hit::Row(_))) {
                3
            } else if hover || (state.focused && state.active) {
                2
            } else {
                1
            })
        } else if hover {
            Some(0)
        } else {
            None
        };
        if let Some(plate) = plate {
            unsafe {
                canvas.context.DrawBitmap(
                    &assets.parts[plate],
                    Some(&D2D_RECT_F {
                        left: 0.0,
                        top: y,
                        right: state.content_width(),
                        bottom: y + pitch,
                    }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
        }
        let indent = row.depth as f32 * 8.0 + if state.compact { 0.0 } else { 8.0 };
        if node.node.has_children && state.opacity > 0.0 {
            let hot = state.hover == Some(Hit::Expando(index));
            let glyph = 4 + if node.expanded { 1 } else { 0 } + if hot { 2 } else { 0 };
            unsafe {
                canvas.context.DrawBitmap(
                    &assets.parts[glyph],
                    Some(&D2D_RECT_F {
                        left: indent + 3.0,
                        top: y + (pitch - 16.0) / 2.0,
                        right: indent + 19.0,
                        bottom: y + (pitch + 16.0) / 2.0,
                    }),
                    state.opacity,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
        }
        if let Some(icon) = assets.icons.get(&node.node.icon) {
            unsafe {
                canvas.context.DrawBitmap(
                    icon.as_ref(),
                    Some(&D2D_RECT_F {
                        left: indent + 24.0,
                        top: y + (pitch - 16.0) / 2.0,
                        right: indent + 40.0,
                        bottom: y + (pitch + 16.0) / 2.0,
                    }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
        }
        let pinned = matches!(
            row.reference,
            crate::app::navtree::RowRef::Node {
                section: crate::app::navtree::PINS,
                ..
            }
        ) && node.node.is_pinned;
        let clip = D2D_RECT_F {
            left: indent + 45.0,
            top: y,
            right: state.content_width() - if pinned { 25.0 } else { 0.0 },
            bottom: y + pitch,
        };
        if clip.right > clip.left && !super::rename::tree_active(&node.node.id) {
            let layout = text.layout(&node.node.name, DIRECTUI, 4096.0)?;
            unsafe {
                canvas
                    .context
                    .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            }
            let drawn = if theme.content.r < 0.5 {
                text.draw(
                    canvas,
                    &node.node.name,
                    DIRECTUI,
                    (clip.left, y - 1.0 + (pitch - layout.height) / 2.0),
                    &theme.dui_text,
                )
            } else {
                assets.text.draw(
                    canvas,
                    text,
                    &node.node.name,
                    DIRECTUI,
                    (clip.left, y - 1.0 + (pitch - layout.height) / 2.0),
                    theme.dui_text,
                    super::text::ClassicBackground {
                        color: theme.content,
                        plate: plate.map(|index| &assets.plates[index]),
                        origin: (0.0, y),
                    },
                    clip,
                )
            };
            unsafe {
                canvas.context.PopAxisAlignedClip();
            }
            drawn?;
        }
        if pinned {
            let x = state.content_width() - 15.0;
            let top = y + ((pitch - 13.0) / 2.0).ceil();
            unsafe {
                canvas.context.DrawBitmap(
                    assets.pin.as_ref().unwrap(),
                    Some(&D2D_RECT_F {
                        left: x,
                        top,
                        right: x + 14.0,
                        bottom: top + 13.0,
                    }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
        }
    }
    if let Some(line) = state.insert_y {
        let y = line + state.offset();
        for bounds in [
            super::address::rect(25.0, y - 1.0, (state.content_width() - 29.0).max(0.0), 2.0),
            super::address::rect(25.0, y - 3.0, 2.0, 6.0),
            super::address::rect(state.content_width() - 6.0, y - 3.0, 2.0, 6.0),
        ] {
            super::details::fill(canvas, bounds, &theme.text);
        }
    }

    Ok(())
}
