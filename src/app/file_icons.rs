use super::*;
use crate::shell::icons;
use std::sync::atomic::Ordering;
use std::time::Duration;

const BATCH_INTERVAL: Duration = Duration::from_millis(100);

impl App {
    pub(super) fn apply_known_icons(&self, entries: &mut [ShellEntry]) {
        let known = self.known_icons.borrow();
        for e in entries.iter_mut().filter(|e| e.icon_pending) {
            if let Some(&(modified, icon)) = known.get(&e.path) {
                if modified == e.modified {
                    e.icon = icon;
                    e.icon_pending = false;
                }
            }
        }
    }

    pub(super) fn resolve_icons(self: &Rc<Self>) {
        let (request, wanted) = {
            let st = self.state.borrow();
            let (a, b) = st.list_window;
            let shown: HashSet<usize> = (a..b).chain(self.grid_slots.iter().map(|r| r as usize)).collect();
            let mut pending = self.pending_icons.borrow_mut();
            let mut wanted: Vec<(bool, String)> = st
                .order
                .iter()
                .enumerate()
                .filter(|&(_, &i)| st.entries[i].icon_pending && pending.insert(st.entries[i].path.clone()))
                .map(|(row, &i)| (!shown.contains(&row), st.entries[i].path.clone()))
                .collect();
            wanted.sort_by_key(|&(later, _)| later);
            (st.request, wanted)
        };
        if wanted.is_empty() {
            return;
        }
        let generation = self.search_generation.clone();
        std::thread::spawn(move || {
            crate::shell::com_init();
            let mut batch = Vec::new();
            let mut sent = Instant::now();
            for (_, path) in wanted {
                if generation.load(Ordering::Acquire) != request {
                    return;
                }
                let icon = icons::file_icon(&path);
                let rgba = icons::icon(icon, 16).ok();
                batch.push((path, icon, rgba));
                if sent.elapsed() >= BATCH_INTERVAL {
                    let ready = std::mem::take(&mut batch);
                    post(move |app| app.icons_resolved(request, ready));
                    sent = Instant::now();
                }
            }
            if !batch.is_empty() {
                post(move |app| app.icons_resolved(request, batch));
            }
        });
    }

    fn icons_resolved(&self, request: u64, ready: Vec<(String, u32, Option<Rgba>)>) {
        {
            let mut pending = self.pending_icons.borrow_mut();
            for (path, _, _) in &ready {
                pending.remove(path);
            }
        }
        if self.state.borrow().request != request {
            return;
        }
        {
            let mut images = self.images.borrow_mut();
            for (_, icon, rgba) in &ready {
                if let Some(rgba) = rgba {
                    images.insert(*icon, 16, rgba);
                }
            }
        }
        let resolved: HashMap<&str, u32> = ready.iter().filter(|(_, icon, _)| *icon != 0).map(|(path, icon, _)| (path.as_str(), *icon)).collect();
        let changed: Vec<(usize, u32)> = {
            let mut st = self.state.borrow_mut();
            let mut known = self.known_icons.borrow_mut();
            let mut updated = HashSet::new();
            for (i, e) in st.entries.iter_mut().enumerate() {
                let Some(&icon) = resolved.get(e.path.as_str()).filter(|_| e.icon_pending) else { continue };
                e.icon_pending = false;
                known.insert(e.path.clone(), (e.modified, icon));
                if e.icon != icon {
                    e.icon = icon;
                    updated.insert(i);
                }
            }
            st.order.iter().enumerate().filter(|(_, i)| updated.contains(i)).map(|(row, &i)| (row, st.entries[i].icon)).collect()
        };
        let mut images = self.images.borrow_mut();
        for (row, icon) in changed {
            if let Some(mut data) = self.entries.row_data(row) {
                data.icon = images.icon(icon, 16);
                self.entries.set_row_data(row, data);
            }
        }
    }
}
