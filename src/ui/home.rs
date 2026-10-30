use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F, D2D_SIZE_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext5, ID2D1SvgDocument, D2D1_ROUNDED_RECT,
};

use super::{
    details,
    list::Folder,
    render::Canvas,
    text::{Family, TextEngine, TextStyle},
    theme::Theme,
};
use crate::app::home::{self, Pivot, RowButton, Section};

const TITLE: TextStyle = TextStyle::new(Family::Text, 14.0, 600);
const BODY: TextStyle = TextStyle::new(Family::Text, 14.0, 400);
const CAPTION: TextStyle = TextStyle::new(Family::Variable, 12.0, 400);
const CHEVRON: TextStyle = TextStyle::new(Family::Icons, 14.0, 400);
const PIN: TextStyle = TextStyle::new(Family::Icons, 10.0, 400);
const GLYPH: TextStyle = TextStyle::new(Family::Icons, 16.0, 400);
const STATUS_X: f32 = 46.0;
const BUTTON_GLYPH: f32 = 4.0;
const NAME_BEFORE_BUTTONS: f32 = 18.0;
const SECTION_BUTTON: f32 = 32.0;
const QUICK_BUTTON_TOP: f32 = 8.0;
const EXPANDED: &str = "\u{e70d}";
const COLLAPSED: &str = "\u{e76c}";
const CHEVRON_X: f32 = 25.0;
const TITLE_X: f32 = 60.0;
const PIVOT_X: f32 = 56.0;
const PIVOT_GAP: f32 = 8.0;
const PIVOT_TEXT: f32 = 36.0;
const PIVOT_END: f32 = 13.0;
const TILE_TEXT: f32 = 136.0;
const TILE_TEXT_WIDTH: f32 = 116.0;
const ROW_TEXT: f32 = 120.0;
const PICTURE_X: f32 = 76.0;
const CELL_GAP: f32 = 8.0;
const NAME_GAP: f32 = 25.0;
const ILLUSTRATION: f32 = 148.0;
const EMPTY_TEXT_WIDTH: f32 = 400.0;
const EMPTY_TITLE: TextStyle = TextStyle::new(Family::Variable, 12.0, 700);

#[derive(Clone, Debug, Default)]
pub struct Labels {
    pub quick: String,
    pub recent: String,
    pub favorites: String,
    pub columns: [String; 3],
}

impl Labels {
    pub fn load() -> Labels {
        let column = |name: &str| {
            crate::shell::columns::key(name)
                .and_then(|key| crate::shell::columns::describe(&key))
                .map(|column| column.label)
                .unwrap_or_default()
        };
        Labels {
            quick: crate::shell::home::grouping_label(home::QUICK_ACCESS),
            recent: crate::shell::home::grouping_label(home::RECENT),
            favorites: crate::shell::home::grouping_label(home::FAVORITES),
            columns: [
                column("System.ItemNameDisplay"),
                column("System.DateAccessed"),
                column("System.ActivityInfo"),
            ],
        }
    }

    fn pivot(&self, pivot: Pivot) -> &str {
        match pivot {
            Pivot::Recent => &self.recent,
            Pivot::Favorites => &self.favorites,
        }
    }
}

pub fn pivots(
    text: &mut TextEngine,
    labels: &Labels,
    top: f32,
) -> Result<[(Pivot, D2D_RECT_F); 2]> {
    let mut left = PIVOT_X;
    let mut rects = [(Pivot::Recent, rect(0.0, 0.0, 0.0, 0.0)); 2];
    for (slot, pivot) in rects.iter_mut().zip([Pivot::Recent, Pivot::Favorites]) {
        let advance = text.layout(labels.pivot(pivot), BODY, 4096.0)?.advance;
        let width = (PIVOT_TEXT + advance + PIVOT_END).round();
        *slot = (pivot, rect(left, top, width, home::PIVOT_HEIGHT));
        left += width + PIVOT_GAP;
    }
    Ok(rects)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Pivot(Pivot),
    Section(Section),
    Button(usize, RowButton),
}

