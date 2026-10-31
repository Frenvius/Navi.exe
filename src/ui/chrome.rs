use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

use super::details::fill;
use super::list::List;
use super::render::Canvas;
use super::text::{Family, TextEngine, TextStyle, DIRECTUI_KERNED};
use super::theme::Theme;

const LABEL: TextStyle = TextStyle::new(Family::Variable, 12.0, 400);
const CHEVRON: TextStyle = TextStyle::new(Family::Icons, 8.0, 400);
const MORE: TextStyle = TextStyle::new(Family::Icons, 20.0, 400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    New,
    Cut,
    Copy,
    Paste,
    Rename,
    Share,
    Delete,
    Sort,
    View,
    More,
    Pane,
    Collection,
    SearchOptions,
    CloseSearch,
    Filter,
    EmptyBin,
    ExtractAll,
    Wallpaper,
}

impl Command {
    pub fn menu(self) -> bool {
        matches!(
            self,
            Self::New
                | Self::Sort
                | Self::View
                | Self::More
                | Self::Collection
                | Self::SearchOptions
                | Self::Filter
        )
    }

    fn icon(self) -> Option<&'static str> {
        Some(match self {
            Self::New => "windows.newitem",
            Self::Cut => "windows.cut",
            Self::Copy => "windows.copy",
            Self::Paste => "windows.paste",
            Self::Rename => "windows.rename",
            Self::Share => "Windows.ModernShare",
            Self::Delete => "windows.ribbondelete",
            Self::Sort => "SortBy",
            Self::View => "windows.iconsize.details",
            Self::Pane => "windows.previewpane",
            Self::Collection => "windows.galleryfiltercommand",
            Self::SearchOptions => "SearchAdvancedOptions",
            Self::CloseSearch => "windows.searchclosetab",
            Self::Filter => "windows.ribbonfiltercommand",
            Self::EmptyBin => "windows.recyclebin.empty",
            Self::ExtractAll => "windows.compressedfile.extract",
            Self::Wallpaper => "windows.setdesktopwallpaper",
            Self::More => return None,
        })
    }

    fn label(self) -> &'static str {
        if self == Self::Collection {
            static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            return LABEL
                .get_or_init(|| crate::i18n::system_string("Windows.UI.FileExplorer.dll", 51856));
        }
        crate::i18n::tr(match self {
            Self::New => "cmd.new",
            Self::Sort => "cmd.sort",
            Self::View => "cmd.view",
            Self::Pane => "cmd.details",
            Self::SearchOptions => "cmd.search-options",
            Self::CloseSearch => "cmd.close-search",
            Self::Filter => "cmd.filter",
            Self::EmptyBin => "cmd.empty-bin",
            Self::ExtractAll => "cmd.extract-all",
            Self::Wallpaper => "cmd.background",
            _ => return "",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Command(Command),
    Status(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    List,
    Header(usize),
    Pane(super::panes::Button),
    Settings(super::settings::Target),
    Command(Command),
    Status(usize),
    Tab,
    NewTab,
    Neutral,
    Tree,
    Address(super::address::Target),
}

pub struct Button {
    pub command: Command,
    pub left: f32,
    pub width: f32,
    label_width: f32,
}

#[derive(Default)]
pub struct State {
    pub focus: Focus,
    pub cues: bool,
    active: bool,
    alt_entry: bool,
    pub hover: Option<Target>,
    pub pressed: Option<Target>,
    pub can_paste: bool,
    pub pane_kind: Option<crate::app::commands::Pane>,
    pub preview: bool,
    pub open_menu: Option<Command>,
    pub enabled: [bool; 18],
    pub buttons: Vec<Button>,
    pub size: (f32, f32),
    pub status_visible: bool,
    pub status: [String; 3],
    keyboard_press: Option<u32>,
    snapshot: Option<(usize, usize, Option<u64>)>,
    separators: Vec<f32>,
    observed_folder: std::sync::Weak<super::list::Folder>,
    observed_selection: std::collections::HashSet<String>,
    observed_path: String,
    observed_paste: bool,
    mode: i32,
    gallery: bool,
    search: bool,
    home: bool,
    recycle: bool,
    extract: bool,
    wallpaper: bool,
}

impl State {
    pub fn asset_names(&self) -> Vec<&'static str> {
        let mut names = super::svg::names(
            self.mode,
            self.gallery.then_some("windows.galleryfiltercommand"),
        );
        if self.preview {
            names.push("windows.readingpane");
        }
        if self.search {
            names.extend(["SearchAdvancedOptions", "windows.searchclosetab"]);
        }
        for command in [
            Command::Filter,
            Command::EmptyBin,
            Command::ExtractAll,
            Command::Wallpaper,
        ] {
            if self.buttons.iter().any(|button| button.command == command)
                || match command {
                    Command::Filter => self.home,
                    Command::EmptyBin => self.recycle,
                    Command::ExtractAll => self.extract,
                    Command::Wallpaper => self.wallpaper,
                    _ => false,
                }
            {
                if let Some(icon) = command.icon() {
                    names.push(icon);
                }
            }
        }
        names
    }
    pub fn sync(&mut self, list: &List, path: &str) -> (bool, bool) {
        let activation = self.active != list.interaction.active;
        self.active = list.interaction.active;
        let same_folder = match (self.observed_folder.upgrade(), list.folder.as_ref()) {
            (Some(previous), Some(current)) => std::sync::Arc::ptr_eq(&previous, current),
            (None, None) => true,
            _ => false,
        };
        if same_folder
            && self.observed_selection == list.interaction.selection.selected
            && self.observed_path == path
            && self.observed_paste == self.can_paste
        {
            return (activation, activation);
        }
        self.observed_folder = list
            .folder
            .as_ref()
            .map(std::sync::Arc::downgrade)
            .unwrap_or_default();
        self.observed_selection
            .clone_from(&list.interaction.selection.selected);
        self.observed_path.clear();
        self.observed_path.push_str(path);
        self.observed_paste = self.can_paste;
        let picked = list
            .folder
            .as_ref()
            .map(|folder| {
                folder
                    .order
                    .iter()
                    .map(|&index| &folder.entries[index])
                    .filter(|entry| list.interaction.selection.selected.contains(&entry.path))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let filesystem = (!crate::app::ids::is_virtual(path) || crate::app::ids::is_library(path))
            && !path.is_empty();
        let selection = !picked.is_empty()
            && picked
                .iter()
                .all(|entry| entry.is_fs || crate::shell::archive::root(&entry.path).is_some());
        let home = crate::app::ids::same(path, crate::app::ids::HOME);
        let recycle = crate::app::ids::same(path, crate::app::ids::RECYCLE_BIN);
        let extract = crate::shell::archive::root(path).is_some()
            || picked.len() == 1
                && picked[0].is_fs
                && crate::shell::archive::root(&picked[0].path).is_some();
        let wallpaper = picked.len() == 1
            && picked[0].is_fs
            && !picked[0].is_dir
            && crate::shell::preview::is_image(&picked[0].path);
        let enabled = [
            filesystem,
            selection && picked.iter().all(|entry| entry.can_move),
            selection && picked.iter().all(|entry| entry.can_copy),
            filesystem && self.can_paste,
            selection && picked.iter().all(|entry| entry.can_rename),
            selection && picked.iter().all(|entry| !entry.is_dir),
            selection && picked.iter().all(|entry| entry.can_delete),
            !crate::app::ids::same(path, crate::app::ids::HOME)
                && !crate::app::ids::same(path, crate::app::ids::GALLERY),
            true,
            true,
            true,
            true,
            true,
            true,
            home,
            recycle
                && list
                    .folder
                    .as_ref()
                    .is_some_and(|folder| !folder.entries.is_empty()),
            extract,
            wallpaper,
        ];
        let gallery = crate::app::ids::same(path, crate::app::ids::GALLERY);
        let search = crate::app::ids::is_search(path);
        let preview = list.folder.as_ref().map_or(self.preview, |folder| {
            if folder.panes.1 {
                true
            } else if folder.panes.0 {
                false
            } else {
                self.preview
            }
        });
        let contextual_changed = self.home != home
            || self.recycle != recycle
            || self.extract != extract
            || self.wallpaper != wallpaper;
        self.home = home;
        self.recycle = recycle;
        self.extract = extract;
        self.wallpaper = wallpaper;
        let commands_changed = contextual_changed
            || self.preview != preview
            || self.enabled != enabled
            || activation
            || self.gallery != gallery
            || self.search != search;
        if contextual_changed
            || self.gallery != gallery
            || self.search != search
            || self.preview != preview
        {
            self.buttons.clear();
        }
        self.preview = preview;
        self.gallery = gallery;
        self.search = search;
        self.enabled = enabled;
        if let Focus::Command(command) = self.focus {
            if !enabled[command as usize] {
                self.focus = Focus::Command(Command::More);
            }
        }
        let visible = list
            .folder
            .as_ref()
            .is_some_and(|folder| folder.settings.status_bar);
        let snapshot = list.folder.as_ref().map(|folder| {
            (
                folder.item_count(),
                picked.len(),
                (!picked.is_empty() && picked.iter().all(|entry| !entry.is_dir))
                    .then(|| picked.iter().map(|entry| entry.size).sum()),
            )
        });
        let mode = list.folder.as_ref().map_or(6, |folder| folder.view.mode);
        let status_changed = self.snapshot != snapshot
            || self.status_visible != visible
            || activation
            || self.mode != mode;
        self.mode = mode;
        self.status_visible = visible;
        if self.snapshot != snapshot {
            self.snapshot = snapshot;
            self.status = snapshot
                .map(|(count, selected, size)| {
                    [
                        crate::app::format::count(count, "count.item", "count.items"),
                        if selected == 0 {
                            String::new()
                        } else {
                            crate::app::format::count(
                                selected,
                                "count.selected",
                                "count.selected-many",
                            )
                        },
                        size.map(crate::app::format::short_size).unwrap_or_default(),
                    ]
                })
                .unwrap_or_default();
        }
        (commands_changed, status_changed)
    }

    pub fn layout(&mut self, size: (f32, f32), text: &mut TextEngine) -> Result<()> {
        if self.size == size && !self.buttons.is_empty() {
            return Ok(());
        }
        self.size = size;
        self.buttons.clear();
        self.separators.clear();
        let mut left = 6.0;
        self.add(Command::New, &mut left, text)?;
        self.separators.push(left + 4.0);
        left += 11.0;
        for command in [
            Command::Cut,
            Command::Copy,
            Command::Paste,
            Command::Rename,
            Command::Share,
            Command::Delete,
        ] {
            self.add(command, &mut left, text)?;
            left += 8.0;
        }
        left -= 2.0;
        self.separators.push(left);
        left += 5.0;
        self.add(Command::Sort, &mut left, text)?;
        let pane_width = 48.0
            + text
                .layout(self.label(Command::Pane), LABEL, 4096.0)?
                .advance
                .ceil();
        let pane_left = size.0 - 10.0 - pane_width;
        let view_width = 61.0
            + text
                .layout(Command::View.label(), LABEL, 4096.0)?
                .advance
                .ceil();
        if left + 4.0 + view_width + 7.0 + 48.0 + 14.0 <= pane_left {
            left += 4.0;
            self.add(Command::View, &mut left, text)?;
            if !self.gallery && !self.home && !self.recycle {
                self.separators.push(left + 4.0);
                left += 7.0;
            }
        } else {
            left += 2.0;
        }
        if self.gallery {
            left += 4.0;
            self.add(Command::Collection, &mut left, text)?;
            left += 8.0;
        }
        if self.home {
            left += 4.0;
            self.add(Command::Filter, &mut left, text)?;
            self.separators.push(left + 4.0);
            left += 7.0;
        }
        for command in [Command::EmptyBin, Command::ExtractAll, Command::Wallpaper] {
            if match command {
                Command::EmptyBin => self.recycle,
                Command::ExtractAll => self.extract,
                Command::Wallpaper => self.wallpaper,
                _ => false,
            } {
                let width = 48.0 + text.layout(command.label(), LABEL, 4096.0)?.advance.ceil();
                if left + 8.0 + width + 48.0 <= pane_left {
                    let inset = if command == Command::Wallpaper {
                        4.0
                    } else {
                        6.0
                    };
                    left += inset;
                    self.add(command, &mut left, text)?;
                    left += 8.0 - inset;
                }
            }
        }
        let search_width = if self.search {
            61.0 + text
                .layout(Command::SearchOptions.label(), LABEL, 4096.0)?
                .advance
                .ceil()
        } else {
            0.0
        };
        if self.search && left + 4.0 + search_width + 4.0 + 48.0 + 14.0 <= pane_left {
            left += 4.0;
            self.add(Command::SearchOptions, &mut left, text)?;
            left += 2.0;
            let close_width = 46.0
                + text
                    .layout(Command::CloseSearch.label(), LABEL, 4096.0)?
                    .advance
                    .ceil();
            let need = left + 6.0 + close_width + 4.0 + 48.0;
            if pane_left - need >= 11.0 {
                left += 6.0;
                self.add(Command::CloseSearch, &mut left, text)?;
                left += 4.0;
                if pane_left - need >= 17.0 {
                    self.separators.push(left + 4.0);
                    left += 7.0;
                } else {
                    left += 2.0;
                }
            } else {
                left += 2.0;
            }
        }
        self.add(Command::More, &mut left, text)?;
        left = pane_left;
        self.add(Command::Pane, &mut left, text)?;
        if let Focus::Command(command) = self.focus {
            if !self.buttons.iter().any(|button| button.command == command) {
                self.focus = Focus::Command(Command::More);
            }
        }
        Ok(())
    }

    fn add(&mut self, command: Command, left: &mut f32, text: &mut TextEngine) -> Result<()> {
        let label_width = if self.label(command).is_empty() {
            0.0
        } else {
            text.layout(self.label(command), LABEL, 4096.0)?
                .advance
                .ceil()
        };
        let width = match command {
            Command::New
            | Command::Sort
            | Command::View
            | Command::Collection
            | Command::Filter => 61.0 + label_width,
            Command::SearchOptions => 61.0 + label_width,
            Command::Pane => 48.0 + label_width,
            Command::CloseSearch => 46.0 + label_width,
            Command::EmptyBin | Command::ExtractAll | Command::Wallpaper => 48.0 + label_width,
            Command::More => 48.0,
            _ => 40.0,
        };
        self.buttons.push(Button {
            command,
            left: *left,
            width,
            label_width,
        });
        *left += width;
        Ok(())
    }

    fn label(&self, command: Command) -> &'static str {
        if command == Command::Pane && self.preview {
            crate::i18n::tr("cmd.preview")
        } else {
            command.label()
        }
    }

    pub fn hit(&self, point: (f32, f32)) -> Option<Target> {
        if super::details::top() == 0.0 && point.1 < self.size.1 - 24.0 {
            return None;
        }
        if (89.0..135.0).contains(&point.1) {
            return self
                .buttons
                .iter()
                .find(|button| {
                    (button.left..button.left + button.width).contains(&point.0)
                        && self.enabled[button.command as usize]
                })
                .map(|button| Target::Command(button.command));
        }
        if self.status_visible && (self.size.1 - 24.0..self.size.1).contains(&point.1) {
            for index in 0..2 {
                let left = self.size.0 - 52.0 + index as f32 * 24.0;
                if (left..left + 24.0).contains(&point.0) {
                    return Some(Target::Status(index));
                }
            }
        }
        None
    }

    pub fn has_command(&self, command: Command) -> bool {
        self.buttons.iter().any(|button| button.command == command)
    }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}

fn rounded(canvas: &Canvas, bounds: D2D_RECT_F, radius: f32, color: &D2D1_COLOR_F) {
    unsafe {
        canvas
            .context
            .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        canvas.brush.SetColor(color);
        canvas.context.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bounds,
                radiusX: radius,
                radiusY: radius,
            },
            &canvas.brush,
        );
    }
}

