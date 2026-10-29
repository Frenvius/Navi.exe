use super::Tab;

#[derive(Clone)]
pub struct Location {
    pub tab: Tab,
    pub visits: Vec<super::selection::Selection>,
    pub views: std::collections::HashMap<String, super::view::ResolvedView>,
    pub scroll: (f32, f32),
    pub tree_pick: Option<super::navtree::Pick>,
    pub tree_top: usize,
    pub group_focus: Option<String>,
}

impl Location {
    pub fn new(path: &str) -> Self {
        Self {
            tab: Tab::new(path),
            visits: vec![Default::default()],
            views: Default::default(),
            scroll: (0.0, 0.0),
            tree_pick: None,
            tree_top: 0,
            group_focus: None,
        }
    }
}

pub struct Collection<T> {
    pub entries: Vec<T>,
    pub current: usize,
}

impl<T> Collection<T> {
    pub fn new(first: T) -> Self {
        Self {
            entries: vec![first],
            current: 0,
        }
    }
    pub fn active(&self) -> &T {
        &self.entries[self.current]
    }
    pub fn active_mut(&mut self) -> &mut T {
        &mut self.entries[self.current]
    }
    pub fn select(&mut self, index: usize) -> bool {
        select(self.entries.len(), &mut self.current, index)
    }
    pub fn close(&mut self, index: usize) -> Close {
        close(&mut self.entries, &mut self.current, index)
    }
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        reorder(&mut self.entries, &mut self.current, from, to)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    Unchanged,
    Window,
    Active,
    Background,
}

pub enum Disposition {
    Foreground,
    Background,
}

pub fn add<T>(
    entries: &mut Vec<T>,
    current: &mut usize,
    entry: T,
    disposition: Disposition,
) -> bool {
    if entries.len() >= 128 {
        return false;
    }
    entries.push(entry);
    if matches!(disposition, Disposition::Foreground) {
        *current = entries.len() - 1;
    }
    true
}

pub fn width(window: f32, count: usize) -> f32 {
    let available = window - 5.0 - 35.0 - 48.0 - 136.0;
    (((available - 8.0) / count.max(1) as f32).floor() + 8.0).clamp(88.0, 248.0)
}

pub fn title_room(width: f32) -> f32 {
    (width - 82.0).max(0.0)
}

pub fn label(path: &str, name: &str, settings: &crate::shell::listing::ExplorerSettings) -> String {
    match super::ids::search_parts(path) {
        Some((_, query)) => format!("{} - {}", query.trim(), name),
        None if !super::ids::is_virtual(path) && settings.full_path => path.into(),
        None => name.into(),
    }
}

pub fn caption(title: &str, count: usize) -> String {
    match count.saturating_sub(1) {
        _ if title.is_empty() => crate::i18n::tr("app.title").into(),
        0 => crate::i18n::system_format("explorerframe.dll", 35104, &[title]),
        1 => crate::i18n::system_format("explorerframe.dll", 35105, &[title]),
        n => crate::i18n::system_format("explorerframe.dll", 35106, &[title, &n.to_string()]),
    }
}

pub fn close_others<T>(entries: &mut Vec<T>, current: &mut usize, index: usize) -> Close {
    if index >= entries.len() || entries.len() == 1 {
        return Close::Unchanged;
    }
    let change = if index == *current {
        Close::Background
    } else {
        Close::Active
    };
    let keep = entries.swap_remove(index);
    *entries = vec![keep];
    *current = 0;
    change
}

pub fn close_right<T>(entries: &mut Vec<T>, current: &mut usize, index: usize) -> Close {
    if index >= entries.len().saturating_sub(1) {
        return Close::Unchanged;
    }
    entries.truncate(index + 1);
    if *current > index {
        *current = index;
        Close::Active
    } else {
        Close::Background
    }
}

pub fn duplicate<T: Clone>(entries: &mut Vec<T>, current: &mut usize, index: usize) -> bool {
    if entries.len() >= 128 {
        return false;
    }
    let Some(copy) = entries.get(index).cloned() else {
        return false;
    };
    entries.insert(index + 1, copy);
    *current = index + 1;
    true
}