pub fn target_at(text: &mut TextEngine, folder: &Folder, point: (f32, f32)) -> Option<Target> {
    let model = folder.grid.as_ref()?;
    let labels = model.home_labels.as_ref()?;
    let inside = |r: &D2D_RECT_F| {
        point.0 >= r.left && point.0 < r.right && point.1 >= r.top && point.1 < r.bottom
    };
    let top = model.home.pivots_top;
    let sections = [
        (
            folder.home.has_quick,
            Section::Quick,
            rect(
                home::MARGIN,
                QUICK_BUTTON_TOP,
                SECTION_BUTTON,
                SECTION_BUTTON,
            ),
        ),
        (
            true,
            Section::List,
            rect(home::MARGIN, top, SECTION_BUTTON, home::PIVOT_HEIGHT),
        ),
    ];
    if let Some((_, section, _)) = sections
        .iter()
        .find(|(shown, _, bounds)| *shown && inside(bounds))
    {
        return Some(Target::Section(*section));
    }
    if let Some((pivot, _)) = pivots(text, labels, top)
        .ok()?
        .into_iter()
        .find(|(_, bounds)| inside(bounds))
    {
        return Some(Target::Pivot(pivot));
    }
    let row = (folder.home.quick..model.layout.cells.len())
        .find(|&row| inside(&cell_rect(model.layout.cells[row])))?;
    home::row_button_at(model.layout.cells[row], model.home.list_columns[1], point)
        .map(|button| Target::Button(row, button))
}

fn cell_rect([x, y, width, height]: [f32; 4]) -> D2D_RECT_F {
    rect(x, y, width, height)
}

pub struct Surface<'a> {
    pub canvas: &'a Canvas,
    pub text: &'a mut TextEngine,
    pub theme: &'a Theme,
}

#[derive(Default)]
pub struct Illustration {
    document: Option<((bool, Pivot), ID2D1SvgDocument)>,
}

impl Illustration {
    fn draw(&mut self, canvas: &Canvas, kind: (bool, Pivot), origin: (f32, f32)) -> Result<()> {
        if self
            .document
            .as_ref()
            .is_none_or(|(shown, _)| *shown != kind)
        {
            self.document = Some((kind, empty_document(canvas, kind.1)?));
        }
        let Some((_, document)) = &self.document else {
            return Ok(());
        };
        let context: ID2D1DeviceContext5 = canvas.context.cast()?;
        shifted(canvas, origin, || {
            unsafe { context.DrawSvgDocument(document) };
            Ok(())
        })
    }
}

fn empty_document(canvas: &Canvas, pivot: Pivot) -> Result<ID2D1SvgDocument> {
    let path = crate::shell::icons::command_svg_path(match pivot {
        Pivot::Recent => "Home.EmptyStateNoRecent",
        Pivot::Favorites => "Home.EmptyStateNoFavorites",
    });
    let bytes = std::fs::read(path).map_err(|error| {
        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
    })?;
    unsafe {
        let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(&bytes))
            .ok_or_else(windows::core::Error::from_win32)?;
        let context: ID2D1DeviceContext5 = canvas.context.cast()?;
        context.CreateSvgDocument(
            &stream,
            D2D_SIZE_F {
                width: ILLUSTRATION,
                height: ILLUSTRATION,
            },
        )
    }
}