fn outline(canvas: &Canvas, bounds: D2D_RECT_F, color: &D2D1_COLOR_F) {
    fill(
        canvas,
        rect(bounds.left, bounds.top, bounds.right, bounds.top + 1.0),
        color,
    );
    fill(
        canvas,
        rect(
            bounds.left,
            bounds.bottom - 1.0,
            bounds.right,
            bounds.bottom,
        ),
        color,
    );
    fill(
        canvas,
        rect(
            bounds.left,
            bounds.top + 1.0,
            bounds.left + 1.0,
            bounds.bottom - 1.0,
        ),
        color,
    );
    fill(
        canvas,
        rect(
            bounds.right - 1.0,
            bounds.top + 1.0,
            bounds.right,
            bounds.bottom - 1.0,
        ),
        color,
    );
}

pub fn focus_ring(canvas: &Canvas, bounds: D2D_RECT_F, theme: &Theme) {
    focus_ring_radius(canvas, bounds, theme, 7.0);
}

pub fn focus_ring_radius(canvas: &Canvas, bounds: D2D_RECT_F, theme: &Theme, radius: f32) {
    unsafe {
        canvas
            .context
            .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        canvas.brush.SetColor(&theme.focus_outer);
        canvas.context.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect(
                    bounds.left + 1.0,
                    bounds.top + 1.0,
                    bounds.right - 1.0,
                    bounds.bottom - 1.0,
                ),
                radiusX: radius,
                radiusY: radius,
            },
            &canvas.brush,
            2.0,
            None,
        );
        canvas.brush.SetColor(&theme.focus_inner);
        canvas.context.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect(
                    bounds.left + 2.5,
                    bounds.top + 2.5,
                    bounds.right - 2.5,
                    bounds.bottom - 2.5,
                ),
                radiusX: (radius - 1.5).max(0.0),
                radiusY: (radius - 1.5).max(0.0),
            },
            &canvas.brush,
            1.0,
            None,
        );
    }
}

