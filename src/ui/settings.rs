use super::{
    address::{rect, rounded},
    chrome::Focus,
    render::Canvas,
    text::{Family, TextEngine, TextStyle},
    theme::Theme,
    WINDOW,
};
use crate::app::launch_preferences::{Destination, Preferences, Source};
use crate::app::zoxide::{Preferences as ZoxidePreferences, Setting as ZoxideSetting};
use crate::app::zoxide_install::{self, Failure as InstallFailure, State as InstallState};
use windows::{
    core::Result,
    Win32::{Foundation::HWND, Graphics::Direct2D::Common::*},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Section,
    Toggle,
    Restore,
    OpenIn(Source, Destination),
    Zoxide(ZoxideSetting),
    ZoxideInstall,
}

const OPEN_CHOICES: [Target; 4] = [
    Target::OpenIn(Source::External, Destination::NewTab),
    Target::OpenIn(Source::External, Destination::NewWindow),
    Target::OpenIn(Source::WinE, Destination::NewTab),
    Target::OpenIn(Source::WinE, Destination::NewWindow),
];

const ZOXIDE_CHOICES: [Target; 2] = [
    Target::Zoxide(ZoxideSetting::Integration),
    Target::Zoxide(ZoxideSetting::Record),
];

#[derive(Default)]
pub struct State {
    pub active: bool,
    loaded: bool,
    enabled: bool,
    recovery: bool,
    error: String,
    opening: Preferences,
    opening_error: String,
    opening_error_height: f32,
    zoxide: ZoxidePreferences,
    zoxide_error: String,
    zoxide_install: InstallState,
    zoxide_winget: bool,
    zoxide_recheck: bool,
    zoxide_action: D2D_RECT_F,
    zoxide_cards: [D2D_RECT_F; 2],
    zoxide_title_heights: [f32; 2],
    zoxide_description_heights: [f32; 2],
    zoxide_top: f32,
    zoxide_status_height: f32,
    opening_cards: [D2D_RECT_F; 2],
    opening_title_heights: [f32; 2],
    opening_description_heights: [f32; 2],
    size: (f32, f32),
    card: D2D_RECT_F,
    title_height: f32,
    description_height: f32,
    error_height: f32,
    hover: Option<Target>,
    pressed: Option<Target>,
    captured: bool,
    offset: f32,
    maximum: f32,
}