#[derive(Clone, Copy)]
pub struct Drag {
    pub from: usize,
    pub origin: (f32, f32),
    pub pointer: (f32, f32),
    pub slot: usize,
    pub shown: usize,
    pub active: bool,
    pub dropping: bool,
}

impl Drag {
    pub fn new(from: usize, origin: (f32, f32)) -> Self {
        Self {
            from,
            origin,
            pointer: origin,
            slot: from,
            shown: from,
            active: false,
            dropping: false,
        }
    }

    pub fn moved(&mut self, point: (f32, f32), pitch: f32, count: usize) {
        self.pointer = point;
        self.active |=
            (point.0 - self.origin.0).abs() > 8.0 || (point.1 - self.origin.1).abs() > 8.0;
        if point.0 >= 8.0 + (self.slot as f32 + 1.8) * pitch {
            self.slot = ((point.0 - 8.0) / pitch + 0.2).floor().max(1.0) as usize - 1;
        } else if point.0 <= 8.0 + (self.slot as f32 - 0.8) * pitch {
            self.slot = ((point.0 - 8.0) / pitch - 0.2).ceil().max(0.0) as usize;
        }
        self.slot = self.slot.min(count.saturating_sub(1));
    }

    pub fn position(&self, index: usize) -> usize {
        if index > self.from && index <= self.shown {
            index - 1
        } else if index < self.from && index >= self.shown {
            index + 1
        } else {
            index
        }
    }
}

pub fn select(count: usize, current: &mut usize, index: usize) -> bool {
    if index >= count || index == *current {
        return false;
    }
    *current = index;
    true
}

pub fn cycle(current: usize, count: usize, delta: i32) -> usize {
    if count == 0 {
        return 0;
    }
    (current as i64 + delta as i64).rem_euclid(count as i64) as usize
}

pub fn close<T>(entries: &mut Vec<T>, current: &mut usize, index: usize) -> Close {
    if index >= entries.len() {
        return Close::Unchanged;
    }
    if entries.len() == 1 {
        return Close::Window;
    }
    entries.remove(index);
    let active = index == *current;
    if index < *current || *current >= entries.len() {
        *current = current.saturating_sub(1).min(entries.len() - 1);
    }
    if active {
        Close::Active
    } else {
        Close::Background
    }
}

