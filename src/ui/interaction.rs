use std::collections::HashSet;
use std::ops::Range;
use std::time::Instant;

use super::{details::Viewport, list::Folder, scroll};
use crate::app::selection::{self, Modifiers, Selection};

pub enum Action {
    Open(Vec<String>),
    Back,
    Forward,
    Up,
    Expand(String, crate::app::group::Expansion),
}

pub struct Band {
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub moved: bool,
    base: HashSet<String>,
    mods: Modifiers,
}

pub struct State {
    pub selection: Selection,
    pub hover: Option<usize>,
    pub home_button: Option<crate::app::home::RowButton>,
    pub drop_row: Option<usize>,
    pub group_focus: Option<String>,
    pub group_hover: Option<String>,
    pub group_action: Option<Action>,
    control_pressed: bool,
    pub has_focus: bool,
    pub active: bool,
    pub cues: bool,
    pub pointer: Option<(f32, f32)>,
    pub band: Option<Band>,
    pub hover_pending: bool,
    hover_anchor: Option<(usize, f32, f32)>,
    narrow: Option<usize>,
    pressed: Option<usize>,
    dirty: Option<Range<usize>>,
    typeahead: (Instant, String),
    last_open: Option<(String, Instant)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selection: Selection::default(),
            hover: None,
            home_button: None,
            drop_row: None,
            group_focus: None,
            group_hover: None,
            group_action: None,
            control_pressed: false,
            has_focus: false,
            active: true,
            cues: false,
            pointer: None,
            band: None,
            hover_pending: false,
            hover_anchor: None,
            narrow: None,
            pressed: None,
            dirty: None,
            typeahead: (Instant::now(), String::new()),
            last_open: None,
        }
    }
}

impl State {
    pub fn restore_view(&mut self, folder: &Folder) {
        self.selection.restored(&folder.order, &folder.entries);
        self.group_focus = if self.selection.selected.is_empty() {
            folder
                .layout
                .headers
                .first()
                .map(|header| header.label.clone())
        } else {
            None
        };
        self.mark_all();
    }

    pub fn drag_eligible(&self) -> bool {
        self.pressed.is_some() && self.band.is_none() && !self.control_pressed
    }

    pub fn has_press(&self) -> bool {
        self.pressed.is_some() || self.band.is_some() || self.control_pressed
    }
    pub fn mark_all(&mut self) {
        self.dirty = Some(0..usize::MAX);
    }
    fn mark(&mut self, row: usize) {
        self.dirty = Some(match self.dirty.take() {
            Some(old) => old.start.min(row)..old.end.max(row + 1),
            None => row..row + 1,
        });
    }
    pub fn take_dirty(&mut self) -> Option<Range<usize>> {
        self.dirty.take()
    }

    pub fn focus_row(&self, folder: &Folder) -> Option<usize> {
        self.selection
            .focused
            .as_ref()
            .and_then(|p| selection::row_of(&folder.order, &folder.entries, p))
    }

    fn page_point(&self, view: Viewport, scroll: &scroll::State) -> Option<(f32, f32)> {
        let (x, y) = self.pointer?;
        if x < view.x
            || x >= view.x + view.right
            || y < view.y + view.header
            || y >= view.y + view.bottom
        {
            return None;
        }
        Some((x - view.x + scroll.x, y - view.y - view.header + scroll.y))
    }

    fn home_button_at(
        &self,
        view: Viewport,
        scroll: &scroll::State,
        folder: &Folder,
    ) -> Option<crate::app::home::RowButton> {
        let row = self.hover.filter(|&row| row >= folder.home.quick)?;
        let model = folder
            .grid
            .as_ref()
            .filter(|model| model.kind == super::grid::Kind::Home)?;
        crate::app::home::row_button_at(
            *model.layout.cells.get(row)?,
            model.home.list_columns[1],
            self.page_point(view, scroll)?,
        )
    }

    pub fn hit(&self, view: Viewport, scroll: &scroll::State, folder: &Folder) -> Option<usize> {
        let (x, y) = self.page_point(view, scroll)?;
        if let Some(model) = &folder.grid {
            return model.hit((x, y));
        }
        if x < 14.0 || x >= view.extent_x - 13.0 || y < crate::app::ROW_TOP || y >= view.extent_y {
            return None;
        }
        folder.layout.row_at(y, view.pitch)
    }

