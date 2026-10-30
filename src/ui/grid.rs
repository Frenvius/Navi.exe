use std::collections::HashMap;
use std::sync::Arc;
use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

use super::{
    details,
    list::{Folder, List},
    render::Canvas,
    text::{TextEngine, DIRECTUI_KERNED},
    theme::Theme,
};
use crate::app::{grid_layout, ids};
use crate::shell::{icons::Rgba, text::IconLabel};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Icons,
    Drives,
    Gallery,
}

#[derive(Clone)]
pub struct Model {
    pub layout_id: usize,
    layout_source: Arc<crate::app::group::DetailsLayout>,
    pub kind: Kind,
    pub spec: grid_layout::Spec,
    pub size: (f32, f32),
    pub layout: grid_layout::Layout,
    pub labels: Vec<Option<Arc<IconLabel>>>,
    pub measured: usize,
    pub photos: Vec<crate::shell::gallery::Photo>,
    pub navigation: Vec<crate::app::drives_layout::NavRow>,
    widths: Vec<f32>,
    header_widths: Vec<f32>,
}

impl Model {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.labels.capacity() * std::mem::size_of::<Option<Arc<IconLabel>>>()
            + self
                .labels
                .iter()
                .flatten()
                .map(|label| {
                    std::mem::size_of::<IconLabel>()
                        + label.full.capacity()
                        + label.collapsed.capacity()
                })
                .sum::<usize>()
            + (self.widths.capacity() + self.header_widths.capacity()) * std::mem::size_of::<f32>()
            + self.photos.capacity() * std::mem::size_of::<crate::shell::gallery::Photo>()
            + self.layout.cells.capacity() * std::mem::size_of::<[f32; 4]>()
            + self.layout.headers.capacity() * std::mem::size_of::<[f32; 3]>()
            + self.layout.items.capacity() * std::mem::size_of::<(f32, f32)>()
            + self.layout.empty.capacity() * std::mem::size_of::<grid_layout::EmptyGroup>()
            + self
                .layout
                .empty
                .iter()
                .map(|group| group.label.capacity())
                .sum::<usize>()
            + self.navigation.capacity() * std::mem::size_of::<crate::app::drives_layout::NavRow>()
            + self
                .navigation
                .iter()
                .map(|row| match &row.kind {
                    crate::app::drives_layout::Kind::Cells(members) => {
                        members.capacity() * std::mem::size_of::<usize>()
                    }
                    _ => 0,
                })
                .sum::<usize>()
    }
    pub fn new(folder: &Folder, size: (f32, f32)) -> Option<Self> {
        let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
        if ids::same(path, ids::HOME) {
            return None;
        }
        let kind = if ids::same(path, ids::GALLERY) {
            Kind::Gallery
        } else if ids::same(path, ids::THIS_PC) && folder.view.mode == 7 {
            Kind::Drives
        } else {
            Kind::Icons
        };
        let mut spec = if kind == Kind::Gallery {
            grid_layout::spec(3, folder.view.icon_size)?
        } else {
            grid_layout::spec(folder.view.mode, folder.view.icon_size)?
        };
        if kind != Kind::Gallery && folder.settings.check_boxes {
            spec.label_indent = grid_layout::check_indent(&spec);
        }
        let count = folder.order.len();
        let mut model = Self {
            layout_id: Arc::as_ptr(&folder.layout) as usize,
            layout_source: folder.layout.clone(),
            kind,
            spec,
            size,
            navigation: Vec::new(),
            layout: grid_layout::Layout {
                empty: Vec::new(),
                cells: Vec::new(),
                headers: Vec::new(),
                items: Vec::new(),
                columns: 1,
                height: 0.0,
                width: 0.0,
            },
            labels: vec![None; count],
            measured: 0,
            widths: vec![0.0; count],
            header_widths: crate::shell::columns::widths(folder.headers.iter().map(String::as_str)),
            photos: if kind == Kind::Gallery {
                folder
                    .order
                    .iter()
                    .map(|&i| folder.entries[i].photo.unwrap_or_default())
                    .collect()
            } else {
                Vec::new()
            },
        };
        model.measure(folder, 64);
        Some(model)
    }

    pub fn bind_layout(&mut self, layout: Arc<crate::app::group::DetailsLayout>) {
        self.layout_id = Arc::as_ptr(&layout) as usize;
        self.layout_source = layout;
    }

    pub fn header(&self) -> f32 {
        if self.kind == Kind::Gallery {
            61.0
        } else {
            0.0
        }
    }

    pub fn measure(&mut self, folder: &Folder, batch: usize) {
        let end = (self.measured + batch).min(folder.order.len());
        if self.kind == Kind::Icons && self.spec.column_major {
            self.widths = crate::shell::columns::widths(
                folder
                    .order
                    .iter()
                    .map(|&i| folder.entries[i].name.as_str()),
            );
            self.measured = folder.order.len();
        } else if self.kind == Kind::Icons && self.spec.kind == 0 {
            let width = grid_layout::label_key(&self.spec) as i32;
            for row in self.measured..end {
                let name = &folder.entries[folder.order[row]].name;
                self.labels[row] = Some(Arc::new(crate::shell::text::icon_label(name, width)));
            }
            self.measured = end;
        } else {
            self.measured = folder.order.len();
        }
        self.arrange(folder);
    }

    pub fn arrange(&mut self, folder: &Folder) {
        if self.kind == Kind::Gallery {
            let height = (folder.view.icon_size as f32 * 2.0 + 64.0).clamp(96.0, 320.0);
            self.spec.base_h = height;
            self.spec.gap_y = 12.0;
            self.spec.kind = 4;
            let (cells, extent) =
                crate::app::gallery::mosaic(&self.photos, (self.size.0 - 120.0).max(64.0), height);
            let mut items = Vec::new();
            for cell in &cells {
                if items.last().is_none_or(|&(top, _)| top != cell[1]) {
                    items.push((cell[1], cell[1] + cell[3] + 12.0));
                }
            }
            self.layout.cells = cells;
            self.layout.height = extent;
            self.layout.items = items;
            return;
        }
        if self.kind == Kind::Drives {
            let device = folder
                .order
                .iter()
                .filter(|&&i| crate::app::drives_layout::is_device(&folder.entries[i]))
                .count();
            let counts = [device, folder.order.len() - device];
            let collapsed = [
                folder.collapsed.contains(crate::i18n::tr("tiles.drives")),
                folder.collapsed.contains(crate::i18n::tr("tiles.network")),
            ];
            let columns = (((self.size.0 - 24.0) / 254.0).floor() as usize).max(1);
            let mut layout = crate::app::drives_layout::layout(counts, columns, collapsed);
            if layout.extent > self.size.1 {
                layout = crate::app::drives_layout::layout(
                    counts,
                    (((self.size.0 - 41.0) / 254.0).floor() as usize).max(1),
                    collapsed,
                );
            }
            self.layout.cells = layout.cells;
            self.layout.height = layout.extent;
            self.layout.items = layout.rows.iter().map(|r| (r.top, r.bottom)).collect();
            self.layout.headers = vec![[0.0, -1.0, -1.0]; folder.order.len()];
            for row in &layout.rows {
                if let crate::app::drives_layout::Kind::Header(index) = row.kind {
                    self.layout.headers[index] = [0.0, row.top, -1.0];
                }
            }
            self.navigation = layout.rows;
            self.layout.columns = columns;
            return;
        }
        let lines: Vec<_> = self
            .labels
            .iter()
            .map(|label| label.as_ref().map_or(1, |label| label.lines))
            .collect();
        let folded: Vec<_> = folder.layout.folded.clone();
        let groups = if folder.library_grouped() {
            folder.library_ranges()
        } else {
            grid_layout::ranges(&folder.headers, folder.order.len())
        };
        self.layout = grid_layout::arrange(
            &grid_layout::Input {
                count: folder.order.len(),
                groups: &groups,
                library: folder.library_grouped(),
                grouped: !folder.headers.is_empty(),
                headers: &folder.headers,
                folded: &folded,
                collapsed: &folder.collapsed,
                label_lines: &lines,
                label_widths: &self.widths,
                header_widths: &self.header_widths,
            },
            &self.spec,
            self.size,
        );
    }

    pub fn visible(
        &self,
        origin: (f32, f32),
        size: (f32, f32),
    ) -> impl Iterator<Item = usize> + '_ {
        self.layout
            .cells
            .iter()
            .enumerate()
            .filter(move |(_, cell)| {
                cell[2] > 0.0
                    && cell[3] > 0.0
                    && cell[0] < origin.0 + size.0
                    && cell[0] + cell[2] > origin.0
                    && cell[1] < origin.1 + size.1
                    && cell[1] + cell[3] > origin.1
            })
            .map(|(row, _)| row)
    }

    pub fn hit(&self, point: (f32, f32)) -> Option<usize> {
        self.visible(point, (1.0, 1.0)).next()
    }

    pub fn damage(
        &self,
        rows: std::ops::Range<usize>,
        origin: (f32, f32),
        size: (f32, f32),
    ) -> Option<[f32; 4]> {
        if rows.start == 0 && rows.end >= self.layout.cells.len() {
            return Some([origin.0, origin.1, origin.0 + size.0, origin.1 + size.1]);
        }
        let mut result: Option<[f32; 4]> = None;
        for row in self.visible(origin, size).filter(|row| rows.contains(row)) {
            let [x, y, width, height] = self.layout.cells[row];
            result = Some(match result {
                Some([left, top, right, bottom]) => [
                    left.min(x),
                    top.min(y),
                    right.max(x + width),
                    bottom.max(y + height),
                ],
                None => [x, y, x + width, y + height],
            });
        }
        result.map(|[left, top, right, bottom]| {
            [
                left.max(origin.0),
                top.max(origin.1),
                right.min(origin.0 + size.0),
                bottom.min(origin.1 + size.1),
            ]
        })
    }

    pub fn same_visible(&self, other: &Self, origin: (f32, f32), size: (f32, f32)) -> bool {
        if self.kind != other.kind
            || self.spec.kind != other.spec.kind
            || self.spec.icon != other.spec.icon
            || self.spec.label_indent != other.spec.label_indent
            || self.spec.column_major != other.spec.column_major
        {
            return false;
        }
        let rows: Vec<_> = self.visible(origin, size).collect();
        if rows.iter().copied().ne(other.visible(origin, size)) {
            return false;
        }
        rows.into_iter().all(|row| {
            self.layout.cells[row] == other.layout.cells[row]
                && match (&self.labels[row], &other.labels[row]) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                }
        })
    }

    pub fn range(&self, origin: (f32, f32), size: (f32, f32)) -> std::ops::Range<usize> {
        let mut visible = self.visible(origin, size);
        let Some(first) = visible.next() else {
            return 0..0;
        };
        let last = visible.last().unwrap_or(first);
        first.saturating_sub(4)..(last + 5).min(self.layout.cells.len())
    }
}