pub fn draw_commands(
    state: &State,
    assets: &mut super::svg::Assets,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
) -> Result<()> {
    fill(canvas, rect(0.0, 1.0, state.size.0, 47.0), &theme.content);
    fill(canvas, rect(0.0, 0.0, state.size.0, 1.0), &theme.line);
    fill(canvas, rect(0.0, 47.0, state.size.0, 48.0), &theme.line);
    for &left in &state.separators {
        fill(
            canvas,
            rect(left, 9.0, left + 1.0, 41.0),
            &theme.command_divider,
        );
    }
    for button in &state.buttons {
        let target = Some(Target::Command(button.command));
        let held =
            state.pressed == target && (state.hover == target || state.keyboard_press.is_some());
        let enabled = state.enabled[button.command as usize];
        let inset = if button.command == Command::More {
            4.0
        } else {
            2.0
        };
        if enabled
            && (held
                || state.open_menu == Some(button.command)
                || (state.focus != Focus::Command(button.command) && state.hover == target))
        {
            rounded(
                canvas,
                rect(
                    button.left + inset,
                    7.0,
                    button.left + button.width - inset,
                    43.0,
                ),
                4.0,
                if held {
                    &theme.subtle_pressed
                } else {
                    &theme.subtle_hover
                },
            );
        }
        if state.active && state.cues && state.focus == Focus::Command(button.command) {
            let offset = if button.command == Command::More {
                -2.0
            } else {
                0.0
            };
            focus_ring(
                canvas,
                rect(
                    button.left + inset - 3.0 + offset,
                    4.0,
                    button.left + button.width - inset + 3.0 + offset,
                    46.0,
                ),
                theme,
            );
        }
        if let Some(icon) = if button.command == Command::View {
            crate::app::VIEW_MODES
                .iter()
                .find(|mode| mode.3 == state.mode)
                .map(|mode| mode.2)
        } else if button.command == Command::Pane && state.preview {
            Some("windows.readingpane")
        } else {
            button.command.icon()
        } {
            assets.icon(
                canvas,
                icon,
                (button.left + 12.0, 17.0),
                if enabled { 1.0 } else { 0.36 },
            )?;
        }
        let color = if enabled {
            if held {
                &theme.text_secondary
            } else {
                &theme.text
            }
        } else {
            &theme.text_disabled
        };
        let label = state.label(button.command);
        if !label.is_empty() {
            let style = LABEL;
            let layout = text.layout(label, style, 4096.0)?;
            text.draw(
                canvas,
                label,
                style,
                (button.left + 36.0, 2.0 + (46.0 - layout.height) / 2.0),
                color,
            )?;
            if button.command.menu() {
                let chevron = text.layout("\u{e972}", CHEVRON, 4096.0)?;
                text.draw(
                    canvas,
                    "\u{e972}",
                    CHEVRON,
                    (
                        button.left + 36.0 + button.label_width + 5.0 + (8.0 - chevron.width) / 2.0,
                        2.0 + (46.0 - chevron.height) / 2.0,
                    ),
                    if !enabled {
                        &theme.text_disabled
                    } else if held {
                        &theme.text_tertiary
                    } else {
                        &theme.text_secondary
                    },
                )?;
            }
        } else if button.command == Command::More {
            let dots = text.layout("\u{e712}", MORE, 4096.0)?;
            unsafe {
                canvas.context.PushAxisAlignedClip(
                    &rect(button.left, 24.0, button.left + button.width, 27.0),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
            }
            let result = text.draw(
                canvas,
                "\u{e712}",
                MORE,
                (button.left + 12.0, 3.0 + (46.0 - dots.height) / 2.0),
                color,
            );
            unsafe {
                canvas.context.PopAxisAlignedClip();
            }
            result?;
        }
    }
    Ok(())
}

pub fn draw_status(
    state: &State,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
) -> Result<()> {
    if !state.status_visible {
        return Ok(());
    }
    let mut left = 14.0;
    for (index, value) in state.status.iter().take(2).enumerate() {
        if value.is_empty() {
            continue;
        }
        if index > 0 {
            left += 10.0;
        }
        text.draw(
            canvas,
            value,
            DIRECTUI_KERNED,
            (left, 4.0),
            &theme.status_text,
        )?;
        left += text.layout(value, DIRECTUI_KERNED, 4096.0)?.width;
        if index == 1 && !state.status[2].is_empty() {
            left += 6.0;
            text.draw(
                canvas,
                &state.status[2],
                DIRECTUI_KERNED,
                (left, 4.0),
                &theme.status_text,
            )?;
            left += text
                .layout(&state.status[2], DIRECTUI_KERNED, 4096.0)?
                .width;
        }
        left += 10.0;
        fill(
            canvas,
            rect(left, 6.0, left + 1.0, 18.0),
            &theme.status_divider,
        );
        left += 1.0;
    }
    for index in 0..2 {
        draw_toggle(state, canvas, theme, index);
    }
    Ok(())
}

fn draw_toggle(state: &State, canvas: &Canvas, theme: &Theme, index: usize) {
    let left = state.size.0 - 52.0 + index as f32 * 24.0;
    let focused = state.active && state.focus == Focus::Status(index);
    let on = if index == 0 {
        state.mode == 6
    } else {
        state.mode == 2
    };
    let hovered = state.hover == Some(Target::Status(index));
    if focused || on {
        fill(
            canvas,
            rect(left, 0.0, left + 24.0, 24.0),
            if focused {
                &theme.toggle_focus
            } else {
                &theme.toggle_on
            },
        );
    }
    if focused || on || hovered {
        outline(
            canvas,
            rect(left, 0.0, left + 24.0, 24.0),
            if focused || !on {
                &theme.toggle_hover_border
            } else {
                &theme.toggle_on_border
            },
        );
    }
    let offset = if on { 1.0 } else { 0.0 };
    let plate = if on {
        &theme.status_icon_active
    } else {
        &theme.status_icon_plate
    };
    if plate.a > 0.0 {
        rounded(
            canvas,
            rect(
                left + 4.0 + offset,
                4.0 + offset,
                left + 20.0 + offset,
                20.0 + offset,
            ),
            2.0,
            plate,
        );
    }
    let color = theme.status_icon_stroke;
    let x = left + 6.0 + offset;
    let y = 6.0 + offset;
    let line = |dy| {
        fill(
            canvas,
            rect(x + 1.0, y + dy, x + 11.0, y + dy + 1.0),
            &color,
        );
        let edge = D2D1_COLOR_F { a: 0.875, ..color };
        fill(canvas, rect(x, y + dy, x + 1.0, y + dy + 1.0), &edge);
        fill(
            canvas,
            rect(x + 11.0, y + dy, x + 12.0, y + dy + 1.0),
            &edge,
        );
    };
    if index == 0 {
        for dy in [1.0, 4.0, 7.0, 10.0] {
            line(dy);
        }
    } else {
        fill(
            canvas,
            rect(x + 1.0, y + 1.0, x + 11.0, y + 9.0),
            &theme.status_icon_picture,
        );
        line(0.0);
        line(9.0);
        line(11.0);
        fill(canvas, rect(x, y + 1.0, x + 1.0, y + 9.0), &color);
        fill(canvas, rect(x + 11.0, y + 1.0, x + 12.0, y + 9.0), &color);
    }
}

pub(super) fn invalidate(hwnd: windows::Win32::Foundation::HWND, damage: super::render::Damage) {
    if !(damage.commands
        || damage.status
        || damage.header
        || damage.title
        || damage.view
        || damage.pane)
    {
        return;
    }
    super::DAMAGE.with(|pending| pending.set(pending.get().merge(damage)));
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
    }
}

