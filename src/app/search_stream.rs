//! Live shell search batches. Requests cancel their predecessors; sorted row models are
//! updated in place, retaining selection, cached fields, scroll and keyboard focus.
use super::*;
use std::sync::atomic::Ordering;

impl App {
    pub(super) fn start_search_stream(
        self: &Rc<Self>,
        request: u64,
        path: String,
        with_crumbs: bool,
    ) {
        self.state.borrow_mut().search_name_hits.clear();
        self.search_pulse_at.set(None);
        self.ui().global::<Shell>().set_search_busy(true);
        self.ui().global::<Shell>().set_search_indicator(false);
        let weak = Rc::downgrade(self);
        slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
            if let Some(app) = weak.upgrade() {
                let valid = app.search_generation.load(Ordering::Acquire) == request;
                if valid && app.ui().global::<Shell>().get_search_busy() {
                    app.ui().global::<Shell>().set_search_indicator(true);
                }
            }
        });
        let options = self.state.borrow().search_options;
        let generation = self.search_generation.clone();
        std::thread::spawn(move || {
            crate::shell::com_init();
            let Some((scope, query)) = ids::search_parts(&path) else {
                return;
            };
            let crumbs = if with_crumbs {
                listing::crumbs(scope).ok().map(|mut chain| {
                    let name = chain.last().map(|c| c.name.clone()).unwrap_or_default();
                    chain.push(Crumb {
                        nav_root: None,
                        id: path.clone(),
                        name: crate::i18n::shell_string(34132, &name),
                        icon: crate::shell::icons::find_icon(),
                        has_subfolders: false,
                    });
                    chain
                })
            } else {
                None
            };
            let view = with_crumbs.then(|| crate::shell::viewstate::read_search(scope));
            let initial_path = path.clone();
            let mut seen_icons = HashSet::new();
            let icons = crumbs
                .iter()
                .flatten()
                .filter_map(|c| {
                    seen_icons
                        .insert(c.icon)
                        .then(|| {
                            crate::shell::icons::icon(c.icon, 16)
                                .ok()
                                .map(|pixels| (c.icon, pixels))
                        })
                        .flatten()
                })
                .collect();
            post(move |app| {
                if !app.search_request_current(request, &initial_path) {
                    return;
                }
                app.listed(request, initial_path, Ok(Vec::new()), crumbs, icons, view);
                app.state.borrow_mut().loaded = false;
                app.sync_view_state();
            });
            let result = crate::shell::search::stream(
                scope,
                query,
                options,
                || generation.load(Ordering::Acquire) == request,
                |batch| {
                    let hits = batch
                        .iter()
                        .map(|entry| Self::name_hit(&entry.name, query))
                        .collect::<Vec<_>>();
                    let icons = batch
                        .iter()
                        .filter_map(|entry| {
                            seen_icons
                                .insert(entry.icon)
                                .then(|| {
                                    crate::shell::icons::icon(entry.icon, 16)
                                        .ok()
                                        .map(|pixels| (entry.icon, pixels))
                                })
                                .flatten()
                        })
                        .collect::<Vec<_>>();
                    let path = path.clone();
                    post(move |app| app.search_batch(request, &path, batch, icons, hits));
                },
            );
            post(move |app| {
                if !app.search_request_current(request, &path) {
                    return;
                }
                {
                    let mut st = app.state.borrow_mut();
                    st.loaded = true;
                    if let Err(error) = result {
                        st.error = error;
                    }
                }
                app.ui().global::<Shell>().set_search_busy(false);
                app.ui().global::<Shell>().set_search_indicator(false);
                app.sync_view_state();
            });
        });
    }

    fn search_request_current(&self, request: u64, path: &str) -> bool {
        let st = self.state.borrow();
        st.request == request
            && st.path == path
            && self.search_generation.load(Ordering::Acquire) == request
    }

    fn search_batch(
        self: &Rc<Self>,
        request: u64,
        path: &str,
        batch: Vec<ShellEntry>,
        icons: Vec<(u32, Rgba)>,
        hits: Vec<crate::Hit>,
    ) {
        if !self.search_request_current(request, path) {
            return;
        }
        let start = Instant::now();
        self.add_icons(&icons);
        let (old_order, first) = {
            let mut st = self.state.borrow_mut();
            let first = st.entries.is_empty();
            let old = st.order.clone();
            for (entry, hit) in batch.iter().zip(hits) {
                st.search_name_hits.insert(entry.path.clone(), hit);
            }
            st.entries.extend(batch);
            (old, first)
        };
        self.rebuild_order();
        let order_done = start.elapsed();
        self.load_sort_texts();
        let sort_done = start.elapsed();
        self.sync_search_rows(&old_order);
        let rows_done = start.elapsed();
        self.sync_view_state();
        if first {
            self.start_search_pulse(request);
        }
        if std::env::var("DEBUG_TIMING").is_ok() {
            eprintln!("search batch {} items, UI {:?}: order {:?}, sort props {:?}, rows {:?}, state {:?}",self.state.borrow().entries.len(),start.elapsed(),order_done,sort_done-order_done,rows_done-sort_done,start.elapsed()-rows_done);
        }
    }

    pub(super) fn sync_search_rows(self: &Rc<Self>, old_order: &[usize]) {
        let start = Instant::now();
        // Stable entry indices let moved rows keep their already-loaded models/images.
        let mut cached = old_order
            .iter()
            .enumerate()
            .filter_map(|(row, &id)| self.entries.row_data(row).map(|data| (id, data)))
            .collect::<HashMap<_, _>>();
        let mut added = Vec::new();
        {
            let st = self.state.borrow();
            for (row, &id) in st.order.iter().enumerate() {
                if old_order.get(row) == Some(&id) {
                    continue;
                }
                let mut data = cached
                    .remove(&id)
                    .unwrap_or_else(|| self.entry_row(&st, &st.entries[id], row as i32));
                data.header = st.headers.get(row).cloned().unwrap_or_default().into();
                data.folded = st.folded.get(row).copied().unwrap_or(false);
                data.collapsed = data.folded;
                if row < self.entries.row_count() {
                    self.entries.set_row_data(row, data);
                } else {
                    added.push(data);
                }
            }
        }
        self.entries.extend(added);
        let models_done = start.elapsed();
        {
            let mut st = self.state.borrow_mut();
            st.row_marks = st
                .order
                .iter()
                .map(|&id| {
                    let e = &st.entries[id];
                    (
                        st.selected.contains(&e.path),
                        e.hidden || st.cut.contains(&e.path),
                    )
                })
                .collect();
        }
        let ui = self.ui();
        {
            let st = self.state.borrow();
            let shell = ui.global::<Shell>();
            shell.set_grouped(!st.headers.is_empty());
            shell.set_list_extent(st.list_extent);
        }
        self.sync_list_window(true);
        if Self::details_list(&self.state.borrow()) {
            self.load_column_texts();
        }
        let columns_done = start.elapsed();
        if Self::icons_spec(&self.state.borrow()).is_some() {
            self.relayout();
            let layout_done = start.elapsed();
            self.load_thumbnails();
            let thumbs_done = start.elapsed();
            self.load_fields();
            if std::env::var("DEBUG_TIMING").is_ok() {
                eprintln!(
                    "search rows: models {:?}, columns {:?}, layout {:?}, thumbs {:?}, fields {:?}",
                    models_done,
                    columns_done - models_done,
                    layout_done - columns_done,
                    thumbs_done - layout_done,
                    start.elapsed() - thumbs_done
                );
            }
        }
    }

    pub(super) fn start_search_pulse(self: &Rc<Self>, request: u64) {
        self.search_pulse_at.set(Some(Instant::now()));
        self.ui().global::<Shell>().set_search_hit_stage(0);
        self.arm_search_pulse(request, std::time::Duration::ZERO);
    }

    fn arm_search_pulse(self: &Rc<Self>, request: u64, elapsed: std::time::Duration) {
        for (stage, delay) in [(1, 1968), (2, 2032)] {
            let weak = Rc::downgrade(self);
            slint::Timer::single_shot(
                std::time::Duration::from_millis(delay).saturating_sub(elapsed),
                move || {
                    if let Some(app) = weak.upgrade() {
                        let valid = {
                            let st = app.state.borrow();
                            st.request == request && ids::is_search(&st.path)
                        };
                        if valid {
                            app.ui().global::<Shell>().set_search_hit_stage(stage);
                        }
                    }
                },
            );
        }
    }

    pub(super) fn cancel_search(self: &Rc<Self>) -> bool {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        if !shell.get_search_busy()
            || shell.get_menu_open()
            || self.state.borrow().renaming.is_some()
        {
            return false;
        }
        self.search_timer.stop();
        let request = {
            let mut st = self.state.borrow_mut();
            st.request += 1;
            st.loaded = true;
            st.request
        };
        self.search_generation.store(request, Ordering::Release);
        self.pending_fields.borrow_mut().clear();
        shell.set_search_busy(false);
        shell.set_search_indicator(false);
        if shell.get_search_hit_stage() < 2 {
            if let Some(start) = self.search_pulse_at.get() {
                self.arm_search_pulse(request, start.elapsed());
            }
        }
        self.sync_view_state();
        self.load_fields();
        true
    }
}