    pub(super) fn header_at(
        &self,
        folder: &Folder,
        view: Viewport,
        scroll: &scroll::State,
    ) -> Option<usize> {
        let (x, y) = self.pointer?;
        if x < view.x
            || x >= view.x + view.right
            || y < view.y + view.header
            || y >= view.y + view.bottom
        {
            return None;
        }
        let y = y - view.y - view.header + scroll.y;
        if let Some(model) = &folder.grid {
            let x = x - view.x + scroll.x;
            return folder.layout.headers.iter().position(|header| {
                let cell = if header.rows.is_empty() {
                    model
                        .layout
                        .empty
                        .iter()
                        .find(|empty| empty.label == header.label)
                        .map(|empty| [empty.x, empty.y, empty.width])
                } else {
                    model.layout.headers.get(header.rows.start).copied()
                };
                cell.is_some_and(|cell| {
                    cell[1] >= 0.0
                        && y >= cell[1] + 4.0
                        && y < cell[1]
                            + if header.label.contains(crate::app::GROUP_SUBTITLE) {
                                49.0
                            } else {
                                31.0
                            }
                        && (!model.spec.column_major || x >= cell[0] && x < cell[0] + cell[2])
                })
            });
        }
        folder.layout.headers.iter().position(|header| {
            y >= header.top + 4.0
                && y < header.top
                    + if header.label.contains(crate::app::GROUP_SUBTITLE) {
                        49.0
                    } else {
                        31.0
                    }
        })
    }

    pub fn rehover(&mut self, view: Viewport, scroll: &scroll::State, folder: &Folder) {
        let next = if self.band.as_ref().is_some_and(|b| b.moved) {
            None
        } else {
            self.hit(view, scroll, folder)
        };
        let header = self
            .header_at(folder, view, scroll)
            .map(|index| folder.layout.headers[index].label.clone());
        if header != self.group_hover {
            self.group_hover = header;
            self.mark_all();
        }
        if next == self.hover {
            self.rehover_home_button(view, scroll, folder);
            return;
        }
        if let Some(row) = self.hover {
            self.mark(row);
        }
        if let Some(row) = next {
            self.mark(row);
        }
        self.hover = next;
        self.hover_anchor = None;
        self.hover_pending = false;
        self.rehover_home_button(view, scroll, folder);
    }

    fn rehover_home_button(&mut self, view: Viewport, scroll: &scroll::State, folder: &Folder) {
        let button = self.home_button_at(view, scroll, folder);
        if button == self.home_button {
            return;
        }
        self.home_button = button;
        if let Some(row) = self.hover {
            self.mark(row);
        }
    }

    pub fn move_pointer(&mut self, point: (f32, f32), folder: &Folder, scroll: &scroll::State) {
        self.pointer = Some(point);
        let Some(view) = scroll.view else { return };
        if let Some(band) = &mut self.band {
            band.end = (
                point.0 - view.x + scroll.x,
                point.1 - view.y - view.header + scroll.y,
            );
            if (band.end.0 - band.start.0).abs() >= 4.0 || (band.end.1 - band.start.1).abs() >= 4.0
            {
                band.moved = true;
            }
            self.update_band(folder, view);
        }
        self.rehover(view, scroll, folder);
    }

    pub fn leave(&mut self) {
        if self.band.is_some() {
            return;
        }
        self.pointer = None;
        if self.group_hover.take().is_some() {
            self.mark_all();
        }
        if let Some(row) = self.hover.take() {
            self.mark(row);
        }
        self.hover_pending = false;
        self.hover_anchor = None;
    }

