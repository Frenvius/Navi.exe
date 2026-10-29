use super::GROUP_SUBTITLE;
#[derive(Clone, Copy)]
pub struct Spec {
    pub icon: f32,
    pub label_indent: f32,
    pub cell_w: f32,
    pub base_h: f32,
    pub gap_x: f32,
    pub gap_y: f32,
    pub kind: i32,
    pub column_major: bool,
    pub full_width: bool,
    pub justify: bool,
}

const fn stacked(icon: f32, cell_w: f32) -> Spec {
    Spec {
        icon,
        label_indent: 0.0,
        cell_w,
        base_h: 2.0 + icon + 18.0,
        gap_x: 1.0,
        gap_y: 1.0,
        kind: 0,
        column_major: false,
        full_width: false,
        justify: true,
    }
}

const fn inline(column_major: bool) -> Spec {
    Spec {
        icon: 16.0,
        label_indent: 0.0,
        cell_w: 308.0,
        base_h: 20.0,
        gap_x: 1.0,
        gap_y: 1.0,
        kind: 1,
        column_major,
        full_width: false,
        justify: false,
    }
}
pub fn spec(mode: i32, size: u32) -> Option<Spec> {
    let s = size as f32;
    match mode {
        1..=3 => Some(stacked(s, (s + (5.5 + s / 27.0).round()).max(74.0))),
        4 => Some(Spec {
            icon: s,
            base_h: s + 4.0,
            ..inline(false)
        }),
        5 => Some(inline(true)),
        7 => Some(Spec {
            icon: 48.0,
            label_indent: 0.0,
            cell_w: 250.0,
            base_h: 52.0,
            gap_x: 4.0,
            gap_y: 4.0,
            kind: 2,
            column_major: false,
            full_width: false,
            justify: false,
        }),
        8 => Some(Spec {
            icon: 32.0,
            label_indent: 0.0,
            cell_w: 0.0,
            base_h: 48.0,
            gap_x: 0.0,
            gap_y: 0.0,
            kind: 3,
            column_major: false,
            full_width: true,
            justify: false,
        }),
        _ => None,
    }
}
pub fn check_indent(spec: &Spec) -> f32 {
    match spec.kind {
        1 => 15.0,
        2 => 21.0,
        3 => 25.0,
        _ => 0.0,
    }
}

pub fn check_y(spec: &Spec, height: f32) -> f32 {
    if spec.kind == 0 {
        2.0
    } else {
        ((height - 13.0) / 2.0).floor()
    }
}

pub fn page_pitch(spec: &Spec) -> f32 {
    match spec.kind {
        0 => spec.base_h + LINE + spec.gap_y,
        1 => spec.base_h + 2.0,
        _ => spec.base_h + spec.gap_y,
    }
}
pub fn label_key(spec: &Spec) -> u32 {
    (spec.cell_w - 13.0).round() as u32
}

pub fn label_x(cell_width: f32, box_width: u32, line_width: f32) -> f32 {
    if box_width < 4 {
        return ((cell_width - line_width) / 2.0).floor();
    }
    ((cell_width - box_width as f32) / 2.0).ceil()
        + 2.0
        + ((box_width as f32 - 4.0 - line_width) / 2.0).floor()
}

pub struct ContentLayout {
    pub name_width: f32,
    pub fields: [(f32, f32, f32); 5],
}

pub fn content(
    width: f32,
    fields: &[Option<(String, String, f32)>],
    leading: f32,
) -> ContentLayout {
    let line = width - 13.0;
    let middle = 373.0;
    let right = 4.0 + (line * 0.6 + 34.4).max(line - 288.0).min(669.0);
    let end = 4.0 + line;
    let fits = |slot: usize, room: f32| {
        fields
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|(label, _, width)| {
                if label.is_empty() {
                    room >= 80.0
                } else {
                    *width <= room
                }
            })
    };
    let mut result = ContentLayout {
        name_width: 0.0,
        fields: [(0.0, 0.0, 0.0); 5],
    };
    for slot in 0..5 {
        let x = if slot == 0 {
            73.0 + leading
        } else if slot <= 2 {
            middle
        } else {
            right
        };
        let room = if slot <= 2 {
            right - x - 8.0
        } else {
            end - x - 8.0
        };
        let y = if slot == 1 || slot == 3 { 10.0 } else { 27.0 };
        result.fields[slot] = (x, y, if fits(slot, room) { room } else { 0.0 });
    }
    result.name_width = if result.fields[1].2 > 0.0 {
        middle
    } else if result.fields[3].2 > 0.0 {
        right
    } else {
        end
    } - 87.0
        - leading;
    let under = if result.fields[2].2 > 0.0 {
        middle
    } else if result.fields[4].2 > 0.0 {
        right
    } else {
        end
    } - 81.0
        - leading;
    result.fields[0].2 = if fits(0, under) { under } else { 0.0 };
    result
}

