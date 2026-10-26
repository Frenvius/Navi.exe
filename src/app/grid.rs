//! Icon views: cell layout (rows grow with two-line names, like Explorer) and thumbnails.

use std::collections::HashSet;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::{ids, images, post, App, State, GROUP_SUBTITLE};
use crate::shell::icons;
use crate::shell::listing::PropList;
use crate::{Field, Measure, Shell};

pub struct Spec {
    pub icon: f32,
    pub cell_w: f32,
    pub base_h: f32,
    pub gap_x: f32,
    pub gap_y: f32,
    /// 0 stacked (name under the icon), 1 inline (icon and name on a line), 2 tile, 3 content.
    pub kind: i32,
    /// List view fills columns top to bottom and scrolls sideways.
    pub column_major: bool,
    /// Content view: one full-width cell per row.
    pub full_width: bool,
    /// Icon views spread the columns over the width: as many as fit at `cell_w + gap_x`,
    /// then each gets an equal share.
    pub justify: bool,
}

const fn stacked(icon: f32, cell_w: f32) -> Spec {
    Spec { icon, cell_w, base_h: 2.0 + icon + 18.0, gap_x: 1.0, gap_y: 1.0, kind: 0, column_major: false, full_width: false, justify: true }
}

const fn inline(column_major: bool) -> Spec {
    Spec { icon: 16.0, cell_w: 308.0, base_h: 20.0, gap_x: 1.0, gap_y: 1.0, kind: 1, column_major, full_width: false, justify: false }
}

/// Cell geometry per view mode, measured from explorer.exe at 100%. Stacked cells are the
/// icon 2px from the top over 18px of name, 15px more per extra line (up to four); the
/// large cell is 105 wide. Small icons are 308x20 cells 1px apart, the list the same rows
/// in columns as wide as their longest name, tiles 250x52 cells 4px apart, content
/// full-width 48px rows.
pub fn spec(mode: i32, size: u32) -> Option<Spec> {
    let s = size as f32;
    match mode {
        // Any size from 32 up: as wide as the icon and a margin that grows with it (271 at
        // 256, 105 at 96), 74 at least.
        1..=3 => Some(stacked(s, (s + (5.5 + s / 27.0).round()).max(74.0))),
        // Small icons up to 31px: the row 4px taller than the icon.
        4 => Some(Spec { icon: s, base_h: s + 4.0, ..inline(false) }),
        5 => Some(inline(true)),
        7 => Some(Spec { icon: 48.0, cell_w: 250.0, base_h: 52.0, gap_x: 4.0, gap_y: 4.0, kind: 2, column_major: false, full_width: false, justify: false }),
        8 => Some(Spec { icon: 32.0, cell_w: 0.0, base_h: 48.0, gap_x: 0.0, gap_y: 0.0, kind: 3, column_major: false, full_width: true, justify: false }),
        _ => None,
    }
}

/// The row height PageUp/PageDown count a page in (a page is as many as the view holds
/// whole): stacked cells as with two-line names, small icons 2px taller than their cell,
/// the others their row.
pub fn page_pitch(spec: &Spec) -> f32 {
    match spec.kind {
        0 => spec.base_h + LINE + spec.gap_y,
        1 => spec.base_h + 2.0,
        _ => spec.base_h + spec.gap_y,
    }
}

/// DirectUI's integer text width (one pixel inside the 6px-side label box).
fn label_key(spec: &Spec) -> u32 {
    (spec.cell_w - 13.0).round() as u32
}

const LEFT: f32 = 14.0;
const TOP: f32 = 6.0;
/// Extra name lines of a stacked cell.
const LINE: f32 = 15.0;
const MAX_LINES: u8 = 4;

/// What a shown scrollbar takes from the height the list's columns fill.
const SCROLLBAR: f32 = 16.0;
/// What the 17px vertical bar takes from the width the columns spread over.
const VSCROLLBAR: f32 = 17.0;
/// List view: a column is its longest name plus the icon before it and a margin after,
/// and the next starts 10px later.
const LIST_TEXT_X: f32 = 50.0;
const LIST_PAD: f32 = 6.0;
const LIST_GAP: f32 = 10.0;
/// A grouped list's header is its name plus 6px before and 21px after.
const LIST_HEADER_PAD: f32 = 27.0;

