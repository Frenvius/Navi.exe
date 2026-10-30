use crate::shell::listing::ShellEntry;

pub const QUICK_ACCESS: i32 = 1;
pub const FAVORITES: i32 = 2;
pub const RECENT: i32 = 3;

pub const TILE_WIDTH: f32 = 264.0;
pub const TILE_HEIGHT: f32 = 58.0;
const TILE_PITCH_X: f32 = 284.0;
const TILE_PITCH_Y: f32 = 60.0;
pub const ROW_HEIGHT: f32 = 40.0;
const ROW_PITCH: f32 = 44.0;
pub const MARGIN: f32 = 16.0;
const RIGHT_MARGIN: f32 = 14.0;
const TILES_TOP: f32 = 47.0;
const PIVOTS_GAP: f32 = 19.0;
pub const PIVOT_HEIGHT: f32 = 31.0;
const ROWS_BELOW_PIVOTS: f32 = 89.0;
const LIST_BOTTOM: f32 = 2.0;
pub const LIST_HEADER_BELOW_PIVOTS: f32 = 51.0;
pub const NAME_COLUMN: f32 = 87.0;
const EMPTY_HEIGHT: f32 = 292.0;
const COLLAPSED_PIVOTS_TOP: f32 = 59.0;
pub const BUTTON_SIZE: f32 = 24.0;
pub const WHEEL_FRACTION: f32 = 0.15;
const BUTTON_TOP: f32 = 8.0;
const BUTTON_OFFSETS: [f32; 2] = [77.0, 37.0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub filter: u8,
    pub pivot: Pivot,
    pub quick: usize,
    pub has_quick: bool,
    pub collapsed: Sections,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sections {
    pub quick: bool,
    pub list: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Quick,
    List,
}

impl Sections {
    pub fn toggled(self, section: Section) -> Sections {
        match section {
            Section::Quick => Sections {
                quick: !self.quick,
                ..self
            },
            Section::List => Sections {
                list: !self.list,
                ..self
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowButton {
    OpenLocation,
    More,
}

pub fn row_buttons(cell: [f32; 4], accessed: f32) -> [(RowButton, [f32; 4]); 2] {
    [RowButton::OpenLocation, RowButton::More].map(|button| {
        let offset = BUTTON_OFFSETS[button as usize];
        (
            button,
            [
                accessed - offset,
                cell[1] + BUTTON_TOP,
                BUTTON_SIZE,
                BUTTON_SIZE,
            ],
        )
    })
}

pub fn row_button_at(cell: [f32; 4], accessed: f32, point: (f32, f32)) -> Option<RowButton> {
    row_buttons(cell, accessed)
        .into_iter()
        .find(|(_, [x, y, width, height])| {
            point.0 >= *x && point.0 < x + width && point.1 >= *y && point.1 < y + height
        })
        .map(|(button, _)| button)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pivot {
    #[default]
    Recent,
    Favorites,
}

impl Pivot {
    pub fn group(self) -> i32 {
        match self {
            Pivot::Recent => RECENT,
            Pivot::Favorites => FAVORITES,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub cells: Vec<[f32; 4]>,
    pub columns: usize,
    pub pivots_top: f32,
    pub height: f32,
    pub list_columns: [f32; 3],
}

pub fn layout(state: &State, listed: usize, width: f32) -> Layout {
    let quick = state.quick;
    let columns = (((width - MARGIN) / TILE_PITCH_X).floor() as usize).max(1);
    let mut cells = Vec::with_capacity(quick + listed);
    for index in 0..quick {
        cells.push([
            MARGIN + (index % columns) as f32 * TILE_PITCH_X,
            TILES_TOP + (index / columns) as f32 * TILE_PITCH_Y,
            TILE_WIDTH,
            TILE_HEIGHT,
        ]);
    }
    let pivots_top = if !state.has_quick {
        PIVOTS_GAP
    } else if state.collapsed.quick || quick == 0 {
        COLLAPSED_PIVOTS_TOP
    } else {
        TILES_TOP + quick.div_ceil(columns) as f32 * TILE_PITCH_Y - (TILE_PITCH_Y - TILE_HEIGHT)
            + PIVOTS_GAP
    };
    let rows_top = pivots_top + ROWS_BELOW_PIVOTS;
    let row_width = (width - MARGIN - RIGHT_MARGIN).max(0.0);
    for index in 0..listed {
        cells.push([
            MARGIN,
            rows_top + index as f32 * ROW_PITCH,
            row_width,
            ROW_HEIGHT,
        ]);
    }
    let span = (MARGIN + row_width - NAME_COLUMN).max(0.0);
    Layout {
        cells,
        columns,
        pivots_top,
        height: if state.collapsed.list {
            pivots_top + PIVOT_HEIGHT + MARGIN
        } else if listed == 0 {
            pivots_top + EMPTY_HEIGHT
        } else {
            rows_top + listed as f32 * ROW_PITCH + LIST_BOTTOM
        },
        list_columns: [
            NAME_COLUMN,
            NAME_COLUMN + (span * 0.4612).round(),
            NAME_COLUMN + (span * 0.7326).round(),
        ],
    }
}

pub fn matches(entry: &ShellEntry, kind: u8) -> bool {
    if kind == 0 {
        return true;
    }
    if entry.is_dir {
        return false;
    }
    let ext = std::path::Path::new(&entry.path)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let any = |list: &[&str]| list.contains(&ext.as_str());
    match kind {
        1 => any(&["doc", "docx", "docm", "dot", "dotx", "dotm", "rtf", "odt"]),
        2 => any(&[
            "xls", "xlsx", "xlsm", "xlsb", "xlt", "xltx", "xltm", "csv", "ods",
        ]),
        3 => any(&[
            "ppt", "pptx", "pptm", "pps", "ppsx", "ppsm", "pot", "potx", "potm", "odp",
        ]),
        4 | 5 => crate::shell::assoc::perceived(&ext) == if kind == 4 { "image" } else { "video" },
        6 => ext == "pdf",
        _ => true,
    }
}

pub fn filter_items(current: u8) -> Vec<super::menu_model::Item> {
    [50292u32, 50293, 50294, 50295, 50296, 50313, 50314]
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            super::menu_model::item(
                &crate::i18n::system_string("explorerframe.dll", id),
                super::menu_model::Icon::None,
                super::menu_model::Action::HomeFilter(index as u8),
            )
            .bullet(current == index as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick(count: usize) -> State {
        State {
            quick: count,
            has_quick: count > 0,
            ..Default::default()
        }
    }

    #[test]
    fn layout_matches_the_measured_page_at_the_default_window_width() {
        let page = layout(&quick(15), 2, 875.0);
        assert_eq!(page.columns, 3);
        assert_eq!(page.cells[0], [16.0, 47.0, 264.0, 58.0]);
        assert_eq!(page.cells[4], [300.0, 107.0, 264.0, 58.0]);
        assert_eq!(page.pivots_top, 364.0);
        assert_eq!(page.cells[15], [16.0, 453.0, 845.0, 40.0]);
        assert_eq!(page.cells[16][1], 497.0);
        assert_eq!(page.list_columns, [87.0, 444.0, 654.0]);
        assert_eq!(page.height, 543.0);
    }

    #[test]
    fn narrow_pages_keep_one_tile_column() {
        let page = layout(&quick(3), 0, 120.0);
        assert_eq!(page.columns, 1);
        assert_eq!(page.cells[2][1], 167.0);
        assert_eq!(page.pivots_top, 244.0);
        assert_eq!(page.height, 536.0);
    }

    #[test]
    fn collapsed_sections_move_the_pills_up_and_drop_the_list() {
        let state = State {
            has_quick: true,
            collapsed: Sections::default().toggled(Section::Quick),
            ..Default::default()
        };
        let page = layout(&state, 2, 875.0);
        assert_eq!(page.pivots_top, 59.0);
        assert_eq!(page.cells[0], [16.0, 148.0, 845.0, 40.0]);
        let state = State {
            collapsed: state.collapsed.toggled(Section::List),
            ..state
        };
        assert_eq!(layout(&state, 0, 875.0).height, 106.0);
    }

    #[test]
    fn row_buttons_sit_before_the_date_column() {
        let cell = [16.0, 453.0, 845.0, 40.0];
        assert_eq!(
            row_buttons(cell, 444.0),
            [
                (RowButton::OpenLocation, [367.0, 461.0, 24.0, 24.0]),
                (RowButton::More, [407.0, 461.0, 24.0, 24.0]),
            ]
        );
        assert_eq!(
            row_button_at(cell, 444.0, (420.0, 470.0)),
            Some(RowButton::More)
        );
        assert_eq!(row_button_at(cell, 444.0, (395.0, 470.0)), None);
    }
}