const LEFT: f32 = 14.0;
const TOP: f32 = 6.0;
const LINE: f32 = 15.0;
const MAX_LINES: u8 = 4;
const SCROLLBAR: f32 = 16.0;
const VSCROLLBAR: f32 = 17.0;
const LIST_TEXT_X: f32 = 50.0;
const LIST_PAD: f32 = 6.0;
const LIST_GAP: f32 = 10.0;
const LIST_HEADER_PAD: f32 = 27.0;
#[derive(Clone)]
pub struct Layout {
    pub empty: Vec<EmptyGroup>,
    pub cells: Vec<[f32; 4]>,
    pub headers: Vec<[f32; 3]>,
    pub items: Vec<(f32, f32)>,
    pub columns: usize,
    pub height: f32,
    pub width: f32,
}

#[derive(Clone)]
pub struct EmptyGroup {
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
}

pub struct Input<'a> {
    pub count: usize,
    pub groups: &'a [(String, usize, usize)],
    pub library: bool,
    pub grouped: bool,
    pub headers: &'a [String],
    pub folded: &'a [bool],
    pub collapsed: &'a std::collections::HashSet<String>,
    pub label_lines: &'a [u8],
    pub label_widths: &'a [f32],
    pub header_widths: &'a [f32],
}

pub fn arrange(data: &Input<'_>, spec: &Spec, size: (f32, f32)) -> Layout {
    let (width, height) = size;
    if spec.column_major {
        let fit = list_layout(data, spec, height);
        if fit.width > width {
            list_layout(data, spec, height - SCROLLBAR)
        } else {
            fit
        }
    } else {
        let fit = rows_layout(data, spec, width - LEFT, 1.0);
        if fit.height > height {
            rows_layout(data, spec, width - LEFT - VSCROLLBAR, 0.0)
        } else {
            fit
        }
    }
}

pub fn ranges(headers: &[String], count: usize) -> Vec<(String, usize, usize)> {
    let starts: Vec<_> = (0..count)
        .filter(|&i| i == 0 || headers.get(i).is_some_and(|h| !h.is_empty()))
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            (
                headers.get(start).cloned().unwrap_or_default(),
                start,
                starts.get(index + 1).copied().unwrap_or(count),
            )
        })
        .collect()
}
fn list_layout(st: &Input<'_>, spec: &Spec, view_h: f32) -> Layout {
    let n = st.count;
    let pitch = spec.base_h + spec.gap_y;
    let library = st.library;
    let grouped = st.grouped || library;
    let top = if grouped {
        2.0 + if library { 50.0 } else { 32.0 }
    } else {
        TOP
    };
    let rows = (((view_h - top) / pitch).floor() as usize).max(1);
    let mut cells = Vec::with_capacity(n);
    let mut headers = vec![[0.0, -1.0, 0.0]; n];
    let mut x = LEFT;
    let mut empty = Vec::new();
    for (label, group_start, group_end) in st.groups.iter().cloned() {
        let group_x = x;
        if group_start == group_end {
            empty.push(EmptyGroup {
                label: label.clone().into(),
                x,
                y: 2.0,
                width: 302.0,
            });
            x += 302.0;
            continue;
        }
        if st.folded.get(group_start).copied().unwrap_or(false) {
            cells.extend(std::iter::repeat_n([0.0; 4], group_end - group_start));
        } else {
            for start in (group_start..group_end).step_by(rows) {
                let end = (start + rows).min(group_end);
                let text = (start..end)
                    .map(|i| st.label_widths.get(i).copied().unwrap_or(0.0))
                    .fold(0.0f32, f32::max);
                let w = (LIST_TEXT_X + spec.label_indent + text + LIST_PAD).ceil();
                for r in 0..end - start {
                    cells.push([x, top + r as f32 * pitch, w, spec.base_h]);
                }
                x += w + LIST_GAP;
            }
            if grouped {
                x -= LIST_GAP - 1.0;
            }
        }
        if grouped {
            let header = st.header_widths.get(group_start).copied().unwrap_or(0.0);
            let header_w = (LIST_HEADER_PAD + header).ceil();
            x = x
                .max(group_x + header_w)
                .max(group_x + if library { 302.0 } else { 0.0 });
            headers[group_start] = [group_x, 2.0, x - group_x - if library { 16.0 } else { 1.0 }];
        }
    }
    let gap = if grouped { 1.0 } else { LIST_GAP };
    Layout {
        empty,
        cells,
        headers,
        items: Vec::new(),
        columns: rows,
        height: 0.0,
        width: x - gap + LEFT,
    }
}

