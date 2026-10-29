use std::collections::HashSet;
use std::time::Instant;

use crate::shell::listing::ShellEntry;

pub const BAND_INTERVAL_MS: u32 = 45;
pub const LIST_BAND_INTERVAL_MS: u32 = 90;

#[derive(Clone, Copy, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Clone, Default)]
pub struct Selection {
    pub selected: HashSet<String>,
    pub anchor: Option<String>,
    pub focused: Option<String>,
}

pub fn row_of(order: &[usize], entries: &[ShellEntry], path: &str) -> Option<usize> {
    order.iter().position(|&i| entries[i].path == path)
}

pub fn range(order: &[usize], entries: &[ShellEntry], a: usize, b: usize) -> HashSet<String> {
    let (lo, hi) = (a.min(b), a.max(b));
    order
        .iter()
        .take(hi.saturating_add(1))
        .skip(lo)
        .map(|&i| entries[i].path.clone())
        .collect()
}

impl Selection {
    pub fn restored(&mut self, order: &[usize], entries: &[ShellEntry]) {
        let paths = entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<HashSet<_>>();
        self.selected.retain(|path| paths.contains(path.as_str()));
        if self.selected.is_empty() {
            self.focused = order.first().map(|&index| entries[index].path.clone());
            self.anchor = None;
        } else {
            if self
                .focused
                .as_ref()
                .is_none_or(|path| !self.selected.contains(path))
            {
                self.focused = order
                    .iter()
                    .map(|&index| &entries[index].path)
                    .find(|path| self.selected.contains(*path))
                    .cloned();
            }
            if self
                .anchor
                .as_ref()
                .is_none_or(|path| !paths.contains(path.as_str()))
            {
                self.anchor = self.focused.clone();
            }
        }
    }

    pub fn group(
        &mut self,
        order: &[usize],
        entries: &[ShellEntry],
        rows: std::ops::Range<usize>,
        mods: Modifiers,
    ) {
        if !mods.ctrl {
            self.selected.clear();
        }
        self.selected
            .extend(rows.clone().map(|row| entries[order[row]].path.clone()));
        self.focused = rows
            .clone()
            .next()
            .map(|row| entries[order[row]].path.clone());
        self.anchor = self.focused.clone();
    }

    pub fn press(
        &mut self,
        order: &[usize],
        entries: &[ShellEntry],
        row: usize,
        mods: Modifiers,
    ) -> bool {
        let Some(&index) = order.get(row) else {
            return false;
        };
        let path = entries[index].path.clone();
        if mods.shift {
            let from = self
                .anchor
                .as_ref()
                .and_then(|p| row_of(order, entries, p))
                .unwrap_or(0);
            let mut next = range(order, entries, from, row);
            if mods.ctrl {
                next.extend(self.selected.iter().cloned());
            }
            self.selected = next;
            if self.anchor.is_none() {
                self.anchor = Some(path.clone());
            }
        } else if mods.ctrl {
            if !self.selected.remove(&path) {
                self.selected.insert(path.clone());
            }
            self.anchor = Some(path.clone());
        } else if self.selected.contains(&path) {
            self.focused = Some(path);
            return true;
        } else {
            self.selected = HashSet::from([path.clone()]);
            self.anchor = Some(path.clone());
        }
        self.focused = Some(path);
        false
    }

    pub fn move_to(
        &mut self,
        order: &[usize],
        entries: &[ShellEntry],
        row: usize,
        mods: Modifiers,
    ) {
        let Some(&index) = order.get(row) else { return };
        let target = entries[index].path.clone();
        if mods.shift {
            let from = self
                .anchor
                .as_ref()
                .and_then(|p| row_of(order, entries, p))
                .unwrap_or(0);
            let mut next = range(order, entries, from, row);
            if mods.ctrl {
                next.extend(self.selected.iter().cloned());
            }
            self.selected = next;
            if self.anchor.is_none() {
                self.anchor = Some(target.clone());
            }
        } else if !mods.ctrl {
            self.selected = HashSet::from([target.clone()]);
            self.anchor = Some(target.clone());
        }
        self.focused = Some(target);
    }