pub fn draw_page(folder: &Folder, surface: &mut Surface, art: &mut Illustration) -> Result<()> {
    let (canvas, theme) = (surface.canvas, surface.theme);
    let text = &mut *surface.text;
    let Some(model) = &folder.grid else {
        return Ok(());
    };
    let Some(labels) = model.home_labels.as_ref() else {
        return Ok(());
    };
    let mut chevron = |collapsed: bool, top: f32| {
        let glyph = if collapsed { COLLAPSED } else { EXPANDED };
        text.draw(canvas, glyph, CHEVRON, (CHEVRON_X, top), &theme.text)
    };
    let collapsed = folder.home.collapsed;
    let top = model.home.pivots_top;
    if folder.home.has_quick {
        chevron(collapsed.quick, 17.0)?;
    }
    chevron(collapsed.list, top + 9.0)?;
    if folder.home.has_quick {
        text.draw(canvas, &labels.quick, TITLE, (TITLE_X, 15.0), &theme.text)?;
    }
    let on_accent = if theme.content.r > 0.5 { WHITE } else { BLACK };
    for (pivot, bounds) in pivots(text, labels, top)? {
        let selected = pivot == folder.home.pivot;
        super::address::rounded(
            canvas,
            bounds,
            home::PIVOT_HEIGHT / 2.0,
            if selected {
                &theme.accent
            } else {
                &theme.control
            },
        );
        let color = if selected { &on_accent } else { &theme.text };
        let glyph = match pivot {
            Pivot::Recent => "\u{e823}",
            Pivot::Favorites => "\u{e734}",
        };
        text.draw(canvas, glyph, GLYPH, (bounds.left + 12.0, top + 8.0), color)?;
        text.draw(
            canvas,
            labels.pivot(pivot),
            BODY,
            (bounds.left + PIVOT_TEXT, top + 5.0),
            color,
        )?;
    }
    if folder.order.len() > folder.home.quick {
        let header = top + home::LIST_HEADER_BELOW_PIVOTS - 5.0;
        for (x, value) in model.home.list_columns.iter().zip(&labels.columns) {
            text.draw(canvas, value, BODY, (*x, header), &theme.text)?;
        }
        return Ok(());
    }
    if collapsed.list {
        return Ok(());
    }
    let center = home::MARGIN + (model.size.0 + 4.0) / 2.0;
    art.draw(
        canvas,
        (theme.content.r > 0.5, folder.home.pivot),
        (center - ILLUSTRATION / 2.0, top + 66.5),
    )?;
    let (title, detail) = match folder.home.pivot {
        Pivot::Recent => ("empty.recent", "empty.recent-detail"),
        Pivot::Favorites => ("empty.favorites", "empty.favorites-detail"),
    };
    let title = crate::i18n::tr(title);
    let width = text.layout(title, EMPTY_TITLE, 4096.0)?.advance;
    text.draw(
        canvas,
        title,
        EMPTY_TITLE,
        ((center - 1.0 - width / 2.0).round(), top + 214.0),
        &theme.text,
    )?;
    let detail = crate::i18n::tr(detail);
    for (index, line) in wrap(text, detail, EMPTY_TEXT_WIDTH)?.iter().enumerate() {
        let width = text.layout(line, CAPTION, 4096.0)?.advance;
        text.draw(
            canvas,
            line,
            CAPTION,
            (
                (center - 1.0 - width / 2.0).round(),
                top + 234.0 + index as f32 * 20.0,
            ),
            &theme.text,
        )?;
    }
    Ok(())
}

fn shifted(canvas: &Canvas, by: (f32, f32), draw: impl FnOnce() -> Result<()>) -> Result<()> {
    unsafe {
        let mut original = Default::default();
        canvas.context.GetTransform(&mut original);
        let mut moved = original;
        moved.M31 += by.0;
        moved.M32 += by.1;
        canvas.context.SetTransform(&moved);
        let result = draw();
        canvas.context.SetTransform(&original);
        result
    }
}

fn wrap(text: &mut TextEngine, value: &str, width: f32) -> Result<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    for word in value.split_whitespace() {
        let candidate = lines
            .last()
            .map_or_else(|| word.to_owned(), |line| format!("{line} {word}"));
        match lines.last_mut() {
            Some(line) if text.layout(&candidate, CAPTION, 4096.0)?.advance <= width => {
                *line = candidate
            }
            _ => lines.push(word.to_owned()),
        }
    }
    Ok(lines)
}

pub struct Item<'a> {
    pub name: &'a str,
    pub cells: Option<&'a [String]>,
    pub cell: [f32; 4],
    pub tile: bool,
    pub hover: bool,
    pub hover_button: Option<RowButton>,
    pub selected: bool,
}

impl Item<'_> {
    fn cell(&self, index: usize) -> &str {
        self.cells
            .and_then(|cells| cells.get(index))
            .map_or("", String::as_str)
    }

    pub fn picture(&self, size: (f32, f32)) -> D2D_RECT_F {
        let [x, y, _, _] = self.cell;
        let (top, side) = if self.tile { (5.0, 48.0) } else { (4.0, 32.0) };
        let scale = (side / size.0.max(size.1).max(1.0)).min(1.0);
        let (width, height) = ((size.0 * scale).round(), (size.1 * scale).round());
        rect(
            x + PICTURE_X + ((side - width) / 2.0).floor(),
            y + top + ((side - height) / 2.0).floor(),
            width,
            height,
        )
    }
}