pub struct Picture {
    pub image: Option<Arc<Rgba>>,
    pub fields: Vec<Option<(String, String, f32)>>,
    pub label: Option<String>,
}

#[derive(Default)]
pub struct Retries {
    entries: HashMap<usize, (u8, std::time::Instant)>,
    visible: std::ops::Range<usize>,
}

impl Retries {
    pub fn retain(&mut self, rows: std::ops::Range<usize>) {
        self.entries.retain(|row, _| rows.contains(row));
        self.visible = rows;
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn complete(&mut self, row: usize) {
        self.entries.remove(&row);
    }
    pub fn missing(&mut self, row: usize) {
        if !self.visible.contains(&row) {
            return;
        }
        let attempt = self.entries.get(&row).map_or(1, |entry| entry.0 + 1);
        self.entries.insert(
            row,
            (
                attempt,
                std::time::Instant::now() + crate::app::gallery::retry_interval(attempt),
            ),
        );
    }
    pub fn wait(&self) -> Option<std::time::Duration> {
        self.entries
            .values()
            .filter(|entry| entry.0 < 8)
            .map(|entry| entry.1.saturating_duration_since(std::time::Instant::now()))
            .min()
    }
    pub fn due(&self) -> Vec<usize> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.0 < 8 && entry.1 <= std::time::Instant::now())
            .map(|(&row, _)| row)
            .collect()
    }
}