    pub fn toggle_focus(&mut self) {
        let Some(path) = self.focused.clone() else {
            return;
        };
        if !self.selected.remove(&path) {
            self.selected.insert(path.clone());
        }
        self.anchor = Some(path);
    }
}

pub fn band_selection(
    base: &HashSet<String>,
    hits: impl Iterator<Item = String>,
    mods: Modifiers,
) -> HashSet<String> {
    let mut next = base.clone();
    for path in hits {
        if mods.ctrl && base.contains(&path) {
            next.remove(&path);
        } else {
            next.insert(path);
        }
    }
    next
}

pub fn typeahead<'a>(
    state: &mut (Instant, String),
    text: &str,
    current: isize,
    names: impl ExactSizeIterator<Item = &'a str>,
) -> Option<usize> {
    let now = Instant::now();
    if now.duration_since(state.0).as_millis() > super::TYPEAHEAD_RESET_MS {
        state.1.clear();
    }
    state.0 = now;
    state.1.push_str(text);
    let repeated = state.1.chars().all(|c| Some(c) == state.1.chars().next());
    let prefix = if repeated {
        text.to_lowercase()
    } else {
        state.1.to_lowercase()
    };
    let n = names.len();
    if n == 0 {
        return None;
    }
    let start = if repeated {
        (current + 1).max(0) as usize
    } else {
        current.max(0) as usize
    } % n;
    names
        .enumerate()
        .filter(|(_, name)| name.to_lowercase().starts_with(&prefix))
        .min_by_key(|(row, _)| (row + n - start) % n)
        .map(|(row, _)| row)
}

pub fn page_target(
    rows: &[(usize, f32, f32)],
    current: isize,
    y: f32,
    view: f32,
    direction: i32,
) -> isize {
    let Some(&(first, _, _)) = rows.first() else {
        return current;
    };
    if direction > 0 {
        let last = |end: f32| {
            rows.iter()
                .rev()
                .find(|(_, _, bottom)| *bottom <= end + 0.01)
                .map(|&(row, _, _)| row)
        };
        let edge = last(y + view).unwrap_or(first);
        if current < edge as isize {
            return edge as isize;
        }
        let top = rows
            .iter()
            .find(|(row, _, _)| *row == current.max(0) as usize)
            .map_or(0.0, |r| r.1);
        last(top + view).map_or(current + 1, |row| (row as isize).max(current + 1))
    } else {
        let at = |start: f32| {
            rows.iter()
                .find(|(_, top, _)| *top >= start - 0.01)
                .map(|&(row, _, _)| row)
        };
        let edge = at(y).unwrap_or(first);
        if current > edge as isize {
            return edge as isize;
        }
        let bottom = rows
            .iter()
            .find(|(row, _, _)| *row == current.max(0) as usize)
            .map_or(0.0, |r| r.2);
        at(bottom - view).map_or(current - 1, |row| (row as isize).min(current - 1))
    }
}

pub fn reveal_offset(y: f32, view: f32, top: f32, bottom: f32) -> f32 {
    if top < y {
        if top <= super::ROW_TOP + 0.01 {
            0.0
        } else {
            top
        }
    } else if bottom > y + view {
        bottom - view
    } else {
        y
    }
}

pub fn typeahead_offset(y: f32, view: f32, top: f32, bottom: f32) -> f32 {
    if top < y || bottom > y + view {
        if top <= super::ROW_TOP + 0.01 {
            0.0
        } else {
            top
        }
    } else {
        y
    }
}

