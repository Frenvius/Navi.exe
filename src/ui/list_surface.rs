use windows::core::Result;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_IGNORE;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED;

use super::details::{self, Viewport};
use super::list::List;
use super::render::{Damage, Layer, Renderer};
use super::scroll::{Axis, State};
use super::text::TextEngine;
use super::theme::Theme;

pub struct Surface {
    pub root: IDCompositionVisual,
    pub presents: usize,
    pub rows_drawn: usize,
    pub pixels_drawn: usize,
    header: Layer,
    rows: Layer,
    vertical: Layer,
    horizontal: Layer,
    band: Layer,
    header_clip: IDCompositionVisual,
    rows_clip: IDCompositionVisual,
    valid: Option<RECT>,
    geometry: Option<std::sync::Arc<crate::app::group::DetailsLayout>>,
    grid: Option<std::sync::Arc<super::grid::Model>>,
    grid_assets: Option<super::grid::Assets>,
    timeline: Option<Layer>,
    rename: Option<(Layer, Option<u64>)>,
    row_assets: details::Assets,
    bar_assets: super::bars::Assets,
}

impl Surface {
    pub fn new(renderer: &Renderer) -> Result<Self> {
        unsafe {
            let root = renderer.composition.CreateVisual()?;
            let header = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let rows = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let vertical = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let horizontal = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let band = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            let header_clip = renderer.composition.CreateVisual()?;
            let rows_clip = renderer.composition.CreateVisual()?;
            header_clip.AddVisual(&header.visual, true, None)?;
            rows_clip.AddVisual(&rows.visual, true, None)?;
            root.AddVisual(&rows_clip, true, None)?;
            root.AddVisual(&header_clip, true, &rows_clip)?;
            root.AddVisual(&vertical.visual, true, &header_clip)?;
            root.AddVisual(&horizontal.visual, true, &vertical.visual)?;
            root.AddVisual(&band.visual, true, &horizontal.visual)?;
            Ok(Self {
                root,
                presents: 0,
                rows_drawn: 0,
                pixels_drawn: 0,
                header,
                rows,
                vertical,
                horizontal,
                band,
                header_clip,
                rows_clip,
                valid: None,
                geometry: None,
                grid: None,
                grid_assets: None,
                timeline: None,
                rename: None,
                row_assets: Default::default(),
                bar_assets: Default::default(),
            })
        }
    }