/// Cells (x, y, w, h), the header of each group start (x, y, width: y negative for none,
/// width negative for the whole view), the rows and headers the scroll steps line up, and
/// the column count, content height and content width of one layout.
struct Layout {
    empty: Vec<crate::EmptyLibraryGroup>,
    cells: Vec<[f32; 4]>,
    headers: Vec<[f32; 3]>,
    items: Vec<(f32, f32)>,
    columns: usize,
    height: f32,
    width: f32,
}

impl App {
    /// The icon view spec when the current folder shows one.
    pub fn icons_spec(st: &State) -> Option<Spec> {
        if Self::drives_view(st) || super::ids::same(&st.path, super::ids::HOME) || super::ids::same(&st.path,super::ids::GALLERY) {
            return None;
        }
        spec(st.view_mode, st.icon_size)
    }

    /// The UI measured a name: kept for every row (and view) showing it.
    pub fn name_measured(self: &Rc<Self>, name: &str, width: f32, _lines: i32) {
        {
            let mut st = self.state.borrow_mut();
            let Some(spec) = Self::icons_spec(&st) else { return };
            if spec.kind != 0 {
                st.name_widths.insert(name.to_string(), width);
            }
        }
        self.schedule_layout();
    }

    /// Fills each row's name lines (stacked views) or width (the list) from the names
    /// measured, and hands the others to the UI to measure; once all are in, the measuring
    /// texts go.
    fn measure_names(&self, st: &mut State, spec: &Spec) {
        let n = st.order.len();
        st.label_lines = vec![1; n];
        st.label_widths = vec![0.0; n];
        let mut misses = Vec::new();
        let mut native_misses = Vec::new();
        if spec.kind == 0 || spec.column_major {
            let key = label_key(spec);
            let lines = st.name_lines.entry(key).or_default();
            for (row, &i) in st.order.iter().enumerate() {
                let name = st.entries[i].name.as_str();
                if spec.kind == 0 {
                    if let Some(label) = lines.get(name) {
                        st.label_lines[row] = label.lines;
                    } else if st.name_pending.insert((key, name.to_string())) {
                        native_misses.push(name.to_string());
                    }
                } else {
                    match st.name_widths.get(name) {
                        Some(&w) => st.label_widths[row] = w,
                        None => misses.push(row),
                    }
                }
            }
        }
        if !native_misses.is_empty() {
            let key = label_key(spec);
            std::thread::spawn(move || {
                for chunk in native_misses.chunks(256) {
                    let measured: Vec<_> = chunk.iter().map(|name| {
                        let label = crate::shell::text::icon_label(name, key as i32);
                        debug_assert_eq!(label.full.lines().count().max(1), label.lines as usize);
                        (name.clone(), label)
                    }).collect();
                    post(move |app| {
                        {
                            let mut state = app.state.borrow_mut();
                            for (name, label) in measured {
                                state.name_pending.remove(&(key, name.clone()));
                                state.name_lines.entry(key).or_default().insert(name, Rc::new(label));
                            }
                        }
                        app.schedule_layout();
                    });
                }
            });
        }
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        if misses.is_empty() {
            if !st.measuring.is_empty() {
                st.measuring.clear();
                shell.set_grid_measure(ModelRc::default());
            }
        } else if misses.iter().any(|r| !st.measuring.contains(r)) {
            st.measuring = misses.iter().copied().collect();
            shell.set_grid_measure_wrap(spec.kind == 0);
            let items: Vec<Measure> = misses.iter().map(|&row| Measure { index: row as i32, name: st.entries[st.order[row]].name.as_str().into() }).collect();
            shell.set_grid_measure(ModelRc::new(VecModel::from(items)));
        }
    }

    pub fn set_header_width(self: &Rc<Self>, row: usize, width: f32) {
        {
            let mut st = self.state.borrow_mut();
            if row >= st.order.len() { return; }
            if st.header_widths.len() <= row {
                st.header_widths.resize(row + 1, 0.0);
            }
            if st.header_widths[row] == width {
                return;
            }
            st.header_widths[row] = width;
        }
        self.schedule_layout();
    }