pub fn band_scroll(pointer: f32, top: f32, bottom: f32) -> f32 {
    if pointer >= bottom - 1.0 {
        (pointer - bottom + 3.0).max(4.0)
    } else if pointer <= top {
        -(top - pointer + 3.0).max(4.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_preserve_anchor_pending_narrow_and_keyboard_focus() {
        let entries = crate::shell::listing::list(r"C:\Windows\System32").unwrap();
        let order = [0, 1, 2, 3];
        let path = |row: usize| entries[order[row]].path.clone();
        let mut selection = Selection::default();
        assert!(!selection.press(&order, &entries, 0, Modifiers::default()));
        selection.press(
            &order,
            &entries,
            2,
            Modifiers {
                ctrl: true,
                shift: false,
            },
        );
        selection.press(
            &order,
            &entries,
            3,
            Modifiers {
                ctrl: true,
                shift: true,
            },
        );
        assert_eq!(
            selection.selected,
            HashSet::from([path(0), path(2), path(3)])
        );
        assert_eq!(selection.anchor, Some(path(2)));
        selection.press(
            &order,
            &entries,
            1,
            Modifiers {
                ctrl: false,
                shift: true,
            },
        );
        assert_eq!(selection.selected, HashSet::from([path(1), path(2)]));
        assert!(selection.press(&order, &entries, 1, Modifiers::default()));
        assert_eq!(selection.selected.len(), 2);
        selection.move_to(
            &order,
            &entries,
            3,
            Modifiers {
                ctrl: true,
                shift: false,
            },
        );
        assert_eq!(selection.anchor, Some(path(2)));
        assert_eq!(selection.selected.len(), 2);
        selection.toggle_focus();
        assert_eq!(
            selection.selected,
            HashSet::from([path(1), path(2), path(3)])
        );
        assert_eq!(selection.anchor, Some(path(3)));
    }

    #[test]
    fn prefix_continues_and_repeated_initial_cycles_after_timeout() {
        let names = ["drivers", "DriverState", "Dism", "abc"];
        let mut state = (Instant::now(), String::new());
        assert_eq!(typeahead(&mut state, "d", 3, names.into_iter()), Some(0));
        assert_eq!(typeahead(&mut state, "r", 0, names.into_iter()), Some(0));
        state.0 = Instant::now() - std::time::Duration::from_millis(1001);
        assert_eq!(typeahead(&mut state, "d", 0, names.into_iter()), Some(1));
        assert_eq!(typeahead(&mut state, "d", 1, names.into_iter()), Some(2));
        assert_eq!(typeahead(&mut state, "d", 2, names.into_iter()), Some(0));
    }

    #[test]
    fn pages_reveal_and_band_speed_match_measured_details_edges() {
        let rows: Vec<_> = (0..60)
            .map(|row| (row, 6.0 + row as f32 * 21.0, 28.0 + row as f32 * 21.0))
            .collect();
        assert_eq!(page_target(&rows, 7, 0.0, 517.0, 1), 23);
        assert_eq!(page_target(&rows, 23, 0.0, 517.0, 1), 46);
        assert_eq!(reveal_offset(0.0, 517.0, rows[46].1, rows[46].2), 477.0);
        assert_eq!(page_target(&rows, 46, 477.0, 517.0, -1), 23);
        assert_eq!(page_target(&rows, 23, 477.0, 517.0, -1), 0);
        assert_eq!(reveal_offset(477.0, 517.0, 6.0, 28.0), 0.0);
        assert_eq!(typeahead_offset(0.0, 517.0, 636.0, 658.0), 636.0);
        assert_eq!(typeahead_offset(636.0, 517.0, 762.0, 784.0), 636.0);
        assert_eq!(band_scroll(677.0, 161.0, 678.0), 4.0);
        assert_eq!(band_scroll(688.0, 161.0, 678.0), 13.0);
        assert_eq!(band_scroll(161.0, 161.0, 678.0), -4.0);
        assert_eq!(band_scroll(300.0, 161.0, 678.0), 0.0);
    }
}

pub fn details_bottom(top: f32, pitch: f32) -> f32 {
    top + if pitch == super::ROW_PITCH_ROOMY {
        26.0
    } else {
        22.0
    }
}
