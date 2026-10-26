//! Native network enumeration can wait between Next calls. Publish its address/view
//! and first devices immediately, retaining selections across shell notifications.
use super::*;
use std::sync::atomic::Ordering;
impl App {
    pub(super) fn start_network(self: &Rc<Self>, request: u64, path: String, initial: bool) {
        let shell = self.ui();
        let shell = shell.global::<Shell>();
        shell.set_search_busy(true);
        shell.set_search_indicator(false);
        let weak = Rc::downgrade(self);
        slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
            if let Some(app) = weak.upgrade() {
                if app.network_current(request) && app.ui().global::<Shell>().get_search_busy() {
                    app.ui().global::<Shell>().set_search_indicator(true);
                }
            }
        });
        let generation = self.search_generation.clone();
        std::thread::spawn(move || {
            crate::shell::com_init();
            let started = Instant::now();
            let crumbs = initial.then(|| listing::crumbs(&path).ok()).flatten();
            let view = initial.then(|| crate::shell::viewstate::read(&path));
            let mut seen_icons = HashSet::new();
            let icons = crumbs
                .iter()
                .flatten()
                .filter_map(|c| {
                    if seen_icons.insert(c.icon) {
                        crate::shell::icons::icon(c.icon, 16)
                            .ok()
                            .map(|i| (c.icon, i))
                    } else {
                        None
                    }
                })
                .collect();
            let mut resource = if ids::same(&path, ids::NETWORK) {
                crate::shell::network::status().ok().and_then(|s| {
                    if std::env::var_os("DEBUG_TIMING").is_some() {
                        eprintln!(
                            "network profile {} discovery={} sharing={}",
                            s.profiles, s.discovery, s.sharing
                        );
                    }
                    s.resource()
                })
            } else {
                None
            };
            // An app-only visual preview never changes or mocks the system policy.
            if cfg!(debug_assertions) && ids::same(&path, ids::NETWORK) {
                resource = match std::env::var("SLINT_NETWORK_NOTICE_PREVIEW").as_deref() {
                    Ok("both-off") => Some(1036),
                    Ok("discovery-off") => Some(1047),
                    Ok("sharing-off") => Some(1048),
                    _ => resource,
                };
            }
            let text = resource
                .map(|id| crate::i18n::system_string("NetworkExplorer.dll", id))
                .unwrap_or_default();
            let first_path = path.clone();
            post(move |app| {
                if !app.network_current(request) {
                    return;
                }
                if initial {
                    let pending = {
                        let mut st = app.state.borrow_mut();
                        (
                            st.selected.clone(),
                            st.anchor.clone(),
                            st.focused.clone(),
                            st.came_from.take(),
                            st.after_load.take(),
                        )
                    };
                    app.listed(request, first_path, Ok(Vec::new()), crumbs, icons, view);
                    let mut st = app.state.borrow_mut();
                    (
                        st.selected,
                        st.anchor,
                        st.focused,
                        st.came_from,
                        st.after_load,
                    ) = pending;
                    st.loaded = false;
                }
                app.ui().global::<Shell>().set_network_notice(text.into());
                app.sync_view_state();
            });
            let mut seen = HashSet::new();
            let result = crate::shell::Pidl::parse(&path).and_then(|parent| {
                listing::for_each_pidl(&parent, |entry| {
                    if generation.load(Ordering::Acquire) != request {
                        return false;
                    }
                    seen.insert(entry.path.clone());
                    let pane = listing::prop_list_with_id(
                        &entry.path,
                        listing::PropList::Pane,
                        &entry.absolute,
                    );
                    let properties = if entry.absolute.is_empty() {
                        true
                    } else {
                        unsafe {
                            use windows::Win32::System::SystemServices::SFGAO_HASPROPSHEET;
                            use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromIDList};
                            SHCreateItemFromIDList::<IShellItem>(entry.absolute.as_ptr().cast())
                                .and_then(|item| item.GetAttributes(SFGAO_HASPROPSHEET))
                                .map(|flags| flags.0 & SFGAO_HASPROPSHEET.0 != 0)
                                .unwrap_or(false)
                        }
                    };
                    let icons = if seen_icons.insert(entry.icon) {
                        crate::shell::icons::icon(entry.icon, 16)
                            .ok()
                            .map(|i| (entry.icon, i))
                            .into_iter()
                            .collect()
                    } else {
                        Vec::new()
                    };
                    post(move |app| app.network_entry(request, entry, icons, pane, properties));
                    true
                })
            });
            if std::env::var_os("DEBUG_TIMING").is_some() {
                eprintln!(
                    "network enumeration {:?}, {} items",
                    started.elapsed(),
                    seen.len()
                );
            }
            post(move |app| {
                if !app.network_current(request) {
                    return;
                }
                {
                    let mut st = app.state.borrow_mut();
                    st.loaded = true;
                    st.came_from = None;
                    if let Err(error) = result {
                        st.error = error;
                    } else {
                        st.entries.retain(|e| seen.contains(&e.path));
                        st.selected.retain(|p| seen.contains(p));
                        if st.focused.as_ref().is_some_and(|p| !seen.contains(p)) {
                            st.focused = None;
                        }
                    }
                }
                let shell = app.ui();
                let shell = shell.global::<Shell>();
                shell.set_search_busy(false);
                shell.set_search_indicator(false);
                app.rebuild_order();
                app.sync_all();
                app.network_focus();
            });
        });
    }
    fn network_current(&self, request: u64) -> bool {
        self.state.borrow().request == request
            && self.search_generation.load(Ordering::Acquire) == request
    }
    fn network_focus(&self) {
        let ui = self.ui();
        let s = ui.global::<Shell>();
        if s.get_focus_zone() == "list" && !s.get_menu_open() {
            ui.invoke_focus_list();
        }
    }
    fn network_entry(
        self: &Rc<Self>,
        request: u64,
        entry: ShellEntry,
        icons: Vec<(u32, Rgba)>,
        pane: Vec<Option<(String, String)>>,
        properties: bool,
    ) {
        if !self.network_current(request) {
            return;
        }
        self.add_icons(&icons);
        let (old, updated) = {
            let mut st = self.state.borrow_mut();
            let old = st.order.clone();
            st.fields.insert(
                (listing::PropList::Pane, entry.path.clone()),
                pane.into_iter()
                    .map(|line| line.map(|(label, value)| (label, value, 0.0)))
                    .collect(),
            );
            st.network_properties.insert(entry.path.clone(), properties);
            let returning = st.after_load.is_none()
                && st.came_from.as_ref().is_some_and(|from| {
                    let from = from.to_lowercase();
                    let path = entry.path.to_lowercase();
                    from == path
                        || (from.starts_with(&path) && from[path.len()..].starts_with('\\'))
                });
            let after = st
                .after_load
                .as_ref()
                .is_some_and(|a| ids::same(&a.select, &entry.path));
            if returning || after {
                st.selected = HashSet::from([entry.path.clone()]);
                st.anchor = Some(entry.path.clone());
                st.focused = Some(entry.path.clone());
                st.came_from = None;
                if let Some(after) = st.after_load.take() {
                    if after.rename {
                        st.renaming = Some(entry.path.clone());
                    }
                }
            }
            if st.focused.is_none() && st.entries.is_empty() {
                st.focused = Some(entry.path.clone());
            }
            let updated = if let Some(index) = st
                .entries
                .iter()
                .position(|e| ids::same(&e.path, &entry.path))
            {
                st.entries[index] = entry;
                Some(index)
            } else {
                st.entries.push(entry);
                None
            };
            st.loaded = true;
            st.error.clear();
            (old, updated)
        };
        self.rebuild_order();
        if let Some(index) = updated {
            let st = self.state.borrow();
            if let Some(row) = st.order.iter().position(|&i| i == index) {
                self.entries
                    .set_row_data(row, self.entry_row(&st, &st.entries[index], row as i32));
            }
        }
        self.load_sort_texts();
        self.sync_search_rows(&old);
        self.sync_view_state();
        self.network_focus();
    }
}