    pub fn press(
        &mut self,
        point: (f32, f32),
        folder: &Folder,
        scroll: &scroll::State,
        mods: Modifiers,
    ) -> bool {
        let Some(view) = scroll.view else {
            return false;
        };
        if point.0 < view.x
            || point.0 >= view.x + view.right
            || point.1 < view.y + view.header
            || point.1 >= view.y + view.bottom
        {
            return false;
        }
        let mods = if folder
            .grid
            .as_ref()
            .is_some_and(|model| model.kind == super::grid::Kind::Gallery)
        {
            crate::app::gallery::selection_modifiers(mods)
        } else {
            mods
        };
        self.pointer = Some(point);
        self.has_focus = true;
        self.hover_pending = false;
        if let Some(index) = self.header_at(folder, view, scroll) {
            let header = &folder.layout.headers[index];
            self.control_pressed = true;
            if folder
                .grid
                .as_ref()
                .is_none_or(|model| !model.spec.column_major)
                && point.0 - view.x + scroll.x < 21.0
            {
                self.group_action = Some(Action::Expand(
                    header.label.clone(),
                    if header.folded {
                        crate::app::group::Expansion::Expanded
                    } else {
                        crate::app::group::Expansion::Collapsed
                    },
                ));
            } else {
                self.selection
                    .group(&folder.order, &folder.entries, header.rows.clone(), mods);
                self.group_focus = Some(header.label.clone());
            }
            self.mark_all();
            return true;
        }
        self.group_focus = None;
        self.pressed = self.hit(view, scroll, folder);
        self.narrow = None;
        if let Some(row) = self.pressed {
            let checkbox = if let Some(model) = &folder.grid {
                let cell = model.layout.cells[row];
                let top = cell[1] + crate::app::grid_layout::check_y(&model.spec, cell[3]);
                folder.settings.check_boxes
                    && point.0 - view.x + scroll.x >= cell[0] + 4.0
                    && point.0 - view.x + scroll.x < cell[0] + 17.0
                    && point.1 - view.y - view.header + scroll.y >= top
                    && point.1 - view.y - view.header + scroll.y < top + 13.0
            } else {
                folder.settings.check_boxes
                    && point.0 - view.x + scroll.x >= 18.0
                    && point.0 - view.x + scroll.x < 31.0
            };
            if checkbox {
                self.selection.press(
                    &folder.order,
                    &folder.entries,
                    row,
                    Modifiers {
                        ctrl: true,
                        shift: false,
                    },
                );
                self.pressed = None;
                self.control_pressed = true;
                self.mark_all();
                return true;
            }
            self.narrow = self
                .selection
                .press(&folder.order, &folder.entries, row, mods)
                .then_some(row);
        } else {
            let base = if mods.ctrl || mods.shift {
                self.selection.selected.clone()
            } else {
                HashSet::new()
            };
            if !mods.ctrl && !mods.shift {
                self.selection.selected.clear();
                self.selection.anchor = None;
            }
            let start = (
                point.0 - view.x + scroll.x,
                point.1 - view.y - view.header + scroll.y,
            );
            self.band = Some(Band {
                start,
                end: start,
                moved: false,
                base,
                mods,
            });
        }
        self.mark_all();
        true
    }

    pub fn release(
        &mut self,
        folder: &Folder,
        scroll: &scroll::State,
        mods: Modifiers,
    ) -> Option<Action> {
        self.control_pressed = false;
        let row = scroll.view.and_then(|view| self.hit(view, scroll, folder));
        let band = self.band.take().is_some();
        if self.narrow.take().is_some_and(|narrow| Some(narrow) == row) {
            if let Some(row) = row {
                self.selection
                    .move_to(&folder.order, &folder.entries, row, Modifiers::default());
            }
        }
        let pressed = self.pressed.take();
        self.mark_all();
        if !band
            && row == pressed
            && row.is_some()
            && folder.settings.single_click
            && !mods.ctrl
            && !mods.shift
        {
            self.cues = false;
            return self.open_row(folder, row.unwrap());
        }
        None
    }

    pub fn cancel(&mut self) {
        self.control_pressed = false;
        self.group_action = None;
        self.band = None;
        self.pressed = None;
        self.narrow = None;
        self.hover_pending = false;
        self.mark_all();
    }

    pub fn open_row(&mut self, folder: &Folder, row: usize) -> Option<Action> {
        let path = folder.entries[*folder.order.get(row)?].path.clone();
        let interval =
            unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as u128;
        if self
            .last_open
            .as_ref()
            .is_some_and(|(old, when)| *old == path && when.elapsed().as_millis() < interval)
        {
            return None;
        }
        self.last_open = Some((path.clone(), Instant::now()));
        Some(Action::Open(vec![path]))
    }