impl State {
    pub fn sync(&mut self, path: &str) {
        let active = crate::app::ids::is_settings(path);
        if self.active != active {
            self.active = active;
            self.loaded = false;
            self.hover = None;
            self.pressed = None;
            self.captured = false;
        }
    }
    fn zoxide_status(&self) -> String {
        if !self.zoxide_error.is_empty() {
            return self.zoxide_error.clone();
        }
        match &self.zoxide_install {
            InstallState::Installing => crate::i18n::tr("settings.zoxide.installing").into(),
            InstallState::Installed => crate::i18n::tr("settings.zoxide.installed").into(),
            InstallState::Failed(error) => {
                let detail = match error {
                    InstallFailure::Launch(message) => {
                        crate::i18n::trf("settings.zoxide.install.launch-error", &[message])
                    }
                    InstallFailure::Exit(code, message) => crate::i18n::trf(
                        "settings.zoxide.install.exit-error",
                        &[&format!("0x{code:08X}"), message],
                    ),
                    InstallFailure::Timeout => {
                        crate::i18n::tr("settings.zoxide.install.timeout").into()
                    }
                    InstallFailure::NotFound => {
                        crate::i18n::tr("settings.zoxide.install.not-found").into()
                    }
                    InstallFailure::Settings(message) => {
                        crate::i18n::trf("settings.zoxide.error", &[message])
                    }
                };
                crate::i18n::trf("settings.zoxide.install.failed", &[&detail])
            }
            InstallState::Idle => crate::i18n::tr(if self.zoxide.available {
                "settings.zoxide.found"
            } else {
                "settings.zoxide.missing"
            })
            .into(),
        }
    }
    fn install_visible(&self) -> bool {
        !self.zoxide.available || self.zoxide_install == InstallState::Installing
    }
    fn install_enabled(&self) -> bool {
        self.install_visible() && self.zoxide_install != InstallState::Installing
    }
    fn left(&self) -> f32 {
        if self.size.0 < 700.0 {
            156.0
        } else {
            208.0
        }
    }
    fn bounds(&self, target: Target) -> D2D_RECT_F {
        match target {
            Target::Section => rect(12.0, 76.0, self.left() - 24.0, 36.0),
            Target::Toggle => rect(self.card.right - 60.0, self.card.top + 32.0, 40.0, 20.0),
            Target::Restore => rect(
                self.card.left + 64.0,
                self.card.bottom - 40.0,
                240.0f32
                    .min(self.card.right - self.card.left - 84.0)
                    .max(1.0),
                28.0,
            ),
            Target::ZoxideInstall => self.zoxide_action,
            Target::Zoxide(setting) => {
                let card = self.zoxide_cards[setting as usize];
                rect(card.right - 60.0, card.top + 24.0, 40.0, 20.0)
            }
            Target::OpenIn(source, destination) => {
                let card = self.opening_cards[source as usize];
                let width = ((card.right - card.left - 48.0) / 2.0).max(1.0);
                let x = card.left
                    + 20.0
                    + if destination == Destination::NewWindow {
                        width + 8.0
                    } else {
                        0.0
                    };
                rect(x, card.bottom - 48.0, width, 32.0)
            }
        }
    }
    fn hit(&self, point: (f32, f32)) -> Option<Target> {
        let point = (point.0, point.1 - 88.0);
        [Target::Section, Target::Toggle, Target::Restore]
            .into_iter()
            .chain(OPEN_CHOICES)
            .chain(ZOXIDE_CHOICES.into_iter().filter(|_| self.zoxide.available))
            .chain(self.install_enabled().then_some(Target::ZoxideInstall))
            .find(|&target| {
                if target == Target::Restore && (!self.recovery || self.enabled) {
                    return false;
                }
                let r = self.bounds(target);
                point.0 >= r.left && point.0 < r.right && point.1 >= r.top && point.1 < r.bottom
            })
    }
    pub fn layout(&mut self, size: (f32, f32), text: &mut TextEngine) -> Result<()> {
        self.size = size;
        let x = self.left() + 28.0;
        let width = (size.0 - x - 28.0).max(1.0);
        let tw = (width - 144.0).max(1.0);
        self.title_height = text
            .layout(
                crate::i18n::tr("settings.default.title"),
                label(14.0, 400).with_wrap(),
                tw,
            )?
            .height;
        self.description_height = text
            .layout(
                crate::i18n::tr("settings.default.description"),
                label(12.0, 400).with_wrap(),
                tw,
            )?
            .height;
        self.error_height = if self.error.is_empty() {
            0.0
        } else {
            text.layout(
                &self.error,
                label(12.0, 400).with_wrap(),
                (width - 84.0).max(1.0),
            )?
            .height
        };
        let height = (44.0 + self.title_height + self.description_height).max(116.0)
            + if self.error.is_empty() {
                0.0
            } else {
                self.error_height + 12.0
            }
            + if self.recovery && !self.enabled {
                44.0
            } else {
                0.0
            };
        let mut top = 84.0 + height + 12.0;
        for source in [Source::External, Source::WinE] {
            let index = source as usize;
            let (title, description) = opening_labels(source);
            self.opening_title_heights[index] = text
                .layout(
                    crate::i18n::tr(title),
                    label(14.0, 400).with_wrap(),
                    (width - 40.0).max(1.0),
                )?
                .height;
            self.opening_description_heights[index] = text
                .layout(
                    crate::i18n::tr(description),
                    label(12.0, 400).with_wrap(),
                    (width - 40.0).max(1.0),
                )?
                .height;
            let card_height =
                88.0 + self.opening_title_heights[index] + self.opening_description_heights[index];
            self.opening_cards[index] = rect(x, top, width, card_height);
            top += card_height + 12.0;
        }
        self.opening_error_height = if self.opening_error.is_empty() {
            0.0
        } else {
            text.layout(
                &self.opening_error,
                label(12.0, 400).with_wrap(),
                (width - 40.0).max(1.0),
            )?
            .height
        };
        top += self.opening_error_height + 20.0;
        self.zoxide_top = top;
        let status = self.zoxide_status();
        self.zoxide_status_height = text
            .layout(
                &status,
                label(12.0, 400).with_wrap(),
                (width - 8.0).max(1.0),
            )?
            .height;
        top += 44.0 + self.zoxide_status_height;
        self.zoxide_action = rect(x, top, width.min(240.0), 32.0);
        if self.install_visible() {
            top += 44.0;
        }
        for (index, setting) in [ZoxideSetting::Integration, ZoxideSetting::Record]
            .into_iter()
            .enumerate()
        {
            let (title, description) = zoxide_labels(setting);
            let tw = (width - 100.0).max(1.0);
            self.zoxide_title_heights[index] = text
                .layout(crate::i18n::tr(title), label(14.0, 400).with_wrap(), tw)?
                .height;
            self.zoxide_description_heights[index] = text
                .layout(
                    crate::i18n::tr(description),
                    label(12.0, 400).with_wrap(),
                    tw,
                )?
                .height;
            let height =
                (44.0 + self.zoxide_title_heights[index] + self.zoxide_description_heights[index])
                    .max(88.0);
            self.zoxide_cards[index] = rect(x, top, width, height);
            top += height + 12.0;
        }
        self.maximum = (top + 20.0 - size.1).max(0.0);
        self.offset = self.offset.clamp(0.0, self.maximum);
        self.card = rect(x, 84.0 - self.offset, width, height);
        self.zoxide_top -= self.offset;
        self.zoxide_action.top -= self.offset;
        self.zoxide_action.bottom -= self.offset;
        for card in self
            .opening_cards
            .iter_mut()
            .chain(self.zoxide_cards.iter_mut())
        {
            card.top -= self.offset;
            card.bottom -= self.offset;
        }
        Ok(())
    }
}
fn zoxide_labels(setting: ZoxideSetting) -> (&'static str, &'static str) {
    match setting {
        ZoxideSetting::Integration => (
            "settings.zoxide.integration.title",
            "settings.zoxide.integration.description",
        ),
        ZoxideSetting::Record => (
            "settings.zoxide.record.title",
            "settings.zoxide.record.description",
        ),
    }
}