fn target_damage(target: Option<Target>) -> super::render::Damage {
    super::render::Damage {
        commands: matches!(target, Some(Target::Command(_))),
        status: matches!(target, Some(Target::Status(_))),
        ..super::render::Damage::NONE
    }
}

pub fn sync(hwnd: windows::Win32::Foundation::HWND) {
    let (damage, focus) = super::WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or((super::render::Damage::NONE, None), |window| {
                let previous = window.chrome.focus;
                let (commands, status) = window
                    .chrome
                    .sync(&window.list, window.tabs.active().location.tab.path());
                let columns = window
                    .list
                    .folder
                    .as_ref()
                    .map_or(0, |folder| folder.columns.len());
                match window.chrome.focus {
                    Focus::Status(_) if !window.chrome.status_visible => {
                        window.chrome.focus = Focus::List
                    }
                    Focus::Header(index) if index >= columns => {
                        window.chrome.focus = if columns == 0 {
                            Focus::List
                        } else {
                            Focus::Header(0)
                        }
                    }
                    _ => {}
                }
                let header = if window.chrome.active && window.chrome.cues {
                    if let Focus::Header(index) = window.chrome.focus {
                        Some(index)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let title = if window.chrome.active && window.chrome.cues {
                    match window.chrome.focus {
                        Focus::Tab => super::titlebar::Target::Tab(window.tabs.current),
                        Focus::NewTab => super::titlebar::Target::Plus,
                        _ => super::titlebar::Target::None,
                    }
                } else {
                    super::titlebar::Target::None
                };
                let header_changed = window.list.header.focus != header;
                let title_changed = window.title.focus != title;
                window.list.header.focus = header;
                window.title.focus = title;
                (
                    super::render::Damage {
                        commands,
                        status,
                        header: header_changed,
                        title: title_changed,
                        ..super::render::Damage::NONE
                    },
                    (previous != window.chrome.focus).then_some(window.chrome.focus),
                )
            })
    });
    invalidate(hwnd, damage);
    if let Some(focus) = focus {
        cancel(hwnd);
        set_focus(hwnd, focus);
    }
}

