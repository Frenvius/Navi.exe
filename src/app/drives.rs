//! This PC's grouped tiles. The items view scrolls by the same steps as scroll.rs;
//! its scrollbar expresses each header/tile row in 100 units, not screen pixels.
use super::*;

const PITCH: f32 = 56.0;
#[derive(Clone)]
pub enum Kind {
    Header(usize),
    Cells(Vec<usize>),
}
#[derive(Clone)]
pub struct NavRow {
    pub top: f32,
    pub bottom: f32,
    pub kind: Kind,
}

struct Layout {
    cells: Vec<[f32; 4]>,
    rows: Vec<NavRow>,
    starts: Vec<f32>,
    heights: [f32; 2],
    extent: f32,
}

fn layout(counts: [usize; 2], columns: usize, collapsed: [bool; 2]) -> Layout {
    let mut result = Layout {
        cells: vec![[0.0; 4]; counts.iter().sum()],
        rows: Vec::new(),
        starts: Vec::new(),
        heights: [0.0; 2],
        extent: 0.0,
    };
    let mut y = 6.0;
    let mut first = 0;
    for group in 0..2 {
        let count = counts[group];
        if count == 0 {
            continue;
        }
        result
            .starts
            .push(if result.starts.is_empty() { 0.0 } else { y });
        result.rows.push(NavRow {
            top: y,
            bottom: y + 27.0,
            kind: Kind::Header(first),
        });
        let row_count = if collapsed[group] {
            0
        } else {
            count.div_ceil(columns)
        };
        for row in 0..row_count {
            let top = y + 28.0 + row as f32 * PITCH;
            let step_top = top - if row == 0 { 1.0 } else { 0.0 };
            let mut members = Vec::new();
            for col in 0..columns {
                let local = row * columns + col;
                if local >= count {
                    break;
                }
                let index = first + local;
                result.cells[index] = [14.0 + col as f32 * 254.0, top, 250.0, 52.0];
                members.push(index);
            }
            result.starts.push(step_top);
            result.rows.push(NavRow {
                top: step_top,
                bottom: top + PITCH,
                kind: Kind::Cells(members),
            });
        }
        result.heights[group] = if collapsed[group] {
            30.0
        } else {
            27.0 + row_count as f32 * PITCH
        };
        y += result.heights[group];
        first += count;
    }
    result.extent = if result.rows.is_empty() { 0.0 } else { y };
    result.starts.push(result.extent);
    result
}

fn units(starts: &[f32], pixels: f32) -> f32 {
    if starts.len() < 2 {
        return 0.0;
    }
    let i = starts
        .partition_point(|&top| top <= pixels)
        .saturating_sub(1)
        .min(starts.len() - 2);
    (i as f32 * 100.0
        + (pixels - starts[i]).max(0.0) / (starts[i + 1] - starts[i]).max(1.0) * 100.0)
        .floor()
}
fn pixels(starts: &[f32], units: f32) -> f32 {
    if starts.len() < 2 {
        return 0.0;
    }
    let i = ((units.max(0.0) / 100.0).floor() as usize).min(starts.len() - 2);
    starts[i] + (starts[i + 1] - starts[i]) * (units / 100.0 - i as f32).clamp(0.0, 1.0)
}

impl App {
    pub fn drives_viewport(&self, width: f32, height: f32) {
        if !Self::drives_view(&self.state.borrow()) {
            return;
        }
        self.state.borrow_mut().grid_viewport = (width, height);
        self.layout_drives();
    }