pub fn picture(folder: &Folder, row: usize) -> Picture {
    let entry = &folder.entries[folder.order[row]];
    let Some(model) = &folder.grid else {
        return Picture {
            image: None,
            fields: Vec::new(),
            label: None,
        };
    };
    let size = model.spec.icon as u32;
    let result = if model.kind == Kind::Icons && size == 16 {
        super::icons::small(if entry.icon_pending {
            crate::shell::icons::file_icon(&entry.path)
        } else {
            entry.icon
        })
    } else {
        let pixels = if model.kind == Kind::Gallery {
            crate::shell::icons::gallery_picture(&entry.path, &entry.absolute, 768)
        } else if model.kind == Kind::Drives {
            crate::shell::icons::icon(entry.icon & 0x00ff_ffff, 48)
        } else if folder
            .crumbs
            .last()
            .is_some_and(|crumb| ids::is_library(&crumb.id))
        {
            crate::shell::icons::library_picture(&entry.path, size)
        } else if entry.is_fs {
            crate::shell::icons::view_picture(&entry.path, size)
        } else {
            crate::shell::icons::network_picture(&entry.path, &entry.absolute, size)
        };
        pixels.and_then(super::icons::shared)
    };
    let image = match result {
        Ok(image) => Some(image),
        Err(error) => {
            if model.kind != Kind::Gallery {
                eprintln!("native picture {}: {error}", entry.path);
            }
            None
        }
    };
    let fields = if model.spec.kind >= 2 && model.kind == Kind::Icons {
        let list = if model.spec.kind == 2 {
            crate::shell::listing::PropList::Tile
        } else if folder
            .crumbs
            .last()
            .is_some_and(|crumb| crate::app::ids::is_search(&crumb.id))
        {
            crate::shell::listing::PropList::ContentSearch
        } else {
            crate::shell::listing::PropList::Content
        };
        let lines = crate::shell::listing::prop_list_with_id(&entry.path, list, &entry.absolute);
        lines
            .into_iter()
            .map(|line| {
                line.map(|(label, value)| {
                    let width = crate::shell::columns::widest([format!("{label}:").as_str()])
                        + if label.is_empty() { 0.0 } else { 4.0 }
                        + crate::shell::columns::widest([value.as_str()]);
                    (label, value, width)
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let label = (model.spec.kind == 2 && model.kind == Kind::Icons).then(|| {
        crate::shell::text::tile_label(&entry.name, (189.0 - model.spec.label_indent) as i32)
    });
    Picture {
        image,
        fields,
        label,
    }
}

#[derive(Default)]
pub struct Assets {
    pictures: HashMap<usize, (Arc<Rgba>, std::rc::Rc<ID2D1Bitmap1>)>,
    checks: details::Assets,
    plates: Option<(bool, Vec<ID2D1Bitmap1>)>,
}

pub fn draw(
    list: &List,
    assets: &mut Assets,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    view: details::Viewport,
    rect: D2D_RECT_F,
) -> Result<()> {
    let Some(folder) = &list.folder else {
        return Ok(());
    };
    let Some(model) = &folder.grid else {
        return Ok(());
    };
    assets.pictures.retain(|row, (source, _)| {
        list.rows
            .get(row)
            .and_then(|ready| ready.picture.as_ref())
            .and_then(|picture| picture.image.as_ref())
            .is_some_and(|image| Arc::ptr_eq(image, source))
    });
    for row in model.visible(
        (rect.left, rect.top),
        (rect.right - rect.left, rect.bottom - rect.top),
    ) {
        let [x, y, width, height] = model.layout.cells[row];
        let cell = D2D_RECT_F {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        };
        let entry = &folder.entries[folder.order[row]];
        let state = &list.interaction;
        let selected = state.selection.selected.contains(&entry.path);
        let dropping = state.drop_row == Some(row);
        let focused =
            state.group_focus.is_none() && state.selection.focused.as_ref() == Some(&entry.path);
        let hover = state.hover == Some(row);
        let active = state.active && state.has_focus;
        let multi = active && selected && state.selection.selected.len() > 1 && !focused;
        let ready = list.rows.get(&row).and_then(|row| row.picture.as_ref());
        if model.kind == Kind::Gallery {
            if let Some(image) = ready.and_then(|picture| picture.image.as_ref()) {
                if !assets.pictures.contains_key(&row) {
                    assets
                        .pictures
                        .insert(row, (image.clone(), canvas.shared_icon(image)?));
                }
            }
            let geometry = unsafe {
                canvas
                    .context
                    .GetFactory()?
                    .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                        rect: cell,
                        radiusX: 4.0,
                        radiusY: 4.0,
                    })?
            };
            let mask = std::mem::ManuallyDrop::new(Some(geometry.cast()?));
            let mut parameters = D2D1_LAYER_PARAMETERS1 {
                contentBounds: cell,
                geometricMask: mask,
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                opacity: 1.0,
                ..Default::default()
            };
            parameters.maskTransform.M11 = 1.0;
            parameters.maskTransform.M22 = 1.0;
            unsafe {
                canvas.context.PushLayer(&parameters, None);
                std::mem::ManuallyDrop::drop(&mut parameters.geometricMask);
            }
            details::fill(
                canvas,
                cell,
                &D2D1_COLOR_F {
                    r: 32.0 / 255.0,
                    g: 32.0 / 255.0,
                    b: 32.0 / 255.0,
                    a: 1.0,
                },
            );
            if let Some(image) = ready.and_then(|picture| picture.image.as_ref()) {
                let scale = (width / image.width as f32).max(height / image.height as f32);
                let crop = (width / scale, height / scale);
                let left = (image.width as f32 - crop.0) / 2.0;
                let top = (image.height as f32 - crop.1) / 2.0;
                unsafe {
                    canvas.context.DrawBitmap(
                        assets.pictures[&row].1.as_ref(),
                        Some(&cell),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        Some(&D2D_RECT_F {
                            left,
                            top,
                            right: left + crop.0,
                            bottom: top + crop.1,
                        }),
                        None,
                    );
                }
            }
            if selected || hover || focused && active && state.cues {
                let thickness = if selected || focused && active && state.cues {
                    2.0
                } else {
                    1.0
                };
                for inset in 0..thickness as usize {
                    border(
                        canvas,
                        D2D_RECT_F {
                            left: x + inset as f32,
                            top: y + inset as f32,
                            right: x + width - inset as f32,
                            bottom: y + height - inset as f32,
                        },
                        if selected {
                            &theme.accent
                        } else if focused && active && state.cues {
                            &theme.dui_text
                        } else {
                            &theme.focus_rect
                        },
                    );
                }
            }
            unsafe {
                canvas.context.PopLayer();
            }
            if !state.selection.selected.is_empty() || hover || focused && active && state.cues {
                gallery_check(
                    canvas,
                    text,
                    theme,
                    (x + width - 24.0, y + 4.0),
                    if selected {
                        super::check::State::Selected
                    } else {
                        super::check::State::Clear
                    },
                )?;
            }
            continue;
        }
        let fill = if selected {
            Some(if multi {
                theme.row_multi
            } else if !active {
                theme.nav_selected
            } else if hover && !folder.settings.single_click {
                theme.row_selected_hover
            } else {
                theme.row_selected
            })
        } else if hover {
            Some(theme.row_hover)
        } else {
            None
        };
        if model.kind == Kind::Drives && (selected || dropping) {
            let light = theme.content.r > 0.5;
            if assets.plates.as_ref().is_none_or(|(old, _)| *old != light) {
                let bytes: [&[u8]; 3] = if light {
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
                assets.plates = Some((
                    light,
                    bytes
                        .iter()
                        .map(|pixels| {
                            canvas.system_icon(&Rgba {
                                width: 8,
                                height: 8,
                                pixels: pixels.to_vec(),
                            })
                        })
                        .collect::<Result<_>>()?,
                ));
            }
            let plate = if dropping || hover {
                0
            } else if !active {
                2
            } else if !focused {
                1
            } else {
                0
            };
            details::draw_sliced(canvas, &assets.plates.as_ref().unwrap().1[plate], cell, 1.0);
        } else if model.kind == Kind::Drives {
            if let Some(fill) = fill {
                details::fill(canvas, cell, &fill);
            }
        } else {
            details::draw_plate(list, &mut assets.checks, canvas, theme, view, row)?;
        }
        if let Some(image) = ready.and_then(|picture| picture.image.as_ref()) {
            if !assets.pictures.contains_key(&row) {
                assets
                    .pictures
                    .insert(row, (image.clone(), canvas.shared_icon(image)?));
            }
            let image = assets.pictures[&row].1.as_ref();
            let side = model.spec.icon;
            let image_x = x
                + model.spec.label_indent
                + if model.spec.kind == 0 {
                    ((width - side) / 2.0).floor()
                } else if model.spec.kind == 1 {
                    32.0
                } else if model.spec.kind == 3 {
                    34.0
                } else {
                    4.0
                };
            let image_y = y + if model.spec.kind == 3 { 7.0 } else { 2.0 };
            unsafe {
                canvas.context.DrawBitmap(
                    image,
                    Some(&D2D_RECT_F {
                        left: image_x,
                        top: image_y,
                        right: image_x + side,
                        bottom: image_y + side,
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
        let color = match crate::app::format::name_style(entry, &folder.settings) {
            1 => theme.compressed_text,
            2 => theme.encrypted_text,
            _ => theme.dui_text,
        };
        if folder.settings.check_boxes && (selected || hover) {
            details::draw_check(
                canvas,
                &mut assets.checks,
                theme,
                (x + 4.0, y + grid_layout::check_y(&model.spec, height)),
                if selected {
                    super::check::State::Selected
                } else {
                    super::check::State::Clear
                },
            )?;
        }
        if model.kind == Kind::Drives {
            let capacity = crate::app::drives_layout::capacity(entry);
            details::draw_cell(
                canvas,
                text,
                &entry.name,
                DIRECTUI_KERNED,
                D2D_RECT_F {
                    left: x + 57.0 + model.spec.label_indent,
                    top: y + if capacity.is_some() { 1.0 } else { 8.0 },
                    right: x + width - 4.0,
                    bottom: y + if capacity.is_some() { 17.0 } else { 24.0 },
                },
                0.0,
                &color,
            )?;
            if let Some(capacity) = capacity {
                let bar = D2D_RECT_F {
                    left: x + 56.0 + model.spec.label_indent,
                    top: y + 19.0,
                    right: x + 245.0,
                    bottom: y + 32.0,
                };
                details::fill(canvas, bar, &theme.drive_bar);
                border(canvas, bar, &theme.drive_bar_border);
                details::fill(
                    canvas,
                    D2D_RECT_F {
                        left: bar.left + 1.0,
                        top: bar.top + 1.0,
                        right: bar.left
                            + 1.0
                            + ((bar.right - bar.left - 2.0) * capacity.used).ceil(),
                        bottom: bar.bottom - 1.0,
                    },
                    if capacity.low {
                        &theme.drive_full
                    } else {
                        &theme.drive_fill
                    },
                );
                details::draw_cell(
                    canvas,
                    text,
                    &capacity.text,
                    DIRECTUI_KERNED,
                    D2D_RECT_F {
                        left: x + 57.0 + model.spec.label_indent,
                        top: y + 32.0,
                        right: x + width - 4.0,
                        bottom: y + 48.0,
                    },
                    0.0,
                    if selected { &color } else { &theme.header_text },
                )?;
            }
            continue;
        }
        match model.spec.kind {
            0 => {
                let fallback;
                let label = if let Some(label) = model.labels.get(row).and_then(Option::as_ref) {
                    label.as_ref()
                } else {
                    fallback = crate::shell::text::icon_label(
                        &entry.name,
                        grid_layout::label_key(&model.spec) as i32,
                    );
                    &fallback
                };
                for (line, value) in label.collapsed.lines().enumerate() {
                    let advance = text.layout(value, super::text::DIRECTUI, 4096.0)?.width;
                    let room = if label.width >= 4 {
                        (label.width - 4) as f32
                    } else {
                        width - 12.0
                    };
                    let left = x + grid_layout::label_x(width, label.width, room);
                    let top = y + model.spec.icon + 2.0 + line as f32 * 15.0;
                    let height = text.layout(value, DIRECTUI_KERNED, 4096.0)?.height;
                    details::draw_line(
                        canvas,
                        text,
                        value,
                        DIRECTUI_KERNED,
                        D2D_RECT_F {
                            left,
                            top,
                            right: left + room,
                            bottom: top + height,
                        },
                        ((room - advance) / 2.0).floor(),
                        &color,
                    )?;
                }
            }
            1 => {
                let bounds = D2D_RECT_F {
                    left: x + model.spec.icon + 34.0 + model.spec.label_indent,
                    top: y,
                    right: x + width - 6.0,
                    bottom: y + height - 2.0,
                };
                if model.spec.column_major {
                    let advance = text
                        .layout(&entry.name, super::text::DIRECTUI, 4096.0)?
                        .width;
                    details::draw_line(
                        canvas,
                        text,
                        &entry.name,
                        DIRECTUI_KERNED,
                        D2D_RECT_F {
                            right: (bounds.left + advance).min(bounds.right),
                            ..bounds
                        },
                        0.0,
                        &color,
                    )?;
                } else {
                    details::draw_cell(
                        canvas,
                        text,
                        &entry.name,
                        DIRECTUI_KERNED,
                        bounds,
                        0.0,
                        &color,
                    )?;
                }
            }
            2 => {
                let label = ready
                    .and_then(|ready| ready.label.as_deref())
                    .unwrap_or(&entry.name);
                let lines = label.lines().count().min(2);
                let fields = ready.map_or(&[][..], |ready| ready.fields.as_slice());
                let props = if entry.is_dir {
                    2usize.saturating_sub(lines)
                } else {
                    fields.len().max(1).min(3 - lines)
                };
                let top = y + if lines + props >= 3 { 1.0 } else { 8.0 };
                for (line, value) in label.lines().take(2).enumerate() {
                    let value = text.elide(
                        value,
                        DIRECTUI_KERNED,
                        width - 61.0 - model.spec.label_indent,
                    )?;
                    text.draw(
                        canvas,
                        &value,
                        DIRECTUI_KERNED,
                        (
                            x + 57.0 + model.spec.label_indent,
                            top + line as f32 * 15.0
                                + if lines > 1 && props == 0 { 2.0 } else { 1.0 },
                        ),
                        &color,
                    )?;
                }
                for (line, field) in fields.iter().take(props).enumerate() {
                    if let Some((_, value, _)) = field {
                        let top = top - if lines > 1 { 1.0 } else { 0.0 };
                        details::draw_cell(
                            canvas,
                            text,
                            value,
                            DIRECTUI_KERNED,
                            D2D_RECT_F {
                                left: x + 57.0 + model.spec.label_indent,
                                top: top + (lines + line) as f32 * 16.0,
                                right: x + width - 4.0,
                                bottom: top + (lines + line + 1) as f32 * 16.0,
                            },
                            0.0,
                            if selected || hover {
                                &theme.cell_active_text
                            } else {
                                &theme.header_text
                            },
                        )?;
                    }
                }
            }
            _ => {
                let search = folder
                    .crumbs
                    .last()
                    .is_some_and(|crumb| crate::app::ids::is_search(&crumb.id));
                let name_style = if search {
                    super::text::SEARCH_NAME
                } else {
                    super::text::GROUP_HEADER
                };
                let fields = ready.map_or(&[][..], |ready| ready.fields.as_slice());
                let layout = grid_layout::content(width, fields, model.spec.label_indent);
                let measure = super::text::TextStyle::new(super::text::Family::DirectUi, 15.0, 400);
                let name = text.elide(&entry.name, measure, layout.name_width)?;
                let name_height = text.layout(&name, name_style, 4096.0)?.height;
                super::search::highlight(
                    canvas,
                    text,
                    theme,
                    list,
                    &name,
                    D2D_RECT_F {
                        left: x + 73.0 + model.spec.label_indent,
                        top: y + 5.0,
                        right: x + 73.0 + model.spec.label_indent + layout.name_width,
                        bottom: y + 26.0,
                    },
                    name_style,
                )?;
                details::draw_line(
                    canvas,
                    text,
                    &name,
                    name_style,
                    D2D_RECT_F {
                        left: x + 73.0 + model.spec.label_indent,
                        top: y + 5.0,
                        right: x + 73.0 + model.spec.label_indent + layout.name_width,
                        bottom: y + 5.0 + name_height,
                    },
                    0.0,
                    &color,
                )?;
                for (slot, &(left, top, room)) in layout
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, field)| field.2 > 0.0)
                {
                    if let Some((label, value, _)) = fields[slot].as_ref() {
                        let indent = if label.is_empty() {
                            0.0
                        } else {
                            let label = format!("{label}:");
                            text.draw(
                                canvas,
                                &label,
                                super::text::DIRECTUI,
                                (x + left, y + top),
                                if selected || hover {
                                    &theme.cell_active_text
                                } else {
                                    &theme.header_text
                                },
                            )?;
                            crate::shell::text::underline_widths_at(&label, 12, f32::MAX)
                                .first()
                                .copied()
                                .unwrap_or(0.0)
                                + 4.0
                        };
                        let height = text.layout(value, DIRECTUI_KERNED, 4096.0)?.height;
                        details::draw_cell(
                            canvas,
                            text,
                            value,
                            DIRECTUI_KERNED,
                            D2D_RECT_F {
                                left: x + left + indent,
                                top: y + top,
                                right: x + left + room
                                    - if search && label.is_empty() { 5.0 } else { 0.0 },
                                bottom: y + top + height,
                            },
                            0.0,
                            &theme.dui_text,
                        )?;
                    }
                }
                if !(search && focused && active && state.cues) {
                    details::fill(
                        canvas,
                        D2D_RECT_F {
                            left: x + 4.0,
                            top: y + 48.0,
                            right: x + width - 9.0,
                            bottom: y + 49.0,
                        },
                        &theme.content_line,
                    );
                }
            }
        }
    }
    Ok(())
}

fn gallery_check(
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    position: (f32, f32),
    state: super::check::State,
) -> Result<()> {
    let selected = state == super::check::State::Selected;
    let (x, y) = position;
    let shape = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: x + 0.5,
            top: y + 0.5,
            right: x + 19.5,
            bottom: y + 19.5,
        },
        radiusX: 4.0,
        radiusY: 4.0,
    };
    unsafe {
        canvas.brush.SetColor(if selected {
            &theme.accent
        } else {
            &theme.content
        });
        canvas.context.FillRoundedRectangle(&shape, &canvas.brush);
        canvas.brush.SetColor(if selected {
            &theme.accent
        } else {
            &theme.thumb
        });
        canvas
            .context
            .DrawRoundedRectangle(&shape, &canvas.brush, 1.0, None);
    }
    if selected {
        text.draw(
            canvas,
            "\u{e73e}",
            super::text::TextStyle::new(super::text::Family::Icons, 12.0, 400),
            (x + 4.0, y + 4.0),
            &theme.content,
        )?;
    }
    Ok(())
}

pub fn border(canvas: &Canvas, rect: D2D_RECT_F, color: &D2D1_COLOR_F) {
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
        details::fill(canvas, edge, color);
    }
}

pub fn draw_gallery_header(
    list: &List,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    offset: f32,
) -> Result<()> {
    let Some(model) = list.folder.as_ref().and_then(|folder| folder.grid.as_ref()) else {
        return Ok(());
    };
    let timeline = crate::app::gallery::timeline(
        &model.photos,
        &model.layout.cells,
        model.layout.height,
        model.size.1,
        offset,
        (model.size.1 - 61.0).max(1.0),
        None,
    );
    details::draw_cell(
        canvas,
        text,
        &timeline.heading,
        super::text::TextStyle::new(super::text::Family::Small, 14.0, 600),
        D2D_RECT_F {
            left: 32.0,
            top: 14.0,
            right: (model.size.0 - 88.0).max(32.0),
            bottom: 43.0,
        },
        0.0,
        &theme.dui_text,
    )
}

pub fn draw_timeline(
    model: &Model,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    offset: f32,
) -> Result<()> {
    let timeline = crate::app::gallery::timeline(
        &model.photos,
        &model.layout.cells,
        model.layout.height,
        model.size.1,
        offset,
        (model.size.1 - 61.0).max(1.0),
        None,
    );
    let style = super::text::TextStyle::new(super::text::Family::Small, 14.0, 400);
    for (date, y) in timeline.dates {
        text.draw(canvas, &date, style, (2.0, y), &theme.dui_text)?;
    }
    unsafe {
        canvas.brush.SetColor(&theme.accent);
        canvas.context.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 3.0,
                    top: timeline.indicator,
                    right: 33.0,
                    bottom: timeline.indicator + 3.0,
                },
                radiusX: 1.5,
                radiusY: 1.5,
            },
            &canvas.brush,
        );
    }
    let pixels = [
        (2, 0, 0.339),
        (3, 0, 0.926),
        (4, 0, 0.926),
        (5, 0, 0.339),
        (2, 1, 0.891),
        (3, 1, 1.0),
        (4, 1, 1.0),
        (5, 1, 0.891),
        (1, 2, 0.696),
        (2, 2, 1.0),
        (3, 2, 1.0),
        (4, 2, 1.0),
        (5, 2, 1.0),
        (6, 2, 0.696),
        (0, 3, 0.339),
        (1, 3, 1.0),
        (2, 3, 1.0),
        (3, 3, 1.0),
        (4, 3, 1.0),
        (5, 3, 1.0),
        (6, 3, 1.0),
        (7, 3, 0.339),
        (0, 4, 0.891),
        (1, 4, 1.0),
        (2, 4, 1.0),
        (3, 4, 1.0),
        (4, 4, 1.0),
        (5, 4, 1.0),
        (6, 4, 1.0),
        (7, 4, 0.891),
        (0, 5, 0.891),
        (1, 5, 1.0),
        (2, 5, 1.0),
        (3, 5, 1.0),
        (4, 5, 1.0),
        (5, 5, 1.0),
        (6, 5, 1.0),
        (7, 5, 0.891),
    ];
    for (top, down) in [(67.0, false), (model.size.1 - 27.0, true)] {
        for (x, y, a) in pixels {
            details::fill(
                canvas,
                D2D_RECT_F {
                    left: 14.0 + x as f32,
                    top: top + if down { 5 - y } else { y } as f32,
                    right: 15.0 + x as f32,
                    bottom: top + if down { 5 - y } else { y } as f32 + 1.0,
                },
                &D2D1_COLOR_F {
                    a,
                    ..theme.dui_text
                },
            );
        }
    }
    Ok(())
}

pub fn set_mode(hwnd: windows::Win32::Foundation::HWND, mode: i32, size: u32) {
    let path = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|window| window.tabs.active().location.tab.path().to_owned())
    });
    if path.as_ref().is_some_and(|path| {
        ids::same(path, ids::HOME) || ids::same(path, ids::GALLERY) && !matches!(mode, 1..=4)
    }) {
        return;
    }
    let unchanged = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.list.folder.as_ref())
            .is_some_and(|folder| {
                folder.view.mode
                    == if ids::same(path.as_deref().unwrap_or(""), ids::GALLERY) {
                        grid_layout::icon_mode(size)
                    } else {
                        mode
                    }
                    && folder.view.icon_size == size
            })
    });
    if unchanged {
        return;
    }
    super::input::cancel(hwnd);
    stop_animation(hwnd);
    let result = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
            window.scroll.x = 0.0;
            let gallery = window
                .list
                .folder
                .as_ref()
                .and_then(|folder| folder.grid.as_ref())
                .filter(|model| model.kind == Kind::Gallery);
            let mode = if let Some(model) = gallery {
                window.scroll.y *=
                    (size as f32 * 2.0 + 64.0).clamp(96.0, 320.0) / model.spec.base_h;
                grid_layout::icon_mode(size)
            } else {
                window.scroll.y = 0.0;
                mode
            };
            window.list.header.focus = None;
            window.list.set_mode(mode, size)
        })
    });
    if let Err(error) = result {
        super::fail(windows::core::Error::new(
            windows::Win32::Foundation::E_FAIL,
            error,
        ));
    }
    super::invalidate_all(hwnd);
}

