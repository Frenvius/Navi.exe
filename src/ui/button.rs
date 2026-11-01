use super::{
    address::{rect, rounded},
    render::Canvas,
    text::{Family, TextEngine, TextStyle},
    theme::Theme,
};
use windows::{
    core::Result,
    Win32::Graphics::Direct2D::{Common::*, *},
};

pub const HEIGHT: f32 = 32.0;
pub const PADDING: f32 = 12.0;
pub const ICON_SPACE: f32 = 24.0;
pub const TEXT: TextStyle = TextStyle::new(Family::Variable, 14.0, 400);

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Standard,
    Accent,
}

#[derive(Clone, Copy)]
pub struct State {
    pub enabled: bool,
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            hovered: false,
            pressed: false,
            focused: false,
        }
    }
}

pub struct Button<'a> {
    pub label: &'a str,
    pub variant: Variant,
    pub state: State,
    pub icon: Option<(&'a super::svg::Assets, &'static str)>,
    pub dropdown: bool,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            variant: Variant::Standard,
            state: State::default(),
            icon: None,
            dropdown: false,
        }
    }

    pub fn width(&self, text: &mut TextEngine) -> Result<f32> {
        Ok(text.layout(self.label, TEXT, 4096.0)?.advance.ceil()
            + 2.0 * PADDING
            + if self.icon.is_some() { ICON_SPACE } else { 0.0 }
            + if self.dropdown { 20.0 } else { 0.0 })
    }

    pub fn draw(
        &self,
        canvas: &Canvas,
        text: &mut TextEngine,
        theme: &Theme,
        bounds: D2D_RECT_F,
    ) -> Result<()> {
        let colors = colors(theme, self.variant, self.state);
        rounded(canvas, bounds, 4.0, &colors.fill);
        let outline = D2D1_ROUNDED_RECT {
            rect: rect(
                bounds.left + 0.5,
                bounds.top + 0.5,
                (bounds.right - bounds.left - 1.0).max(0.0),
                (bounds.bottom - bounds.top - 1.0).max(0.0),
            ),
            radiusX: 3.5,
            radiusY: 3.5,
        };
        unsafe {
            let elevated = self.state.enabled && !(self.state.pressed && self.state.hovered);
            if elevated {
                canvas.context.PushAxisAlignedClip(
                    &rect(
                        bounds.left,
                        bounds.top,
                        bounds.right - bounds.left,
                        (bounds.bottom - bounds.top - 1.0).max(0.0),
                    ),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
            }
            canvas.brush.SetColor(&colors.border);
            canvas
                .context
                .DrawRoundedRectangle(&outline, &canvas.brush, 1.0, None);
            if elevated {
                canvas.context.PopAxisAlignedClip();
                canvas.context.PushAxisAlignedClip(
                    &rect(
                        bounds.left,
                        bounds.bottom - 1.0,
                        bounds.right - bounds.left,
                        1.0,
                    ),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
                canvas.brush.SetColor(&colors.bottom);
                canvas
                    .context
                    .DrawRoundedRectangle(&outline, &canvas.brush, 1.0, None);
                canvas.context.PopAxisAlignedClip();
            }
        }
        let content = rect(
            bounds.left + PADDING,
            bounds.top + 1.0,
            (bounds.right - bounds.left - 2.0 * PADDING).max(0.0),
            (bounds.bottom - bounds.top - 2.0).max(0.0),
        );
        unsafe {
            canvas
                .context
                .PushAxisAlignedClip(&content, D2D1_ANTIALIAS_MODE_ALIASED)
        };
        let result = (|| -> Result<()> {
            let layout = text.layout(self.label, TEXT, 4096.0)?;
            let leading = if self.icon.is_some() { ICON_SPACE } else { 0.0 };
            let trailing = if self.dropdown { 20.0 } else { 0.0 };
            let room = (content.right - content.left - leading - trailing).max(0.0);
            let left = if self.dropdown {
                content.left
            } else {
                content.left + ((room - layout.advance) / 2.0).max(0.0)
            };
            if let Some((assets, icon)) = self.icon {
                assets.icon(
                    canvas,
                    icon,
                    (left, (bounds.top + bounds.bottom - 16.0) / 2.0),
                    if self.state.enabled { 1.0 } else { 0.36 },
                )?;
            }
            text.draw(
                canvas,
                self.label,
                TEXT,
                (
                    left + leading,
                    ((bounds.top + bounds.bottom - layout.height) / 2.0).round(),
                ),
                &colors.text,
            )?;
            if self.dropdown {
                text.draw(
                    canvas,
                    "\u{e70d}",
                    TextStyle::new(Family::Icons, 10.0, 400),
                    (
                        bounds.right - 22.0,
                        (bounds.top + bounds.bottom - 10.0) / 2.0,
                    ),
                    &colors.text,
                )?;
            }
            Ok(())
        })();
        unsafe { canvas.context.PopAxisAlignedClip() };
        result?;
        if self.state.enabled && self.state.focused {
            super::chrome::focus_ring_radius(
                canvas,
                rect(
                    bounds.left - 3.0,
                    bounds.top - 3.0,
                    bounds.right - bounds.left + 6.0,
                    bounds.bottom - bounds.top + 6.0,
                ),
                theme,
                4.0,
            );
        }
        Ok(())
    }
}

struct Colors {
    fill: D2D1_COLOR_F,
    border: D2D1_COLOR_F,
    bottom: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
}

fn colors(theme: &Theme, variant: Variant, state: State) -> Colors {
    let pressed = state.pressed && state.hovered;
    if variant == Variant::Accent {
        let mut fill = if state.enabled {
            theme.accent
        } else {
            theme.button_accent_disabled
        };
        if state.enabled {
            fill.a *= if pressed {
                0.8
            } else if state.hovered {
                0.9
            } else {
                1.0
            };
        }
        let mut text = if state.enabled {
            theme.button_accent_text
        } else {
            theme.button_accent_text_disabled
        };
        if state.enabled && pressed {
            text.a *= 0.7;
        }
        Colors {
            fill,
            border: if state.enabled && !pressed {
                theme.button_accent_border
            } else {
                D2D1_COLOR_F::default()
            },
            bottom: theme.button_accent_bottom,
            text,
        }
    } else {
        Colors {
            fill: if !state.enabled {
                theme.button_disabled
            } else if pressed {
                theme.control_pressed
            } else if state.hovered {
                theme.control_hover
            } else {
                theme.control
            },
            border: theme.control_border,
            bottom: theme.button_bottom,
            text: if !state.enabled {
                theme.text_disabled
            } else if pressed {
                theme.text_secondary
            } else {
                theme.text
            },
        }
    }
}

#[cfg(test)]
#[path = "button_tests.rs"]
pub(super) mod tests;
