//! The details list and the row-major icon views scroll as Explorer's items view does: the
//! wheel and the scrollbar's arrows move by whole rows and the track by pages, lining a row
//! up with the edge they move toward (its bottom going down, its top going up). The details
//! list draws only the rows near its shown part.

use super::*;

/// Rows drawn past each end of the shown part: a viewport's worth.
const MARGIN: f32 = 1.0;
const EPSILON: f32 = 0.01;

/// What a view's scroll steps line up (each row and group header, in content order), how
/// tall its shown part is and how far it scrolls.
struct Steps {
    items: Vec<(f32, f32)>,
    view: f32,
    max: f32,
    /// What a row must clear the bottom edge by to count as shown whole for the track's
    /// pages: the details list's highlight, 1px taller than its rows' spacing, must not
    /// touch it.
    page_clear: f32,
}

impl App {
    /// The folder shows the details list (not icons, tiles or This PC's drives).
    pub fn details_list(st: &State) -> bool {
        !ids::same(&st.path,ids::GALLERY) && !Self::drives_view(st) && Self::icons_spec(st).is_none()
    }

    /// An icon view laid out in rows (not the list, which runs sideways).
    fn icon_rows(st: &State) -> bool {
        Self::icons_spec(st).is_some_and(|spec| !spec.column_major)
    }

    fn steps(st: &State) -> Option<Steps> {
        if Self::details_list(st) {
            Some(Steps { items: Self::list_items(st), view: st.viewport, max: Self::list_max(st), page_clear: 1.0 })
        } else if Self::icon_rows(st) || Self::drives_view(st) {
            let view = st.grid_viewport.1;
            Some(Steps { items: st.grid_items.clone(), view, max: (st.grid_extent - view).max(0.0), page_clear: 0.0 })
        } else {
            None
        }
    }

    /// The wheel scrolls this view by rows.
    pub fn wheel_rows(&self) -> bool {
        self.state.try_borrow().is_ok_and(|st| Self::details_list(&st) || Self::icon_rows(&st) || Self::drives_view(&st))
    }

    /// What the steps line up, in content order: each shown row's highlight and each group
    /// header.
    fn list_items(st: &State) -> Vec<(f32, f32)> {
        let plate = Self::row_height(st);
        let mut items = Vec::with_capacity(st.row_tops.len());
        for (row, &top) in st.row_tops.iter().enumerate() {
            let folded = st.folded.get(row).copied().unwrap_or(false);
            if st.headers.get(row).is_some_and(|h| !h.is_empty()) {
                let label=&st.headers[row];
                let head = Self::group_height(label,folded);
                items.push((top - head, top - head + Self::group_height(label,true)));
            }
            if !folded {
                items.push((top, top + plate));
            }
        }
        items
    }

    fn list_offset(&self) -> f32 {
        -self.ui().global::<Shell>().get_list_scroll_y()
    }

    fn list_max(st: &State) -> f32 {
        (st.list_extent - st.viewport).max(0.0)
    }

    /// Scrolls the view to `y` (within its content) and draws the rows it shows.
    pub fn scroll_list(&self, y: f32) {
        let max = Self::steps(&self.state.borrow()).map_or(0.0, |s| s.max);
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let before = shell.get_list_scroll_y();
        shell.set_list_scroll_y(-y.clamp(0.0, max).round());
        self.sync_list_window(false);
        self.sync_grid_window(false);
        self.sync_drives_scrollbar();
        if shell.get_list_scroll_y() != before {
            self.rehover();
        }
    }

    /// Scrolls the view `px` down (negative: up).
    pub fn nudge_list(&self, px: f32) {
        self.scroll_list(self.list_offset() + px);
    }

    /// Where `lines` rows down (negative: up) from `y` leaves the list.
    fn lines_from(items: &[(f32, f32)], y: f32, view: f32, lines: i32) -> f32 {
        if lines > 0 {
            // The rows shown whole down to the bottom edge, then `lines` more.
            let whole = items.partition_point(|&(_, bottom)| bottom <= y + view + EPSILON);
            match items.get(whole + lines as usize - 1) {
                Some(&(_, bottom)) => bottom - view,
                None => f32::MAX,
            }
        } else {
            // The rows starting above the top edge, less `lines`.
            let above = items.partition_point(|&(top, _)| top < y - EPSILON);
            match above.checked_sub(lines.unsigned_abs() as usize) {
                Some(i) if i > 0 => items[i].0,
                _ => 0.0,
            }
        }
    }