fn opening_labels(source: Source) -> (&'static str, &'static str) {
    match source {
        Source::External => (
            "settings.open.external.title",
            "settings.open.external.description",
        ),
        Source::WinE => (
            "settings.open.win-e.title",
            "settings.open.win-e.description",
        ),
    }
}
const fn label(size: f32, weight: u16) -> TextStyle {
    TextStyle::new(Family::Variable, size, weight)
}
fn paragraph(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    style: TextStyle,
    bounds: D2D_RECT_F,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    text.draw_wrapped(canvas, value, style, bounds, color)
}
pub fn draw(
    state: &mut State,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
    focus: Focus,
    cues: bool,
) -> Result<()> {
    let left = state.left();
    paragraph(
        canvas,
        text,
        crate::i18n::tr("settings.title"),
        label(20.0, 600),
        rect(24.0, 24.0, left - 36.0, 32.0),
        &theme.text,
    )?;
    let section = state.bounds(Target::Section);
    rounded(canvas, section, 4.0, &theme.subtle_hover);
    rounded(
        canvas,
        rect(section.left, section.top + 10.0, 3.0, 16.0),
        1.5,
        &theme.accent,
    );
    paragraph(
        canvas,
        text,
        "\u{e713}",
        TextStyle::new(Family::Icons, 14.0, 400),
        rect(24.0, 87.0, 20.0, 18.0),
        &theme.text,
    )?;
    paragraph(
        canvas,
        text,
        crate::i18n::tr("settings.general"),
        label(14.0, 400),
        rect(56.0, 85.0, left - 76.0, 20.0),
        &theme.text,
    )?;
    paragraph(
        canvas,
        text,
        crate::i18n::tr("settings.general"),
        label(28.0, 600),
        rect(
            left + 28.0,
            24.0 - state.offset,
            state.size.0 - left - 56.0,
            40.0,
        ),
        &theme.text,
    )?;
    let c = state.card;
    rounded(canvas, c, 4.0, &theme.card_border);
    rounded(
        canvas,
        rect(
            c.left + 1.0,
            c.top + 1.0,
            c.right - c.left - 2.0,
            c.bottom - c.top - 2.0,
        ),
        3.0,
        &theme.settings_card,
    );
    paragraph(
        canvas,
        text,
        "\u{e8b7}",
        TextStyle::new(Family::Icons, 24.0, 400),
        rect(c.left + 20.0, c.top + 24.0, 24.0, 24.0),
        &theme.text,
    )?;
    paragraph(
        canvas,
        text,
        crate::i18n::tr("settings.default.title"),
        label(14.0, 400),
        rect(
            c.left + 64.0,
            c.top + 20.0,
            c.right - c.left - 144.0,
            state.title_height,
        ),
        &theme.text,
    )?;
    paragraph(
        canvas,
        text,
        crate::i18n::tr("settings.default.description"),
        label(12.0, 400),
        rect(
            c.left + 64.0,
            c.top + 24.0 + state.title_height,
            c.right - c.left - 144.0,
            state.description_height,
        ),
        &theme.text_secondary,
    )?;
    draw_toggle(
        canvas,
        state.bounds(Target::Toggle),
        state.enabled,
        true,
        state.hover == Some(Target::Toggle),
        theme,
    );
    if !state.error.is_empty() {
        paragraph(
            canvas,
            text,
            &state.error,
            label(12.0, 400),
            rect(
                c.left + 64.0,
                c.top + 36.0 + state.title_height + state.description_height,
                c.right - c.left - 84.0,
                state.error_height,
            ),
            &theme.settings_error,
        )?;
    }
    if state.recovery && !state.enabled {
        let r = state.bounds(Target::Restore);
        rounded(canvas, r, 4.0, &theme.control_border);
        rounded(
            canvas,
            rect(r.left + 1.0, r.top + 1.0, r.right - r.left - 2.0, 26.0),
            3.0,
            if state.pressed == Some(Target::Restore) {
                &theme.control_pressed
            } else if state.hover == Some(Target::Restore) {
                &theme.control_hover
            } else {
                &theme.control
            },
        );
        super::details::draw_line(
            canvas,
            text,
            crate::i18n::tr("settings.default.restore"),
            label(12.0, 400),
            r,
            0.0,
            &theme.text,
        )?;
    }
    for source in [Source::External, Source::WinE] {
        let index = source as usize;
        let card = state.opening_cards[index];
        rounded(canvas, card, 4.0, &theme.card_border);
        rounded(
            canvas,
            rect(
                card.left + 1.0,
                card.top + 1.0,
                card.right - card.left - 2.0,
                card.bottom - card.top - 2.0,
            ),
            3.0,
            &theme.settings_card,
        );
        let (title, description) = opening_labels(source);
        paragraph(
            canvas,
            text,
            crate::i18n::tr(title),
            label(14.0, 400),
            rect(
                card.left + 20.0,
                card.top + 16.0,
                card.right - card.left - 40.0,
                state.opening_title_heights[index],
            ),
            &theme.text,
        )?;
        paragraph(
            canvas,
            text,
            crate::i18n::tr(description),
            label(12.0, 400),
            rect(
                card.left + 20.0,
                card.top + 22.0 + state.opening_title_heights[index],
                card.right - card.left - 40.0,
                state.opening_description_heights[index],
            ),
            &theme.text_secondary,
        )?;
        for destination in [Destination::NewTab, Destination::NewWindow] {
            let target = Target::OpenIn(source, destination);
            let bounds = state.bounds(target);
            let selected = state.opening.destination(source) == destination;
            rounded(
                canvas,
                bounds,
                4.0,
                if selected {
                    &theme.accent
                } else {
                    &theme.control_border
                },
            );
            rounded(
                canvas,
                rect(
                    bounds.left + 1.0,
                    bounds.top + 1.0,
                    bounds.right - bounds.left - 2.0,
                    30.0,
                ),
                3.0,
                &theme.settings_card,
            );
            if state.pressed == Some(target) || state.hover == Some(target) {
                rounded(
                    canvas,
                    rect(
                        bounds.left + 1.0,
                        bounds.top + 1.0,
                        bounds.right - bounds.left - 2.0,
                        30.0,
                    ),
                    3.0,
                    if state.pressed == Some(target) {
                        &theme.control_pressed
                    } else {
                        &theme.control_hover
                    },
                );
            }
            rounded(
                canvas,
                rect(bounds.left + 12.0, bounds.top + 9.0, 14.0, 14.0),
                7.0,
                if selected {
                    &theme.accent
                } else {
                    &theme.text_secondary
                },
            );
            rounded(
                canvas,
                rect(bounds.left + 13.0, bounds.top + 10.0, 12.0, 12.0),
                6.0,
                &theme.settings_card,
            );
            if selected {
                rounded(
                    canvas,
                    rect(bounds.left + 16.0, bounds.top + 13.0, 6.0, 6.0),
                    3.0,
                    &theme.accent,
                );
            }
            let key = match destination {
                Destination::NewTab => "settings.open.new-tab",
                Destination::NewWindow => "settings.open.new-window",
            };
            super::details::draw_line(
                canvas,
                text,
                crate::i18n::tr(key),
                label(12.0, 400),
                rect(
                    bounds.left + 36.0,
                    bounds.top + 5.0,
                    (bounds.right - bounds.left - 44.0).max(1.0),
                    22.0,
                ),
                0.0,
                &theme.text,
            )?;
        }
    }
    if !state.opening_error.is_empty() {
        let card = state.opening_cards[1];
        paragraph(
            canvas,
            text,
            &state.opening_error,
            label(12.0, 400),
            rect(
                card.left + 20.0,
                card.bottom + 12.0,
                card.right - card.left - 40.0,
                state.opening_error_height,
            ),
            &theme.settings_error,
        )?;
    }
    let x = state.left() + 28.0;
    let width = (state.size.0 - x - 28.0).max(1.0);
    paragraph(
        canvas,
        text,
        crate::i18n::tr("settings.zoxide.title"),
        label(20.0, 600),
        rect(x, state.zoxide_top, width, 28.0),
        &theme.text,
    )?;
    paragraph(
        canvas,
        text,
        &state.zoxide_status(),
        label(12.0, 400),
        rect(
            x,
            state.zoxide_top + 32.0,
            width,
            state.zoxide_status_height,
        ),
        if state.zoxide_error.is_empty() && !matches!(state.zoxide_install, InstallState::Failed(_))
        {
            &theme.text_secondary
        } else {
            &theme.settings_error
        },
    )?;
    if state.install_visible() {
        let target = Target::ZoxideInstall;
        let r = state.bounds(target);
        let busy = state.zoxide_install == InstallState::Installing;
        if state.zoxide_winget {
            rounded(canvas, r, 4.0, &theme.control_border);
            rounded(
                canvas,
                rect(
                    r.left + 1.0,
                    r.top + 1.0,
                    r.right - r.left - 2.0,
                    r.bottom - r.top - 2.0,
                ),
                3.0,
                if !busy && state.pressed == Some(target) {
                    &theme.control_pressed
                } else if !busy && state.hover == Some(target) {
                    &theme.control_hover
                } else {
                    &theme.control
                },
            );
        }
        super::details::draw_line(
            canvas,
            text,
            crate::i18n::tr(if busy {
                "settings.zoxide.installing"
            } else if state.zoxide_winget {
                "settings.zoxide.install"
            } else {
                "settings.zoxide.install.help"
            }),
            label(12.0, 400),
            r,
            if state.zoxide_winget { 12.0 } else { 0.0 },
            if busy {
                &theme.text_disabled
            } else if state.zoxide_winget {
                &theme.text
            } else {
                &theme.accent
            },
        )?;
    }
    for (index, setting) in [ZoxideSetting::Integration, ZoxideSetting::Record]
        .into_iter()
        .enumerate()
    {
        let card = state.zoxide_cards[index];
        rounded(canvas, card, 4.0, &theme.card_border);
        rounded(
            canvas,
            rect(
                card.left + 1.0,
                card.top + 1.0,
                card.right - card.left - 2.0,
                card.bottom - card.top - 2.0,
            ),
            3.0,
            &theme.settings_card,
        );
        let (title, description) = zoxide_labels(setting);
        let color = if state.zoxide.available {
            &theme.text
        } else {
            &theme.text_disabled
        };
        paragraph(
            canvas,
            text,
            crate::i18n::tr(title),
            label(14.0, 400),
            rect(
                card.left + 20.0,
                card.top + 16.0,
                card.right - card.left - 100.0,
                state.zoxide_title_heights[index],
            ),
            color,
        )?;
        paragraph(
            canvas,
            text,
            crate::i18n::tr(description),
            label(12.0, 400),
            rect(
                card.left + 20.0,
                card.top + 22.0 + state.zoxide_title_heights[index],
                card.right - card.left - 100.0,
                state.zoxide_description_heights[index],
            ),
            &theme.text_secondary,
        )?;
        let target = Target::Zoxide(setting);
        let enabled = if setting == ZoxideSetting::Integration {
            state.zoxide.enabled
        } else {
            state.zoxide.record
        };
        draw_toggle(
            canvas,
            state.bounds(target),
            enabled,
            state.zoxide.available,
            state.hover == Some(target),
            theme,
        );
    }
    if cues {
        if let Focus::Settings(target) = focus {
            let r = state.bounds(target);
            super::chrome::focus_ring_radius(
                canvas,
                rect(
                    r.left - 2.0,
                    r.top - 2.0,
                    r.right - r.left + 4.0,
                    r.bottom - r.top + 4.0,
                ),
                theme,
                if matches!(target, Target::Toggle | Target::Zoxide(_)) {
                    12.0
                } else {
                    4.0
                },
            );
        }
    }
    Ok(())
}
fn draw_toggle(
    canvas: &Canvas,
    t: D2D_RECT_F,
    enabled: bool,
    available: bool,
    hover: bool,
    theme: &Theme,
) {
    rounded(
        canvas,
        t,
        10.0,
        if enabled {
            &theme.accent
        } else if available {
            &theme.text_secondary
        } else {
            &theme.text_disabled
        },
    );
    if !enabled {
        rounded(
            canvas,
            rect(t.left + 1.0, t.top + 1.0, 38.0, 18.0),
            9.0,
            if hover {
                &theme.subtle_hover
            } else {
                &theme.settings_card
            },
        );
    }
    rounded(
        canvas,
        rect(
            t.left + if enabled { 24.0 } else { 4.0 },
            t.top + 4.0,
            12.0,
            12.0,
        ),
        6.0,
        if enabled {
            &theme.settings_toggle_on
        } else if available {
            &theme.text_secondary
        } else {
            &theme.text_disabled
        },
    );
}

