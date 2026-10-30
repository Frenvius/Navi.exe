use std::collections::HashMap;
use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay;

use super::list::{Folder, List};
use super::render::Canvas;
use super::text::{TextEngine, TextStyle, DIRECTUI_KERNED as DIRECTUI, HEADER as HEADER_STYLE};
use super::theme::Theme;

const HEADER: f32 = 25.0;
pub(super) const TOP: f32 = 136.0;

pub(super) fn top() -> f32 {
    if crate::frame::is_full_screen_for(super::context::hwnd()) {
        0.0
    } else {
        TOP
    }
}

#[derive(Clone, Copy)]
pub struct Viewport {
    pub size: (f32, f32),
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub bottom: f32,
    pub right: f32,
    pub rows_height: f32,
    pub header: f32,
    pub extent_x: f32,
    pub extent_y: f32,
    pub pitch: f32,
    pub plate: f32,
    pub count: usize,
    pub bar: bool,
    pub overlay: bool,
    pub horizontal: bool,
    pub column_maximum: Option<f32>,
}

impl Viewport {
    pub fn new(size: (f32, f32), folder: &Folder, left: f32) -> Self {
        Self::at(size, folder, left, top())
    }

    pub fn at(size: (f32, f32), folder: &Folder, left: f32, top: f32) -> Self {
        let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
        let home = crate::app::ids::same(path, crate::app::ids::HOME);
        let xaml = home || crate::app::ids::same(path, crate::app::ids::GALLERY);
        let left = left - if xaml { 2.0 } else { 0.0 };
        let pane = if (folder.panes.0 || folder.panes.1) && size.0 - left >= 335.0 {
            folder.pane_width
        } else {
            0.0
        };
        let height = (size.1
            - top
            - if folder.settings.status_bar {
                24.0
            } else {
                0.0
            })
        .max(0.0);
        let pitch = if folder.settings.compact {
            crate::app::ROW_PITCH
        } else {
            crate::app::ROW_PITCH_ROOMY
        };
        let plate = if folder.settings.compact { 22.0 } else { 24.0 };
        let width = (size.0 - left - pane).max(0.0);
        let header = if home {
            0.0
        } else {
            folder.grid.as_ref().map_or(HEADER, |model| model.header())
        } + if folder.network_notice.is_some() {
            32.0
        } else {
            0.0
        };
        let extent_y = folder
            .grid
            .as_ref()
            .map_or(folder.layout.extent, |model| model.layout.height);
        let extent_x = folder.grid.as_ref().map_or_else(
            || {
                folder.columns.iter().map(|c| c.width).sum::<f32>()
                    + if folder
                        .crumbs
                        .last()
                        .is_some_and(|crumb| crate::app::ids::is_search(&crumb.id))
                    {
                        10.0
                    } else {
                        9.0
                    }
            },
            |model| model.layout.width,
        );
        let tall = extent_y > (height - header).max(0.0);
        let wide = extent_x > width;
        let bar = !xaml
            && !folder.order.is_empty()
            && (tall || (wide && extent_y > height - header - 17.0));
        let horizontal =
            !xaml && !folder.order.is_empty() && (wide || (tall && extent_x > width - 17.0));
        let bottom = (height - if horizontal { 17.0 } else { 0.0 }).max(0.0);
        let right = (width - if bar { 17.0 } else { 0.0 }).max(0.0);
        let rows_height = (bottom - header).max(0.0);
        Self {
            size,
            x: left,
            y: top,
            width,
            height,
            bottom,
            right,
            rows_height,
            header,
            extent_x,
            extent_y,
            pitch,
            plate,
            count: folder.order.len(),
            bar,
            overlay: home && tall,
            horizontal,
            column_maximum: folder
                .grid
                .as_ref()
                .filter(|model| model.spec.column_major && horizontal)
                .map(|model| {
                    crate::app::grid_layout::column_lefts(&model.layout.cells)
                        .last()
                        .copied()
                        .unwrap_or(0.0)
                }),
        }
    }

    pub fn row_top(&self, folder: &Folder, row: usize) -> f32 {
        if let Some(model) = &folder.grid {
            return model.layout.cells.get(row).map_or(0.0, |cell| cell[1]);
        }
        folder
            .layout
            .tops
            .get(row)
            .copied()
            .unwrap_or(crate::app::ROW_TOP + row as f32 * self.pitch)
    }

    pub fn row_bottom(&self, folder: &Folder, row: usize) -> f32 {
        if let Some(model) = &folder.grid {
            return model
                .layout
                .cells
                .get(row)
                .map_or(0.0, |cell| cell[1] + cell[3]);
        }
        crate::app::selection::details_bottom(self.row_top(folder, row), self.pitch)
    }

    pub fn visible(&self, folder: &Folder, offset: f32) -> std::ops::Range<usize> {
        if self.right <= 0.0 || self.rows_height <= 0.0 {
            return 0..0;
        }
        if let Some(model) = &folder.grid {
            model.range((0.0, offset), (self.right, self.rows_height))
        } else {
            folder.layout.visible(offset, self.rows_height, self.plate)
        }
    }

    pub fn maximum_y(&self) -> f32 {
        (self.extent_y - self.rows_height).max(0.0)
    }