pub fn clipboard(hwnd: windows::Win32::Foundation::HWND) {
    let can_paste = crate::shell::ops::can_paste();
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.chrome.can_paste = can_paste;
        }
    });
    sync(hwnd);
}

pub fn moved(hwnd: windows::Win32::Foundation::HWND, point: (f32, f32)) {
    let damage = super::WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or(super::render::Damage::NONE, |window| {
                let target = window.chrome.hit(point);
                let previous = window.chrome.hover;
                if target == previous {
                    return super::render::Damage::NONE;
                }
                window.chrome.hover = target;
                target_damage(previous).merge(target_damage(target))
            })
    });
    invalidate(hwnd, damage);
}

pub fn leave(hwnd: windows::Win32::Foundation::HWND) {
    moved(hwnd, (-1.0, -1.0));
}

pub fn cancel(hwnd: windows::Win32::Foundation::HWND) {
    let damage = super::WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or(super::render::Damage::NONE, |window| {
                window.chrome.keyboard_press = None;
                target_damage(window.chrome.pressed.take())
            })
    });
    invalidate(hwnd, damage);
}

pub fn press(hwnd: windows::Win32::Foundation::HWND, point: (f32, f32)) -> bool {
    let cues = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let cues = std::mem::take(&mut window.chrome.cues);
            if cues {
                window.title.focus = super::titlebar::Target::None;
                window.list.header.focus = None;
            }
            cues
        })
    });
    if cues {
        invalidate(
            hwnd,
            super::render::Damage {
                commands: true,
                status: true,
                pane: true,
                title: true,
                header: true,
                ..super::render::Damage::NONE
            },
        );
    }
    let target = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|window| {
            let target = window.chrome.hit(point)?;
            window.chrome.hover = Some(target);
            window.chrome.pressed = Some(target);
            Some(target)
        })
    });
    if let Some(Target::Status(index)) = target {
        super::WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.chrome.focus = Focus::Status(index);
                window.chrome.alt_entry = false;
                if window.list.interaction.has_focus {
                    window.list.interaction.has_focus = false;
                    window.list.interaction.mark_all();
                }
            }
        });
        publish_focus(hwnd, Focus::Status(index));
        super::input::flush(hwnd);
    }
    if target.is_none() {
        return false;
    }
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
    }
    invalidate(hwnd, target_damage(target));
    true
}

pub fn release(hwnd: windows::Win32::Foundation::HWND, point: (f32, f32)) -> bool {
    let pressed = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().and_then(|window| {
            let over = window.chrome.hit(point);
            window.chrome.hover = over;
            window.chrome.pressed.take().map(|pressed| (pressed, over))
        })
    });
    let Some((pressed, over)) = pressed else {
        return false;
    };
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    invalidate(hwnd, target_damage(Some(pressed)));
    if over == Some(pressed) {
        activate(hwnd, pressed);
    }
    true
}

