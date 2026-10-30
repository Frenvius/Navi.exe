use windows::core::Result;
use windows::Win32::Foundation::RECT;

use super::text::{Family, TextEngine, TextStyle};
use crate::app::menu_model::{Icon, Menu};

pub const LABEL: TextStyle = TextStyle::new(Family::Text, 14.0, 400);
pub const SHORTCUT: TextStyle = TextStyle::new(Family::Variable, 12.0, 400).with_ideal_advances();
pub const CAPTION: TextStyle = TextStyle::new(Family::Variable, 12.0, 400).with_ideal_advances();

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Root,
    Child,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Row(usize),
    Button(usize),
}

pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub rows: Vec<(f32, f32)>,
    pub row: f32,
    pub padding: f32,
    pub label: f32,
    pub icon: f32,
    pub toggle_column: bool,
    pub dropdown: bool,
    pub overflow: bool,
    pub context: bool,
    pub buttons_height: f32,
    pub button_width: f32,
    pub buttons_bottom: bool,
    pub shortcut_width: f32,
}

impl Layout {
    pub fn new(menu: &Menu, level: Level, text: &mut TextEngine) -> Result<Self> {
        let dropdown = level == Level::Child
            || matches!(
                menu.owner,
                "new"
                    | "sort"
                    | "view"
                    | "more"
                    | "filter"
                    | "collection"
                    | "search-options"
                    | "crumbs"
                    | "crumbs-more"
                    | "address"
                    | "history"
                    | "tab"
                    | "nav-options"
            );
        let overflow = menu.owner == "more" && level == Level::Root;
        let nav = menu.owner == "nav-options";
        let icons = menu
            .items
            .iter()
            .any(|item| !matches!(item.entry.icon, Icon::None) || !item.entry.glyph.is_empty());
        let toggles = menu.items.iter().any(|item| item.toggle);
        let toggle_column = dropdown && icons && toggles;
        let (icon, label, separator) = if nav {
            (16.0, 43.0, 3.0)
        } else if !icons && !toggles {
            (16.0, 16.0, if dropdown { 3.0 } else { 9.0 })
        } else if overflow {
            (13.0, 39.0, 9.0)
        } else if toggle_column {
            (44.0, 72.0, 3.0)
        } else if dropdown {
            (16.0, 44.0, 3.0)
        } else {
            (16.0, 43.0, 9.0)
        };
        let padding = if nav {
            5.0
        } else if overflow {
            5.0
        } else if dropdown {
            3.0
        } else {
            4.0
        };
        let right = if overflow { 13.0 } else { 16.0 };
        let row = if menu.owner == "tab" {
            33.0
        } else if menu.owner == "crumbs-more" {
            40.0
        } else {
            32.0
        };
        let mut width = if level == Level::Child {
            0.0
        } else {
            crate::app::menu_model::minimum_width(
                menu.owner,
                menu.items.iter().map(|item| &item.action),
            )
        };
        let mut shortcut_width: f32 = 0.0;
        for item in &menu.items {
            if item.entry.separator {
                continue;
            }
            let label_width = text.layout(&item.entry.label, LABEL, 4096.0)?.advance;
            let shortcut = if item.entry.shortcut.is_empty() {
                0.0
            } else {
                text.layout(&item.entry.shortcut, SHORTCUT, 4096.0)?.advance
            };
            shortcut_width = shortcut_width.max(shortcut);
            let shortcut = if shortcut == 0.0 {
                0.0
            } else {
                24.0 + shortcut
            };
            let child = if item.entry.submenu { 36.0 } else { 0.0 };
            width = width.max((label + label_width + shortcut + child + right).ceil());
        }
        if !dropdown {
            shortcut_width = shortcut_width.ceil();
        }
        let mut buttons_height = 0.0;
        let mut button_width: f32 = 56.0;
        if !menu.buttons.is_empty() {
            for (button, _) in &menu.buttons {
                let label = text.layout(&button.label, CAPTION, 4096.0)?.advance;
                button_width = button_width.max((label + 8.0).ceil());
            }
            width = width.max(menu.buttons.len() as f32 * (button_width + 9.0) + 9.0);
            buttons_height = 59.0;
        }
        let mut top = padding + buttons_height;
        let mut rows = Vec::with_capacity(menu.items.len());
        for item in &menu.items {
            let height = if item.entry.separator { separator } else { row };
            rows.push((top, height));
            top += height;
        }
        Ok(Self {
            width,
            height: top + padding,
            rows,
            row,
            padding,
            label,
            icon,
            toggle_column,
            dropdown,
            overflow,
            context: !dropdown,
            buttons_height,
            button_width,
            buttons_bottom: false,
            shortcut_width,
        })
    }