    pub fn set_grid_viewport(self: &Rc<Self>, width: f32, height: f32) {
        {
            let mut st = self.state.borrow_mut();
            st.grid_viewport = (width, height);
        }
        self.schedule_layout();
    }

    /// Lays the cells out once the current batch of size reports is in.
    pub fn schedule_layout(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.layout_timer.start(slint::TimerMode::SingleShot, std::time::Duration::ZERO, move || {
            if let Some(app) = weak.upgrade() {
                app.relayout();
            }
        });
    }

    pub fn relayout(&self) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let mut st = self.state.borrow_mut();
        let Some(spec) = Self::icons_spec(&st) else {
            shell.set_icons_view(false);
            drop(st);
            self.sync_grid_window(true);
            return;
        };
        shell.set_icons_view(true);
        shell.set_icon_size(spec.icon);
        shell.set_grid_label_width(spec.cell_w - 12.0);
        shell.set_grid_list(spec.column_major);
        self.measure_names(&mut st, &spec);
        shell.set_grid_kind(spec.kind);
        let (width, view_h) = st.grid_viewport;
        let layout = if spec.column_major {
            // List: columns fill top to bottom; with more than fit, a horizontal scrollbar
            // takes a row's worth of height.
            let fit = Self::list_layout(&st, &spec, view_h);
            if fit.width > width { Self::list_layout(&st, &spec, view_h - SCROLLBAR) } else { fit }
        } else {
            // Without the bar the columns spread over the width less the left margin, and the
            // last one may end 1px into it; the bar takes 17px and that pixel.
            let fit = Self::rows_layout(&st, &spec, width - LEFT, 1.0);
            if fit.height > view_h { Self::rows_layout(&st, &spec, width - LEFT - VSCROLLBAR, 0.0) } else { fit }
        };
        shell.set_grid_width(layout.width);
        shell.set_empty_library_groups(slint::ModelRc::new(slint::VecModel::from(layout.empty.clone())));
        st.grid_columns = layout.columns;
        st.grid_column_major = spec.column_major;
        let labels: Vec<_> = if spec.kind == 0 {
            st.order.iter().map(|&i| st.name_lines.get(&label_key(&spec)).and_then(|m| m.get(&st.entries[i].name)).cloned()).collect()
        } else { Vec::new() };
        drop(st);
        for (row, cell) in layout.cells.iter().enumerate() {
            if let Some(mut data) = self.entries.row_data(row) {
                let [header_x, header_y, header_w] = layout.headers[row];
                let label = labels.get(row).and_then(|label| label.as_ref());
                let label_changed = label.is_some_and(|label| data.stacked_collapsed != label.collapsed);
                if data.cell_x != cell[0] || data.cell_y != cell[1] || data.cell_w != cell[2] || data.cell_h != cell[3] || data.header_y != header_y || data.header_x != header_x || data.header_w != header_w || label_changed {
                    if let Some(label) = label {
                        data.stacked_collapsed = label.collapsed.as_str().into();
                    }
                    data.header_x = header_x;
                    data.header_w = header_w;
                    data.cell_x = cell[0];
                    data.cell_y = cell[1];
                    data.cell_w = cell[2];
                    data.cell_h = cell[3];
                    data.header_y = header_y;
                    self.entries.set_row_data(row, data);
                }
            }
        }
        let heads: Vec<i32> = layout.headers.iter().enumerate().filter(|(_, h)| h[1] >= 0.0).map(|(i, _)| i as i32).collect();
        shell.set_grid_heads(ModelRc::new(VecModel::from(heads)));
        {
            let mut st = self.state.borrow_mut();
            st.cells = layout.cells;
            st.grid_items = layout.items;
            st.grid_extent = layout.height;
        }
        shell.set_grid_height(layout.height);
        self.sync_grid_window(true);
        self.sync_column_bar();
    }

    /// List view: as many 21px rows per column as the height holds; each column as wide as
    /// its longest name. Grouped, each group is a run of columns under its header (15px
    /// text, no chevron), as wide as the header at least, and the next follows 1px later.
    fn list_layout(st: &State, spec: &Spec, view_h: f32) -> Layout {
        let n = st.order.len();
        let pitch = spec.base_h + spec.gap_y;
        let library=Self::library_location_group(st);
        let grouped = !st.headers.is_empty() || library;
        let top = if grouped { 2.0 + if library {50.0}else{32.0} } else { TOP };
        let rows = (((view_h - top) / pitch).floor() as usize).max(1);
        let mut cells = Vec::with_capacity(n);
        let mut headers = vec![[0.0, -1.0, 0.0]; n];
        let mut x = LEFT;
        let mut empty = Vec::new();
        for (label, group_start, group_end) in Self::view_groups(st) {
            let group_x = x;
            if group_start == group_end {
                empty.push(crate::EmptyLibraryGroup {label:label.clone().into(), x, y:2.0, width:302.0, folded:st.collapsed.contains(&label)});
                x += 302.0;
                continue;
            }
            if st.folded.get(group_start).copied().unwrap_or(false) {
                cells.extend(std::iter::repeat_n([0.0; 4], group_end - group_start));
            } else {
                for start in (group_start..group_end).step_by(rows) {
                    let end = (start + rows).min(group_end);
                    let text = (start..end).map(|i| st.label_widths.get(i).copied().unwrap_or(0.0)).fold(0.0f32, f32::max);
                    let w = (LIST_TEXT_X + text + LIST_PAD).ceil();
                    for r in 0..end - start {
                        cells.push([x, top + r as f32 * pitch, w, spec.base_h]);
                    }
                    x += w + if grouped { 1.0 } else { LIST_GAP };
                }
            }
            if grouped {
                let header = st.header_widths.get(group_start).copied().unwrap_or(0.0);
                let header_w = (LIST_HEADER_PAD + header).ceil();
                x = x.max(group_x + header_w).max(group_x+if library {302.0}else{0.0});
                headers[group_start] = [group_x, 2.0, x - group_x - if library {16.0}else{1.0}];
            }
        }
        let gap = if grouped { 1.0 } else { LIST_GAP };
        Layout { empty, cells, headers, items: Vec::new(), columns: rows, height: 0.0, width: x - gap + LEFT }
    }

    /// The groups as row ranges (one, everything, when not grouped).
    fn groups(st: &State) -> Vec<(usize, usize)> {
        let n = st.order.len();
        let grouped = !st.headers.is_empty();
        let mut starts: Vec<usize> = (0..n).filter(|&i| i == 0 || (grouped && st.headers.get(i).is_some_and(|h| !h.is_empty()))).collect();
        starts.dedup();
        starts.iter().enumerate().map(|(k, &s)| (s, starts.get(k + 1).copied().unwrap_or(n))).collect()
    }

    fn view_groups(st:&State)->Vec<(String,usize,usize)> {
        if Self::library_location_group(st) {Self::library_ranges(st)}
        else {Self::groups(st).into_iter().map(|(a,b)|(st.headers.get(a).cloned().unwrap_or_default(),a,b)).collect()}
    }

    /// Row-major views over `avail` pixels from the first column's left edge; a column fits
    /// when its cell ends within `avail + slack`.
    fn rows_layout(st: &State, spec: &Spec, avail: f32, slack: f32) -> Layout {
        let library=Self::library_location_group(st);
        let n = st.order.len();
        let mut cells = Vec::with_capacity(n);
        let mut headers = vec![[0.0, -1.0, -1.0]; n];
        let (cell_w, columns, pitch_x) = if spec.full_width {
            // The separator (the cell less 13px) ends 10px before the list's edge; the bar
            // covers 2px of that.
            let w = (if slack > 0.0 { avail } else { avail + 2.0 }).max(200.0);
            (w, 1, w)
        } else {
            let pitch = spec.cell_w + spec.gap_x;
            let columns = (((avail + slack) / pitch).floor() as usize).max(1);
            let pitch = if spec.justify { (avail / columns as f32).floor().max(pitch) } else { pitch };
            (spec.cell_w, columns, pitch)
        };
        // Grouped, each group starts a row under its 32px header (31 over small icons); the
        // next header follows the group's last row 1px closer than a row would. The first
        // sits 2px from the top.
        let grouped = !st.headers.is_empty() || library;
        let (head, tail) = match spec.kind {
            1 => (31.0, spec.gap_y),
            3 => (32.0, spec.gap_y),
            _ => (32.0, spec.gap_y + 1.0),
        };
        let mut y = if grouped { 2.0 } else { TOP };
        let mut items = Vec::new();
        let mut empty = Vec::new();
        for (k,(label, group_start, group_end)) in Self::view_groups(st).into_iter().enumerate() {
            if library && k>0 {y+=13.0;}
            if group_start == group_end {
                let folded=st.collapsed.contains(&label);
                empty.push(crate::EmptyLibraryGroup {label:label.into(),x:0.0,y,width:-1.0,folded});
                y+=if folded {48.0}else{82.0};
                continue;
            }
            if grouped {
                headers[group_start] = [0.0, y, -1.0];
                let extra=if st.headers[group_start].contains(GROUP_SUBTITLE){18.0}else{0.0};
                items.push((y, y + 30.0+extra));
                if st.folded.get(group_start).copied().unwrap_or(false) {
                    // Collapsed: the header alone, 30px.
                    cells.extend(std::iter::repeat_n([0.0; 4], group_end - group_start));
                    y += 30.0+extra;
                    continue;
                }
                y += head+extra;
            }
            for row_start in (group_start..group_end).step_by(columns) {
                let row_end = (row_start + columns).min(group_end);
                let lines = if spec.kind == 0 {
                    (row_start..row_end).map(|i| st.label_lines.get(i).copied().unwrap_or(1)).max().unwrap_or(1).min(MAX_LINES)
                } else {
                    1
                };
                let h = spec.base_h + LINE * (lines as f32 - 1.0);
                for (c, _) in (row_start..row_end).enumerate() {
                    cells.push([LEFT + c as f32 * pitch_x, y, cell_w, h]);
                }
                items.push((y, y + h + spec.gap_y));
                y += h + spec.gap_y;
            }
            if grouped {
                y -= tail;
            }
        }
        let end = if grouped { y } else { y - spec.gap_y };
        // The content ends at the last row: scrolled to the end, it sits on the bottom edge.
        Layout { empty, cells, headers, items, columns, height: if n == 0 && !Self::library_location_group(st) { 0.0 } else { end }, width: 0.0 }
    }

    /// Rubber band over the icon cells (content coordinates).
    pub fn band_cells(&self, x: f32, y: f32, w: f32, h: f32) -> Vec<usize> {
        let st = self.state.borrow();
        st.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c[0] + c[2] > x && c[0] < x + w && c[1] + c[3] > y && c[1] < y + h)
            .map(|(i, _)| i)
            .collect()
    }

    /// Tiles and content: reads each item's property lines for the view off the UI thread.
    pub fn load_fields(self: &Rc<Self>) {
        let (list, paths, request, identities) = {
            let st = self.state.borrow();
            let list = match Self::icons_spec(&st).map(|s| s.kind) {
                Some(2) => PropList::Tile,
                Some(3) if super::ids::is_search(&st.path) => PropList::ContentSearch,
                Some(3) => PropList::Content,
                _ => return,
            };
            let shell=self.ui();let shell=shell.global::<Shell>();
            let top=-shell.get_list_scroll_y();let height=st.grid_viewport.1.max(1.0);
            let paths: Vec<String> = st.order.iter().enumerate()
                .filter(|(row,_)|st.cells.get(*row).is_none_or(|c|c[1]+c[3]>=top-height && c[1]<=top+2.0*height))
                .map(|(_,&i)| st.entries[i].path.clone())
                .filter(|p| !st.fields.contains_key(&(list,p.clone())) && self.pending_fields.borrow_mut().insert((st.request,list,p.clone()))).collect();
            let wanted=paths.iter().map(String::as_str).collect::<HashSet<_>>();
            let identities=st.entries.iter().filter(|e|!e.absolute.is_empty() && wanted.contains(e.path.as_str())).map(|e|(e.path.clone(),e.absolute.clone())).collect::<std::collections::HashMap<_,_>>();
            (list, paths, st.request,identities)
        };
        self.apply_fields(list);
        if paths.is_empty() {
            return;
        }
        let generation=self.search_generation.clone();
        std::thread::spawn(move || {
            crate::shell::com_init();
            let mut batch = Vec::new();
            for (k, path) in paths.into_iter().enumerate() {
                if generation.load(std::sync::atomic::Ordering::Acquire)!=request {return;}
                let lines = crate::shell::listing::prop_list_with_id(&path,list,identities.get(&path).map(Vec::as_slice).unwrap_or_default());
                // As a content field shows them: "label:", 4px, the value.
                let texts: Vec<String> = lines.iter().flatten().flat_map(|(l, v)| [if l.is_empty() { String::new() } else { format!("{l}:") }, v.clone()]).collect();
                let mut widths = crate::shell::columns::widths(texts.iter().map(String::as_str)).into_iter();
                let lines = lines
                    .into_iter()
                    .map(|line| {
                        line.map(|(label, value)| {
                            let (l, v) = (widths.next().unwrap_or(0.0), widths.next().unwrap_or(0.0));
                            let width = if label.is_empty() { v } else { l + 4.0 + v };
                            (label, value, width)
                        })
                    })
                    .collect();
                batch.push((path, lines));
                if k % 16 == 15 {
                    let ready = std::mem::take(&mut batch);
                    post(move |app| app.fields_ready(request, list, ready));
                }
            }
            if !batch.is_empty() {
                post(move |app| app.fields_ready(request, list, batch));
            }
        });
    }

    fn fields_ready(&self, request: u64, list: PropList, ready: Vec<(String, Vec<Option<(String, String, f32)>>)>) {
        for (path,_) in &ready {self.pending_fields.borrow_mut().remove(&(request,list,path.clone()));}
        {
            let mut st = self.state.borrow_mut();
            if st.request != request {
                return;
            }
            for (path, lines) in &ready {
                st.fields.insert((list, path.clone()), lines.clone());
            }
        }
        let changed: HashSet<String> = ready.into_iter().map(|(p, _)| p).collect();
        self.apply_fields_for(list, Some(&changed));
    }

    /// The pointer rests on an item: reads its infotip (the type's "InfoTip" list as
    /// "label: value" lines) off the UI thread, once.
    pub fn item_hovered(self: &Rc<Self>, row: usize) {
        if !self.state.borrow().click_settings.info_tips {return;}
        let (path, request) = {
            let st = self.state.borrow();
            let Some(&i) = st.order.get(row) else { return };
            let path = st.entries[i].path.clone();
            if st.fields.contains_key(&(PropList::InfoTip, path.clone())) {
                drop(st);
                self.apply_tip(row);
                return;
            }
            (path, st.request)
        };
        std::thread::spawn(move || {
            crate::shell::com_init();
            let lines = crate::shell::listing::info_tip(&path).into_iter().map(|line| Some((String::new(), line, 0.0))).collect();
            post(move |app| {
                {
                    let mut st = app.state.borrow_mut();
                    if st.request != request {
                        return;
                    }
                    st.fields.insert((PropList::InfoTip, path.clone()), lines);
                }
                let row = { let st = app.state.borrow(); st.order.iter().position(|&i| st.entries[i].path == path) };
                if let Some(row) = row {
                    app.apply_tip(row);
                }
            });
        });
    }

    /// A click puts the keyboard's tip away (and one still to come).
    pub fn hide_key_tip(&self) {
        self.key_tip_timer.stop();
        self.key_tip.set(None);
        let ui = self.ui();
        if ui.global::<Shell>().get_key_tip_row() >= 0 {
            ui.global::<Shell>().set_key_tip_row(-1);
            ui.global::<crate::Tip>().set_shown(false);
        }
    }

    /// A key was handled: if it moved the focus, the new item's tip shows under it two
    /// seconds later (as Explorer's does), and any shown tip goes.
    pub fn key_moved(self: &Rc<Self>, before: Option<String>) {
        let ui = self.ui();
        ui.global::<Shell>().set_key_tip_row(-1);
        self.key_tip.set(None);
        if !self.state.borrow().click_settings.info_tips {self.key_tip_timer.stop();return;}
        let now = self.state.borrow().focused.clone();
        if now.is_none() || now == before {
            self.key_tip_timer.stop();
            return;
        }
        let weak = Rc::downgrade(self);
        self.key_tip_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(2000), move || {
            let Some(app) = weak.upgrade() else { return };
            let row = { let st = app.state.borrow(); st.focused.as_ref().and_then(|f| Self::row_of(&st, f)) };
            if let Some(row) = row {
                app.key_tip.set(Some(row));
                app.item_hovered(row);
            }
        });
    }

    fn apply_tip(&self, row: usize) {
        let st = self.state.borrow();
        let Some(&i) = st.order.get(row) else { return };
        let Some(lines) = st.fields.get(&(PropList::InfoTip, st.entries[i].path.clone())) else { return };
        let tip = lines
            .iter()
            .flatten()
            .map(|(label, value, _)| clean(&if label.is_empty() { value.clone() } else { format!("{label}: {value}") }))
            .collect::<Vec<_>>()
            .join("
");
        if let Some(mut data) = self.entries.row_data(row) {
            if data.tip != tip.as_str() {
                data.tip = tip.into();
                self.entries.set_row_data(row, data);
            }
        }
        drop(st);
        if self.key_tip.get() == Some(row) {
            self.key_tip.set(None);
            self.ui().global::<Shell>().set_key_tip_row(row as i32);
        }
    }

    fn apply_fields(&self, list: PropList) {
        self.apply_fields_for(list, None);
    }

    /// Sets each row's property lines (bidi marks stripped) from what has been read: all
    /// rows, or those of `only` (a batch that just came in).
    fn apply_fields_for(&self, list: PropList, only: Option<&HashSet<String>>) {
        let st = self.state.borrow();
        let clean = |s: &str| s.chars().filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}')).collect::<String>();
        for (row, &i) in st.order.iter().enumerate() {
            if only.is_some_and(|set| !set.contains(&st.entries[i].path)) {
                continue;
            }
            let Some(lines) = st.fields.get(&(list, st.entries[i].path.clone())) else { continue };
            let Some(mut data) = self.entries.row_data(row) else { continue };
            let fields: Vec<Field> = lines
                .iter()
                .map(|l| match l {
                    Some((label, value, width)) => Field { shown: true, label: clean(label).into(), value: clean(value).into(), width: *width },
                    None => Field::default(),
                })
                .collect();
            data.fields = ModelRc::new(VecModel::from(fields));
            self.entries.set_row_data(row, data);
        }
    }

    /// Fills the icon view with system icons right away and swaps in shell thumbnails
    /// (folder previews, images...) as a worker thread produces them.
    pub fn load_thumbnails(self: &Rc<Self>) {
        let (paths, size, request, library) = {
            let st = self.state.borrow();
            let Some(spec) = Self::icons_spec(&st) else { return };
            // Small icons and the list draw the 16px system icons.
            if spec.kind == 1 && spec.icon <= 16.0 {
                return;
            }
            let network=ids::same(&st.path,ids::NETWORK);
            let paths: Vec<(String, u32, Vec<u8>)> = self.grid_slots.iter().filter_map(|row|st.order.get(row as usize))
                .map(|&i|&st.entries[i]).filter(|e|e.is_fs||network)
                .map(|e|(e.path.clone(),e.icon,if network {e.absolute.clone()}else{Vec::new()})).collect();
            (paths, spec.icon as u32, st.request, ids::is_library(&st.path))
        };
        let mut missing: Vec<(String, u32, Vec<u8>)> = {
            let images = self.images.borrow();
            paths.into_iter().filter(|(p, _, _)| !images.has_thumb(p,size) && self.pending_thumbs.borrow_mut().insert((p.clone(),size))).collect()
        };
        // The cells drawn now first.
        let shown: HashSet<String> = {
            let st = self.state.borrow();
            self.grid_slots.iter().filter_map(|i| st.order.get(i as usize).map(|&e| st.entries[e].path.clone())).collect()
        };
        missing.sort_by_key(|(p, _, _)| !shown.contains(p));
        self.apply_thumbnails(size);
        if missing.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            crate::shell::com_init();
            let mut batch = Vec::new();
            for (k, (path, icon, id)) in missing.into_iter().enumerate() {
                // No picture: its system icon at the size.
                let picture=if library {icons::library_picture(&path,size)}else{icons::network_picture(&path,&id,size)};
                if let Ok(rgba) = picture.or_else(|_| icons::icon(icon, size)) {
                    batch.push((path, rgba));
                }
                if batch.len() >= 8 || k % 8 == 7 {
                    let ready = std::mem::take(&mut batch);
                    post(move |app| app.thumbnails_ready(request, size, ready));
                }
            }
            if !batch.is_empty() {
                post(move |app| app.thumbnails_ready(request, size, batch));
            }
        });
    }

    fn thumbnails_ready(&self, request: u64, size: u32, ready: Vec<(String, icons::Rgba)>) {
        for (path,_) in &ready {self.pending_thumbs.borrow_mut().remove(&(path.clone(),size));}
        if self.state.borrow().request != request {return;}
        {
            let mut images = self.images.borrow_mut();
            for (path, rgba) in &ready {
                images.insert_thumb(path, size, images::to_image(rgba));
            }
        }
        if self.state.borrow().request == request {
            let changed: HashSet<String> = ready.into_iter().map(|(p, _)| p).collect();
            self.apply_thumbnails_for(size, Some(&changed));
        }
    }

    fn apply_thumbnails(&self, size: u32) {
        self.apply_thumbnails_for(size, None);
    }

    /// Sets each row's large image: its thumbnail when known, else the system icon.
    fn apply_thumbnails_for(&self, size: u32, only: Option<&HashSet<String>>) {
        let st = self.state.borrow();
        let mut images = self.images.borrow_mut();
        let visible=self.grid_slots.iter().map(|i|i as usize).collect::<HashSet<_>>();
        for (row, &i) in st.order.iter().enumerate() {
            let e = &st.entries[i];
            if !visible.contains(&row){continue;}
            if only.map(|set| !set.contains(&e.path)).unwrap_or(false) {
                continue;
            }
            // File system items get their picture from the worker; until then an icon
            // already rendered stands in, as Explorer shows the generic one first.
            let image = match images.thumb(&e.path, size) {
                Some(thumb) => thumb,
                None if e.is_fs && only.is_none() => images.icon_cached(e.icon, size).unwrap_or_default(),
                None => images.icon(e.icon, size),
            };
            if let Some(mut data) = self.entries.row_data(row) {
                if data.big_icon==image {continue;}
                data.big_icon = image;
                self.entries.set_row_data(row, data);
            }
        }
    }
}