fn rows_layout(st: &Input<'_>, spec: &Spec, avail: f32, slack: f32) -> Layout {
    let library = st.library;
    let n = st.count;
    let mut cells = Vec::with_capacity(n);
    let mut headers = vec![[0.0, -1.0, -1.0]; n];
    let (cell_w, columns, pitch_x) = if spec.full_width {
        let w = avail.max(200.0);
        (w, 1, w)
    } else {
        let pitch = spec.cell_w + spec.gap_x;
        let columns = (((avail + slack) / pitch).floor() as usize).max(1);
        let pitch = if spec.justify {
            (avail / columns as f32).floor().max(pitch)
        } else {
            pitch
        };
        (spec.cell_w, columns, pitch)
    };
    let grouped = st.grouped || library;
    let (head, tail) = match spec.kind {
        1 => (31.0, spec.gap_y),
        3 => (32.0, spec.gap_y),
        _ => (32.0, spec.gap_y + 1.0),
    };
    let mut y = if grouped { 2.0 } else { TOP };
    let mut items = Vec::new();
    let mut empty = Vec::new();
    for (k, (label, group_start, group_end)) in st.groups.iter().cloned().enumerate() {
        if library && k > 0 {
            y += 13.0;
        }
        if group_start == group_end {
            let folded = st.collapsed.contains(&label);
            empty.push(EmptyGroup {
                label: label.into(),
                x: 0.0,
                y,
                width: -1.0,
            });
            y += if folded { 48.0 } else { 82.0 };
            continue;
        }
        if grouped {
            headers[group_start] = [0.0, y, -1.0];
            let extra = if st.headers[group_start].contains(GROUP_SUBTITLE) {
                18.0
            } else {
                0.0
            };
            items.push((y, y + 30.0 + extra));
            if st.folded.get(group_start).copied().unwrap_or(false) {
                cells.extend(std::iter::repeat_n([0.0; 4], group_end - group_start));
                y += 30.0 + extra;
                continue;
            }
            y += head + extra;
        }
        for row_start in (group_start..group_end).step_by(columns) {
            let row_end = (row_start + columns).min(group_end);
            let lines = if spec.kind == 0 {
                (row_start..row_end)
                    .map(|i| st.label_lines.get(i).copied().unwrap_or(1))
                    .max()
                    .unwrap_or(1)
                    .min(MAX_LINES)
            } else {
                1
            };
            let h = spec.base_h + LINE * (lines as f32 - 1.0);
            for (c, index) in (row_start..row_end).enumerate() {
                let cell_h = if spec.kind == 0 {
                    spec.base_h
                        + LINE
                            * (st
                                .label_lines
                                .get(index)
                                .copied()
                                .unwrap_or(1)
                                .clamp(1, MAX_LINES) as f32
                                - 1.0)
                } else if spec.kind == 3 {
                    h + 1.0
                } else {
                    h
                };
                cells.push([LEFT + c as f32 * pitch_x, y, cell_w, cell_h]);
            }
            items.push((y, y + h + spec.gap_y));
            y += h + spec.gap_y;
        }
        if grouped {
            y -= tail;
        }
    }
    let end = if grouped { y } else { y - spec.gap_y };
    Layout {
        empty,
        cells,
        headers,
        items,
        columns,
        height: if n == 0 && !st.library { 0.0 } else { end },
        width: 0.0,
    }
}

