//! The details view's columns in the user's order, their row texts, and the header menu.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Author, PKEY_DateCreated, PKEY_DateModified, PKEY_ItemNameDisplay, PKEY_ItemTypeText, PKEY_Keywords, PKEY_Size, PKEY_Title,
    PKEY_ItemFolderPathDisplay, PKEY_ItemFolderPathDisplayNarrow,
};

use super::{format, post, App, State};
use crate::shell::columns::{self, Column};
use crate::shell::listing::ShellEntry;
use crate::{ColumnHead, Shell};

/// The generic folder type's list, for types that have none.
const GENERIC: [(PROPERTYKEY, bool); 8] = [
    (PKEY_ItemNameDisplay, true),
    (PKEY_DateModified, true),
    (PKEY_ItemTypeText, true),
    (PKEY_Size, true),
    (PKEY_DateCreated, false),
    (PKEY_Author, false),
    (PKEY_Keywords, false),
    (PKEY_Title, false),
];

/// Literal search decorations in Name/Folder. The complete literal wins; the shell
/// also marks its word tokens in a containing folder that doesn't contain the phrase.
/// AQS highlighting requires resolved query terms, so do not decorate its syntax.
pub fn search_hits(text: &str, query: &str) -> Vec<crate::Hit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || query.contains(':') { return Vec::new(); }
    // Preserve original byte boundaries when case folding expands a character.
    let mut folded = String::new();
    let mut boundaries = Vec::new();
    for (byte,ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            let start=folded.len();folded.push(lower);
            for _ in start..folded.len() { boundaries.push((byte,byte+ch.len_utf8())); }
        }
    }
    let terms: Vec<&str> = if folded.contains(&query) { vec![query.as_str()] }
        else { query.split(|c:char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect() };
    let mut spans: Vec<(usize,usize)> = terms.iter().flat_map(|term| folded.match_indices(term)
        .map(|(at,matched)| (boundaries[at].0,boundaries[at+matched.len()-1].1))).collect();
    spans.sort_unstable();spans.dedup();
    spans.into_iter().map(|(start,end)| {
        let widths=columns::widths_at([&text[..start],&text[start..end]],12);
        crate::Hit{x:widths[0],width:widths[1]}
    }).collect()
}

/// Explorer's default widths of the generic type's shown columns.
fn default_width(key: &PROPERTYKEY) -> Option<f32> {
    [(PKEY_ItemNameDisplay, 272.0), (PKEY_DateModified, 144.0), (PKEY_ItemTypeText, 120.0), (PKEY_Size, 80.0)]
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, w)| w)
}

thread_local! {
    static DESCRIPTIONS: RefCell<Vec<Column>> = const { RefCell::new(Vec::new()) };
    /// The open folder's own header texts, when it gives them (the Recycle Bin's).
    static FOLDER_LABELS: RefCell<Vec<(PROPERTYKEY, String)>> = const { RefCell::new(Vec::new()) };
    static FOLDER_RIGHT: RefCell<Vec<PROPERTYKEY>> = const { RefCell::new(Vec::new()) };
}

/// The header texts the open folder gives its columns, over the property system's, and the
/// columns it right-aligns.
pub fn set_folder_labels(labels: Vec<(PROPERTYKEY, String)>, right: Vec<PROPERTYKEY>) {
    FOLDER_LABELS.with(|l| *l.borrow_mut() = labels);
    FOLDER_RIGHT.with(|r| *r.borrow_mut() = right);
}

/// A column's description, read once, with the open folder's header text.
fn describe(key: &PROPERTYKEY) -> Option<Column> {
    let own = FOLDER_LABELS.with(|l| l.borrow().iter().find(|(k, _)| k == key).map(|(_, label)| label.clone()));
    let right = FOLDER_RIGHT.with(|r| r.borrow().contains(key));
    let with_own = |mut c: Column| {
        if let Some(label) = own {
            c.label = label;
        }
        c.right |= right;
        c
    };
    if let Some(hit) = DESCRIPTIONS.with(|d| d.borrow().iter().find(|c| c.key == *key).cloned()) {
        return Some(with_own(hit));
    }
    let column = columns::describe(key)?;
    DESCRIPTIONS.with(|d| d.borrow_mut().push(column.clone()));
    Some(with_own(column))
}

/// A column's header text, as the folder or the property system names it.
pub fn label(key: &PROPERTYKEY) -> Option<String> {
    describe(key).map(|c| c.label)
}