    pub fn needed(&self, folder: &Folder, offset: f32) -> std::ops::Range<usize> {
        let visible = self.visible(folder, offset);
        if visible.is_empty() {
            0..0
        } else {
            visible.start.saturating_sub(8)..visible.end.saturating_add(8).min(self.count)
        }
    }
    pub fn maximum_x(&self) -> f32 {
        if let Some(maximum) = self.column_maximum {
            return maximum;
        }
        if self.horizontal {
            (self.extent_x + 1.0 - self.right).max(0.0)
        } else {
            0.0
        }
    }
}

#[derive(Default)]
pub struct Assets {
    images: HashMap<u32, std::rc::Rc<ID2D1Bitmap1>>,
    groups: HashMap<(i32, u32, bool), ID2D1Bitmap1>,
    checks: HashMap<(super::check::State, u32, bool), ID2D1Bitmap1>,
    plates: Vec<ID2D1Bitmap1>,
    light: Option<bool>,
}

pub fn draw_header(
    list: &List,
    assets: &mut Assets,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    right: f32,
) -> Result<()> {
    let Some(folder) = &list.folder else {
        return Ok(());
    };
    let positions = list.header.positions(&folder.columns);
    for (i, column) in folder.columns.iter().enumerate() {
        if list.header.ghost.is_some_and(|g| g.0 == i) {
            continue;
        }
        let x = positions[i];
        if x < right {
            header_cell(
                folder,
                column,
                i,
                x,
                list.header.look(i),
                canvas,
                text,
                theme,
            )?;
        }
    }
    if let Some((i, x, over)) = list.header.ghost {
        header_cell(
            folder,
            &folder.columns[i],
            i,
            x,
            if over {
                super::header::Look::Pressed
            } else {
                super::header::Look::Hover
            },
            canvas,
            text,
            theme,
        )?;
    }
    if folder.settings.check_boxes {
        draw_check(
            canvas,
            assets,
            theme,
            (11.0, 6.0),
            if !folder.order.is_empty()
                && list.interaction.selection.selected.len() == folder.order.len()
            {
                super::check::State::Selected
            } else {
                super::check::State::Clear
            },
        )?;
    }
    Ok(())
}

fn header_cell(
    folder: &Folder,
    column: &crate::app::columns::Col,
    index: usize,
    x: f32,
    look: super::header::Look,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
) -> Result<()> {
    use super::header::Look;
    let rect = D2D_RECT_F {
        left: x,
        top: 0.0,
        right: x + column.width,
        bottom: HEADER,
    };
    let background = match look {
        Look::Idle => &theme.content,
        Look::Hover => &theme.header_hover,
        Look::Pressed => &theme.header_pressed,
        Look::Focused => &theme.header_open,
    };
    fill(canvas, rect, background);
    if look == Look::Idle {
        fill(
            canvas,
            D2D_RECT_F {
                left: rect.right - 1.0,
                ..rect
            },
            &theme.header_divider,
        );
    } else {
        fill(
            canvas,
            D2D_RECT_F {
                left: rect.right - 16.0,
                right: rect.right - 15.0,
                ..rect
            },
            &theme.header_chevron_divider,
        );
        for (px, py, alpha) in [
            (0, 0, 1.0),
            (1, 0, 0.22),
            (5, 0, 0.43),
            (6, 0, 0.57),
            (0, 1, 0.22),
            (1, 1, 1.0),
            (2, 1, 0.22),
            (4, 1, 0.43),
            (5, 1, 0.71),
            (1, 2, 0.22),
            (2, 2, 1.0),
            (3, 2, 0.57),
            (4, 2, 0.71),
            (2, 3, 0.22),
            (3, 3, 0.64),
        ] {
            fill(
                canvas,
                D2D_RECT_F {
                    left: rect.right - 12.0 + px as f32,
                    top: 10.0 + py as f32,
                    right: rect.right - 11.0 + px as f32,
                    bottom: 11.0 + py as f32,
                },
                &D2D1_COLOR_F {
                    a: alpha,
                    ..theme.header_chevron
                },
            );
        }
    }
    if look == Look::Focused {
        for edge in [
            D2D_RECT_F {
                bottom: 1.0,
                ..rect
            },
            D2D_RECT_F { top: 24.0, ..rect },
            D2D_RECT_F {
                right: rect.left + 1.0,
                ..rect
            },
            D2D_RECT_F {
                left: rect.right - 1.0,
                ..rect
            },
        ] {
            fill(canvas, edge, &theme.header_open_border);
        }
        let corner = D2D1_COLOR_F {
            r: theme.header_open.r + (theme.header_open_border.r - theme.header_open.r) * 0.43,
            g: theme.header_open.g + (theme.header_open_border.g - theme.header_open.g) * 0.43,
            b: theme.header_open.b + (theme.header_open_border.b - theme.header_open.b) * 0.43,
            a: 1.0,
        };
        for (left, top) in [
            (rect.left, rect.top),
            (rect.right - 1.0, rect.top),
            (rect.left, rect.bottom - 1.0),
            (rect.right - 1.0, rect.bottom - 1.0),
        ] {
            fill(
                canvas,
                D2D_RECT_F {
                    left,
                    top,
                    right: left + 1.0,
                    bottom: top + 1.0,
                },
                &corner,
            );
        }
    }
    let indent = if index == 0 {
        if folder.settings.check_boxes {
            if folder.settings.compact {
                34.0
            } else {
                47.0
            }
        } else if folder.settings.compact {
            17.0
        } else {
            30.0
        }
    } else {
        6.0
    };
    draw_cell(
        canvas,
        text,
        &column.label,
        HEADER_STYLE,
        D2D_RECT_F {
            left: x + indent,
            top: 1.0,
            right: rect.right - 6.0,
            bottom: 24.0,
        },
        0.0,
        &theme.header_text,
    )?;
    if let Some(sort) = Some(folder.view.sort)
        .filter(|sort| !folder.view.search_ranked && sort.key.key() == column.key)
    {
        sort_arrow(
            canvas,
            theme,
            x,
            column.width,
            index,
            &sort,
            &folder.settings,
        );
    }
    Ok(())
}

