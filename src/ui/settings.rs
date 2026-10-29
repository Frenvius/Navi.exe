use super::{
    address::{rect, rounded},
    chrome::Focus,
    render::Canvas,
    text::{Family, TextEngine, TextStyle},
    theme::Theme,
    WINDOW,
};
use windows::{
    core::Result,
    Win32::{Foundation::HWND, Graphics::Direct2D::Common::*},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Section,
    Toggle,
    Restore,
}

#[derive(Default)]
pub struct State {
    pub active: bool,
    loaded: bool,
    enabled: bool,
    recovery: bool,
    error: String,
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
        }
    }
    fn hit(&self, point: (f32, f32)) -> Option<Target> {
        let point = (point.0, point.1 - 88.0);
        [Target::Section, Target::Toggle, Target::Restore]
            .into_iter()
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
        self.maximum = (116.0 + height - size.1).max(0.0);
        self.offset = self.offset.clamp(0.0, self.maximum);
        self.card = rect(x, 84.0 - self.offset, width, height);
        Ok(())
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
    let t = state.bounds(Target::Toggle);
    rounded(
        canvas,
        t,
        10.0,
        if state.enabled {
            &theme.accent
        } else {
            &theme.text_secondary
        },
    );
    if !state.enabled {
        rounded(
            canvas,
            rect(t.left + 1.0, t.top + 1.0, 38.0, 18.0),
            9.0,
            if state.hover == Some(Target::Toggle) {
                &theme.subtle_hover
            } else {
                &theme.settings_card
            },
        );
    }
    rounded(
        canvas,
        rect(
            t.left + if state.enabled { 24.0 } else { 4.0 },
            t.top + 4.0,
            12.0,
            12.0,
        ),
        6.0,
        if state.enabled {
            &theme.settings_toggle_on
        } else {
            &theme.text_secondary
        },
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
                if target == Target::Toggle { 12.0 } else { 4.0 },
            );
        }
    }
    Ok(())
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
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let s = &mut window.settings;
        let before = (s.enabled, s.recovery, s.error.clone());
        s.loaded = true;
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
        before != (s.enabled, s.recovery, s.error.clone())
    });
    if changed {
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
        let current = ring
            .iter()
            .position(|candidate| *candidate == focus)
            .unwrap_or(ring.len() - 1);
        let next = (current + if mods.shift { ring.len() - 1 } else { 1 }) % ring.len();
        super::chrome::set_focus(hwnd, ring[next]);
        return true;
    }
    if let Focus::Settings(target) = focus {
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