    /// Where a page down (or up) from `y` leaves the list: the first row not shown whole at
    /// the bottom comes to the top; going up, the first one shown whole at the top goes to
    /// be that row.
    fn page_from(items: &[(f32, f32)], y: f32, view: f32, down: bool, clear: f32) -> f32 {
        let whole_to = |y: f32| items.partition_point(|&(_, bottom)| bottom + clear <= y + view + EPSILON);
        if down {
            match items.get(whole_to(y)) {
                Some(&(top, _)) => top.max(y + 1.0),
                None => f32::MAX,
            }
        } else {
            let above = items.partition_point(|&(top, _)| top < y - EPSILON);
            let first = items[..above].partition_point(|&(top, _)| whole_to(top) < above);
            if first == 0 { 0.0 } else { items[first].0.min(y - 1.0) }
        }
    }

    /// The scrollbar's arrows: `lines` rows down (negative: up).
    pub fn list_lines(&self, lines: i32) {
        let Some(steps) = Self::steps(&self.state.borrow()) else { return };
        self.scroll_list(Self::lines_from(&steps.items, self.list_offset(), steps.view, lines));
    }

    /// The scrollbar's track: `pages` pages down (negative: up).
    pub fn list_pages(&self, pages: i32) {
        let Some(steps) = Self::steps(&self.state.borrow()) else { return };
        let mut y = self.list_offset();
        for _ in 0..pages.unsigned_abs() {
            y = Self::page_from(&steps.items, y, steps.view, pages > 0, steps.page_clear).clamp(0.0, steps.max);
        }
        self.scroll_list(y);
    }

    /// The wheel over the view: `notches` toward the user scroll down (negative),
    /// each the system's wheel lines (3), or a page when it is set to scroll pages.
    pub fn wheel_list(&self, notches: i32) {
        let lines = wheel_lines();
        if lines == u32::MAX {
            return self.list_pages(-notches);
        }
        let Some(steps) = Self::steps(&self.state.borrow()) else { return };
        let step = if notches < 0 { lines as i32 } else { -(lines as i32) };
        let mut y = self.list_offset();
        for _ in 0..notches.unsigned_abs() {
            y = Self::lines_from(&steps.items, y, steps.view, step).clamp(0.0, steps.max);
        }
        self.scroll_list(y);
    }

    /// Scrolls the view just enough to show `row` whole: its top (or its group's header) at
    /// the top edge, or its bottom at the bottom one.
    pub fn reveal_list_row(&self, row: usize) {
        if let Some(y) = self.revealed_offset(row) {
            self.scroll_list(y);
        }
    }

    /// Where showing `row` whole leaves the view (none: it already shows).
    fn revealed_offset(&self, row: usize) -> Option<f32> {
        let st = self.state.borrow();
        let (top, bottom, head) = if Self::details_list(&st) {
            let top = Self::row_top(&st, row);
            let head = match st.headers.get(row) {
                Some(h) if !h.is_empty() => Self::group_height(h,false),
                _ => 0.0,
            };
            (top, top + Self::row_height(&st), head)
        } else {
            let cell = st.cells.get(row)?;
            let k = if Self::drives_view(&st) {
                st.drives_rows.iter().position(|r|matches!(&r.kind, super::drives::Kind::Cells(v) if v.contains(&row)))?
            } else {st.grid_items.iter().position(|&(top, _)| top == cell[1])?};
            // A group's first row brings its header (the item before it) along.
            let start = (0..=row).rev().find(|&r| st.headers.get(r).is_some_and(|h| !h.is_empty()));
            let first = start.is_some_and(|g| st.cells.get(g).is_some_and(|c| c[1] == cell[1]));
            let head = if first && k > 0 { cell[1] - st.grid_items[k - 1].0 } else { 0.0 };
            (cell[1], st.grid_items[k].1, head)
        };
        let view = if Self::details_list(&st) { st.viewport } else { st.grid_viewport.1 };
        let y = self.list_offset();
        if top - head < y {
            // The first row shows the list's margin above it too.
            Some(if top - head <= ROW_TOP + EPSILON { 0.0 } else { top - head })
        } else if bottom > y + view {
            Some(bottom - view)
        } else {
            None
        }
    }

    /// The offset a page key works from: the focused row is first scrolled into view (and
    /// stays so even when the focus then moves within it).
    fn page_offset(&self, current: isize) -> f32 {
        if current >= 0 {
            self.reveal_list_row(current as usize);
        }
        self.list_offset()
    }