    pub fn band_scroll(&self, view: Viewport, folder: &Folder) -> Option<(scroll::Axis, f32)> {
        if !self.band.as_ref().is_some_and(|b| b.moved) {
            return None;
        }
        let (x, y) = self.pointer?;
        let (axis, delta) = if folder
            .grid
            .as_ref()
            .is_some_and(|model| model.spec.column_major)
        {
            (
                scroll::Axis::Horizontal,
                selection::band_scroll(x, view.x, view.x + view.right),
            )
        } else {
            (
                scroll::Axis::Vertical,
                selection::band_scroll(y, view.y + view.header, view.y + view.bottom),
            )
        };
        (delta != 0.0).then_some((axis, delta))
    }

    pub fn update_band(&mut self, folder: &Folder, view: Viewport) {
        let Some(band) = &self.band else { return };
        if !band.moved {
            return;
        }
        let (left, right) = (band.start.0.min(band.end.0), band.start.0.max(band.end.0));
        let (top, bottom) = (band.start.1.min(band.end.1), band.start.1.max(band.end.1));
        let hits: Vec<_> = if let Some(model) = &folder.grid {
            model
                .visible((left, top), (right - left, bottom - top))
                .collect()
        } else if right > 14.0 && left < view.extent_x - 13.0 {
            folder
                .layout
                .visible(top, bottom - top, view.plate)
                .filter(|&row| !folder.layout.folded[row])
                .collect()
        } else {
            Vec::new()
        };
        self.selection.selected = selection::band_selection(
            &band.base,
            hits.iter()
                .map(|&row| folder.entries[folder.order[row]].path.clone()),
            band.mods,
        );
        self.mark_all();
    }

    pub fn hover_delay(&mut self, folder: &Folder, mods: Modifiers) -> Option<u32> {
        let row = self.hover?;
        let point = self.pointer?;
        if !folder.settings.single_click
            || !self.active
            || mods.ctrl
            || mods.shift
            || self.pressed.is_some()
            || self.band.is_some()
            || self
                .selection
                .selected
                .contains(&folder.entries[folder.order[row]].path)
        {
            self.hover_pending = false;
            return None;
        }
        if self.hover_anchor.is_some_and(|a| {
            a.0 == row && (a.1 - point.0).abs() <= 2.0 && (a.2 - point.1).abs() <= 2.0
        }) {
            return None;
        }
        self.hover_anchor = Some((row, point.0, point.1));
        self.hover_pending = true;
        Some(folder.settings.hover_ms)
    }

    pub fn hover_select(&mut self, folder: &Folder, mods: Modifiers) {
        let pending = std::mem::take(&mut self.hover_pending);
        if !pending || !self.active || mods.ctrl || mods.shift {
            return;
        }
        let Some(row) = self.hover else { return };
        self.has_focus = true;
        self.selection
            .move_to(&folder.order, &folder.entries, row, Modifiers::default());
        self.mark_all();
    }