fn activate(hwnd: windows::Win32::Foundation::HWND, target: Target) {
    use crate::app::commands::FileAction;
    let action = match target {
        Target::Command(Command::Cut) => FileAction::Cut,
        Target::Command(Command::Copy) => FileAction::Copy,
        Target::Command(Command::Paste) => FileAction::Paste,
        Target::Command(Command::Delete) => FileAction::Delete,
        Target::Command(Command::Share) => FileAction::Share,
        Target::Command(Command::Collection) => {
            super::grid::collection_menu(hwnd);
            return;
        }
        Target::Status(index) => {
            let mode = if index == 0 { 6 } else { 2 };
            super::grid::set_mode(hwnd, mode, crate::app::grid_layout::mode_icon_size(mode));
            return;
        }
        Target::Command(
            command @ (Command::New
            | Command::Sort
            | Command::View
            | Command::More
            | Command::SearchOptions
            | Command::Filter),
        ) => {
            let origin = super::WINDOW.with(|slot| {
                if slot.borrow().as_ref().is_some_and(|window| {
                    window.chrome.cues && window.chrome.focus == Focus::Command(command)
                }) {
                    super::menu::Origin::Keyboard
                } else {
                    super::menu::Origin::Pointer
                }
            });
            super::menu_actions::command(hwnd, command, origin);
            return;
        }
        Target::Command(
            command @ (Command::EmptyBin | Command::ExtractAll | Command::Wallpaper),
        ) => {
            let Some(context) = super::menu_actions::context(hwnd) else {
                return;
            };
            match command {
                Command::EmptyBin => {
                    crate::shell::ops::empty_recycle_bin(hwnd);
                    super::address_input::activate(hwnd, super::address::Target::Refresh);
                }
                Command::Wallpaper => {
                    let paths = context
                        .selected
                        .iter()
                        .map(|entry| entry.path.clone())
                        .collect();
                    super::menu_actions::run(
                        hwnd,
                        context,
                        crate::app::menu_model::Action::Wallpaper(paths),
                    );
                }
                Command::ExtractAll => {
                    let paths = crate::shell::archive::root(&context.path).map_or_else(
                        || {
                            context
                                .selected
                                .iter()
                                .map(|entry| entry.path.clone())
                                .collect()
                        },
                        |root| vec![root.to_owned()],
                    );
                    super::menu_actions::run(
                        hwnd,
                        context,
                        crate::app::menu_model::Action::ShellVerb("extract".into(), paths),
                    );
                }
                _ => {}
            }
            return;
        }
        Target::Command(Command::Rename) => {
            if let Some(context) = super::menu_actions::context(hwnd) {
                super::menu_actions::run(hwnd, context, crate::app::menu_model::Action::Rename);
            }
            return;
        }
        Target::Command(Command::CloseSearch) => {
            if let Err(error) = super::search::close(hwnd) {
                super::fail(error);
            }
            return;
        }
        Target::Command(Command::Pane) => {
            let pane = super::WINDOW
                .with(|slot| {
                    slot.borrow().as_ref().map(|window| {
                        if window.chrome.preview {
                            crate::app::commands::Pane::Preview
                        } else {
                            crate::app::commands::Pane::Details
                        }
                    })
                })
                .unwrap_or(crate::app::commands::Pane::Details);
            crate::app::commands::toggle_pane(pane);
            if super::input::update_settings() {
                super::invalidate_all(hwnd);
            }
            return;
        }
    };
    run_file(hwnd, action);
}

fn run_file(hwnd: windows::Win32::Foundation::HWND, action: crate::app::commands::FileAction) {
    let target = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|window| {
            (
                window.tabs.active().location.tab.path().to_owned(),
                window
                    .list
                    .folder
                    .as_ref()
                    .map(|folder| {
                        folder
                            .order
                            .iter()
                            .map(|&index| &folder.entries[index])
                            .filter(|entry| {
                                window
                                    .list
                                    .interaction
                                    .selection
                                    .selected
                                    .contains(&entry.path)
                            })
                            .map(|entry| entry.path.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
        })
    });
    if let Some((folder, selected)) = target {
        if let Err(error) = super::menu_actions::file(hwnd, &folder, &selected, action) {
            eprintln!("native command: {error}");
        }
    }
}

pub(super) fn set_focus(hwnd: windows::Win32::Foundation::HWND, focus: Focus) {
    apply_focus(hwnd, focus, super::address_input::Entry::Direct);
}

fn apply_focus(
    hwnd: windows::Win32::Foundation::HWND,
    focus: Focus,
    entry: super::address_input::Entry,
) {
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.chrome.focus = focus;
            if matches!(focus, Focus::Tab | Focus::NewTab) {
                window.title.cancelled_drag = false;
            }
            if !matches!(focus, Focus::Command(_)) {
                window.chrome.alt_entry = false;
            }
            window.chrome.cues = true;
            window.list.header.focus = if let Focus::Header(index) = focus {
                Some(index)
            } else {
                None
            };
            window.title.focus = match focus {
                Focus::Tab => super::titlebar::Target::Tab(window.tabs.current),
                Focus::NewTab => super::titlebar::Target::Plus,
                _ => super::titlebar::Target::None,
            };
            if let (Focus::Header(index), Some(folder), Some(view)) =
                (focus, window.list.folder.as_ref(), window.scroll.view)
            {
                let left = folder
                    .columns
                    .iter()
                    .take(index)
                    .map(|column| column.width)
                    .sum::<f32>();
                let right = left + folder.columns[index].width;
                if left < window.scroll.x {
                    window.scroll.x = left;
                } else if right > window.scroll.x + view.right {
                    window.scroll.x = (right - view.right).min(view.maximum_x());
                }
            }
            window.panes.focus = match focus {
                Focus::Pane(button) => Some(button),
                _ => None,
            };
            window.list.interaction.has_focus = focus == Focus::List;
            window.list.interaction.cues = true;
            window.list.interaction.mark_all();
        }
    });
    super::address_input::focus_changed(hwnd, focus, entry);
    super::navigation_input::focus_changed(hwnd);
    publish_focus(hwnd, focus);
    invalidate(
        hwnd,
        super::render::Damage {
            commands: true,
            status: true,
            pane: true,
            title: true,
            header: true,
            view: true,
            ..super::render::Damage::NONE
        },
    );
    super::input::flush(hwnd);
    if focus == Focus::List {
        super::hosted::focus(hwnd);
    }
}