    pub fn paint(
        &mut self,
        renderer: &Renderer,
        size: (u32, u32),
        dpi: u32,
        list: &List,
        text: &mut TextEngine,
        theme: &Theme,
        scroll: &State,
        damage: Damage,
    ) -> Result<()> {
        let Some(folder) = &list.folder else {
            return Ok(());
        };
        if folder.awaiting_grid() {
            return Ok(());
        }
        if self.grid.is_some() && folder.grid.is_none() {
            let rows = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let header = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
            unsafe {
                self.rows_clip.RemoveVisual(&self.rows.visual)?;
                self.header_clip.RemoveVisual(&self.header.visual)?;
                self.rows.surface.Trim(Some(&[]))?;
                self.header.surface.Trim(Some(&[]))?;
                self.rows_clip.AddVisual(&rows.visual, true, None)?;
                self.header_clip.AddVisual(&header.visual, true, None)?;
            }
            self.rows = rows;
            self.header = header;
            self.grid = None;
            self.grid_assets = None;
            self.valid = None;
            self.geometry = None;
            text.release_layouts();
        }
        let scale = dpi as f32 / 96.0;
        let view = Viewport::new(
            (size.0 as f32 / scale, size.1 as f32 / scale),
            folder,
            list.navigation_width,
        );
        unsafe {
            self.root.SetOffsetX2(view.x * scale)?;
            self.root.SetOffsetY2(view.y * scale)?;
            self.root.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: view.width * scale,
                bottom: view.height * scale,
            })?;
            self.header_clip.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: view.right * scale,
                bottom: view.header * scale,
            })?;
            self.rows_clip.SetOffsetY2(view.header * scale)?;
            self.rows_clip.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: view.right * scale,
                bottom: view.rows_height * scale,
            })?;
            self.header.visual.SetOffsetX2(-scroll.x * scale)?;
            self.rows.visual.SetOffsetX2(-scroll.x * scale)?;
            self.rows.visual.SetOffsetY2(-scroll.y * scale)?;
        }
        if view.width <= 0.0 || view.height <= 0.0 {
            return Ok(());
        };
        let width = ((view.extent_x + 1.0)
            .max(view.maximum_x() + view.right)
            .max(view.right)
            * scale)
            .ceil()
            .max(1.0) as u32;
        if damage.header && !damage.view && !damage.all && damage.rows.is_none() {
            self.header_update(renderer, dpi, list, text, theme, view, damage, scroll)?;
            self.presents += 1;
            return Ok(());
        }
        if let Some((origin, serial, image)) = super::rename::image() {
            if self.rename.is_none() {
                let layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                unsafe {
                    self.root
                        .AddVisual(&layer.visual, true, Some(&self.band.visual))?;
                }
                self.rename = Some((layer, None));
            }
            let (layer, key) = self.rename.as_mut().unwrap();
            let current = serial;
            if *key != Some(current) {
                layer.resize((image.width, image.height))?;
                renderer.update(layer, None, dpi, &theme.content, |canvas| {
                    let bitmap = canvas.system_icon(&image)?;
                    unsafe { canvas.context.DrawBitmap(&bitmap, Some(&super::address::rect(0.0, 0.0, image.width as f32 / scale, image.height as f32 / scale)), 1.0, windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None); }
                    Ok(())
                })?;
                *key = Some(current);
            }
            unsafe {
                layer.visual.SetOffsetX2((origin.0 - scroll.x) * scale)?;
                layer
                    .visual
                    .SetOffsetY2((view.header + origin.1 - scroll.y) * scale)?;
            }
        } else if let Some((layer, _)) = self.rename.take() {
            unsafe {
                self.root.RemoveVisual(&layer.visual)?;
                layer.surface.Trim(Some(&[]))?;
            }
        }
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetPropW(
                super::context::hwnd(),
                windows::core::w!("NativeRenameBackingCount"),
                Some(windows::Win32::Foundation::HANDLE(
                    usize::from(self.rename.is_some()) as _,
                )),
            )?;
        }
        let previous_width = self.rows.size.0;
        let resized = self.rows.resize((
            width,
            (view.extent_y.max(view.rows_height) * scale)
                .ceil()
                .max(1.0) as u32,
        ))?;
        let grid_changed = match (&self.grid, &folder.grid) {
            (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        let visible_changed = match (&self.grid, &folder.grid) {
            (Some(a), Some(b)) => {
                !a.same_visible(b, (scroll.x, scroll.y), (view.right, view.rows_height))
            }
            (None, None) => false,
            _ => true,
        };
        if grid_changed {
            if folder.grid.is_none() {
                self.grid_assets = None;
            } else if self.grid.is_none() {
                self.row_assets = Default::default();
            }
            self.grid = folder.grid.clone();
        }
        let changed = visible_changed
            || self
                .geometry
                .as_ref()
                .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &folder.layout));
        self.geometry = Some(folder.layout.clone());
        if resized && previous_width != width || damage.all || changed {
            self.valid = None;
        }
        let visible = RECT {
            left: (scroll.x * scale).round() as i32,
            top: (scroll.y * scale).round() as i32,
            right: ((scroll.x + view.right) * scale).round() as i32,
            bottom: ((scroll.y + view.rows_height) * scale).round() as i32,
        };
        let previous = self.valid;
        let updates = exposed(visible, previous);
        for rect in updates {
            self.rows_update(renderer, rect, dpi, list, text, theme, view)?;
        }
        if let (Some((first, last)), Some(previous)) = (damage.rows, previous) {
            let rect = if let Some(model) = &folder.grid {
                model
                    .damage(
                        first..last,
                        (scroll.x, scroll.y),
                        (view.right, view.rows_height),
                    )
                    .map_or(RECT::default(), |[left, top, right, bottom]| RECT {
                        left: (left * scale).floor() as i32,
                        top: (top * scale).floor() as i32,
                        right: (right * scale).ceil() as i32,
                        bottom: (bottom * scale).ceil() as i32,
                    })
            } else {
                RECT {
                    left: visible.left.max(previous.left),
                    right: visible.right.min(previous.right),
                    top: ((if first == 0 && !folder.layout.headers.is_empty() {
                        0.0
                    } else {
                        view.row_top(folder, first)
                    } * scale)
                        .floor() as i32)
                        .max(visible.top)
                        .max(previous.top),
                    bottom: (((if last >= view.count {
                        view.extent_y
                    } else {
                        view.row_top(folder, last.saturating_sub(1)) + view.plate
                    }) * scale)
                        .ceil() as i32)
                        .min(visible.bottom)
                        .min(previous.bottom),
                }
            };
            if rect.bottom > rect.top {
                self.rows_update(renderer, rect, dpi, list, text, theme, view)?;
            }
        }
        if visible.right > visible.left && visible.bottom > visible.top {
            unsafe {
                self.rows.surface.Trim(Some(&[RECT {
                    left: visible.left,
                    top: visible.top,
                    right: visible.right,
                    bottom: visible.bottom,
                }]))?;
            }
            self.valid = Some(visible);
        }
        self.header_update(renderer, dpi, list, text, theme, view, damage, scroll)?;
        if let Some(model) = folder
            .grid
            .as_ref()
            .filter(|model| model.kind == super::grid::Kind::Gallery)
        {
            if self.timeline.is_none() {
                let layer = renderer.layer(DXGI_ALPHA_MODE_IGNORE)?;
                unsafe {
                    self.root
                        .AddVisual(&layer.visual, true, Some(&self.band.visual))?;
                }
                self.timeline = Some(layer);
            }
            let layer = self.timeline.as_mut().unwrap();
            layer.resize((
                (53.0 * scale).round() as u32,
                (view.height * scale).round().max(1.0) as u32,
            ))?;
            unsafe {
                layer.visual.SetOffsetX2((view.width - 53.0) * scale)?;
            }
            renderer.update(layer, None, dpi, &theme.content, |canvas| {
                super::grid::draw_timeline(model, canvas, text, theme, scroll.y)
            })?;
        } else if let Some(layer) = self.timeline.take() {
            unsafe {
                self.root.RemoveVisual(&layer.visual)?;
                layer.surface.Trim(Some(&[]))?;
            }
        }
        for axis in [Axis::Vertical, Axis::Horizontal] {
            let layer = match axis {
                Axis::Vertical => &mut self.vertical,
                Axis::Horizontal => &mut self.horizontal,
            };
            let Some(bar) = scroll.bar(axis) else {
                unsafe {
                    layer.visual.SetClip2(&D2D_RECT_F::default())?;
                }
                continue;
            };
            let dimensions = match axis {
                Axis::Vertical => (17.0, view.bottom),
                Axis::Horizontal => (view.right, 17.0),
            };
            layer.resize((
                (dimensions.0 * scale).round().max(1.0) as u32,
                (dimensions.1 * scale).round().max(1.0) as u32,
            ))?;
            renderer.update(layer, None, dpi, &theme.track, |canvas| {
                super::bars::draw(canvas, &mut self.bar_assets, theme, scroll, bar)
            })?;
            unsafe {
                layer.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: dimensions.0 * scale,
                    bottom: dimensions.1 * scale,
                })?;
                layer.visual.SetOffsetX2(if axis == Axis::Vertical {
                    view.right * scale
                } else {
                    0.0
                })?;
                layer.visual.SetOffsetY2(if axis == Axis::Horizontal {
                    view.bottom * scale
                } else {
                    0.0
                })?;
            }
        }
        if let Some(band) = list.interaction.band.as_ref().filter(|b| b.moved) {
            self.band.resize((
                (view.right * scale).round().max(1.0) as u32,
                (view.rows_height * scale).round().max(1.0) as u32,
            ))?;
            renderer.update(
                &mut self.band,
                None,
                dpi,
                &D2D1_COLOR_F::default(),
                |canvas| {
                    let rect = D2D_RECT_F {
                        left: band.start.0.min(band.end.0) - scroll.x,
                        top: band.start.1.min(band.end.1) - scroll.y,
                        right: band.start.0.max(band.end.0) - scroll.x + 1.0,
                        bottom: band.start.1.max(band.end.1) - scroll.y + 1.0,
                    };
                    for edge in [
                        D2D_RECT_F {
                            bottom: rect.top + 1.0,
                            ..rect
                        },
                        D2D_RECT_F {
                            top: rect.bottom - 1.0,
                            ..rect
                        },
                        D2D_RECT_F {
                            right: rect.left + 1.0,
                            ..rect
                        },
                        D2D_RECT_F {
                            left: rect.right - 1.0,
                            ..rect
                        },
                    ] {
                        details::fill(canvas, edge, &theme.band_border);
                    }
                    details::fill(
                        canvas,
                        D2D_RECT_F {
                            left: rect.left + 1.0,
                            top: rect.top + 1.0,
                            right: rect.right - 1.0,
                            bottom: rect.bottom - 1.0,
                        },
                        &theme.band,
                    );
                    Ok(())
                },
            )?;
            unsafe {
                self.band.visual.SetOffsetY2(view.header * scale)?;
                self.band.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: view.right * scale,
                    bottom: view.rows_height * scale,
                })?;
            }
        } else {
            unsafe {
                self.band.visual.SetClip2(&D2D_RECT_F::default())?;
            }
            if self.band.presents > 0 {
                unsafe {
                    self.band.surface.Trim(Some(&[]))?;
                }
                self.band.presents = 0;
            }
        }
        self.presents += 1;
        Ok(())
    }

    fn header_update(
        &mut self,
        renderer: &Renderer,
        dpi: u32,
        list: &List,
        text: &mut TextEngine,
        theme: &Theme,
        view: Viewport,
        damage: Damage,
        scroll: &State,
    ) -> Result<()> {
        let Some(folder) = &list.folder else {
            return Ok(());
        };
        let scale = dpi as f32 / 96.0;
        let width = ((view.extent_x + 1.0)
            .max(view.maximum_x() + view.right)
            .max(view.right)
            * scale)
            .ceil()
            .max(1.0) as u32;
        let header_changed = self
            .header
            .resize((width, (view.header * scale).round().max(1.0) as u32))?;
        if view.header == 0.0 && self.header.presents > 0 {
            unsafe {
                self.header.surface.Trim(Some(&[]))?;
            }
            self.header.presents = 0;
        }
        if folder.network_notice.is_some()
            && (header_changed || self.header.presents == 0 || damage.all || damage.header)
        {
            renderer.update(&mut self.header, None, dpi, &theme.content, |canvas| {
                details::draw_network_notice(
                    folder,
                    canvas,
                    text,
                    theme,
                    view.extent_x.max(view.right),
                )?;
                if folder.grid.is_none() {
                    let mut original = Default::default();
                    unsafe {
                        canvas.context.GetTransform(&mut original);
                        let mut translated = original;
                        translated.M32 += 32.0;
                        canvas.context.SetTransform(&translated);
                    }
                    let drawn = details::draw_header(
                        list,
                        &mut self.row_assets,
                        canvas,
                        text,
                        theme,
                        view.extent_x.max(view.right),
                    );
                    unsafe {
                        canvas.context.SetTransform(&original);
                    }
                    drawn?;
                }
                Ok(())
            })?;
        } else if folder.grid.is_none()
            && (header_changed || self.header.presents == 0 || damage.all || damage.header)
        {
            renderer.update(&mut self.header, None, dpi, &theme.content, |canvas| {
                details::draw_header(
                    list,
                    &mut self.row_assets,
                    canvas,
                    text,
                    theme,
                    view.extent_x.max(view.right),
                )
            })?;
        } else if folder
            .grid
            .as_ref()
            .is_some_and(|model| model.kind == super::grid::Kind::Gallery)
        {
            renderer.update(&mut self.header, None, dpi, &theme.content, |canvas| {
                super::grid::draw_gallery_header(list, canvas, text, theme, scroll.y)
            })?;
        }
        Ok(())
    }

    fn rows_update(
        &mut self,
        renderer: &Renderer,
        rect: RECT,
        dpi: u32,
        list: &List,
        text: &mut TextEngine,
        theme: &Theme,
        view: Viewport,
    ) -> Result<()> {
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Ok(());
        };
        let scale = dpi as f32 / 96.0;
        let Some(folder) = list.folder.as_ref() else {
            return Ok(());
        };
        if let Some(model) = &folder.grid {
            self.pixels_drawn +=
                (rect.right - rect.left) as usize * (rect.bottom - rect.top) as usize;
            let bounds = D2D_RECT_F {
                left: rect.left as f32 / scale,
                top: rect.top as f32 / scale,
                right: rect.right as f32 / scale,
                bottom: rect.bottom as f32 / scale,
            };
            self.rows_drawn += model
                .visible(
                    (bounds.left, bounds.top),
                    (bounds.right - bounds.left, bounds.bottom - bounds.top),
                )
                .count();
            let assets = self.grid_assets.get_or_insert_with(Default::default);
            return renderer.update(&mut self.rows, Some(rect), dpi, &theme.content, |canvas| {
                details::draw_groups(
                    list,
                    &mut self.row_assets,
                    canvas,
                    text,
                    theme,
                    view,
                    bounds.top..bounds.bottom,
                )?;
                super::grid::draw(list, assets, canvas, text, theme, view, bounds)?;
                details::draw_empty(list, canvas, text, theme, view)
            });
        }
        let range = folder.layout.visible(
            rect.top as f32 / scale,
            (rect.bottom - rect.top) as f32 / scale,
            view.plate,
        );
        self.rows_drawn += range
            .clone()
            .filter(|&row| !folder.layout.folded[row])
            .count();
        self.pixels_drawn += (rect.right - rect.left) as usize * (rect.bottom - rect.top) as usize;
        renderer.update(&mut self.rows, Some(rect), dpi, &theme.content, |canvas| {
            details::draw_groups(
                list,
                &mut self.row_assets,
                canvas,
                text,
                theme,
                view,
                rect.top as f32 / scale..rect.bottom as f32 / scale,
            )?;
            details::draw_rows(list, &mut self.row_assets, canvas, text, theme, view, range)?;
            details::draw_empty(list, canvas, text, theme, view)
        })
    }
}