    pub fn key(
        &mut self,
        key: u32,
        folder: &Folder,
        scroll: &mut scroll::State,
        mods: Modifiers,
    ) -> Option<Action> {
        if !self.has_focus
            || !(matches!(key, 0x21..=0x28 | 0x20 | 0x0d | 0x08) || key == 0x41 && mods.ctrl)
        {
            return None;
        }
        self.hover_pending = false;
        self.cues = true;
        let view = scroll.view?;
        if let Some(label) = &self.group_focus {
            if matches!(key, 0x25 | 0x27)
                && folder
                    .grid
                    .as_ref()
                    .is_none_or(|model| !model.spec.column_major)
            {
                return Some(Action::Expand(
                    label.clone(),
                    if key == 0x27 {
                        crate::app::group::Expansion::Expanded
                    } else {
                        crate::app::group::Expansion::Collapsed
                    },
                ));
            }
        }
        if let Some(model) = &folder.grid {
            if matches!(key, 0x21..=0x28) {
                let current = self.focus_row(folder).unwrap_or(0);
                let cells = &model.layout.cells;
                let last = cells.len().saturating_sub(1);
                let page_origin = scroll.y;
                if matches!(key, 0x21 | 0x22) {
                    if let Some(header) = self.group_focus.as_ref().and_then(|label| {
                        folder
                            .layout
                            .headers
                            .iter()
                            .find(|header| &header.label == label)
                    }) {
                        if let Some(cell) = model.layout.headers.get(header.rows.start) {
                            scroll.y = selection::reveal_offset(
                                scroll.y,
                                view.rows_height,
                                cell[1],
                                cell[1] + 27.0,
                            )
                            .clamp(0.0, view.maximum_y())
                            .round();
                        }
                    } else {
                        self.reveal(current, folder, view, scroll);
                    }
                }
                if model.kind == super::grid::Kind::Drives
                    && matches!(key, 0x21 | 0x22 | 0x26 | 0x28)
                {
                    use crate::app::drives_layout::{self, Kind, Navigation};
                    let header = self
                        .group_focus
                        .as_ref()
                        .and_then(|label| {
                            folder
                                .layout
                                .headers
                                .iter()
                                .find(|header| &header.label == label)
                        })
                        .map(|header| header.rows.start);
                    let direction = if matches!(key, 0x22 | 0x28) { 1 } else { -1 };
                    let navigation = if matches!(key, 0x21 | 0x22) {
                        Navigation::Page(direction)
                    } else {
                        Navigation::Line(direction)
                    };
                    if let Some(target) = drives_layout::target(
                        &model.navigation,
                        cells,
                        header,
                        self.focus_row(folder),
                        scroll.y,
                        view.rows_height,
                        navigation,
                    ) {
                        match target {
                            Kind::Header(index) => {
                                if let Some(header) = folder
                                    .layout
                                    .headers
                                    .iter()
                                    .find(|header| header.rows.start == index)
                                {
                                    self.group_focus = Some(header.label.clone());
                                    self.selection.group(
                                        &folder.order,
                                        &folder.entries,
                                        header.rows.clone(),
                                        mods,
                                    );
                                    let top = model.layout.headers[index][1];
                                    scroll.y = selection::reveal_offset(
                                        scroll.y,
                                        view.rows_height,
                                        top,
                                        top + 27.0,
                                    )
                                    .clamp(0.0, view.maximum_y())
                                    .round();
                                }
                            }
                            Kind::Cells(members) => {
                                self.group_focus = None;
                                self.selection.move_to(
                                    &folder.order,
                                    &folder.entries,
                                    members[0],
                                    mods,
                                );
                                self.reveal(members[0], folder, view, scroll);
                            }
                        }
                    }
                    self.mark_all();
                    self.rehover(view, scroll, folder);
                    return None;
                }
                let target = match key {
                    0x24 => 0,
                    0x23 => last,
                    0x25 | 0x27 if model.spec.column_major => {
                        let columns = model.layout.columns.max(1);
                        if key == 0x25 {
                            current.saturating_sub(columns)
                        } else {
                            (current + columns).min(last)
                        }
                    }
                    0x25 => current.saturating_sub(1),
                    0x27 => (current + 1).min(last),
                    0x26 | 0x28 if model.spec.column_major => {
                        if key == 0x26 {
                            current.saturating_sub(1)
                        } else {
                            (current + 1).min(last)
                        }
                    }
                    0x26 | 0x28 => crate::app::grid_layout::beside(
                        cells,
                        current,
                        if key == 0x28 { 1 } else { -1 },
                    )
                    .unwrap_or(current),
                    0x21 | 0x22 => {
                        let direction = if key == 0x22 { 1 } else { -1 };
                        if model.spec.column_major {
                            crate::app::grid_layout::column_page_target(
                                cells,
                                current as isize,
                                scroll.x,
                                view.right,
                                direction,
                            )
                            .max(0) as usize
                        } else {
                            crate::app::grid_layout::page_target(
                                cells,
                                &folder.layout.folded,
                                &model.spec,
                                current as isize,
                                page_origin,
                                view.rows_height,
                                direction,
                            )
                            .max(0) as usize
                        }
                    }
                    _ => current,
                };
                let shown = |row: usize| {
                    cells
                        .get(row)
                        .is_some_and(|cell| cell[2] > 0.0 && cell[3] > 0.0)
                };
                let target = if target < current {
                    (0..=target)
                        .rev()
                        .find(|&row| shown(row))
                        .or_else(|| (target..cells.len()).find(|&row| shown(row)))
                } else {
                    (target..cells.len())
                        .find(|&row| shown(row))
                        .or_else(|| (0..=target).rev().find(|&row| shown(row)))
                };
                if let Some(target) = target {
                    let preserve = model.kind == super::grid::Kind::Drives
                        && matches!(key, 0x23 | 0x24)
                        && !mods.shift
                        && self.group_focus.is_none()
                        && self.focus_row(folder) == Some(target);
                    self.group_focus = None;
                    if !preserve {
                        let navigation_mods = if model.kind == super::grid::Kind::Gallery {
                            crate::app::gallery::selection_modifiers(mods)
                        } else {
                            mods
                        };
                        self.selection.move_to(
                            &folder.order,
                            &folder.entries,
                            target,
                            navigation_mods,
                        );
                    }
                    self.reveal(target, folder, view, scroll);
                    if key == 0x24 {
                        scroll.x = 0.0;
                        scroll.y = 0.0;
                    }
                    if key == 0x23 {
                        if model.spec.column_major {
                            scroll.x = view.maximum_x();
                        } else {
                            scroll.y = view.maximum_y();
                        }
                    }
                }
                self.mark_all();
                self.rehover(view, scroll, folder);
                return None;
            }
        }
        if let Some(label) = &self.group_focus {
            if matches!(key, 0x25 | 0x27) {
                return Some(Action::Expand(
                    label.clone(),
                    if key == 0x27 {
                        crate::app::group::Expansion::Expanded
                    } else {
                        crate::app::group::Expansion::Collapsed
                    },
                ));
            }
        }
        if !folder.layout.headers.is_empty() && matches!(key, 0x26 | 0x28 | 0x24 | 0x23) {
            let mut targets = Vec::new();
            for header in &folder.layout.headers {
                targets.push((
                    Some(header.label.as_str()),
                    header.rows.start,
                    header.top,
                    header.top + 30.0,
                ));
                for row in header
                    .rows
                    .clone()
                    .filter(|&row| !folder.layout.folded[row])
                {
                    targets.push((
                        None,
                        row,
                        view.row_top(folder, row),
                        view.row_bottom(folder, row),
                    ));
                }
            }
            let current = targets
                .iter()
                .position(|(label, row, _, _)| match &self.group_focus {
                    Some(focus) => *label == Some(focus.as_str()),
                    None => label.is_none() && Some(*row) == self.focus_row(folder),
                });
            let target = match key {
                0x24 => targets
                    .iter()
                    .position(|(label, _, _, _)| label.is_none())
                    .unwrap_or(0),
                0x23 => targets
                    .iter()
                    .rposition(|(label, _, _, _)| label.is_none())
                    .unwrap_or_else(|| targets.len().saturating_sub(1)),
                0x26 => current.unwrap_or(1).saturating_sub(1),
                _ => current
                    .map_or(0, |i| i + 1)
                    .min(targets.len().saturating_sub(1)),
            };
            if let Some((label, row, top, bottom)) = targets.get(target) {
                self.group_focus = label.map(str::to_owned);
                if label.is_none() {
                    self.selection
                        .move_to(&folder.order, &folder.entries, *row, mods);
                } else if let Some(header) = folder
                    .layout
                    .headers
                    .iter()
                    .find(|header| Some(header.label.as_str()) == *label)
                {
                    self.selection
                        .group(&folder.order, &folder.entries, header.rows.clone(), mods);
                }
                scroll.y = match key {
                    0x24 => 0.0,
                    0x23 => view.maximum_y(),
                    _ => selection::reveal_offset(scroll.y, view.rows_height, *top, *bottom)
                        .clamp(0.0, view.maximum_y())
                        .round(),
                };
            }
            self.mark_all();
            self.rehover(view, scroll, folder);
            return None;
        }
        let current = self.focus_row(folder).map_or(-1, |r| r as isize);
        let count = folder.order.len();
        let target = match key {
            0x26 => Some(if current < 0 { 0 } else { current - 1 }),
            0x28 => Some(current + 1),
            0x24 => Some(0),
            0x23 => Some(count as isize - 1),
            0x21 | 0x22 => {
                if current >= 0 {
                    self.reveal(current as usize, folder, view, scroll);
                }
                let rows: Vec<_> = (0..count)
                    .filter(|&row| !folder.layout.folded[row])
                    .map(|r| (r, view.row_top(folder, r), view.row_bottom(folder, r)))
                    .collect();
                Some(selection::page_target(
                    &rows,
                    current,
                    scroll.y,
                    view.rows_height,
                    if key == 0x22 { 1 } else { -1 },
                ))
            }
            0x20 => {
                if mods.ctrl {
                    self.selection.toggle_focus();
                }
                None
            }
            0x41 if mods.ctrl => {
                self.selection.selected = folder
                    .order
                    .iter()
                    .map(|&i| folder.entries[i].path.clone())
                    .collect();
                None
            }
            0x0d => {
                return Some(Action::Open(
                    folder
                        .order
                        .iter()
                        .map(|&i| &folder.entries[i].path)
                        .filter(|p| self.selection.selected.contains(*p))
                        .cloned()
                        .collect(),
                ))
            }
            0x08 => return Some(Action::Back),
            _ => return None,
        };
        if let Some(row) = target {
            if count > 0 {
                let row = row.clamp(0, count as isize - 1) as usize;
                self.selection
                    .move_to(&folder.order, &folder.entries, row, mods);
                self.reveal(row, folder, view, scroll);
            }
        }
        self.mark_all();
        self.rehover(view, scroll, folder);
        None
    }