pub fn active() -> bool {
    WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            crate::app::ids::is_settings(window.tabs.active().location.tab.path())
        })
    })
}
pub fn open(hwnd: HWND) -> Result<()> {
    let existing = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()?
            .tabs
            .entries
            .iter()
            .enumerate()
            .find_map(|(tab, saved)| {
                saved
                    .location
                    .tab
                    .history
                    .iter()
                    .position(|path| crate::app::ids::is_settings(path))
                    .map(|history| (tab, history))
            })
    });
    if let Some((tab, history)) = existing {
        super::tabs::select(hwnd, tab)?;
        super::navigate(hwnd, super::Navigation::History(history))?;
    } else {
        super::tabs::open(hwnd, crate::app::ids::SETTINGS)?;
    }
    refresh(hwnd);
    Ok(())
}
pub fn refresh(hwnd: HWND) {
    if !active() {
        return;
    }
    let status = crate::app::default_manager::status();
    let opening = Preferences::read();
    let zoxide = ZoxidePreferences::read();
    let install = zoxide_install::state();
    let winget = zoxide_install::winget_available();
    let mut detect = None;
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let s = &mut window.settings;
        let before = (
            s.enabled,
            s.recovery,
            s.error.clone(),
            s.opening,
            s.opening_error.clone(),
            s.zoxide,
            s.zoxide_install.clone(),
            s.zoxide_winget,
        );
        if !s.loaded || s.zoxide_recheck {
            detect = Some(s.zoxide_recheck);
            s.zoxide_recheck = false;
        }
        s.loaded = true;
        s.zoxide = zoxide;
        s.zoxide_install = install;
        s.zoxide_winget = winget;
        match status {
            Ok(status) => {
                s.enabled = status.enabled;
                s.recovery = status.recovery;
                if (status.enabled || !status.recovery)
                    && s.error == crate::i18n::tr("settings.default.changed")
                {
                    s.error.clear();
                }
                if status.recovery && !status.enabled && s.error.is_empty() {
                    s.error = crate::i18n::tr("settings.default.changed").into();
                }
            }
            Err(error) => {
                s.error = crate::i18n::trf("settings.default.error", &[&error.to_string()])
            }
        }
        match opening {
            Ok(preferences) => {
                s.opening = preferences;
                s.opening_error.clear();
            }
            Err(error) => {
                s.opening_error = crate::i18n::trf("settings.open.error", &[&error.to_string()])
            }
        }
        before
            != (
                s.enabled,
                s.recovery,
                s.error.clone(),
                s.opening,
                s.opening_error.clone(),
                s.zoxide,
                s.zoxide_install.clone(),
                s.zoxide_winget,
            )
    });
    if let Some(enable) = detect.filter(|_| !zoxide.available) {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow().as_ref() {
                window
                    .address_worker
                    .send(super::address::Request::ZoxideDetect(enable));
            }
        });
    }
    if changed {
        let normalize = WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                window.chrome.focus == Focus::Settings(Target::ZoxideInstall)
                    && window.settings.zoxide.available
                    && !window.settings.install_visible()
            })
        });
        if normalize {
            super::chrome::set_focus(
                hwnd,
                Focus::Settings(Target::Zoxide(ZoxideSetting::Integration)),
            );
        }
        super::invalidate_all(hwnd);
    }
}
pub fn after_paint(hwnd: HWND) {
    if WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.settings.active && !window.settings.loaded)
    }) {
        refresh(hwnd);
    }
}
fn activate(hwnd: HWND, target: Target) {
    if target == Target::Section {
        return;
    }
    if target == Target::ZoxideInstall {
        let action = WINDOW.with(|slot| {
            slot.borrow().as_ref().and_then(|window| {
                window
                    .settings
                    .install_enabled()
                    .then_some(window.settings.zoxide_winget)
            })
        });
        match action {
            Some(true) => {
                let mut senders = Vec::new();
                for handle in super::context::handles() {
                    let _context = super::context::enter(handle);
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow().as_ref() {
                            senders.push(window.address_worker.dispatcher());
                        }
                    });
                }
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        window.settings.zoxide_error.clear();
                    }
                });
                zoxide_install::start(move || {
                    for sender in &senders {
                        let _ = sender.send(super::address::Request::ZoxideInstallationChanged);
                    }
                });
                refresh(hwnd);
            }
            Some(false) => {
                if let Err(error) =
                    crate::shell::ops::shell_execute(hwnd, zoxide_install::INSTALL_URL, None)
                {
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow_mut().as_mut() {
                            window.settings.zoxide_error =
                                crate::i18n::trf("settings.zoxide.install.launch-error", &[&error]);
                        }
                    });
                } else {
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow_mut().as_mut() {
                            window.settings.zoxide_recheck = true;
                        }
                    });
                }
            }
            None => {}
        }
        super::invalidate_all(hwnd);
        return;
    }
    if let Target::Zoxide(setting) = target {
        let preferences = ZoxidePreferences::read();
        if !preferences.available {
            return;
        }
        let enabled = if setting == ZoxideSetting::Integration {
            preferences.enabled
        } else {
            preferences.record
        };
        let result = setting.save(!enabled);
        refresh(hwnd);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.settings.zoxide_error = match result {
                    Ok(()) => String::new(),
                    Err(error) => crate::i18n::trf("settings.zoxide.error", &[&error.to_string()]),
                };
            }
        });
        super::invalidate_all(hwnd);
        return;
    }
    if let Target::OpenIn(source, destination) = target {
        let result = source.save(destination);
        refresh(hwnd);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.settings.opening_error = match result {
                    Ok(()) => String::new(),
                    Err(error) => crate::i18n::trf("settings.open.error", &[&error.to_string()]),
                };
            }
        });
        super::invalidate_all(hwnd);
        return;
    }
    let enabled = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.settings.enabled)
    });
    let result = crate::app::default_manager::set_enabled(target == Target::Toggle && !enabled);
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.settings.error = match result {
                Ok(()) => String::new(),
                Err(error) => crate::i18n::trf("settings.default.error", &[&error.to_string()]),
            };
        }
    });
    refresh(hwnd);
    super::invalidate_all(hwnd);
}
pub fn moved(hwnd: HWND, point: (f32, f32)) -> bool {
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let w = slot.as_mut()?;
        if !w.settings.active || point.1 < 88.0 {
            return None;
        }
        let hover = w.settings.hit(point);
        let changed = hover != w.settings.hover;
        w.settings.hover = hover;
        Some(changed)
    });
    if changed == Some(true) {
        super::invalidate_all(hwnd);
    }
    changed.is_some()
}
pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    if point.1 < 88.0 || !active() {
        return false;
    }
    let target = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let w = slot.as_mut()?;
        let target = w.settings.hit(point);
        w.settings.pressed = target;
        w.settings.captured = true;
        target
    });
    if let Some(target) = target {
        super::chrome::set_focus(hwnd, Focus::Settings(target));
    }
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
    }
    super::invalidate_all(hwnd);
    true
}
pub fn release(hwnd: HWND, point: (f32, f32)) -> bool {
    if !active() {
        return false;
    }
    let captured = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.settings.captured)
    });
    if !captured {
        return false;
    }
    let pressed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let w = slot.as_mut()?;
        w.settings.captured = false;
        w.settings
            .pressed
            .take()
            .filter(|target| Some(*target) == w.settings.hit(point))
    });
    unsafe {
        if windows::Win32::UI::Input::KeyboardAndMouse::GetCapture() == hwnd {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
    }
    if let Some(target) = pressed {
        activate(hwnd, target);
    }
    super::invalidate_all(hwnd);
    true
}
pub fn key(hwnd: HWND, key: u32) -> bool {
    if !active() {
        return false;
    }
    let mods = super::input::modifiers();
    let alt = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
    let state = WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|w| {
            (
                w.chrome.focus,
                w.address.enabled,
                w.settings.recovery && !w.settings.enabled,
            )
        })
    });
    let Some((focus, enabled, recovery)) = state else {
        return false;
    };
    if key == 0x09 && !mods.ctrl || key == 0x75 {
        let mut ring = vec![Focus::Tab, Focus::NewTab];
        if enabled[0] {
            ring.push(Focus::Address(super::address::Target::Back));
        }
        if enabled[1] {
            ring.push(Focus::Address(super::address::Target::Forward));
        }
        ring.extend([
            Focus::Address(super::address::Target::Refresh),
            Focus::Address(super::address::Target::Address),
            Focus::Address(super::address::Target::Root),
            Focus::Address(super::address::Target::Settings),
            Focus::Settings(Target::Section),
            Focus::Settings(Target::Toggle),
        ]);
        if recovery {
            ring.push(Focus::Settings(Target::Restore));
        }
        ring.extend(OPEN_CHOICES.map(Focus::Settings));
        if ZoxidePreferences::read().available {
            ring.extend(ZOXIDE_CHOICES.map(Focus::Settings));
        }
        if WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.settings.install_enabled())
        }) {
            ring.push(Focus::Settings(Target::ZoxideInstall));
        }
        let current = ring
            .iter()
            .position(|candidate| *candidate == focus)
            .unwrap_or(ring.len() - 1);
        let next = (current + if mods.shift { ring.len() - 1 } else { 1 }) % ring.len();
        if let Focus::Settings(target) = ring[next] {
            WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    let bounds = window.settings.bounds(target);
                    let shift = if bounds.top < 12.0 {
                        bounds.top - 12.0
                    } else if bounds.bottom > window.settings.size.1 - 12.0 {
                        bounds.bottom - window.settings.size.1 + 12.0
                    } else {
                        0.0
                    };
                    window.settings.offset =
                        (window.settings.offset + shift).clamp(0.0, window.settings.maximum);
                }
            });
        }
        super::chrome::set_focus(hwnd, ring[next]);
        super::invalidate_all(hwnd);
        return true;
    }
    if let Focus::Settings(target) = focus {
        if let Target::OpenIn(source, _) = target {
            if matches!(key, 0x25 | 0x27) && !mods.ctrl && !alt {
                let destination = if key == 0x25 {
                    Destination::NewTab
                } else {
                    Destination::NewWindow
                };
                let target = Target::OpenIn(source, destination);
                super::chrome::set_focus(hwnd, Focus::Settings(target));
                activate(hwnd, target);
                return true;
            }
        }
        if matches!(key, 0x20 | 0x0d) && !mods.ctrl && !alt {
            activate(hwnd, target);
        }
        return !mods.ctrl && !alt;
    }
    false
}

pub fn icon() -> crate::shell::icons::Rgba {
    crate::shell::icons::Rgba {
        width: 16,
        height: 16,
        pixels: vec![0; 16 * 16 * 4],
    }
}

pub fn cancel() {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.settings.pressed = None;
            window.settings.captured = false;
        }
    });
}

pub fn wheel(hwnd: HWND, point: (f32, f32), delta: i32) -> bool {
    let handled = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let state = &mut window.settings;
        if !state.active || point.0 < state.left() || point.1 < 88.0 {
            return false;
        }
        state.offset = (state.offset - delta as f32 * 48.0 / 120.0).clamp(0.0, state.maximum);
        true
    });
    if handled {
        super::invalidate_all(hwnd);
    }
    handled
}