    /// The row PageDown (or PageUp) moves the focus to: the last (first) row shown whole,
    /// or from there one page on, as Explorer's list does.
    pub fn list_page_target(&self, current: isize, down: bool) -> isize {
        let y = self.page_offset(current);
        let st = self.state.borrow();
        let view = st.viewport;
        let plate = Self::row_height(&st);
        let shown: Vec<usize> = (0..st.order.len()).filter(|&r| !st.folded.get(r).copied().unwrap_or(false)).collect();
        let top = |r: usize| Self::row_top(&st, r);
        let Some(&first) = shown.first() else { return current };
        if down {
            let last = |limit: f32| shown.iter().copied().rev().find(|&r| top(r) + plate <= limit + EPSILON);
            let edge = last(y + view).unwrap_or(first);
            if current < edge as isize {
                return edge as isize;
            }
            let from = current.max(0) as usize;
            last(top(from) + view).map_or(current + 1, |r| (r as isize).max(current + 1))
        } else {
            let first_from = |limit: f32| shown.iter().copied().find(|&r| top(r) >= limit - EPSILON);
            let edge = first_from(y).unwrap_or(first);
            if current > edge as isize {
                return edge as isize;
            }
            let from = current.max(0) as usize;
            first_from(top(from) + plate - view).map_or(current - 1, |r| (r as isize).min(current - 1))
        }
    }

    /// The cell PageDown (or PageUp) moves the focus to in a row-major icon view: under the
    /// focused cell, in the last (first) row shown whole, or from there a page of rows on.
    pub fn grid_page_target(&self, current: isize, down: bool) -> isize {
        let y = self.page_offset(current);
        let st = self.state.borrow();
        let Some(spec) = Self::icons_spec(&st) else { return current };
        let view = st.grid_viewport.1;
        // The shown cells by row: top, bottom (with the gap under it, which counts toward
        // being shown whole), cells.
        let mut rows: Vec<(f32, f32, Vec<usize>)> = Vec::new();
        for (i, cell) in st.cells.iter().enumerate() {
            if st.folded.get(i).copied().unwrap_or(false) {
                continue;
            }
            match rows.last_mut() {
                Some(row) if row.0 == cell[1] => row.2.push(i),
                _ => rows.push((cell[1], cell[1] + cell[3] + spec.gap_y, vec![i])),
            }
        }
        let Some(at) = rows.iter().position(|r| r.2.contains(&(current.max(0) as usize))) else { return current };
        let page = ((view / super::grid::page_pitch(&spec)).floor() as usize).max(1);
        let to = if down {
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
        // The cell in the same column, or the row's last one when it ends sooner.
        let x = st.cells[current.max(0) as usize][0];
        rows[to].2.iter().copied().filter(|&i| st.cells[i][0] <= x + EPSILON).last().unwrap_or(rows[to].2[0]) as isize
    }

    /// The item PageDown (or PageUp) moves the focus to in the List view, which pages by
    /// columns as the details list does by rows: the same row of the last (first) column
    /// shown whole, or from there of the last column whose right edge is within a view of the
    /// focused column's left (25 rows, four whole columns: 1, 76, 151, 226).
    pub fn column_page_target(&self, current: isize, down: bool) -> isize {
        if current >= 0 {
            let path = {
                let st = self.state.borrow();
                st.order.get(current as usize).map(|&i| st.entries[i].path.clone())
            };
            if let Some(path) = path {
                self.reveal(&path);
            }
        }
        let scroll = -self.ui().global::<Shell>().get_grid_scroll_x();
        let st = self.state.borrow();
        let view = st.grid_viewport.0;
        // The columns, left to right: left and right edges, and their cells top to bottom.
        let mut columns: Vec<(f32, f32, Vec<usize>)> = Vec::new();
        for (i, cell) in st.cells.iter().enumerate() {
            match columns.iter_mut().find(|c| (c.0 - cell[0]).abs() < EPSILON) {
                Some(column) => column.2.push(i),
                None => columns.push((cell[0], cell[0] + cell[2], vec![i])),
            }
        }
        columns.sort_by(|a, b| a.0.total_cmp(&b.0));
        let current = current.max(0) as usize;
        let Some(at) = columns.iter().position(|c| c.2.contains(&current)) else { return current as isize };
        let row = columns[at].2.iter().position(|&i| i == current).unwrap_or(0);
        let last = columns.len() - 1;
        let to = if down {
            match columns.iter().rposition(|c| c.1 <= scroll + view + EPSILON) {
                Some(edge) if edge > at => edge,
                _ => columns.iter().rposition(|c| c.1 <= columns[at].0 + view + EPSILON).unwrap_or(at).max(at + 1).min(last),
            }
        } else {
            match columns.iter().position(|c| c.0 >= scroll - EPSILON) {
                Some(edge) if edge < at => edge,
                _ => columns.iter().position(|c| c.0 >= columns[at].1 - view - EPSILON).unwrap_or(at).min(at.saturating_sub(1)),
            }
        };
        let cells = &columns[to].2;
        cells[row.min(cells.len() - 1)] as isize
    }

    /// Keeps the drawn rows to the shown part and a viewport past each end, redrawing them
    /// only once the shown part leaves them (or when the rows themselves changed).
    pub fn sync_list_window(&self, force: bool) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let slots = {
            let mut st = self.state.borrow_mut();
            let details = Self::details_list(&st);
            // A smaller content (or a taller view) pulls the list back.
            let max = Self::list_max(&st);
            let mut y = -shell.get_list_scroll_y();
            if details && y > max {
                y = max;
                shell.set_list_scroll_y(-y);
            }
            let view = st.viewport.max(1.0);
            let pitch = Self::row_pitch(&st);
            let span = |from: f32, to: f32| {
                let first = st.row_tops.partition_point(|&top| top + pitch < from);
                let max_header=if st.headers.iter().any(|h|h.contains(GROUP_SUBTITLE)){50.0}else{GROUP_HEADER};
                let last = st.row_tops.partition_point(|&top| top - max_header <= to);
                (first, last.max(first))
            };
            let needed = if details { span(y, y + view) } else { (0, 0) };
            let (a, b) = st.list_window;
            if !force && a <= needed.0 && needed.1 <= b && (details || a == b) {
                return;
            }
            st.list_window = if details { span(y - view * MARGIN, y + view * (1.0 + MARGIN)) } else { (0, 0) };
            let (a, b) = st.list_window;
            (a..b)
                .filter(|&r| !st.folded.get(r).copied().unwrap_or(false) || st.headers.get(r).is_some_and(|h| !h.is_empty()))
                .map(|r| ListSlot { index: r as i32, y: st.row_tops[r] })
                .collect::<Vec<_>>()
        };
        // In place, so the rows that stay keep their elements.
        let model = &self.list_rows;
        for (i, slot) in slots.iter().enumerate() {
            match model.row_data(i) {
                Some(old) if old == *slot => {}
                Some(_) => model.set_row_data(i, slot.clone()),
                None => model.push(slot.clone()),
            }
        }
        while model.row_count() > slots.len() {
            model.remove(model.row_count() - 1);
        }
    }
}