    pub fn character(&mut self, ch: char, folder: &Folder, scroll: &mut scroll::State) {
        if !self.has_focus || ch.is_control() {
            return;
        }
        self.group_focus = None;
        let current = self.focus_row(folder).map_or(-1, |r| r as isize);
        if let Some(row) = selection::typeahead(
            &mut self.typeahead,
            &ch.to_string(),
            current,
            folder
                .order
                .iter()
                .map(|&i| folder.entries[i].name.as_str()),
        ) {
            let shown = if folder.layout.folded[row] {
                (row..folder.order.len())
                    .find(|&row| !folder.layout.folded[row])
                    .or_else(|| (0..row).rev().find(|&row| !folder.layout.folded[row]))
            } else {
                Some(row)
            };
            let Some(row) = shown else {
                return;
            };
            self.selection
                .move_to(&folder.order, &folder.entries, row, Modifiers::default());
            if let Some(view) = scroll.view {
                if folder.grid.as_ref().is_some_and(|model| {
                    model.spec.column_major || model.kind != super::grid::Kind::Icons
                }) {
                    self.reveal(row, folder, view, scroll);
                    self.rehover(view, scroll, folder);
                    self.mark_all();
                    return;
                }
                scroll.y = selection::typeahead_offset(
                    scroll.y,
                    view.rows_height,
                    view.row_top(folder, row),
                    view.row_bottom(folder, row),
                )
                .clamp(0.0, view.maximum_y())
                .round();
                self.rehover(view, scroll, folder);
            }
            self.mark_all();
        }
    }

    pub(super) fn reveal(
        &self,
        row: usize,
        folder: &Folder,
        view: Viewport,
        scroll: &mut scroll::State,
    ) {
        if let Some(model) = &folder.grid {
            if let Some(&[x, y, width, height]) = model.layout.cells.get(row) {
                if model.spec.column_major {
                    scroll.x = crate::app::grid_layout::reveal_column(
                        &crate::app::grid_layout::column_lefts(&model.layout.cells),
                        scroll.x,
                        view.right,
                        x,
                        x + width,
                    )
                    .clamp(0.0, view.maximum_x())
                    .round();
                } else {
                    let (top, bottom) = crate::app::grid_layout::row_bounds(
                        &model.layout.cells,
                        &model.layout.items,
                        &folder.headers,
                        row,
                    )
                    .unwrap_or((y, y + height));
                    scroll.y = selection::reveal_offset(scroll.y, view.rows_height, top, bottom)
                        .clamp(0.0, view.maximum_y())
                        .round();
                }
            }
            return;
        }
        scroll.y = selection::reveal_offset(
            scroll.y,
            view.rows_height,
            view.row_top(folder, row),
            view.row_bottom(folder, row),
        )
        .clamp(0.0, view.maximum_y())
        .round();
    }
}