pub(super) fn publish_focus(hwnd: windows::Win32::Foundation::HWND, focus: Focus) {
    let zone = match focus {
        Focus::Pane(super::panes::Button::Share) => 15,
        Focus::Pane(super::panes::Button::Properties) => 16,
        Focus::Settings(super::settings::Target::Section) => 19,
        Focus::Settings(super::settings::Target::Toggle) => 20,
        Focus::Settings(super::settings::Target::Restore) => 21,
        Focus::Settings(super::settings::Target::ZoxideInstall) => 28,
        Focus::Settings(super::settings::Target::Zoxide(setting)) => 26 + setting as usize,
        Focus::Settings(super::settings::Target::OpenIn(source, destination)) => {
            22 + source as usize * 2 + destination as usize
        }
        Focus::List => 0,
        Focus::Tree => 14,
        Focus::Header(_) => 1,
        Focus::Status(_) => 2,
        Focus::Tab => 3,
        Focus::NewTab => 4,
        Focus::Command(Command::Pane) => 13,
        Focus::Command(_) => 12,
        Focus::Neutral => 17,
        Focus::Address(target) => match target {
            super::address::Target::Back => 5,
            super::address::Target::Forward => 6,
            super::address::Target::Up => 7,
            super::address::Target::Refresh => 8,
            super::address::Target::Address => 9,
            super::address::Target::Search => 11,
            super::address::Target::Settings => 18,
            _ => 10,
        },
    };
    crate::frame::focus_debug(hwnd, zone);
    if std::env::var_os("FOCUS_TRACE").is_none() {
        return;
    }
    let item = match focus {
        Focus::Header(index) | Focus::Status(index) => index,
        Focus::Command(command) => command as usize,
        _ => 0,
    };
    if let Err(error) = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetPropW(
            hwnd,
            windows::core::w!("NativeFocusItem"),
            Some(windows::Win32::Foundation::HANDLE((item + 1) as _)),
        )
    } {
        super::fail(error);
    }
}

pub fn key_up(hwnd: windows::Win32::Foundation::HWND, key: u32) -> bool {
    let pressed = super::WINDOW.with(|slot| slot.borrow_mut().as_mut().and_then(|window| {
        if window.chrome.keyboard_press != Some(key) {return None;}
        window.chrome.keyboard_press = None;
        let target = window.chrome.pressed.take();
        let active = target.is_some_and(|target|matches!(target,Target::Command(command) if window.chrome.focus == Focus::Command(command) && window.chrome.enabled[command as usize]));
        Some((target,active))
    }));
    let Some((target, active)) = pressed else {
        return false;
    };
    invalidate(hwnd, target_damage(target));
    if active {
        if let Some(target) = target {
            activate(hwnd, target);
        }
    }
    true
}

pub fn list_focus(hwnd: windows::Win32::Foundation::HWND) {
    let changed = super::WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            let changed = window.chrome.focus != Focus::List;
            window.chrome.focus = Focus::List;
            window.chrome.alt_entry = false;
            window.chrome.cues = false;
            window.panes.focus = None;
            window.list.header.focus = None;
            window.title.focus = super::titlebar::Target::None;
            changed
        })
    });
    if changed {
        super::navigation_input::focus_changed(hwnd);
        publish_focus(hwnd, Focus::List);
        invalidate(
            hwnd,
            super::render::Damage {
                commands: true,
                status: true,
                title: true,
                header: true,
                ..super::render::Damage::NONE
            },
        );
    }
}