/// The values the listing already has (the rest are read from each item).
fn known(key: &PROPERTYKEY) -> bool {
    [PKEY_ItemNameDisplay, PKEY_DateModified, PKEY_DateCreated, PKEY_ItemTypeText, PKEY_Size].contains(key)
}

/// A shown column: its key, header text, alignment and width.
#[derive(Clone, Debug)]
pub struct Col {
    pub key: PROPERTYKEY,
    pub label: String,
    pub right: bool,
    pub width: f32,
}

/// What the folder's view says about its columns: the shown ones with their widths (empty
/// for its type's defaults) and its type's list.
pub fn initial(saved: &[(PROPERTYKEY, u32)], type_columns: &[(PROPERTYKEY, bool, Option<u32>)]) -> (Vec<(PROPERTYKEY, f32)>, Vec<PROPERTYKEY>) {
    let list: Vec<(PROPERTYKEY, bool, Option<f32>)> = if type_columns.is_empty() {
        GENERIC.iter().map(|(k, shown)| (*k, *shown, None)).collect()
    } else {
        type_columns.iter().map(|(k, shown, chars)| (*k, *shown, chars.map(|c| c as f32 * 8.0))).collect()
    };
    let shown = if saved.is_empty() {
        list.iter()
            .filter(|(_, shown, _)| *shown)
            .map(|(k, _, w)| (*k, w.or_else(|| default_width(k)).or_else(|| describe(k).map(|c| c.width)).unwrap_or(120.0)))
            .collect()
    } else {
        saved.iter().map(|(k, w)| (*k, *w as f32)).collect()
    };
    (shown, list.into_iter().map(|(k, _, _)| k).collect())
}

/// A header cell pressed, and the column drag it becomes past the threshold.
pub struct HeaderDrag {
    column: usize,
    press_x: f32,
    /// Where the mouse was when the drag started; the column follows it from there.
    anchor: Option<f32>,
    /// The columns' widths as they were, and the order they show in now.
    widths: Vec<f32>,
    order: Vec<usize>,
    /// The drag ended (the column dropped); later moves of the same press do nothing.
    dropped: bool,
}

impl HeaderDrag {
    /// Where each column (by its index) starts in the order shown.
    fn lefts(&self) -> Vec<f32> {
        let mut lefts = vec![0.0; self.widths.len()];
        let mut x = 0.0;
        for &i in &self.order {
            lefts[i] = x;
            x += self.widths[i];
        }
        lefts
    }
}

impl App {
    /// Shows these columns, keeping the rows' texts. Name is always among them, and usually
    /// first; the first column, whichever it is, takes the icon.
    pub fn set_columns(&self, shown: Vec<(PROPERTYKEY, f32)>) {
        let mut cols: Vec<Col> = shown
            .into_iter()
            .filter_map(|(key, width)| describe(&key).map(|c| Col { key, label: c.label, right: c.right, width }))
            .collect();
        if !cols.iter().any(|c| c.key == PKEY_ItemNameDisplay) {
            if let Some(c) = describe(&PKEY_ItemNameDisplay) {
                cols.insert(0, Col { key: PKEY_ItemNameDisplay, label: c.label, right: false, width: 272.0 });
            }
        }
        let name_width = cols.first().map(|c| c.width).unwrap_or(272.0);
        self.state.borrow_mut().cols = cols;
        self.ui().global::<Shell>().set_col_name(name_width);
        self.sync_column_heads();
    }