    pub fn button_left(&self, index: usize) -> f32 {
        6.0 + index as f32 * (self.button_width + 9.0)
    }

    pub fn buttons_top(&self) -> f32 {
        if self.buttons_bottom {
            self.height - self.buttons_height + 5.0
        } else {
            self.padding + 2.0
        }
    }

    pub fn flip_buttons(&mut self) {
        if self.buttons_height == 0.0 {
            return;
        }
        self.buttons_bottom = true;
        for (top, _) in &mut self.rows {
            *top -= self.buttons_height;
        }
    }

    pub fn row_area(&self) -> (f32, f32) {
        (
            self.padding
                + if self.buttons_bottom {
                    0.0
                } else {
                    self.buttons_height
                },
            self.height
                - self.padding
                - if self.buttons_bottom {
                    self.buttons_height
                } else {
                    0.0
                },
        )
    }

    pub fn maximum_offset(&self) -> f32 {
        (self.rows.last().map_or(0.0, |(top, height)| top + height) - self.row_area().1).max(0.0)
    }

    pub fn hit(&self, menu: &Menu, point: (f32, f32), offset: f32) -> Option<Hit> {
        if point.0 < 1.0
            || point.0 >= self.width - 1.0
            || point.1 < self.padding
            || point.1 >= self.height - self.padding
        {
            return None;
        }
        if self.buttons_height > 0.0
            && point.1 >= self.buttons_top()
            && point.1 < self.buttons_top() + self.buttons_height - 11.0
        {
            let index = ((point.0 - 5.0) / (self.button_width + 9.0)).floor() as usize;
            let left = self.button_left(index);
            return menu
                .buttons
                .get(index)
                .filter(|(button, _)| {
                    !button.disabled && point.0 >= left && point.0 < left + self.button_width
                })
                .map(|_| Hit::Button(index));
        }
        let (top, bottom) = self.row_area();
        if point.1 < top || point.1 >= bottom {
            return None;
        }
        self.rows
            .iter()
            .enumerate()
            .find_map(|(index, &(top, height))| {
                (point.1 + offset >= top
                    && point.1 + offset < top + height
                    && !menu.items[index].entry.separator
                    && !menu.items[index].entry.disabled)
                    .then_some(Hit::Row(index))
            })
    }
}

pub enum ContextCommands {
    None,
    Primary(i32),
}

pub struct ContextPlacement {
    pub point: (i32, i32),
    pub buttons_bottom: bool,
}

pub fn context_position(
    point: (i32, i32),
    size: (i32, i32),
    work: RECT,
    commands: ContextCommands,
    scale: f32,
) -> ContextPlacement {
    let x = point.0.min(work.right - size.0).max(work.left);
    let (reserve, primary) = match commands {
        ContextCommands::None => ((84.0 * scale).round() as i32, 0),
        ContextCommands::Primary(height) => (0, height + scale.round() as i32),
    };
    let above = size.1 - primary;
    let (y, buttons_bottom) = if point.1 + size.1 + reserve <= work.bottom {
        (point.1, false)
    } else if point.1 - above >= work.top {
        (
            (point.1 - above).min(work.bottom - size.1).max(work.top),
            primary > 0,
        )
    } else {
        (
            (point.1 - ((if primary > 0 { 86.0 } else { 14.0 }) * scale).round() as i32)
                .max(work.top)
                .min((work.bottom - size.1).max(work.top)),
            false,
        )
    };
    ContextPlacement {
        point: (x, y),
        buttons_bottom,
    }
}

pub fn next(menu: &Menu, current: Option<Hit>, direction: i32) -> Option<Hit> {
    let rows: Vec<_> = menu
        .buttons
        .iter()
        .enumerate()
        .filter(|(_, (button, _))| !button.disabled)
        .map(|(index, _)| Hit::Button(index))
        .chain(
            menu.items
                .iter()
                .enumerate()
                .filter(|(_, item)| !item.entry.separator && !item.entry.disabled)
                .map(|(index, _)| Hit::Row(index)),
        )
        .collect();
    if rows.is_empty() {
        return None;
    }
    let index = current.and_then(|current| rows.iter().position(|&row| row == current));
    Some(
        rows[match index {
            Some(index) => (index as i32 + direction).rem_euclid(rows.len() as i32) as usize,
            None if direction > 0 => 0,
            None => rows.len() - 1,
        }],
    )
}