const ANIMATION: usize = 23;

pub fn start_animation(hwnd: windows::Win32::Foundation::HWND) {
    let running = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.scroll.gallery_motion.is_some())
    });
    if running {
        super::input::arm(hwnd, ANIMATION, 16);
    }
}

fn stop_animation(hwnd: windows::Win32::Foundation::HWND) {
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.scroll.gallery_motion = None;
        }
    });
    super::input::stop(hwnd, ANIMATION);
}

pub fn timer(hwnd: windows::Win32::Foundation::HWND, timer: usize) -> bool {
    if timer != ANIMATION {
        return false;
    }
    let (changed, done) = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((false, true), |window| {
            let changed = window.scroll.animate_gallery();
            (changed, window.scroll.gallery_motion.is_none())
        })
    });
    if done {
        super::input::stop(hwnd, ANIMATION);
    }
    if changed {
        super::invalidate_view(hwnd);
    }
    true
}

pub fn timeline_press(hwnd: windows::Win32::Foundation::HWND, point: (f32, f32)) -> bool {
    let used = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let Some(model) = window
                .list
                .folder
                .as_ref()
                .and_then(|folder| folder.grid.as_ref())
                .filter(|model| model.kind == Kind::Gallery)
            else {
                return false;
            };
            let Some(view) = window.scroll.view else {
                return false;
            };
            let (x, y) = (point.0 - view.x, point.1 - view.y);
            if x < view.width - 53.0 || x >= view.width || y < 61.0 || y >= view.height {
                return false;
            }
            window.scroll.gallery_motion = None;
            if y < 75.0 || y > view.height - 31.0 {
                let target = (window.scroll.y + if y < 75.0 { -48.0 } else { 48.0 })
                    .clamp(0.0, view.maximum_y());
                window.scroll.gallery_motion =
                    Some((window.scroll.y, target, std::time::Instant::now()));
            } else {
                let timeline = crate::app::gallery::timeline(
                    &model.photos,
                    &model.layout.cells,
                    model.layout.height,
                    model.size.1,
                    window.scroll.y,
                    view.rows_height,
                    None,
                );
                window.scroll.y = crate::app::gallery::scrub_offset(
                    &model.photos,
                    &model.layout.cells,
                    model.layout.height,
                    view.height,
                    window.scroll.y,
                    &timeline.dates,
                    y,
                )
                .clamp(0.0, view.maximum_y());
            }
            true
        })
    });
    if used {
        super::chrome::list_focus(hwnd);
        start_animation(hwnd);
        super::invalidate_view(hwnd);
    }
    used
}