pub fn mode_icon_size(mode: i32) -> u32 {
    match mode {
        1 => 256,
        2 => 96,
        3 | 7 => 48,
        8 => 32,
        _ => 16,
    }
}
const ZOOM_SIZES: [u32; 39] = [
    16, 18, 20, 22, 23, 25, 27, 29, 31, 33, 35, 38, 41, 44, 47, 50, 54, 58, 62, 67, 72, 77, 83, 89,
    95, 102, 110, 118, 126, 136, 145, 156, 168, 180, 193, 207, 223, 239, 256,
];
pub const ZOOM_STEPS: usize = ZOOM_SIZES.len() + 2;
pub fn zoom_index(mode: i32, size: u32) -> Option<usize> {
    match mode {
        super::VIEW_DETAILS => Some(0),
        5 => Some(1),
        1..=4 => Some(2 + ZOOM_SIZES.iter().rposition(|&s| s <= size).unwrap_or(0)),
        _ => None,
    }
}
pub fn zoom_step(index: usize) -> (i32, u32) {
    match index {
        0 => (super::VIEW_DETAILS, 16),
        1 => (5, 16),
        _ => {
            let size = ZOOM_SIZES[index - 2];
            let mode = icon_mode(size);
            (mode, size)
        }
    }
}

pub fn icon_mode(size: u32) -> i32 {
    match size {
        256.. => 1,
        96.. => 2,
        32.. => 3,
        _ => 4,
    }
}

const EPSILON: f32 = 0.01;
pub fn page_target(
    cells: &[[f32; 4]],
    folded: &[bool],
    spec: &Spec,
    current: isize,
    y: f32,
    view: f32,
    direction: i32,
) -> isize {
    let down = direction > 0;
    let mut rows: Vec<(f32, f32, Vec<usize>)> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        if folded.get(i).copied().unwrap_or(false) {
            continue;
        }
        match rows.last_mut() {
            Some(row) if row.0 == cell[1] => {
                row.1 = row.1.max(cell[1] + cell[3] + spec.gap_y);
                row.2.push(i);
            }
            _ => rows.push((cell[1], cell[1] + cell[3] + spec.gap_y, vec![i])),
        }
    }
    let Some(at) = rows
        .iter()
        .position(|r| r.2.contains(&(current.max(0) as usize)))
    else {
        return current;
    };
    let page = ((view / page_pitch(&spec)).floor() as usize).max(1);
    let outside = rows[at].0 < y - EPSILON || rows[at].1 > y + view + EPSILON;
    let to = if outside {
        if down {
            (at + page).min(rows.len() - 1)
        } else {
            at.saturating_sub(page)
        }
    } else if down {
        match rows.iter().rposition(|r| r.1 <= y + view + EPSILON) {
            Some(edge) if edge > at => edge,
            _ => (at + page).min(rows.len() - 1),
        }
    } else {
        match rows.iter().position(|r| r.0 >= y - EPSILON) {
            Some(edge) if edge < at => edge,
            _ => at.saturating_sub(page),
        }
    };
    let x = cells[current.max(0) as usize][0];
    rows[to]
        .2
        .iter()
        .copied()
        .filter(|&i| cells[i][0] <= x + EPSILON)
        .last()
        .unwrap_or(rows[to].2[0]) as isize
}
pub fn column_page_target(
    cells: &[[f32; 4]],
    current: isize,
    scroll: f32,
    view: f32,
    direction: i32,
) -> isize {
    let down = direction > 0;
    let mut columns: Vec<(f32, f32, Vec<usize>)> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        match columns.iter_mut().find(|c| (c.0 - cell[0]).abs() < EPSILON) {
            Some(column) => column.2.push(i),
            None => columns.push((cell[0], cell[0] + cell[2], vec![i])),
        }
    }
    columns.sort_by(|a, b| a.0.total_cmp(&b.0));
    let current = current.max(0) as usize;
    let Some(at) = columns.iter().position(|c| c.2.contains(&current)) else {
        return current as isize;
    };
    let row = columns[at]
        .2
        .iter()
        .position(|&i| i == current)
        .unwrap_or(0);
    let last = columns.len() - 1;
    let to = if down {
        match columns.iter().rposition(|c| c.1 <= scroll + view + EPSILON) {
            Some(edge) if edge > at => edge,
            _ => columns
                .iter()
                .rposition(|c| c.1 <= columns[at].0 + view + EPSILON)
                .unwrap_or(at)
                .max(at + 1)
                .min(last),
        }
    } else {
        match columns.iter().position(|c| c.0 >= scroll - EPSILON) {
            Some(edge) if edge < at => edge,
            _ => columns
                .iter()
                .position(|c| c.0 >= columns[at].1 - view - EPSILON)
                .unwrap_or(at)
                .min(at.saturating_sub(1)),
        }
    };
    let cells = &columns[to].2;
    cells[row.min(cells.len() - 1)] as isize
}
pub fn beside(cells: &[[f32; 4]], row: usize, direction: i32) -> Option<usize> {
    let down = direction > 0;
    let here = *cells.get(row)?;
    let next_y = cells
        .iter()
        .filter(|cell| cell[2] > 0.0 && cell[3] > 0.0)
        .map(|c| c[1])
        .filter(|&y| if down { y > here[1] } else { y < here[1] })
        .fold(None, |best: Option<f32>, y| {
            Some(match best {
                Some(b) if (down && b <= y) || (!down && b >= y) => b,
                _ => y,
            })
        })?;
    cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c[1] == next_y && c[2] > 0.0 && c[3] > 0.0)
        .min_by(|(_, a), (_, b)| (a[0] - here[0]).abs().total_cmp(&(b[0] - here[0]).abs()))
        .map(|(i, _)| i)
}