pub fn draw_item(item: &Item, list_columns: [f32; 3], surface: &mut Surface) -> Result<()> {
    let theme = surface.theme;
    let [x, y, width, height] = item.cell;
    if item.selected || item.hover {
        super::address::rounded(
            surface.canvas,
            rect(x, y, width, height),
            4.0,
            &theme.subtle_hover,
        );
    }
    if item.selected {
        unsafe {
            surface.canvas.brush.SetColor(&theme.accent);
            surface.canvas.context.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(x + 1.0, y + 1.0, width - 2.0, height - 2.0),
                    radiusX: 3.0,
                    radiusY: 3.0,
                },
                &surface.canvas.brush,
                2.0,
                None,
            );
        }
    }
    surface.status(item)?;
    if item.tile {
        let left = x + TILE_TEXT;
        let span = TILE_TEXT_WIDTH.min(width - TILE_TEXT - CELL_GAP);
        surface.line(item.name, rect(left, y + 5.0, span, 16.0), &theme.text)?;
        surface.line(
            item.cell(0),
            rect(left, y + 21.0, span, 16.0),
            &theme.text_secondary,
        )?;
        if !item.cell(3).is_empty() {
            surface.text.draw(
                surface.canvas,
                "\u{e841}",
                PIN,
                (x + 140.0, y + 40.0),
                if item.selected {
                    &theme.text
                } else {
                    &theme.text_tertiary
                },
            )?;
        }
        return Ok(());
    }
    let [_, accessed, activity] = list_columns;
    let left = x + ROW_TEXT;
    let mut name_end = accessed - NAME_GAP;
    if item.hover || item.selected {
        let buttons = home::row_buttons(item.cell, accessed);
        name_end = buttons[0].1[0] - NAME_BEFORE_BUTTONS;
        for (button, bounds) in buttons {
            surface.row_button(button, bounds, item.hover_button == Some(button))?;
        }
    }
    let span = name_end - left;
    surface.line(item.name, rect(left, y + 4.0, span, 16.0), &theme.text)?;
    surface.line(
        item.cell(0),
        rect(left, y + 20.0, span, 16.0),
        &theme.text_secondary,
    )?;
    surface.line(
        item.cell(1),
        rect(accessed + 3.0, y, activity - accessed - CELL_GAP, height),
        &theme.text_secondary,
    )?;
    surface.line(
        item.cell(2),
        rect(
            activity + 2.0,
            y,
            x + width - activity - 2.0 - CELL_GAP,
            height,
        ),
        &theme.text_secondary,
    )
}

impl Surface<'_> {
    fn line(&mut self, value: &str, bounds: D2D_RECT_F, color: &D2D1_COLOR_F) -> Result<()> {
        details::draw_cell(self.canvas, self.text, value, CAPTION, bounds, 0.0, color)
    }

    fn status(&mut self, item: &Item) -> Result<()> {
        let Some((glyph, color)) = status_glyph(item.cell(4)) else {
            return Ok(());
        };
        let [x, y, _, height] = item.cell;
        self.text.draw(
            self.canvas,
            glyph,
            GLYPH,
            (x + STATUS_X, y + ((height - 16.0) / 2.0).round()),
            &color,
        )
    }

    fn row_button(
        &mut self,
        button: RowButton,
        [x, y, width, height]: [f32; 4],
        hover: bool,
    ) -> Result<()> {
        if hover {
            super::address::rounded(
                self.canvas,
                rect(x, y, width, height),
                4.0,
                &self.theme.subtle_hover,
            );
        }
        let glyph = match button {
            RowButton::OpenLocation => "\u{e838}",
            RowButton::More => "\u{e712}",
        };
        self.text.draw(
            self.canvas,
            glyph,
            GLYPH,
            (x + BUTTON_GLYPH, y + BUTTON_GLYPH),
            &self.theme.text,
        )
    }
}

fn status_glyph(state: &str) -> Option<(&'static str, D2D1_COLOR_F)> {
    match state {
        "1" => Some(("\u{e753}", CLOUD)),
        "2" => Some(("\u{e930}", SYNCED)),
        "3" => Some(("\u{ec61}", SYNCED)),
        "4" | "5" | "6" | "10" => Some(("\u{e895}", CLOUD)),
        _ => None,
    }
}

const CLOUD: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 120.0 / 255.0,
    b: 215.0 / 255.0,
    a: 1.0,
};
const SYNCED: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 24.0 / 255.0,
    g: 143.0 / 255.0,
    b: 42.0 / 255.0,
    a: 1.0,
};

fn rect(x: f32, y: f32, width: f32, height: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

const BLACK: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
const WHITE: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