pub fn key(hwnd: windows::Win32::Foundation::HWND, key: u32) -> bool {
    if super::settings::key(hwnd, key) {
        return true;
    }
    let mods = super::input::modifiers();
    let alt = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0 };
    if alt && key != 0x12 {
        return false;
    }
    let shortcut = match key {
        0x58 if mods.ctrl => Some(Command::Cut),
        0x43 if mods.ctrl => Some(Command::Copy),
        0x56 if mods.ctrl => Some(Command::Paste),
        0x2e if !mods.ctrl => Some(Command::Delete),
        _ => None,
    };
    if let Some(command) = shortcut {
        let enabled = super::WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.chrome.enabled[command as usize])
        });
        if enabled {
            if command == Command::Delete && mods.shift {
                run_file(hwnd, crate::app::commands::FileAction::DeletePermanently);
            } else {
                activate(hwnd, Target::Command(command));
            }
        }
        return true;
    }
    if mods.ctrl {
        return false;
    }
    let state = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|window| {
            (
                window.chrome.focus,
                window.chrome.status_visible,
                window
                    .chrome
                    .buttons
                    .iter()
                    .filter(|button| window.chrome.enabled[button.command as usize])
                    .map(|button| button.command)
                    .collect::<Vec<_>>(),
                window
                    .list
                    .folder
                    .as_ref()
                    .map_or(0, |folder| folder.columns.len()),
            )
        })
    });
    let Some((focus, status, commands, column_count)) = state else {
        return false;
    };
    if key == 0x12 {
        if let Some(command) = commands.first() {
            super::WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    window.chrome.alt_entry = true;
                }
            });
            set_focus(hwnd, Focus::Command(*command));
        }
        return true;
    }
    if key == 0x09 || key == 0x75 {
        use super::address::Target as Address;
        use crate::app::focus::{Facts, Step};
        let facts = super::WINDOW.with(|slot| {
            slot.borrow().as_ref().map(|window| Facts {
                header: column_count > 0
                    && window.list.folder.as_ref().is_some_and(|folder| {
                        folder.grid.is_none()
                            && folder.crumbs.last().is_some_and(|crumb| {
                                !crate::app::ids::same(&crumb.id, crate::app::ids::HOME)
                                    && !crate::app::ids::is_settings(&crumb.id)
                            })
                    }),
                tree: window.navigation.visible,
                upper: !crate::frame::is_full_screen_for(hwnd),
                share: window.panes.kind == Some(crate::app::pane_model::Kind::Details)
                    && window
                        .panes
                        .model
                        .as_ref()
                        .is_some_and(|model| model.can_share),
                properties: window.panes.kind == Some(crate::app::pane_model::Kind::Details)
                    && window
                        .panes
                        .model
                        .as_ref()
                        .is_some_and(|model| model.can_properties),
                status,
                back: window.address.enabled[0],
                forward: window.address.enabled[1],
                up: window.address.enabled[2],
                settings: true,
            })
        });
        let Some(facts) = facts else { return true };
        let at = match focus {
            Focus::Pane(super::panes::Button::Share) => "pane-share",
            Focus::Pane(super::panes::Button::Properties) => "pane-properties",
            Focus::Settings(_) => "settings-section",
            Focus::List => "list",
            Focus::Header(_) => "header",
            Focus::Status(_) => "status",
            Focus::Tab => "tab",
            Focus::NewTab => "new-tab",
            Focus::Tree => "tree",
            Focus::Neutral => "neutral",
            Focus::Command(Command::Pane) => "pane-command",
            Focus::Command(_) => "commands",
            Focus::Address(target) => match target {
                Address::Back => "back",
                Address::Forward => "forward",
                Address::Up => "up",
                Address::Refresh => "refresh",
                Address::Address => "address",
                Address::Search => "search",
                Address::Settings => "settings",
                _ => "crumbs",
            },
        };
        let step = match (mods.shift, key) {
            (false, 0x09) => Step::Tab,
            (true, 0x09) => Step::ReverseTab,
            (false, _) => Step::Pane,
            (true, _) => Step::ReversePane,
        };
        let next = match crate::app::focus::destination(at, &facts, step) {
            "pane-share" => Focus::Pane(super::panes::Button::Share),
            "pane-properties" => Focus::Pane(super::panes::Button::Properties),
            "header" => Focus::Header(0),
            "status" => Focus::Status(0),
            "tab" => Focus::Tab,
            "new-tab" => Focus::NewTab,
            "tree" => Focus::Tree,
            "back" => Focus::Address(Address::Back),
            "forward" => Focus::Address(Address::Forward),
            "up" => Focus::Address(Address::Up),
            "refresh" => Focus::Address(Address::Refresh),
            "address" => Focus::Address(Address::Address),
            "crumbs" => Focus::Address(Address::Root),
            "search" => Focus::Address(Address::Search),
            "settings" => Focus::Address(Address::Settings),
            "commands" => commands
                .first()
                .copied()
                .map_or(Focus::List, Focus::Command),
            "commands-last" => Focus::Command(Command::More),
            "pane-command" => Focus::Command(Command::Pane),
            _ => Focus::List,
        };
        apply_focus(hwnd, next, super::address_input::Entry::Traversal);
        return true;
    }
    match focus {
        Focus::Pane(button) => {
            if matches!(key, 0x0d | 0x20) {
                super::panes::activate(hwnd, button);
            }
            true
        }
        Focus::Command(command) => {
            if key == 0x1b {
                set_focus(hwnd, Focus::Neutral);
                return true;
            }
            if matches!(key, 0x25 | 0x27) {
                if let Some(index) = commands.iter().position(|candidate| *candidate == command) {
                    let next = (index as isize + if key == 0x25 { -1 } else { 1 })
                        .clamp(0, commands.len().saturating_sub(1) as isize)
                        as usize;
                    set_focus(hwnd, Focus::Command(commands[next]));
                }
                return true;
            }
            if matches!(key, 0x0d | 0x20) || (key == 0x28 && command.menu()) {
                super::WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        if window.chrome.keyboard_press.is_none() {
                            window.chrome.keyboard_press = Some(key);
                            window.chrome.pressed = Some(Target::Command(command));
                        }
                    }
                });
                invalidate(hwnd, target_damage(Some(Target::Command(command))));
                return true;
            }
            true
        }
        Focus::Status(index) => {
            if matches!(key, 0x25 | 0x27) {
                set_focus(hwnd, Focus::Status(if key == 0x25 { 0 } else { 1 }));
                return true;
            }
            if matches!(key, 0x0d | 0x20) {
                activate(hwnd, Target::Status(index));
                return true;
            }
            true
        }
        Focus::Header(index) => {
            if key == 0x28 {
                let anchor = super::WINDOW.with(|slot| {
                    let slot = slot.borrow();
                    let window = slot.as_ref()?;
                    let folder = window.list.folder.as_ref()?;
                    let view = window.scroll.view?;
                    let left = folder.columns[..index]
                        .iter()
                        .map(|column| column.width)
                        .sum::<f32>();
                    Some((view.x + left - window.scroll.x, view.y + view.header))
                });
                if let Some(anchor) = anchor {
                    super::filter::open(hwnd, index, anchor);
                }
                return true;
            }

            if matches!(key, 0x0d | 0x20) {
                let result = super::WINDOW.with(|slot| {
                    slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                        let Some(folder) = window.list.folder.as_ref() else {
                            return Ok(());
                        };
                        let Some(column) = folder.columns.get(index) else {
                            return Ok(());
                        };
                        let sort = crate::app::header::clicked(
                            (!folder.view.search_ranked).then_some(folder.view.sort),
                            crate::app::sort::SortKey::from_property(&column.key),
                        );
                        window.list.sort(sort, &window.scroll)
                    })
                });
                if let Err(error) = result {
                    eprintln!("native header sort: {error}");
                }
                super::header::flush(hwnd);
                return true;
            }
            if matches!(key, 0x25 | 0x27) && column_count > 0 {
                let next = (index as isize + if key == 0x25 { -1 } else { 1 })
                    .rem_euclid(column_count as isize) as usize;
                set_focus(hwnd, Focus::Header(next));
            }
            true
        }
        Focus::Tab | Focus::NewTab | Focus::Neutral => true,
        Focus::List => false,
        Focus::Address(_) | Focus::Tree | Focus::Settings(_) => false,
    }
}