pub fn column_lefts(cells: &[[f32; 4]]) -> Vec<f32> {
    let mut lefts = cells
        .iter()
        .filter(|cell| cell[2] > 0.0)
        .map(|cell| cell[0])
        .collect::<Vec<_>>();
    lefts.sort_by(f32::total_cmp);
    lefts.dedup();
    if let Some(first) = lefts.first_mut() {
        *first = 0.0;
    }
    lefts
}

pub fn column_pixels(lefts: &[f32], units: f32) -> f32 {
    let at = (units / 48.0).max(0.0);
    let i = (at.floor() as usize).min(lefts.len().saturating_sub(1));
    match (lefts.get(i), lefts.get(i + 1)) {
        (Some(&a), Some(&b)) => a + (b - a) * (at - i as f32),
        (Some(&a), None) => a,
        _ => 0.0,
    }
}

pub fn column_units(lefts: &[f32], pixels: f32) -> f32 {
    let i = lefts
        .iter()
        .rposition(|&left| left <= pixels + 0.01)
        .unwrap_or(0);
    let at = match (lefts.get(i), lefts.get(i + 1)) {
        (Some(&a), Some(&b)) if b > a => i as f32 + ((pixels - a) / (b - a)).min(1.0),
        _ => i as f32,
    };
    at * 48.0
}

pub fn column_offset(lefts: &[f32], offset: f32, steps: i32) -> f32 {
    let max = lefts.last().copied().unwrap_or(0.0);
    let mut x = offset;
    for _ in 0..steps.unsigned_abs() {
        x = if steps < 0 {
            lefts.iter().copied().find(|&l| l > x + 0.5).unwrap_or(max)
        } else {
            lefts
                .iter()
                .copied()
                .rev()
                .find(|&l| l < x - 0.5)
                .unwrap_or(0.0)
        };
    }
    x.clamp(0.0, max)
}