impl App {
    /// Keeps the icon views' drawn cells to the shown part and a viewport past each end (down
    /// the rows, or across the list's columns), redrawing them only once the shown part
    /// leaves them (or when the cells themselves changed).
    pub fn sync_grid_window(&self, force: bool) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let slots: Vec<i32> = {
            let mut st = self.state.borrow_mut();
            if Self::icons_spec(&st).is_none() && !Self::drives_view(&st) {
                st.grid_window = (0.0, 0.0);
                Vec::new()
            } else {
                let across = st.grid_column_major;
                let (from, view) = if across { (-shell.get_grid_scroll_x(), st.grid_viewport.0) } else { (-shell.get_list_scroll_y(), st.grid_viewport.1) };
                let view = view.max(1.0);
                let (a, b) = st.grid_window;
                if !force && a <= from && from + view <= b {
                    return;
                }
                let (a, b) = (from - view * MARGIN, from + view * (1.0 + MARGIN));
                st.grid_window = (a, b);
                st.cells
                    .iter()
                    .enumerate()
                    .filter(|&(i, c)| {
                        let (start, size) = if across { (c[0], c[2]) } else { (c[1], c[3]) };
                        !st.folded.get(i).copied().unwrap_or(false) && c[2] > 0.0 && start + size > a && start < b
                    })
                    .map(|(i, _)| i as i32)
                    .collect()
            }
        };
        let model = &self.grid_slots;
        for (i, &slot) in slots.iter().enumerate() {
            match model.row_data(i) {
                Some(old) if old == slot => {}
                Some(_) => model.set_row_data(i, slot),
                None => model.push(slot),
            }
        }
        while model.row_count() > slots.len() {
            model.remove(model.row_count() - 1);
        }
    }
}

impl App {
    /// The view moved under a still pointer: the item now under it takes the hover (and its
    /// tip in time), as in Explorer. A pointer move where it is, once the frame is laid out.
    pub fn rehover(&self) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::ScreenToClient;
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let hwnd = self.hwnd.get();
        let mut pt = POINT::default();
        let scale = unsafe {
            if GetCursorPos(&mut pt).is_err() || !ScreenToClient(hwnd, &mut pt).as_bool() {
                return;
            }
            GetDpiForWindow(hwnd).max(96) as f32 / 96.0
        };
        let weak = self.ui.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(ui) = weak.upgrade() {
                let size = ui.window().size().to_logical(ui.window().scale_factor());
                let (x, y) = (pt.x as f32 / scale, pt.y as f32 / scale);
                if x >= 0.0 && y >= 0.0 && x < size.width && y < size.height {
                    ui.window().dispatch_event(slint::platform::WindowEvent::PointerMoved { position: slint::LogicalPosition::new(x, y) });
                }
            }
        });
    }
}

/// The lines a wheel notch scrolls (u32::MAX: a page).
fn wheel_lines() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut lines: u32 = 3;
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    lines.max(1)
}