pub fn key(hwnd: windows::Win32::Foundation::HWND, key: u32) -> bool {
    let mods = super::input::modifiers();
    if !mods.ctrl || !mods.shift || !(0x31..=0x38).contains(&key) {
        return false;
    }
    let mode = (key - 0x30) as i32;
    set_mode(hwnd, mode, grid_layout::mode_icon_size(mode));
    true
}

pub fn zoom(hwnd: windows::Win32::Foundation::HWND, delta: i32) -> bool {
    if !super::input::modifiers().ctrl {
        return false;
    }
    let next = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.list.folder.as_ref())
            .and_then(|folder| {
                let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
                if ids::same(path, ids::GALLERY) {
                    return Some((
                        3,
                        crate::app::gallery::zoom_size(folder.view.icon_size, delta / 120),
                    ));
                }
                grid_layout::zoom_index(folder.view.mode, folder.view.icon_size).map(|index| {
                    grid_layout::zoom_step(
                        (index as i32 + delta / 120).clamp(0, grid_layout::ZOOM_STEPS as i32 - 1)
                            as usize,
                    )
                })
            })
    });
    if let Some((mode, size)) = next {
        set_mode(hwnd, mode, size);
    }
    true
}

pub fn collection_menu(hwnd: windows::Win32::Foundation::HWND) {
    super::menu_actions::collection(hwnd, super::menu::Origin::Pointer);
}