pub fn reveal_column(lefts: &[f32], scroll: f32, view: f32, left: f32, right: f32) -> f32 {
    let column = lefts
        .iter()
        .copied()
        .rev()
        .find(|&x| x <= left)
        .unwrap_or(0.0);
    if left < scroll {
        return column;
    }
    if right > scroll + view {
        return lefts
            .iter()
            .copied()
            .find(|&x| right <= x + view)
            .unwrap_or(column)
            .min(column);
    }
    scroll
}

pub fn row_bounds(
    cells: &[[f32; 4]],
    items: &[(f32, f32)],
    headers: &[String],
    row: usize,
) -> Option<(f32, f32)> {
    let cell = cells.get(row)?;
    let step = items
        .iter()
        .position(|&(top, _)| top == cell[1] || top == cell[1] - 1.0)?;
    let start = (0..=row)
        .rev()
        .find(|&i| headers.get(i).is_some_and(|label| !label.is_empty()));
    let first = start.is_some_and(|i| cells.get(i).is_some_and(|other| other[1] == cell[1]));
    let top = if first && step > 0 {
        items[step - 1].0
    } else {
        cell[1]
    };
    Some((top, items[step].1))
}

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn list_reveal_snaps_to_first_column_that_fits_the_target() {
        let lefts = [0.0, 130.0, 290.0, 450.0];
        assert_eq!(reveal_column(&lefts, 0.0, 300.0, 304.0, 440.0), 290.0);
        assert_eq!(reveal_column(&lefts, 290.0, 300.0, 144.0, 280.0), 130.0);
        assert_eq!(reveal_column(&lefts, 130.0, 300.0, 144.0, 280.0), 130.0);
    }

    #[test]
    fn list_columns_keep_ten_pixels_inside_a_group_and_one_between_groups() {
        let groups = [("A".into(), 0, 6), ("B".into(), 6, 10)];
        let mut headers = vec![String::new(); 10];
        headers[0] = "A".into();
        headers[6] = "B".into();
        let data = Input {
            count: 10,
            groups: &groups,
            library: false,
            grouped: true,
            headers: &headers,
            folded: &[false; 10],
            collapsed: &Default::default(),
            label_lines: &[],
            label_widths: &[100.0; 10],
            header_widths: &[20.0; 10],
        };
        let layout = list_layout(&data, &spec(5, 16).unwrap(), 118.0);
        assert_eq!(layout.columns, 4);
        assert_eq!(
            layout.cells[4][0] - layout.cells[0][0] - layout.cells[0][2],
            10.0
        );
        assert_eq!(
            layout.cells[6][0] - layout.cells[4][0] - layout.cells[4][2],
            1.0
        );
    }

    #[test]
    fn reveal_uses_the_whole_row_and_brings_the_first_group_header() {
        let cells = [
            [14.0, 34.0, 74.0, 68.0],
            [94.0, 34.0, 74.0, 113.0],
            [14.0, 148.0, 74.0, 68.0],
        ];
        let items = [(6.0, 34.0), (34.0, 148.0), (148.0, 217.0)];
        let headers = ["A".to_owned(), String::new(), String::new()];
        assert_eq!(row_bounds(&cells, &items, &headers, 0), Some((6.0, 148.0)));
        assert_eq!(row_bounds(&cells, &items, &headers, 1), Some((6.0, 148.0)));
        assert_eq!(
            row_bounds(&cells, &items, &headers, 2),
            Some((148.0, 217.0))
        );
    }

    #[test]
    fn measured_large_page_uses_four_rows_when_the_focus_is_above_the_view() {
        let mut cells = Vec::new();
        for (row, y) in [6.0, 138.0, 255.0, 372.0, 489.0].into_iter().enumerate() {
            for column in 0..8 {
                cells.push([
                    14.0 + column as f32 * 108.0,
                    y,
                    105.0,
                    if row == 0 && column == 1 {
                        131.0
                    } else {
                        116.0
                    },
                ]);
            }
        }
        let spec = spec(2, 96).unwrap();
        assert_eq!(page_target(&cells, &[], &spec, 0, 0.0, 559.0, 1), 24);
        assert_eq!(page_target(&cells, &[], &spec, 0, 281.0, 559.0, 1), 32);
    }
}