pub fn reorder<T>(tabs: &mut Vec<T>, current: &mut usize, from: usize, to: usize) -> bool {
    if from >= tabs.len() || to >= tabs.len() || from == to {
        return false;
    }
    let tab = tabs.remove(from);
    tabs.insert(to, tab);
    *current = if *current == from {
        to
    } else if from < *current && *current <= to {
        *current - 1
    } else if to <= *current && *current < from {
        *current + 1
    } else {
        *current
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{filter::Filter, sort::SortKey};

    fn tabs() -> Vec<Location> {
        (0..4)
            .map(|i| Location {
                tab: Tab {
                    title: i.to_string(),
                    icon: i,
                    history: vec![format!("folder-{i}-before"), format!("folder-{i}-current")],
                    filters: vec![
                        None,
                        Some(Filter {
                            key: SortKey::Name,
                            labels: vec![i.to_string()],
                        }),
                    ],
                    libraries: vec![None, None],
                    index: 1,
                },
                visits: vec![
                    Default::default(),
                    super::super::selection::Selection {
                        selected: [format!("selected-{i}")].into(),
                        ..Default::default()
                    },
                ],
                ..Location::new("")
            })
            .collect()
    }

    #[test]
    fn every_move_preserves_active_tab_history_and_other_tabs_order() {
        for from in 0..4 {
            for to in 0..4 {
                for active in 0..4 {
                    let mut tabs = tabs();
                    let mut current = active;
                    reorder(&mut tabs, &mut current, from, to);
                    assert_eq!(tabs[current].tab.title, active.to_string());
                    assert_eq!(tabs[to].tab.title, from.to_string());
                    let others: Vec<_> = tabs
                        .iter()
                        .filter(|t| t.tab.icon != from as u32)
                        .map(|t| t.tab.icon)
                        .collect();
                    assert_eq!(
                        others,
                        (0..4).filter(|&i| i != from as u32).collect::<Vec<_>>()
                    );
                    for tab in &tabs {
                        assert_eq!(tab.tab.path(), format!("folder-{}-current", tab.tab.icon));
                        assert_eq!(
                            tab.tab.history[0],
                            format!("folder-{}-before", tab.tab.icon)
                        );
                        assert_eq!(tab.tab.filters.len(), 2);
                        assert_eq!(
                            tab.tab.filter().unwrap().labels,
                            vec![tab.tab.title.clone()]
                        );
                        assert_eq!(tab.tab.index, 1);
                        assert!(tab.visits[1]
                            .selected
                            .contains(&format!("selected-{}", tab.tab.icon)));
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
            assert_eq!(
                tabs.iter().map(|t| t.tab.icon).collect::<Vec<_>>(),
                vec![0, 1, 2, 3]
            );
        }
    }

    #[test]
    fn closing_background_tabs_keeps_the_active_journal_and_duplicate_is_independent() {
        let mut entries = tabs();
        let mut current = 2;
        assert!(duplicate(&mut entries, &mut current, 1));
        entries[current].tab.history.push("copy-only".into());
        assert_eq!(entries[1].tab.history.len(), 2);
        assert_eq!(close(&mut entries, &mut current, 0), Close::Background);
        assert_eq!(current, 1);
        assert_eq!(entries[current].tab.history.last().unwrap(), "copy-only");
        assert_eq!(close_right(&mut entries, &mut current, 0), Close::Active);
        assert_eq!(entries[current].tab.title, "1");
        assert_eq!(close(&mut entries, &mut current, 0), Close::Window);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn measured_drag_threshold_and_neighbour_edge_bands_are_grab_independent() {
        for grab in [92.0, 142.0] {
            let mut drag = Drag::new(0, (grab, 24.0));
            drag.moved((grab + 8.0, 24.0), 240.0, 3);
            assert!(!drag.active);
            drag.moved((grab + 9.0, 24.0), 240.0, 3);
            assert!(drag.active);
            drag.moved((439.0, 24.0), 240.0, 3);
            assert_eq!(drag.slot, 0);
            drag.moved((440.0, 24.0), 240.0, 3);
            assert_eq!(drag.slot, 1);
            assert_eq!(drag.shown, 0);
            drag.shown = drag.slot;
            assert_eq!(drag.position(1), 0);
            drag.moved((680.0, 24.0), 240.0, 3);
            assert_eq!(drag.slot, 2);
        }
        let mut reverse = Drag::new(2, (580.0, 24.0));
        reverse.moved((297.0, 24.0), 240.0, 3);
        assert_eq!(reverse.slot, 2);
        reverse.moved((296.0, 24.0), 240.0, 3);
        assert_eq!(reverse.slot, 1);
        assert_eq!(width(1100.0, 4), 225.0);
        assert_eq!(cycle(0, 3, -1), 2);
    }

    #[test]
    fn background_open_preserves_the_active_location_and_the_journal_limit() {
        let mut collection = Collection::new(Location::new("first"));
        assert!(add(
            &mut collection.entries,
            &mut collection.current,
            Location::new("second"),
            Disposition::Background
        ));
        assert_eq!(collection.current, 0);
        assert_eq!(collection.active().tab.path(), "first");
        while collection.entries.len() < 128 {
            assert!(add(
                &mut collection.entries,
                &mut collection.current,
                Location::new("more"),
                Disposition::Background
            ));
        }
        assert!(!add(
            &mut collection.entries,
            &mut collection.current,
            Location::new("overflow"),
            Disposition::Foreground
        ));
        assert!(!duplicate(
            &mut collection.entries,
            &mut collection.current,
            0
        ));
        assert_eq!(collection.entries.len(), 128);
        assert_eq!(collection.current, 0);
    }
}
