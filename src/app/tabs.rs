//! Tab order changes keep the current folder/listing alive.
use super::{App, State, Tab};

impl App {
    pub fn reorder_tab(&self, from: usize, to: usize) {
        let changed = {
            let mut state = self.state.borrow_mut();
            let State { tabs, current, .. } = &mut *state;
            reorder(tabs, current, from, to)
        };
        if changed {
            self.sync_tabs();
        }
    }
}

fn reorder(tabs: &mut Vec<Tab>, current: &mut usize, from: usize, to: usize) -> bool {
    if from >= tabs.len() || to >= tabs.len() || from == to {
        return false;
    }
    let tab = tabs.remove(from);
    tabs.insert(to, tab);
    *current = if *current == from { to }
        else if from < *current && *current <= to { *current - 1 }
        else if to <= *current && *current < from { *current + 1 }
        else { *current };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{filter::Filter, sort::SortKey};

    fn tabs() -> Vec<Tab> {
        (0..4).map(|i| Tab {
            title: i.to_string(),
            icon: i,
            selection: super::super::TabSelection {
                path: format!("folder-{i}-current"),
                selected: [format!("selected-{i}")].into(),
                ..Default::default()
            },
            history: vec![format!("folder-{i}-before"), format!("folder-{i}-current")],
            filters: vec![None, Some(Filter { key: SortKey::Name, labels: vec![i.to_string()] })],
            libraries: vec![None, None],
            index: 1,
        }).collect()
    }

    #[test]
    fn every_move_preserves_active_tab_history_and_other_tabs_order() {
        for from in 0..4 {
            for to in 0..4 {
                for active in 0..4 {
                    let mut tabs = tabs();
                    let mut current = active;
                    reorder(&mut tabs, &mut current, from, to);
                    assert_eq!(tabs[current].title, active.to_string());
                    assert_eq!(tabs[to].title, from.to_string());
                    let others: Vec<_> = tabs.iter().filter(|t| t.icon != from as u32).map(|t| t.icon).collect();
                    assert_eq!(others, (0..4).filter(|&i| i != from as u32).collect::<Vec<_>>());
                    for tab in &tabs {
                        assert_eq!(tab.path(), format!("folder-{}-current", tab.icon));
                        assert_eq!(tab.history[0], format!("folder-{}-before", tab.icon));
                        assert_eq!(tab.filters.len(), 2);
                        assert_eq!(tab.filter().unwrap().labels, vec![tab.title.clone()]);
                        assert_eq!(tab.index, 1);
                        assert!(tab.selection.selected.contains(&format!("selected-{}", tab.icon)));
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_or_unchanged_drop_does_nothing() {
        let mut tabs = tabs();
        let mut current = 2;
        for (from, to) in [(4, 0), (0, 4), (usize::MAX, 1), (2, 2)] {
            assert!(!reorder(&mut tabs, &mut current, from, to));
            assert_eq!(current, 2);
            assert_eq!(tabs.iter().map(|t| t.icon).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        }
    }
}
