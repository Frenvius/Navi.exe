//! The details header's filter (a column's chevron): the column's groups, as "Agrupar por"
//! makes them, found among the folder's items, each with a check box. As in Explorer, every
//! change of the filter is a step of the tab's history: Back shows the folder as it was.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::sort::SortKey;
use super::{group, App, State};
use crate::shell::columns::SortValue;
use crate::shell::listing::ShellEntry;
use crate::{FilterRow, Shell};

#[derive(Clone, PartialEq, Debug)]
pub struct Filter {
    pub key: SortKey,
    /// The groups shown, by label.
    pub labels: Vec<String>,
}

/// The open filter: its column's key and its values, in order.
pub struct FilterMenu {
    pub key: SortKey,
    pub labels: Vec<String>,
}

/// An entry's group for a key, with the values read for keys that aren't in the listing.
pub fn bucket(st: &State, key: SortKey, e: &ShellEntry, today: &group::Today) -> group::Bucket {
    let read = if st.sort_texts_key == Some(key) {
        st.sort_texts.get(&e.path).cloned()
    } else if st.group_texts_key == Some(key) {
        st.group_texts.get(&e.path).cloned()
    } else {
        // A shown column's text.
        key.property().and_then(|k| {
            let texts = st.col_texts.get(&e.path)?;
            let (_, text) = texts.iter().find(|(c, _)| *c == k)?;
            Some(SortValue { text: text.clone(), number: None, date: false })
        })
    };
    group::bucket(key, e, today, read.as_ref())
}

fn row(label: &str, checked: bool) -> FilterRow {
    FilterRow { label: label.into(), checked, width: crate::shell::text::ui_width(label) }
}

/// Whether an entry shows under the filter.
pub fn passes(st: &State, filter: &Filter, e: &ShellEntry, today: &group::Today) -> bool {
    filter.labels.contains(&bucket(st, filter.key, e, today).label)
}

impl App {
    /// The chevron of a column was clicked: lists the column's groups under it, at x, y.
    pub fn open_filter(self: &Rc<Self>, column: usize, x: f32, y: f32) {
        let (key, labels, checked) = {
            let st = self.state.borrow();
            let Some(col) = st.cols.get(column) else { return };
            let key = SortKey::from_property(&col.key);
            let today = group::Today::now();
            let mut buckets: Vec<group::Bucket> = Vec::new();
            for e in &st.entries {
                let b = bucket(&st, key, e, &today);
                if !buckets.contains(&b) {
                    buckets.push(b);
                }
            }
            // In the grouping's order for the column (newest and largest first).
            let descending = group::Group::new(key).descending;
            buckets.sort_by(|a, b| if descending { b.cmp(a) } else { a.cmp(b) });
            let labels: Vec<String> = buckets.into_iter().map(|b| b.label).collect();
            let shown = st.filter.as_ref().filter(|f| f.key == key);
            let checked: Vec<bool> = labels.iter().map(|l| shown.is_some_and(|f| f.labels.contains(l))).collect();
            (key, labels, checked)
        };
        let icon = self.images.borrow_mut().icon(crate::shell::icons::stack_icon(), 16);
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let rows: Vec<FilterRow> = labels.iter().zip(&checked).map(|(l, &c)| row(l, c)).collect();
        shell.set_filter_rows(ModelRc::new(VecModel::from(rows)));
        shell.set_filter_icon(icon);
        shell.set_filter_x(x);
        shell.set_filter_y(y);
        shell.set_filter_hot(0);
        shell.set_filter_column(column as i32);
        shell.set_filter_framed(true);
        self.state.borrow_mut().filter_menu = Some(FilterMenu { key, labels });
    }

    pub fn close_filter(&self) {
        self.state.borrow_mut().filter_menu = None;
        self.ui().global::<Shell>().set_filter_column(-1);
    }

    /// A value's check box: adds it to the filter or takes it out; the filter stays open.
    pub fn filter_toggle(self: &Rc<Self>, index: usize) {
        let next = {
            let st = self.state.borrow();
            let Some(menu) = st.filter_menu.as_ref() else { return };
            let Some(label) = menu.labels.get(index) else { return };
            let mut labels = st.filter.as_ref().filter(|f| f.key == menu.key).map(|f| f.labels.clone()).unwrap_or_default();
            match labels.iter().position(|l| l == label) {
                Some(at) => {
                    labels.remove(at);
                }
                None => labels.push(label.clone()),
            }
            (!labels.is_empty()).then(|| Filter { key: menu.key, labels })
        };
        self.set_filter(next.clone());
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let rows: Vec<FilterRow> = {
            let st = self.state.borrow();
            let Some(menu) = st.filter_menu.as_ref() else { return };
            menu.labels
                .iter()
                .map(|l| row(l, next.as_ref().is_some_and(|f| f.labels.contains(l))))
                .collect()
        };
        shell.set_filter_rows(ModelRc::new(VecModel::from(rows)));
        shell.set_filter_framed(false);
    }

    /// A value's label: the folder filtered by it alone; the filter closes.
    pub fn filter_only(self: &Rc<Self>, index: usize) {
        let next = {
            let st = self.state.borrow();
            let Some(menu) = st.filter_menu.as_ref() else { return };
            let Some(label) = menu.labels.get(index) else { return };
            Filter { key: menu.key, labels: vec![label.clone()] }
        };
        self.close_filter();
        self.set_filter(Some(next));
    }

    /// Shows the folder under another filter, as a new step of the tab's history.
    fn set_filter(self: &Rc<Self>, filter: Option<Filter>) {
        {
            let mut st = self.state.borrow_mut();
            if st.filter == filter {
                return;
            }
            let cur = st.current;
            let tab = &mut st.tabs[cur];
            let path = tab.path().to_string();
            tab.push(path, filter);
        }
        self.open_current();
    }
}