pub fn button_next(menu: &Menu, current: Option<Hit>, direction: i32) -> Option<Hit> {
    let enabled: Vec<_> = menu
        .buttons
        .iter()
        .enumerate()
        .filter(|(_, (button, _))| !button.disabled)
        .map(|(index, _)| Hit::Button(index))
        .collect();
    let index = current.and_then(|current| enabled.iter().position(|&hit| hit == current))?;
    Some(enabled[(index as i32 + direction).rem_euclid(enabled.len() as i32) as usize])
}

pub fn accelerator(menu: &Menu, chord: &str) -> Option<crate::app::menu_model::Action> {
    use crate::app::menu_model::Action;
    menu.items
        .iter()
        .find(|item| !item.entry.disabled && !item.entry.separator && item.entry.shortcut == chord)
        .map(|item| item.action.clone())
        .or_else(|| {
            menu.buttons
                .iter()
                .find(|(button, action)| {
                    !button.disabled
                        && match action {
                            Action::Cut => chord == "Ctrl+X",
                            Action::Copy => chord == "Ctrl+C",
                            Action::Paste => chord == "Ctrl+V",
                            Action::Rename => chord == "F2",
                            Action::Delete => chord == "Delete",
                            _ => false,
                        }
                })
                .map(|(_, action)| action.clone())
        })
}

pub fn access_key_input(menu: &Menu, prefix: &mut String, character: char) -> Option<Hit> {
    prefix.push(character.to_ascii_uppercase());
    let buttons = menu
        .buttons
        .iter()
        .enumerate()
        .filter(|(_, (button, _))| !button.disabled)
        .map(|(index, (button, _))| (&button.key_tip, Hit::Button(index)));
    let rows = menu
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.entry.disabled && !item.entry.separator)
        .map(|(index, item)| (&item.entry.key_tip, Hit::Row(index)));
    let mut partial = false;
    for (key, hit) in buttons.chain(rows) {
        if key == prefix {
            prefix.clear();
            return Some(hit);
        }
        partial |= key.starts_with(prefix.as_str());
    }
    if !partial {
        prefix.clear();
    }
    None
}