pub fn draw_rows(
    list: &List,
    assets: &mut Assets,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    viewport: Viewport,
    range: std::ops::Range<usize>,
) -> Result<()> {
    let Some(folder) = &list.folder else {
        return Ok(());
    };
    let mut dpi_x = 96.0;
    let mut dpi_y = 96.0;
    unsafe {
        canvas.context.GetDpi(&mut dpi_x, &mut dpi_y);
    }
    let scale = dpi_x / 96.0;
    let right = viewport.extent_x.max(viewport.right);
    for row in range.clone().filter(|&row| !folder.layout.folded[row]) {
        draw_plate(list, assets, canvas, theme, viewport, row)?;
    }
    for row in range.clone().filter(|&row| !folder.layout.folded[row]) {
        let y = viewport.row_top(folder, row);
        let lower = if folder.settings.compact { 0.0 } else { 2.0 };
        let ready = list.rows.get(&row);
        let shift = if folder.settings.check_boxes {
            15.0
        } else {
            0.0
        };
        let entry = &folder.entries[folder.order[row]];
        let icon = ready.map_or(entry.icon, |ready| ready.icon);
        let bitmap = if let Some(image) = ready.and_then(|ready| ready.image.as_ref()) {
            Some(match assets.images.get(&icon) {
                Some(bitmap) => bitmap.clone(),
                None => {
                    let bitmap = canvas.shared_icon(image)?;
                    assets.images.insert(icon, bitmap.clone());
                    bitmap
                }
            })
        } else {
            assets.images.get(&icon).cloned()
        };
        if let Some(bitmap) = bitmap {
            unsafe {
                canvas.context.DrawBitmap(
                    bitmap.as_ref(),
                    Some(&D2D_RECT_F {
                        left: 18.0 + shift,
                        top: y + lower + 2.0,
                        right: 34.0 + shift,
                        bottom: y + lower + 18.0,
                    }),
                    if entry.hidden || list.cut.contains(&entry.path) {
                        0.5
                    } else {
                        1.0
                    },
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
        }
        let selected = list.interaction.selection.selected.contains(&entry.path);
        if folder.settings.check_boxes && (selected || list.interaction.hover == Some(row)) {
            draw_check(
                canvas,
                assets,
                theme,
                (18.0, y + lower + 4.0),
                if selected {
                    super::check::State::Selected
                } else {
                    super::check::State::Clear
                },
            )?;
        }
        let mut x = 0.0;
        for (i, column) in folder.columns.iter().enumerate() {
            let indent = if i == 0 { 38.0 + shift } else { 6.0 };
            let name = column.key == PKEY_ItemNameDisplay;
            if name && super::rename::active(&entry.path) {
                x += column.width;
                continue;
            }
            let color = if name {
                if selected && theme.content.r < 0.5 {
                    &theme.dui_text
                } else if crate::app::format::name_style(entry, &folder.settings) == 2 {
                    &theme.encrypted_text
                } else if crate::app::format::name_style(entry, &folder.settings) == 1 {
                    &theme.compressed_text
                } else {
                    &theme.dui_text
                }
            } else if list.interaction.selection.selected.contains(&entry.path)
                || list.interaction.hover == Some(row)
            {
                &theme.cell_active_text
            } else {
                &theme.cell_text
            };
            let value = ready
                .and_then(|ready| ready.cells.get(i))
                .map(|value| std::borrow::Cow::Borrowed(value.as_str()))
                .unwrap_or_else(|| {
                    std::borrow::Cow::Owned(crate::app::columns::cell_text(
                        entry,
                        &column.key,
                        None,
                    ))
                });
            let value = value.as_ref();
            let padding = if column.right {
                8.0
            } else if column.key
                == (windows::Win32::Foundation::PROPERTYKEY {
                    fmtid: windows::core::GUID::from_u128(0x9b174b33_40ff_11d2_a27e_00c04fc30871),
                    pid: 2,
                })
            {
                7.0
            } else {
                6.0
            };
            let width = column.width - indent - padding;
            let advance = text.layout(value, DIRECTUI, 4096.0)?.width;
            let offset = if column.right {
                (width - advance / scale).max(0.0)
            } else {
                0.0
            };
            if name
                || column.key
                    == windows::Win32::Storage::EnhancedStorage::PKEY_ItemFolderPathDisplayNarrow
            {
                super::search::highlight(
                    canvas,
                    text,
                    theme,
                    list,
                    value,
                    D2D_RECT_F {
                        left: x + indent,
                        top: y + lower + 3.0,
                        right: x + indent + width,
                        bottom: y + lower + 19.0,
                    },
                    DIRECTUI,
                )?;
            }
            draw_cell(
                canvas,
                text,
                value,
                DIRECTUI,
                D2D_RECT_F {
                    left: x + indent,
                    top: y + lower,
                    right: x + indent + width,
                    bottom: y + lower + 20.0,
                },
                offset,
                color,
            )?;
            if folder.settings.single_click
                && (!folder.settings.underline_hover_only || list.interaction.hover == Some(row))
            {
                let clipped = text.elide(value, DIRECTUI, width)?;
                let advance = text.layout(&clipped, DIRECTUI, 4096.0)?.width / scale;
                fill(
                    canvas,
                    D2D_RECT_F {
                        left: x + indent + offset,
                        top: y + lower + 16.0,
                        right: x + indent + offset + advance,
                        bottom: y + lower + 17.0,
                    },
                    color,
                );
            }
            x += column.width;
            if x >= right {
                break;
            }
        }
    }
    let used: std::collections::HashSet<_> = list
        .rows
        .values()
        .map(|r| r.icon)
        .chain(range.map(|row| folder.entries[folder.order[row]].icon))
        .collect();
    assets.images.retain(|key, _| used.contains(key));
    Ok(())
}

pub(super) fn draw_plate(
    list: &List,
    assets: &mut Assets,
    canvas: &Canvas,
    theme: &Theme,
    view: Viewport,
    row: usize,
) -> Result<()> {
    let Some(folder) = &list.folder else {
        return Ok(());
    };
    let state = &list.interaction;
    let dropping = state.drop_row == Some(row);
    let selected = dropping
        || state
            .selection
            .selected
            .contains(&folder.entries[folder.order[row]].path);
    let hover = state.hover == Some(row);
    let focus = state
        .selection
        .focused
        .as_deref()
        .is_some_and(|path| path == folder.entries[folder.order[row]].path)
        && state.group_focus.is_none();
    let active = state.has_focus && state.active;
    if !selected && !hover && !(active && focus && state.cues) {
        return Ok(());
    }
    let light = theme.content.r > 0.5;
    if assets.light != Some(light) {
        assets.plates.clear();
        let sources: [&[u8]; 3] = if light {
            [
                include_bytes!("../../ui/assets/tile-light-focused.rgba"),
                include_bytes!("../../ui/assets/tile-light-multi.rgba"),
                include_bytes!("../../ui/assets/tile-light-unfocused.rgba"),
            ]
        } else {
            [
                include_bytes!("../../ui/assets/tile-dark-focused.rgba"),
                include_bytes!("../../ui/assets/tile-dark-multi.rgba"),
                include_bytes!("../../ui/assets/tile-dark-unfocused.rgba"),
            ]
        };
        for (index, source) in sources.iter().enumerate() {
            let mut pixels = source.to_vec();
            if index != 0 {
                transparent_corners(&mut pixels, theme);
            }
            assets
                .plates
                .push(canvas.system_icon(&crate::shell::icons::Rgba {
                    width: 8,
                    height: 8,
                    pixels,
                })?);
        }
        let rgba = |color: &D2D1_COLOR_F| {
            [color.r, color.g, color.b, 1.0].map(|c| (c * 255.0).round() as u8)
        };
        let mut hot = vec![0u8; 256];
        for pixel in hot.chunks_exact_mut(4) {
            pixel.copy_from_slice(&rgba(&theme.row_hover));
        }
        for &(x, y, coverage) in &[(0, 0, 1.0 / 3.0), (1, 0, 14.0 / 15.0), (0, 1, 14.0 / 15.0)] {
            let color = D2D1_COLOR_F {
                r: theme.content.r + (theme.row_hover.r - theme.content.r) * coverage,
                g: theme.content.g + (theme.row_hover.g - theme.content.g) * coverage,
                b: theme.content.b + (theme.row_hover.b - theme.content.b) * coverage,
                a: 1.0,
            };
            for (xx, yy) in [(x, y), (7 - x, y), (x, 7 - y), (7 - x, 7 - y)] {
                hot[(yy * 8 + xx) * 4..(yy * 8 + xx) * 4 + 4].copy_from_slice(&rgba(&color));
            }
        }
        transparent_corners(&mut hot, theme);
        assets
            .plates
            .push(canvas.system_icon(&crate::shell::icons::Rgba {
                width: 8,
                height: 8,
                pixels: hot,
            })?);
        let ink = rgba(&theme.list_focus);
        let mut outline = vec![0u8; 256];
        for y in 0..8 {
            for x in 0..8 {
                if x == 0 || x == 7 || y == 0 || y == 7 {
                    outline[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].copy_from_slice(&ink);
                }
            }
        }
        for &(x, y, alpha) in &[(0, 0, 87), (1, 0, 235), (0, 1, 235), (1, 1, 5)] {
            for (xx, yy) in [(x, y), (7 - x, y), (x, 7 - y), (7 - x, 7 - y)] {
                outline[(yy * 8 + xx) * 4..(yy * 8 + xx) * 4 + 4]
                    .copy_from_slice(&[ink[0], ink[1], ink[2], alpha]);
            }
        }
        assets
            .plates
            .push(canvas.system_icon(&crate::shell::icons::Rgba {
                width: 8,
                height: 8,
                pixels: outline,
            })?);
        assets.light = Some(light);
    }
    let index = if dropping {
        Some(1)
    } else if selected {
        if super::rename::selected().is_some() {
            Some(1)
        } else if !active {
            Some(2)
        } else if !focus && !hover {
            Some(1)
        } else {
            Some(0)
        }
    } else if hover {
        Some(3)
    } else {
        None
    };
    let rect = if let Some(model) = &folder.grid {
        let [x, y, width, height] = model.layout.cells[row];
        D2D_RECT_F {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        }
    } else {
        D2D_RECT_F {
            left: 14.0,
            top: view.row_top(folder, row),
            right: view.extent_x
                - if folder
                    .crumbs
                    .last()
                    .is_some_and(|crumb| crate::app::ids::is_search(&crumb.id))
                {
                    14.0
                } else {
                    13.0
                },
            bottom: view.row_top(folder, row) + view.plate,
        }
    };
    for index in [
        index,
        (active && focus && !selected && state.cues).then_some(4),
    ]
    .into_iter()
    .flatten()
    {
        draw_sliced(canvas, &assets.plates[index], rect, 1.0);
    }
    Ok(())
}

fn transparent_corners(pixels: &mut [u8], theme: &Theme) {
    let background = [theme.content.r, theme.content.g, theme.content.b].map(|c| c * 255.0);
    for &(x, y, alpha) in &[(0, 0, 87), (1, 0, 235), (0, 1, 235)] {
        let a = alpha as f32 / 255.0;
        for (xx, yy) in [(x, y), (7 - x, y), (x, 7 - y), (7 - x, 7 - y)] {
            let pixel = &mut pixels[(yy * 8 + xx) * 4..(yy * 8 + xx) * 4 + 4];
            for channel in 0..3 {
                pixel[channel] = ((pixel[channel] as f32 - background[channel] * (1.0 - a))
                    .round()
                    .max(0.0)
                    / a)
                    .floor()
                    .min(255.0) as u8;
            }
            pixel[3] = alpha;
        }
    }
}

pub fn fill(canvas: &Canvas, rect: D2D_RECT_F, color: &D2D1_COLOR_F) {
    unsafe {
        canvas.brush.SetColor(color);
        canvas.context.FillRectangle(&rect, &canvas.brush);
    }
}

pub(super) fn draw_cell(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    style: TextStyle,
    bounds: D2D_RECT_F,
    offset: f32,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    let width = bounds.right - bounds.left;
    if width <= 0.0 {
        return Ok(());
    }
    let value = text.elide(value, style, width)?;
    draw_line(canvas, text, &value, style, bounds, offset, color)
}

pub(super) fn draw_line(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    style: TextStyle,
    bounds: D2D_RECT_F,
    offset: f32,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    if bounds.right <= bounds.left {
        return Ok(());
    }
    let layout = text.layout(&value, style, 4096.0)?;
    unsafe {
        canvas
            .context
            .PushAxisAlignedClip(&bounds, D2D1_ANTIALIAS_MODE_ALIASED);
    }
    let result = text.draw(
        canvas,
        &value,
        style,
        (
            bounds.left + offset,
            bounds.top + (bounds.bottom - bounds.top - layout.height) / 2.0,
        ),
        color,
    );
    unsafe {
        canvas.context.PopAxisAlignedClip();
    }
    result
}

fn sort_arrow(
    canvas: &Canvas,
    theme: &Theme,
    x: f32,
    width: f32,
    column: usize,
    sort: &crate::app::sort::Sort,
    settings: &crate::shell::listing::ExplorerSettings,
) {
    let from = if column == 0 {
        if settings.compact {
            11.0
        } else {
            24.0
        }
    } else {
        0.0
    };
    let x = x + from + ((width - from - 7.0) / 2.0).floor();
    let up = [
        (3, 0, 0.08),
        (4, 0, 0.77),
        (5, 0, 0.08),
        (2, 1, 0.08),
        (3, 1, 1.0),
        (4, 1, 0.61),
        (5, 1, 0.93),
        (6, 1, 0.08),
        (1, 2, 0.08),
        (2, 2, 1.0),
        (3, 2, 0.47),
        (5, 2, 0.23),
        (6, 2, 0.93),
        (7, 2, 0.08),
        (0, 3, 0.08),
        (1, 3, 1.0),
        (2, 3, 0.47),
        (6, 3, 0.23),
        (7, 3, 0.93),
        (8, 3, 0.08),
        (0, 4, 0.08),
        (1, 4, 0.38),
        (7, 4, 0.23),
        (8, 4, 0.23),
    ];
    let down = [
        (0, 0, 0.9),
        (1, 0, 0.34),
        (5, 0, 0.34),
        (6, 0, 0.9),
        (1, 1, 1.0),
        (2, 1, 0.34),
        (4, 1, 0.34),
        (5, 1, 1.0),
        (2, 2, 1.0),
        (3, 2, 0.56),
        (4, 2, 1.0),
        (3, 3, 0.67),
    ];
    let (pixels, color, dx, dy) = if sort.descending {
        (down.as_slice(), theme.sort_down, 0.0, 1.0)
    } else {
        (up.as_slice(), theme.sort_up, -1.0, 0.0)
    };
    for &(px, py, alpha) in pixels {
        fill(
            canvas,
            D2D_RECT_F {
                left: x + dx + px as f32,
                top: dy + py as f32,
                right: x + dx + px as f32 + 1.0,
                bottom: dy + py as f32 + 1.0,
            },
            &D2D1_COLOR_F {
                a: color.a * alpha,
                ..color
            },
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn realized_rows_exclude_horizontal_bar_and_include_partial_last_row() {
        let mut settings = crate::shell::listing::settings();
        settings.compact = true;
        settings.status_bar = true;
        let (shown, _) = crate::app::columns::initial(&[], &[]);
        let folder = super::Folder {
            crumbs: Default::default(),
            title: String::new(),
            icon: crate::shell::icons::Rgba {
                width: 1,
                height: 1,
                pixels: vec![0; 4],
            },
            entries: std::sync::Arc::new(Vec::new()),
            order: std::sync::Arc::new((0..5018).collect()),
            headers: std::sync::Arc::new(Vec::new()),
            collapsed: Default::default(),
            layout: std::sync::Arc::new(crate::app::group::DetailsLayout::new(
                5018,
                &[],
                &Default::default(),
                21.0,
                22.0,
            )),
            grid: None,
            gallery_filter: None,
            home: Default::default(),
            filter: None,
            network_notice: None,
            library_origins: std::sync::Arc::new(Vec::new()),
            columns: crate::app::columns::shown_columns(shown),
            view: crate::app::view::resolve(
                "C:\\",
                &crate::shell::viewstate::ViewState::default(),
                &crate::app::FolderView::default(),
            ),
            settings,
            panes: (true, false),
            pane_width: 302.0,
        };
        let view = super::Viewport::new((1100.0, 719.0), &folder, 202.0);
        assert_eq!(
            (view.x, view.y, view.width, view.height),
            (202.0, 136.0, 596.0, 559.0)
        );
        assert_eq!((view.bottom, view.visible(&folder, 0.0).len()), (542.0, 25));
        assert!(view.row_top(&folder, view.visible(&folder, 0.0).end - 1) < view.rows_height);
        assert!(view.row_top(&folder, view.visible(&folder, 0.0).end) >= view.rows_height);
        assert_eq!(
            super::Viewport::new((180.0, 120.0), &folder, 202.0)
                .visible(&folder, 0.0)
                .len(),
            0
        );
    }
}

pub fn draw_groups(
    list: &List,
    assets: &mut Assets,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    view: Viewport,
    visible: std::ops::Range<f32>,
) -> Result<()> {
    let Some(folder) = &list.folder else {
        return Ok(());
    };
    let state = &list.interaction;
    let style = super::text::GROUP_HEADER;
    let column_major = folder
        .grid
        .as_ref()
        .is_some_and(|model| model.spec.column_major);
    for header in &folder.layout.headers {
        let cell = folder.grid.as_ref().and_then(|model| {
            if header.rows.is_empty() {
                model
                    .layout
                    .empty
                    .iter()
                    .find(|empty| empty.label == header.label)
                    .map(|empty| [empty.x, empty.y, empty.width])
            } else {
                model.layout.headers.get(header.rows.start).copied()
            }
        });
        let [left, mut y, width] = cell.unwrap_or([0.0, header.top, -1.0]);
        if folder
            .grid
            .as_ref()
            .is_some_and(|model| model.kind == super::grid::Kind::Drives)
        {
            y -= 4.0;
        }
        if y < 0.0
            || y + header.height
                + if header.rows.is_empty() && !header.folded {
                    32.0
                } else {
                    0.0
                }
                <= visible.start
            || y >= visible.end
        {
            continue;
        }
        let right = if width < 0.0 {
            view.extent_x.max(view.right)
        } else {
            left + width
        };
        let focus = state.group_focus.as_deref() == Some(header.label.as_str())
            && state.has_focus
            && state.active;
        let rect = D2D_RECT_F {
            left,
            top: y + 4.0,
            right,
            bottom: y + if header.label.contains(crate::app::GROUP_SUBTITLE) {
                49.0
            } else {
                31.0
            },
        };
        let hover = state.group_hover.as_deref() == Some(header.label.as_str());
        let native_state = match (header.folded, focus, hover) {
            (true, true, true) => Some(windows::Win32::UI::Controls::LVGH_CLOSESELECTEDHOT),
            (true, true, false) => Some(windows::Win32::UI::Controls::LVGH_CLOSESELECTED),
            (true, false, true) => Some(windows::Win32::UI::Controls::LVGH_CLOSEHOT),
            (false, _, true) => Some(windows::Win32::UI::Controls::LVGH_OPENHOT),
            _ => None,
        };
        if let Some(native_state) = native_state {
            draw_group_background(canvas, assets, theme, rect, native_state)?;
        }
        if focus && !header.folded {
            for edge in [
                D2D_RECT_F {
                    left: rect.left + 2.0,
                    bottom: rect.top + 1.0,
                    right: rect.right - 2.0,
                    ..rect
                },
                D2D_RECT_F {
                    left: rect.left + 2.0,
                    top: rect.bottom - 1.0,
                    right: rect.right - 2.0,
                    ..rect
                },
                D2D_RECT_F {
                    top: rect.top + 2.0,
                    bottom: rect.bottom - 2.0,
                    right: rect.left + 1.0,
                    ..rect
                },
                D2D_RECT_F {
                    top: rect.top + 2.0,
                    bottom: rect.bottom - 2.0,
                    left: rect.right - 1.0,
                    ..rect
                },
            ] {
                fill(canvas, edge, &theme.group_focus);
            }
            let soft = D2D1_COLOR_F {
                a: 5.0 / 255.0,
                ..theme.group_focus
            };
            for edge in [
                D2D_RECT_F {
                    left: rect.left + 2.0,
                    top: rect.top + 1.0,
                    bottom: rect.top + 2.0,
                    right: rect.right - 2.0,
                    ..rect
                },
                D2D_RECT_F {
                    left: rect.left + 2.0,
                    top: rect.bottom - 2.0,
                    bottom: rect.bottom - 1.0,
                    right: rect.right - 2.0,
                    ..rect
                },
                D2D_RECT_F {
                    left: rect.left + 1.0,
                    right: rect.left + 2.0,
                    top: rect.top + 2.0,
                    bottom: rect.bottom - 2.0,
                    ..rect
                },
                D2D_RECT_F {
                    left: rect.right - 2.0,
                    right: rect.right - 1.0,
                    top: rect.top + 2.0,
                    bottom: rect.bottom - 2.0,
                    ..rect
                },
            ] {
                fill(canvas, edge, &soft);
            }
            for (x, y, alpha) in [
                (0.0, 0.0, 87.0),
                (1.0, 0.0, 235.0),
                (0.0, 1.0, 235.0),
                (1.0, 1.0, 26.0),
            ] {
                for top in [rect.top + y, rect.bottom - y - 1.0] {
                    for left in [rect.left + x, rect.right - x - 1.0] {
                        fill(
                            canvas,
                            D2D_RECT_F {
                                left,
                                top,
                                right: left + 1.0,
                                bottom: top + 1.0,
                            },
                            &D2D1_COLOR_F {
                                a: alpha / 255.0,
                                ..theme.group_focus
                            },
                        );
                    }
                }
            }
        }
        let open = &[
            (0, 0, 0.081),
            (10, 0, 0.081),
            (0, 1, 0.756),
            (1, 1, 0.674),
            (9, 1, 0.674),
            (10, 1, 0.674),
            (1, 2, 0.844),
            (2, 2, 0.674),
            (8, 2, 0.674),
            (9, 2, 0.837),
            (2, 3, 0.844),
            (3, 3, 0.674),
            (7, 3, 0.674),
            (8, 3, 0.837),
            (3, 4, 0.844),
            (4, 4, 0.674),
            (6, 4, 0.674),
            (7, 4, 0.837),
            (4, 5, 0.844),
            (5, 5, 1.0),
            (6, 5, 0.837),
            (5, 6, 0.511),
        ];
        let closed = &[
            (0, 0, 0.081),
            (1, 0, 0.674),
            (1, 1, 0.674),
            (2, 1, 0.837),
            (2, 2, 0.674),
            (3, 2, 0.837),
            (3, 3, 0.674),
            (4, 3, 0.837),
            (4, 4, 0.674),
            (5, 4, 0.837),
            (5, 5, 1.0),
            (6, 5, 0.504),
            (4, 6, 0.674),
            (5, 6, 0.837),
            (3, 7, 0.674),
            (4, 7, 0.837),
            (2, 8, 0.674),
            (3, 8, 0.837),
            (1, 9, 0.674),
            (2, 9, 0.837),
            (0, 10, 0.081),
            (1, 10, 0.756),
        ];
        for &(px, py, coverage) in if column_major {
            &[]
        } else if header.folded {
            closed.as_slice()
        } else {
            open.as_slice()
        } {
            let (x, dy) = if header.folded {
                (8.0 + px as f32, 11.0 + py as f32)
            } else {
                (6.0 + px as f32, 13.0 + py as f32)
            };
            fill(
                canvas,
                D2D_RECT_F {
                    left: left + x,
                    top: y + dy,
                    right: left + x + 1.0,
                    bottom: y + dy + 1.0,
                },
                &D2D1_COLOR_F {
                    a: coverage,
                    ..theme.chevron
                },
            );
        }
        let (name, subtitle) = header
            .label
            .split_once(crate::app::GROUP_SUBTITLE)
            .map_or((header.label.as_str(), None), |(name, path)| {
                (name, Some(path))
            });
        let x = left + if column_major { 6.0 } else { 21.0 };
        draw_cell(
            canvas,
            text,
            name,
            style,
            D2D_RECT_F {
                left: x,
                top: y,
                right: right - 4.0,
                bottom: y + 32.0,
            },
            0.0,
            &theme.group_text,
        )?;
        if let Some(subtitle) = subtitle {
            draw_cell(
                canvas,
                text,
                subtitle,
                super::text::DIRECTUI_KERNED,
                D2D_RECT_F {
                    left: x,
                    top: y + 25.0,
                    right: right - 4.0,
                    bottom: y + 43.0,
                },
                0.0,
                &theme.text,
            )?;
            if header.rows.is_empty() && !header.folded {
                draw_cell(
                    canvas,
                    text,
                    &crate::i18n::shell_string(4159, ""),
                    super::text::DIRECTUI_KERNED,
                    D2D_RECT_F {
                        left: left + 21.0,
                        top: y + 50.0,
                        right: right - 4.0,
                        bottom: y + 82.0,
                    },
                    0.0,
                    &theme.text,
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn draw_check(
    canvas: &Canvas,
    assets: &mut Assets,
    theme: &Theme,
    point: (f32, f32),
    state: super::check::State,
) -> Result<()> {
    let mut dpi = 96.0;
    let mut dpi_y = 96.0;
    unsafe {
        canvas.context.GetDpi(&mut dpi, &mut dpi_y);
    }
    let key = (state, dpi.round() as u32, theme.content.r > 0.5);
    if !assets.checks.contains_key(&key) {
        assets.checks.clear();
        for state in [super::check::State::Clear, super::check::State::Selected] {
            assets.checks.insert(
                (state, key.1, key.2),
                canvas.system_icon(&super::check::image(state, key.1)?)?,
            );
        }
    }
    let (x, y) = point;
    unsafe {
        canvas.context.DrawBitmap(
            &assets.checks[&key],
            Some(&D2D_RECT_F {
                left: x,
                top: y,
                right: x + 13.0,
                bottom: y + 13.0,
            }),
            1.0,
            D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            None,
            None,
        );
    }
    Ok(())
}

pub(super) fn draw_sliced(canvas: &Canvas, bitmap: &ID2D1Bitmap1, rect: D2D_RECT_F, scale: f32) {
    let xs = [rect.left, rect.left + 3.0, rect.right - 3.0, rect.right];
    let ys = [rect.top, rect.top + 3.0, rect.bottom - 3.0, rect.bottom];
    let src = [0.0, 3.0 * scale, 5.0 * scale, 8.0 * scale];
    for y in 0..3 {
        for x in 0..3 {
            unsafe {
                canvas.context.DrawBitmap(
                    bitmap,
                    Some(&D2D_RECT_F {
                        left: xs[x],
                        top: ys[y],
                        right: xs[x + 1],
                        bottom: ys[y + 1],
                    }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    Some(&D2D_RECT_F {
                        left: src[x],
                        top: src[y],
                        right: src[x + 1],
                        bottom: src[y + 1],
                    }),
                    None,
                );
            }
        }
    }
}

fn draw_group_background(
    canvas: &Canvas,
    assets: &mut Assets,
    theme: &Theme,
    rect: D2D_RECT_F,
    state: windows::Win32::UI::Controls::GROUPHEADERSTATES,
) -> Result<()> {
    let mut dpi = 96.0;
    let mut dpi_y = 96.0;
    unsafe {
        canvas.context.GetDpi(&mut dpi, &mut dpi_y);
    }
    let key = (state.0, dpi.round() as u32, theme.content.r > 0.5);
    if !assets.groups.contains_key(&key) {
        if assets.groups.len() >= 8 {
            assets.groups.clear();
        }
        assets.groups.insert(
            key,
            canvas.system_icon(&super::check::group(state, theme, key.1)?)?,
        );
    }
    draw_sliced(canvas, &assets.groups[&key], rect, dpi / 96.0);
    Ok(())
}

pub fn draw_empty(
    list: &List,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    view: Viewport,
) -> Result<()> {
    let Some(folder) = list.folder.as_ref() else {
        return Ok(());
    };
    if folder.item_count() > 0 || !folder.layout.headers.is_empty() || list.search.busy() {
        return Ok(());
    }
    let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
    let home = folder.is_home();
    let value = if home {
        crate::i18n::tr("empty.home").to_owned()
    } else if crate::app::ids::is_search(path) {
        crate::i18n::tr("empty.search").to_owned()
    } else {
        crate::i18n::shell_string(4159, "")
    };
    let style = if home {
        TextStyle::new(super::text::Family::Variable, 12.0, 400)
    } else {
        super::text::DIRECTUI_KERNED
    };
    let layout = text.layout(&value, style, 4096.0)?;
    text.draw(
        canvas,
        &value,
        style,
        (
            if home {
                ((view.right - layout.advance) / 2.0).ceil()
            } else {
                (view.right - layout.advance) / 2.0
            },
            if home { 36.0 } else { 39.0 - view.header },
        ),
        if home {
            &theme.text_tertiary
        } else {
            &theme.text
        },
    )
}

pub fn draw_network_notice(
    folder: &Folder,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    width: f32,
) -> Result<()> {
    let Some(value) = folder.network_notice.as_deref() else {
        return Ok(());
    };
    fill(
        canvas,
        super::address::rect(0.0, 0.0, width, 32.0),
        &theme.network_notice,
    );
    draw_line(
        canvas,
        text,
        "\u{e7ba}",
        TextStyle::new(super::text::Family::Icons, 16.0, 400),
        super::address::rect(12.0, 0.0, 16.0, 32.0),
        0.0,
        &theme.network_warning,
    )?;
    draw_cell(
        canvas,
        text,
        value,
        TextStyle::new(super::text::Family::Text, 14.0, 400),
        super::address::rect(40.0, 0.0, (width - 52.0).max(0.0), 32.0),
        0.0,
        &theme.text,
    )
}