    pub fn layout_drives(&self) {
        let (layout, collapsed) = {
            let mut st = self.state.borrow_mut();
            if !Self::drives_view(&st) {
                return;
            }
            let mut counts = [0; 2];
            for &i in &st.order {
                counts[usize::from(!Self::is_device(&st.entries[i]))] += 1;
            }
            let labels = [tr("tiles.drives"), tr("tiles.network")];
            let collapsed = [
                st.collapsed.contains(labels[0]),
                st.collapsed.contains(labels[1]),
            ];
            let (width, height) = st.grid_viewport;
            let fit = |w: f32| (((w - 24.0) / 254.0).floor() as usize).max(1);
            let columns = fit(width);
            let initial = layout(counts, columns, collapsed);
            let columns = if initial.extent > height {
                fit(width - 17.0)
            } else {
                columns
            };
            let result = if columns == fit(width) {
                initial
            } else {
                layout(counts, columns, collapsed)
            };
            st.columns = columns;
            st.grid_columns = columns;
            st.grid_column_major = false;
            st.cells = result.cells.clone();
            st.grid_items = result.rows.iter().map(|r| (r.top, r.bottom)).collect();
            st.grid_extent = result.extent;
            st.drives_rows = result.rows.clone();
            st.drives_unit_starts = result.starts.clone();
            st.headers = vec![String::new(); st.order.len()];
            let mut first = 0;
            st.folded = Vec::with_capacity(st.order.len());
            for g in 0..2 {
                if counts[g] > 0 {
                    st.headers[first] = labels[g].to_string();
                }
                st.folded
                    .extend(std::iter::repeat_n(collapsed[g], counts[g]));
                first += counts[g];
            }
            (result, collapsed)
        };
        for (row, cell) in layout.cells.iter().enumerate() {
            if let Some(mut data) = self.entries.row_data(row) {
                if [data.cell_x, data.cell_y, data.cell_w, data.cell_h] != *cell {
                    [data.cell_x, data.cell_y, data.cell_w, data.cell_h] = *cell;
                    self.entries.set_row_data(row, data);
                }
            }
        }
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        shell.set_drives_device_height(layout.heights[0]);
        shell.set_drives_devices_collapsed(collapsed[0]);
        shell.set_drives_network_collapsed(collapsed[1]);
        shell.set_grid_height(layout.extent);
        self.scroll_list(-shell.get_list_scroll_y());
        self.sync_grid_window(true);
    }