pub fn grace(
    previous: windows::Win32::Foundation::POINT,
    point: windows::Win32::Foundation::POINT,
    child: (i32, i32),
    size: (i32, i32),
) -> bool {
    let edge = if previous.x < child.0 {
        child.0
    } else {
        child.0 + size.0
    };
    if (point.x - previous.x) * (edge - previous.x) <= 0 {
        return false;
    }
    let cross = |a: (i64, i64), b: (i64, i64), p: (i64, i64)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let a = (previous.x as i64, previous.y as i64);
    let b = (edge as i64, (child.1 - 8) as i64);
    let c = (edge as i64, (child.1 + size.1 + 8) as i64);
    let p = (point.x as i64, point.y as i64);
    let signs = [cross(a, b, p), cross(b, c, p), cross(c, a, p)];
    signs.iter().all(|&value| value >= 0) || signs.iter().all(|&value| value <= 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::menu_model::{self, Action, Icon};
    use windows::Win32::Foundation::POINT;

    #[test]
    fn context_placement_reserves_toolbar_and_handles_tall_bodies() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        assert_eq!(
            context_position((560, 560), (300, 385), work, ContextCommands::None, 1.0).point,
            (560, 560)
        );
        assert_eq!(
            context_position((560, 570), (300, 385), work, ContextCommands::None, 1.0).point,
            (560, 185)
        );
        assert_eq!(
            context_position((1900, 300), (300, 900), work, ContextCommands::None, 1.0).point,
            (1620, 132)
        );
        let placement = context_position(
            (239, 862),
            (348, 610),
            work,
            ContextCommands::Primary(72),
            1.0,
        );
        assert_eq!(placement.point, (239, 325));
        assert!(placement.buttons_bottom);
        let placement = context_position(
            (239, 570),
            (348, 610),
            work,
            ContextCommands::Primary(72),
            1.0,
        );
        assert_eq!(placement.point, (239, 33));
        assert!(placement.buttons_bottom);
        let placement = context_position(
            (239, 300),
            (348, 990),
            work,
            ContextCommands::Primary(72),
            1.0,
        );
        assert!(!placement.buttons_bottom);
    }

    #[test]
    fn keyboard_skips_disabled_rows_and_wraps() {
        let menu = Menu {
            owner: "test",
            buttons: Vec::new(),
            items: vec![
                menu_model::separator(),
                menu_model::item("disabled", Icon::None, Action::None).disabled(true),
                menu_model::item("first", Icon::None, Action::None),
                menu_model::separator(),
                menu_model::item("last", Icon::None, Action::None),
            ],
        };
        assert_eq!(next(&menu, None, 1), Some(Hit::Row(2)));
        assert_eq!(next(&menu, Some(Hit::Row(2)), -1), Some(Hit::Row(4)));
        assert_eq!(next(&menu, Some(Hit::Row(4)), 1), Some(Hit::Row(2)));
    }

    #[test]
    fn down_visits_primary_commands_before_normal_rows() {
        let mut disabled = menu_model::button("disabled", Icon::None, Action::None);
        disabled.0.disabled = true;
        let menu = Menu {
            owner: "item",
            buttons: vec![
                menu_model::button("cut", Icon::None, Action::Cut),
                disabled,
                menu_model::button("rename", Icon::None, Action::Rename),
            ],
            items: vec![menu_model::item("open", Icon::None, Action::OpenSelection)],
        };
        assert_eq!(next(&menu, Some(Hit::Button(0)), 1), Some(Hit::Button(2)));
        assert_eq!(next(&menu, Some(Hit::Button(2)), 1), Some(Hit::Row(0)));
        assert_eq!(next(&menu, Some(Hit::Row(0)), -1), Some(Hit::Button(2)));
    }

    #[test]
    fn invalid_and_completed_access_keys_release_the_prefix() {
        let mut row = menu_model::item("open with", Icon::None, Action::None);
        row.entry.key_tip = "I1".into();
        let menu = Menu {
            owner: "item",
            items: vec![row],
            buttons: Vec::new(),
        };
        let mut prefix = String::new();
        assert_eq!(access_key_input(&menu, &mut prefix, 'q'), None);
        assert!(prefix.is_empty());
        assert_eq!(access_key_input(&menu, &mut prefix, 'i'), None);
        assert_eq!(prefix, "I");
        assert_eq!(access_key_input(&menu, &mut prefix, '1'), Some(Hit::Row(0)));
        assert!(prefix.is_empty());
    }

    #[test]
    fn accelerators_only_offer_enabled_menu_commands() {
        use crate::app::menu_model::{button, item};
        let menu = Menu {
            owner: "test",
            items: vec![
                item("properties", Icon::None, Action::Properties(Vec::new()))
                    .shortcut("Alt+Enter"),
                item("undo", Icon::None, Action::Undo)
                    .shortcut("Ctrl+Z")
                    .disabled(true),
            ],
            buttons: vec![button("rename", Icon::None, Action::Rename)],
        };
        assert!(matches!(
            accelerator(&menu, "Alt+Enter"),
            Some(Action::Properties(_))
        ));
        assert!(matches!(accelerator(&menu, "F2"), Some(Action::Rename)));
        assert!(accelerator(&menu, "Ctrl+Z").is_none());
        assert!(accelerator(&menu, "Ctrl+C").is_none());
    }

    #[test]
    fn pointer_grace_keeps_diagonal_travel_toward_either_submenu() {
        assert!(grace(
            POINT { x: 200, y: 200 },
            POINT { x: 240, y: 230 },
            (300, 100),
            (180, 300)
        ));
        assert!(!grace(
            POINT { x: 200, y: 200 },
            POINT { x: 190, y: 230 },
            (300, 100),
            (180, 300)
        ));
        assert!(grace(
            POINT { x: 500, y: 200 },
            POINT { x: 460, y: 230 },
            (100, 100),
            (180, 300)
        ));
        assert!(!grace(
            POINT { x: 200, y: 200 },
            POINT { x: 240, y: 500 },
            (300, 100),
            (180, 300)
        ));
    }
}