    /// The header's cells after the first, where they show (moved aside during a column
    /// drag) and how wide they are together; the first's label (when it isn't Name) and arrow.
    pub fn sync_column_heads(&self) {
        let lefts = self.header_drag.borrow().as_ref().filter(|d| d.anchor.is_some() && !d.dropped).map(|d| d.lefts());
        let name_width = self.ui().global::<Shell>().get_col_name();
        let heads: Vec<ColumnHead> = {
            let st = self.state.borrow();
            let sorted = |key: &PROPERTYKEY| match super::sort::SortKey::from_property(key) {
                _ if st.search_ranked => 0,
                k if k == st.sort.key => if st.sort.descending { 2 } else { 1 },
                _ => 0,
            };
            let ui = self.ui();
            let shell = ui.global::<Shell>();
            let lead = st.cols.first();
            shell.set_lead_right(lead.is_some_and(|c|c.right));
            let folder_key = |key: &PROPERTYKEY| [PKEY_ItemFolderPathDisplay,PKEY_ItemFolderPathDisplayNarrow].contains(key);
            shell.set_lead_folder(lead.is_some_and(|c|folder_key(&c.key)));
            let name_column=st.cols.iter().position(|c|c.key==PKEY_ItemNameDisplay).unwrap_or(0);
            let name_x=lefts.as_ref().map_or_else(||st.cols.iter().take(name_column).enumerate().map(|(i,c)|if i==0{name_width}else{c.width}).sum(),|xs|xs[name_column]);
            shell.set_name_column(name_column as i32);
            shell.set_name_column_x(name_x);
            shell.set_name_column_width(if name_column==0{name_width}else{st.cols[name_column].width});
            shell.set_lead_label(lead.filter(|c| c.key != PKEY_ItemNameDisplay).map(|c| c.label.as_str().into()).unwrap_or_default());
            shell.set_lead_sorted(lead.map_or(0, |c| sorted(&c.key)));
            shell.set_lead_x(lefts.as_ref().map_or(0.0, |l| l[0]));
            let mut x = 0.0;
            st.cols
                .iter()
                .enumerate()
                .skip(1)
                .map(|(i, c)| {
                    let shown_x = lefts.as_ref().map_or(x, |l| l[i] - name_width);
                    let head = ColumnHead { label: c.label.as_str().into(), x: shown_x, width: c.width, right: c.right, sorted: sorted(&c.key), name: c.key == PKEY_ItemNameDisplay, folder: folder_key(&c.key) };
                    x += c.width;
                    head
                })
                .collect()
        };
        let width: f32 = heads.iter().map(|h| h.width).sum();
        let model = &self.column_heads;
        for (i, head) in heads.iter().enumerate() {
            match model.row_data(i) {
                Some(old) if old == *head => {}
                Some(_) => model.set_row_data(i, head.clone()),
                None => model.push(head.clone()),
            }
        }
        while model.row_count() > heads.len() {
            model.remove(model.row_count() - 1);
        }
        self.ui().global::<Shell>().set_columns_width(width);
    }

    /// The left button on a header cell (`column`, 0 the first): pressed (phase 0), moved (1)
    /// or released (2), at `x` in the header. As in Explorer, moving it more than 4px drags
    /// the column: it follows the mouse, and the column whose slot (as they were, the left edge
    /// out and the right one in) holds the mouse makes room for it, sliding aside. Releasing
    /// it there moves the column.
    pub fn header_pointer(self: &Rc<Self>, column: usize, phase: i32, x: f32) {
        const THRESHOLD: f32 = 4.0;
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        match phase {
            0 => {
                // A header can receive input while an asynchronous namespace load has
                // not supplied columns yet (Network), or after its model changed.
                if column >= self.state.borrow().cols.len() { return; }
                let name_width = shell.get_col_name();
                let widths: Vec<f32> = self.state.borrow().cols.iter().enumerate().map(|(i, c)| if i == 0 { name_width } else { c.width }).collect();
                let order = (0..widths.len()).collect();
                *self.header_drag.borrow_mut() = Some(HeaderDrag { column, press_x: x, anchor: None, widths, order, dropped: false });
            }
            1 => {
                let (ghost, over) = {
                    let mut drag = self.header_drag.borrow_mut();
                    let Some(d) = drag.as_mut().filter(|d| !d.dropped && d.column == column) else { return };
                    let anchor = match d.anchor {
                        Some(a) => a,
                        None if (x - d.press_x).abs() > THRESHOLD => {
                            d.anchor = Some(x);
                            x
                        }
                        None => return,
                    };
                    let mut right = 0.0;
                    let target = d
                        .widths
                        .iter()
                        .position(|w| {
                            right += w;
                            x <= right
                        })
                        .unwrap_or(d.widths.len() - 1);
                    d.order.retain(|&i| i != column);
                    d.order.insert(target, column);
                    let original: f32 = d.widths[..column].iter().sum();
                    // Measured: the ghost looks pressed anywhere to the right, and to the left
                    // while the pointer is no more than about 17px left of where the column
                    // that was beside it shows (Tipo dragged left: pressed at 460, hovered at
                    // 450, with Data de modificação from 475; Tamanho: pressed at 640, hovered
                    // at 600). Further left it looks hovered. These x are 7px past the cells'
                    // drawn edges, hence 24.
                    let neighbour = if column > 0 { d.widths[..column - 1].iter().sum::<f32>() } else { f32::MIN };
                    (original + x - anchor, x >= neighbour - 24.0)
                };
                shell.set_header_drag(column as i32);
                shell.set_header_drag_x(ghost);
                shell.set_header_drag_over(over);
                self.sync_column_heads();
            }
            _ => {
                let order = {
                    let mut drag = self.header_drag.borrow_mut();
                    let Some(d) = drag.as_mut().filter(|d| d.anchor.is_some() && !d.dropped) else {
                        *drag = None;
                        return;
                    };
                    // Kept until the next press, so the click that ends the drag doesn't sort.
                    d.dropped = true;
                    std::mem::take(&mut d.order)
                };
                shell.set_header_drag(-1);
                if order.iter().enumerate().all(|(i, &c)| i == c) {
                    self.sync_column_heads();
                    return;
                }
                let shown = self.shown_columns();
                let shown: Vec<(PROPERTYKEY, f32)> = order.iter().filter_map(|&i| shown.get(i).copied()).collect();
                self.remember_view(|v| v.columns = Some(shown.clone()));
                self.set_columns(shown);
                self.apply_cells(None);
                self.sync_list_window(true);
            }
        }
    }