/// Drops the bidi marks the property system wraps dates and dimensions in.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}')).collect()
}

/// The icon size a view mode starts at.
pub fn mode_icon_size(mode: i32) -> u32 {
    match mode {
        1 => 256,
        2 => 96,
        3 => 48,
        _ => 16,
    }
}

/// The sizes Ctrl+wheel stops at, past details and list (Explorer's own, 16 to 256; up to
/// 31 they are small icons).
const ZOOM_SIZES: [u32; 39] = [
    16, 18, 20, 22, 23, 25, 27, 29, 31, 33, 35, 38, 41, 44, 47, 50, 54, 58, 62, 67, 72, 77, 83, 89, 95, 102, 110, 118, 126,
    136, 145, 156, 168, 180, 193, 207, 223, 239, 256,
];
pub const ZOOM_STEPS: usize = ZOOM_SIZES.len() + 2;

/// Where a view sits on the Ctrl+wheel scale (none for tiles and content). A size between
/// stops takes the stop below it.
pub fn zoom_index(mode: i32, size: u32) -> Option<usize> {
    match mode {
        super::VIEW_DETAILS => Some(0),
        5 => Some(1),
        1..=4 => Some(2 + ZOOM_SIZES.iter().rposition(|&s| s <= size).unwrap_or(0)),
        _ => None,
    }
}

/// The view mode and icon size of a Ctrl+wheel stop.
pub fn zoom_step(index: usize) -> (i32, u32) {
    match index {
        0 => (super::VIEW_DETAILS, 16),
        1 => (5, 16),
        _ => {
            let size = ZOOM_SIZES[index - 2];
            let mode = match size {
                s if s >= 256 => 1,
                s if s >= 96 => 2,
                s if s >= 32 => 3,
                _ => 4,
            };
            (mode, size)
        }
    }
}