pub(super) fn exposed(new: RECT, old: Option<RECT>) -> Vec<RECT> {
    let Some(old) = old else { return vec![new] };
    let left = new.left.max(old.left);
    let right = new.right.min(old.right);
    let top = new.top.max(old.top);
    let bottom = new.bottom.min(old.bottom);
    if right <= left || bottom <= top {
        return vec![new];
    };
    [
        RECT {
            left: new.left,
            top: new.top,
            right: new.right,
            bottom: top,
        },
        RECT {
            left: new.left,
            top: bottom,
            right: new.right,
            bottom: new.bottom,
        },
        RECT {
            left: new.left,
            top,
            right: left,
            bottom,
        },
        RECT {
            left: right,
            top,
            right: new.right,
            bottom,
        },
    ]
    .into_iter()
    .filter(|r| r.right > r.left && r.bottom > r.top)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exposed_strips_cover_only_new_pixels_after_diagonal_scroll() {
        let old = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let new = RECT {
            left: 10,
            top: 20,
            right: 110,
            bottom: 120,
        };
        let strips = exposed(new, Some(old));
        assert_eq!(
            strips
                .iter()
                .map(|r| (r.right - r.left) * (r.bottom - r.top))
                .sum::<i32>(),
            2800
        );
        for y in 0..120 {
            for x in 0..110 {
                let count = strips
                    .iter()
                    .filter(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
                    .count();
                let needed = x >= new.left
                    && x < new.right
                    && y >= new.top
                    && y < new.bottom
                    && !(x < old.right && y < old.bottom);
                assert_eq!(count, usize::from(needed));
            }
        }
    }
}