    /// A click on a header cell sorts, unless it ended a column drag.
    pub fn header_click_sorts(&self) -> bool {
        // Slint may report the click before the release: a started drag is enough.
        !self.header_drag.borrow().as_ref().is_some_and(|d| d.anchor.is_some())
    }

    /// A column after Name was dragged to a new width.
    pub fn column_resized(&self, index: usize, width: f32) {
        {
            let mut st = self.state.borrow_mut();
            let Some(col) = st.cols.get_mut(index + 1) else { return };
            col.width = width;
        }
        // Widths are remembered when the folder is left; this marks the view changed.
        self.remember_view(|_| {});
        self.sync_column_heads();
    }

    /// The shown columns and their widths (Name's from the header), for the folder's
    /// session view.
    pub fn shown_columns(&self) -> Vec<(PROPERTYKEY, f32)> {
        let name = self.ui().global::<Shell>().get_col_name();
        let st = self.state.borrow();
        st.cols.iter().enumerate().map(|(i, c)| (c.key, if i == 0 { name } else { c.width })).collect()
    }

    /// A row's texts for the columns after the first: the listing's own values, the others
    /// as they have been read.
    pub fn cells(st: &State, e: &ShellEntry) -> ModelRc<SharedString> {
        let texts: Vec<SharedString> = st.cols.iter().skip(1).map(|c| Self::cell(st, e, &c.key)).collect();
        ModelRc::new(VecModel::from(texts))
    }

    /// A row's text for the first column when it isn't Name (the row shows its name then).
    pub fn lead(st: &State, e: &ShellEntry) -> SharedString {
        st.cols.first().filter(|c| c.key != PKEY_ItemNameDisplay).map(|c| Self::cell(st, e, &c.key)).unwrap_or_default()
    }

    fn cell(st: &State, e: &ShellEntry, k: &PROPERTYKEY) -> SharedString {
        if *k == PKEY_ItemNameDisplay {
            e.name.as_str().into()
        } else if *k == PKEY_DateModified {
            format::modified(e.modified).into()
        } else if *k == PKEY_DateCreated {
            format::modified(e.created).into()
        } else if *k == PKEY_ItemTypeText {
            e.type_name.as_str().into()
        } else if *k == PKEY_Size {
            if e.is_dir || (!e.is_fs && crate::shell::archive::root(&e.path).is_none()) { SharedString::new() } else { format::size_kb(e.size).into() }
        } else {
            let read = st.col_texts.get(&e.path);
            read.and_then(|r| r.iter().find(|(key, _)| key == k)).map(|(_, t)| t.as_str().into()).unwrap_or_default()
        }
    }

    /// "Dimensionar Coluna para Caber": as Explorer sizes it, the widest of its header (the
    /// label and 28px) and its values (14px around them; the first column's text starts at `name_x` and
    /// ends 8px before the edge), and never under 80px.
    fn fit_width(st: &State, index: usize, name_x: f32) -> Option<f32> {
        let col = st.cols.get(index)?;
        let header = columns::widest([col.label.as_str()]) + 28.0;
        // The first column's texts sit after the icon.
        let texts: Vec<SharedString> = st.order.iter().map(|&i| Self::cell(st, &st.entries[i], &col.key)).collect();
        let widest = columns::widest(texts.iter().map(|t| t.as_str()));
        let values = if index == 0 { widest + name_x + 8.0 } else { widest + 14.0 };
        Some(header.max(values).max(80.0))
    }