    pub fn sync_drives_scrollbar(&self) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let (total, page, value) = {
            let st = self.state.borrow();
            if !Self::drives_view(&st) {
                return;
            }
            let total = st.drives_unit_starts.len().saturating_sub(1) as f32 * 100.0;
            let page = (st.grid_viewport.1 / PITCH * 100.0).ceil();
            let value = units(&st.drives_unit_starts, -shell.get_list_scroll_y())
                .min((total - page).max(0.0));
            (total, page, value)
        };
        shell.set_drives_scroll_extent(total);
        shell.set_drives_scroll_page(page);
        shell.set_drives_scroll_units(-value);
        self.trace_drives_scroll();
    }

    fn trace_drives_scroll(&self) {
        if std::env::var_os("PC_SCROLL_TRACE").is_none() {
            return;
        }
        let st = self.state.borrow();
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        crate::frame::pc_scroll_debug(
            self.hwnd.get(),
            -shell.get_list_scroll_y(),
            st.grid_viewport.1,
            st.grid_extent,
            -shell.get_drives_scroll_units(),
            shell.get_focused_row(),
            st.focused_group.map(|g| g as i32).unwrap_or(-1),
            st.selected.len(),
        );
    }

    pub fn drives_thumb(&self, value: f32) {
        let value = value.round();
        let target = {
            let st = self.state.borrow();
            pixels(&st.drives_unit_starts, value)
        };
        self.scroll_list(target);
        self.ui().global::<Shell>().set_drives_scroll_units(-value);
        self.trace_drives_scroll();
    }

    fn reveal_drive_header(&self, row: usize) {
        let (top, bottom, view) = {
            let st = self.state.borrow();
            let Some(r) = st
                .drives_rows
                .iter()
                .find(|r| matches!(r.kind,Kind::Header(i) if i==row))
            else {
                return;
            };
            (r.top, r.bottom, st.grid_viewport.1)
        };
        let offset = -self.ui().global::<Shell>().get_list_scroll_y();
        if top < offset {
            self.scroll_list(if row == 0 { 0.0 } else { top });
        } else if bottom > offset + view {
            self.scroll_list(bottom - view);
        } else {
            self.sync_drives_scrollbar();
        }
    }

    pub fn drives_key(self: &Rc<Self>, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if alt || !Self::drives_view(&self.state.borrow()) {
            return false;
        }
        let (header, current) = {
            let st = self.state.borrow();
            (
                st.focused_group,
                st.focused.as_ref().and_then(|p| Self::row_of(&st, p)),
            )
        };
        if let Some(row) = header {
            if is(text, Key::LeftArrow) || is(text, Key::RightArrow) {
                self.set_group_open(row, is(text, Key::RightArrow));
                self.reveal_drive_header(row);
                return true;
            }
        }
        if is(text, Key::Home) || is(text, Key::End) {
            let target = {
                let st = self.state.borrow();
                let shown = st
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c[2] > 0.0)
                    .map(|(i, _)| i);
                if is(text, Key::End) {
                    shown.last()
                } else {
                    shown.into_iter().next()
                }
            };
            if let Some(row) = target {
                // ListView does not collapse a multiple selection when Home/End
                // lands on the item that already has focus (e.g. after Ctrl+Home).
                if !shift && header.is_none() && current == Some(row) {
                    let path = {
                        let st = self.state.borrow();
                        Self::row_path(&st, row)
                    };
                    if let Some(path) = path {
                        self.reveal(&path);
                    }
                } else {
                    self.move_to(row as isize, ctrl, shift);
                }
                if is(text, Key::Home) {
                    self.scroll_list(0.0);
                }
            }
            return true;
        }
        if is(text, Key::LeftArrow) || is(text, Key::RightArrow) {
            let row =
                current.unwrap_or(0) as isize + if is(text, Key::RightArrow) { 1 } else { -1 };
            self.move_to(row, ctrl, shift);
            return true;
        }
        let down = is(text, Key::DownArrow) || is(text, Key::PageDown);
        let up = is(text, Key::UpArrow) || is(text, Key::PageUp);
        if !down && !up {
            return false;
        }
        let page = is(text, Key::PageDown) || is(text, Key::PageUp);
        if page {
            if let Some(row) = header {
                self.reveal_drive_header(row);
            } else if let Some(row) = current {
                self.reveal_list_row(row);
            }
        }
        let target = {
            let st = self.state.borrow();
            let rows = &st.drives_rows;
            if rows.is_empty() {
                return true;
            }
            let at = rows.iter().position(|r| match &r.kind {
                Kind::Header(i) => header == Some(*i),
                Kind::Cells(v) => header.is_none() && current.is_some_and(|i| v.contains(&i)),
            });
            let at = at.unwrap_or_else(|| {
                rows.iter()
                    .position(|r| matches!(r.kind, Kind::Cells(_)))
                    .unwrap_or(0)
            });
            let to = if page {
                let offset = -self.ui().global::<Shell>().get_list_scroll_y();
                let view = st.grid_viewport.1;
                let count = (view / PITCH).floor().max(1.0) as usize;
                if down {
                    match rows.iter().rposition(|r| r.bottom <= offset + view + 0.01) {
                        Some(edge) if edge > at => edge,
                        _ => (at + count).min(rows.len() - 1),
                    }
                } else {
                    match rows.iter().position(|r| r.top >= offset - 0.01) {
                        Some(edge) if edge < at => edge,
                        _ => at.saturating_sub(count),
                    }
                }
            } else if current.is_none() && header.is_none() {
                at
            } else if down {
                (at + 1).min(rows.len() - 1)
            } else {
                at.saturating_sub(1)
            };
            let x = current.and_then(|i| st.cells.get(i)).map_or(14.0, |c| c[0]);
            match &rows[to].kind {
                Kind::Header(i) => (true, *i),
                Kind::Cells(v) => (
                    false,
                    *v.iter()
                        .min_by(|&&a, &&b| {
                            (st.cells[a][0] - x)
                                .abs()
                                .total_cmp(&(st.cells[b][0] - x).abs())
                        })
                        .unwrap(),
                ),
            }
        };
        if target.0 {
            self.group_clicked(target.1, ctrl);
            self.reveal_drive_header(target.1);
        } else {
            self.move_to(target.1 as isize, ctrl, shift);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouped_drive_scroll_geometry_and_units() {
        let l = layout([1, 32], 2, [false, false]);
        assert_eq!(l.extent, 1012.0);
        assert_eq!(l.cells[1], [14.0, 117.0, 250.0, 52.0]);
        assert_eq!(l.cells[2][0], 268.0);
        assert_eq!(units(&l.starts, 118.0), 303.0);
        assert_eq!(units(&l.starts, 286.0), 601.0);
        assert_eq!(units(&l.starts, 453.0), 900.0);
        assert_eq!(pixels(&l.starts, 362.0).round(), 151.0);
        let folded = layout([1, 32], 2, [false, true]);
        assert_eq!(folded.extent, 119.0);
    }
}