    /// Sizes a column (all of them with `None`) to fit, and keeps it for the folder.
    pub fn fit_columns(self: &Rc<Self>, index: Option<usize>) {
        // Where the rows' names start (as in DetailRow).
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let name_x = 38.0 + if shell.get_check_boxes() { 15.0 } else { 0.0 } + if shell.get_compact() { 0.0 } else { 1.0 };
        let widths: Vec<(usize, f32)> = {
            let st = self.state.borrow();
            let indices: Vec<usize> = match index {
                Some(i) => vec![i],
                None => (0..st.cols.len()).collect(),
            };
            indices.into_iter().filter_map(|i| Self::fit_width(&st, i, name_x).map(|w| (i, w))).collect()
        };
        {
            let mut st = self.state.borrow_mut();
            for &(i, w) in &widths {
                st.cols[i].width = w;
            }
        }
        if let Some(&(_, w)) = widths.iter().find(|(i, _)| *i == 0) {
            shell.set_col_name(w);
        }
        self.sync_column_heads();
        let shown = self.shown_columns();
        self.remember_view(|v| v.columns = Some(shown.clone()));
        self.sync_list_window(true);
    }

    /// Reads, off the UI thread, the shown columns' values the listing doesn't have, and puts
    /// them in the rows as they come.
    pub fn load_column_texts(self: &Rc<Self>) {
        let (keys, paths, request, folder, children) = {
            let st = self.state.borrow();
            let keys: Vec<PROPERTYKEY> = st.cols.iter().map(|c| c.key).filter(|k| !known(k)).collect();
            if keys.is_empty() {
                return;
            }
            let has_all = |p: &String| st.col_texts.get(p).is_some_and(|r| keys.iter().all(|k| r.iter().any(|(key, _)| key == k)));
            let paths: Vec<String> = st.order.iter().map(|&i| st.entries[i].path.clone()).filter(|p| !has_all(p)).collect();
            let children = st.entries.iter().filter(|e| !e.child.is_empty()).map(|e| (e.path.clone(), e.child.clone())).collect();
            (keys, paths, st.request, st.path.clone(), children)
        };
        if paths.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            let items = columns::Items::new(&folder, children);
            let mut batch = Vec::new();
            for (n, path) in paths.into_iter().enumerate() {
                let texts = columns::texts(&items, &path, &keys);
                batch.push((path, keys.iter().copied().zip(texts).collect::<Vec<_>>()));
                if n % 32 == 31 {
                    let ready = std::mem::take(&mut batch);
                    post(move |app| app.column_texts_ready(request, ready));
                }
            }
            if !batch.is_empty() {
                post(move |app| app.column_texts_ready(request, batch));
            }
        });
    }

    fn column_texts_ready(&self, request: u64, ready: Vec<(String, Vec<(PROPERTYKEY, String)>)>) {
        let changed: HashSet<String> = {
            let mut st = self.state.borrow_mut();
            if st.request != request {
                return;
            }
            for (path, texts) in &ready {
                let row = st.col_texts.entry(path.clone()).or_default();
                for (key, text) in texts {
                    match row.iter_mut().find(|(k, _)| k == key) {
                        Some(slot) => slot.1 = text.clone(),
                        None => row.push((*key, text.clone())),
                    }
                }
            }
            ready.into_iter().map(|(p, _)| p).collect()
        };
        self.apply_cells(Some(&changed));
    }

    /// Sets the rows' column texts: all rows, or those of `only`.
    pub fn apply_cells(&self, only: Option<&HashSet<String>>) {
        let st = self.state.borrow();
        for (row, &i) in st.order.iter().enumerate() {
            let e = &st.entries[i];
            if only.is_some_and(|set| !set.contains(&e.path)) {
                continue;
            }
            let Some(mut data) = self.entries.row_data(row) else { continue };
            data.cells = Self::cells(&st, e);
            data.lead = Self::lead(&st, e);
            self.entries.set_row_data(row, data);
        }
    }

    /// Shows or hides a column from the header's menu.
    pub fn toggle_column(self: &Rc<Self>, key: PROPERTYKEY) {
        let mut shown = self.shown_columns();
        match shown.iter().position(|(k, _)| *k == key) {
            Some(i) => {
                shown.remove(i);
            }
            None => {
                let width = default_width(&key).or_else(|| describe(&key).map(|c| c.width)).unwrap_or(120.0);
                shown.push((key, width));
            }
        }
        self.remember_view(|v| v.columns = Some(shown.clone()));
        self.set_columns(shown);
        self.apply_cells(None);
        self.load_column_texts();
        self.sync_list_window(true);
    }

    /// "Mais...": the Choose Details dialog; on OK, its columns in its order, the new ones at
    /// their default width.
    pub fn choose_details(self: &Rc<Self>) {
        let Some(chosen) = super::choose::choose(self.hwnd.get(), &self.shown_columns()) else { return };
        let shown: Vec<(PROPERTYKEY, f32)> = chosen
            .into_iter()
            .map(|(key, width)| {
                let width = width.map(|w| w as f32).or_else(|| default_width(&key)).or_else(|| describe(&key).map(|c| c.width)).unwrap_or(120.0);
                (key, width)
            })
            .collect();
        self.remember_view(|v| v.columns = Some(shown.clone()));
        self.set_columns(shown);
        self.apply_cells(None);
        self.load_column_texts();
        self.sync_list_window(true);
    }

    /// Ctrl with the keypad's +: in the details view, sizes every column to fit.
    pub fn fit_all_key(self: &Rc<Self>) {
        if self.ui().global::<Shell>().get_view_mode() == super::VIEW_DETAILS {
            self.fit_columns(None);
        }
    }

    /// The header's menu (Explorer's classic one): sizing the column clicked (`column`, -1
    /// past the last, where the item isn't there) or all to fit, then the folder type's columns and any other shown one,
    /// checked when shown (Name can't be hidden), then "Mais...".
    pub fn header_menu(self: &Rc<Self>, column: i32, x: f32, y: f32) {
        use windows::Win32::UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, TrackPopupMenu, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON,
        };
        let entries: Vec<(PROPERTYKEY, String, bool)> = {
            let st = self.state.borrow();
            let mut keys = st.menu_columns.clone();
            for c in &st.cols {
                if !keys.contains(&c.key) {
                    keys.push(c.key);
                }
            }
            keys.iter()
                .filter_map(|k| describe(k).map(|c| (*k, c.label, st.cols.iter().any(|s| s.key == *k))))
                .collect()
        };
        let text = |s: &str| crate::shell::wide(s);
        let fit = text(&crate::i18n::system_string("shell32.dll", 37200));
        let fit_all = text(&crate::i18n::system_string("shell32.dll", 37201));
        let more = text(&crate::i18n::system_string("shell32.dll", 4249));
        let labels: Vec<Vec<u16>> = entries.iter().map(|(_, label, _)| text(label)).collect();
        let hwnd = self.hwnd.get();
        let scale = self.ui().window().scale_factor();
        let mut at = windows::Win32::Foundation::POINT { x: (x * scale) as i32, y: (y * scale) as i32 };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at);
        }
        let picked = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            // Past the last column there's no column to size.
            if column >= 0 {
                let _ = AppendMenuW(menu, MF_STRING, 1, windows::core::PCWSTR(fit.as_ptr()));
            }
            let _ = AppendMenuW(menu, MF_STRING, 2, windows::core::PCWSTR(fit_all.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            for (i, (key, _, shown)) in entries.iter().enumerate() {
                let mut flags = MF_STRING;
                if *shown {
                    flags |= MF_CHECKED;
                }
                if *key == PKEY_ItemNameDisplay {
                    flags |= MF_GRAYED;
                }
                let _ = AppendMenuW(menu, flags, 100 + i, windows::core::PCWSTR(labels[i].as_ptr()));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, 3, windows::core::PCWSTR(more.as_ptr()));
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                at.x,
                at.y,
                Some(0),
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            cmd.0 as usize
        };
        match picked {
            1 => self.fit_columns(Some(column as usize)),
            2 => self.fit_columns(None),
            3 => self.choose_details(),
            _ => {}
        }
        if picked >= 100 {
            if let Some((key, _, _)) = entries.get(picked - 100) {
                self.toggle_column(*key);
            }
        }
    }
}

/// Columns' texts read so far, by item path.
pub type ColumnTexts = HashMap<String, Vec<(PROPERTYKEY, String)>>;
