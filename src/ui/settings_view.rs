use super::*;
use windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED;

fn contains(r: D2D_RECT_F, p: (f32, f32)) -> bool {
    p.0 >= r.left && p.0 < r.right && p.1 >= r.top && p.1 < r.bottom
}

impl State {
    pub(super) fn left(&self) -> f32 {
        if self.size.0 < 560.0 {
            56.0
        } else if self.size.0 < 700.0 {
            164.0
        } else if self.size.0 < 900.0 {
            200.0
        } else {
            236.0
        }
    }

    fn control_width(&self, row: &Row) -> f32 {
        let width = row.bounds.right - row.bounds.left;
        let desired = match row.control {
            Control::Dropdown => (width * 0.42).clamp(
                144.0,
                if row.target == Target::Startup {
                    244.0
                } else {
                    188.0
                },
            ),
            Control::Toggle(_) => 136.0,
            Control::Expand => 32.0,
            Control::Button => 120.0,
        };
        desired.min((width - 32.0).max(40.0))
    }

    pub(super) fn bounds(&self, target: Target) -> D2D_RECT_F {
        if matches!(target, Target::Section | Target::Integrations) {
            return rect(
                12.0,
                if target == Target::Section {
                    64.0
                } else {
                    104.0
                },
                self.left() - 24.0,
                36.0,
            );
        }
        if matches!(target, Target::StartupChoice(_) | Target::OpenIn(_, _)) {
            let r = self.dropdown_bounds();
            let index = match target {
                Target::StartupChoice(value) => value as usize,
                Target::OpenIn(_, value) => value as usize,
                _ => 0,
            };
            return rect(
                r.left + 4.0,
                r.top + 4.0 + index as f32 * 36.0,
                r.right - r.left - 8.0,
                36.0,
            );
        }
        let Some(row) = self.rows.iter().find(|r| r.target == target) else {
            return D2D_RECT_F::default();
        };
        let r = row.bounds;
        let width = self.control_width(row);
        let height = if matches!(row.control, Control::Toggle(_)) {
            20.0
        } else {
            32.0
        };
        let top = if r.right - r.left < 360.0 {
            r.bottom - 16.0 - height
        } else {
            (r.top + r.bottom - height) / 2.0
        };
        rect(r.right - width - 16.0, top, width, height)
    }

    pub(super) fn choices(&self) -> [Target; 2] {
        match self.dropdown {
            Some(Target::Open(source)) => [
                Target::OpenIn(source, Destination::NewTab),
                Target::OpenIn(source, Destination::NewWindow),
            ],
            _ => [
                Target::StartupChoice(Startup::NewTab),
                Target::StartupChoice(Startup::Continue),
            ],
        }
    }

    fn dropdown_bounds(&self) -> D2D_RECT_F {
        let Some(target) = self.dropdown else {
            return D2D_RECT_F::default();
        };
        let r = self.bounds(target);
        let height = 80.0;
        let top = if r.bottom + height + 8.0 <= self.size.1 {
            r.bottom + 4.0
        } else {
            r.top - height - 4.0
        };
        rect(r.left, top.max(64.0), r.right - r.left, height)
    }

    pub(super) fn hit(&self, point: (f32, f32)) -> Option<Target> {
        let p = (point.0, point.1 - 88.0);
        if self.dropdown.is_some() {
            if let Some(target) = self
                .choices()
                .into_iter()
                .find(|&target| contains(self.bounds(target), p))
            {
                return Some(target);
            }
        }
        for target in [Target::Section, Target::Integrations] {
            if contains(self.bounds(target), p) {
                return Some(target);
            }
        }
        if p.1 < 64.0 || p.1 >= self.size.1 {
            return None;
        }
        self.rows
            .iter()
            .filter(|row| !matches!(row.target, Target::Zoxide(_)) || self.zoxide.available)
            .filter(|row| row.target != Target::ZoxideInstall || self.install_enabled())
            .find_map(|row| contains(self.bounds(row.target), p).then_some(row.target))
            .or_else(|| {
                self.rows.iter().find_map(|row| {
                    matches!(row.control, Control::Expand)
                        .then_some(row)
                        .filter(|row| contains(row.bounds, p))
                        .map(|row| row.target)
                })
            })
    }

    fn group(&mut self, top: &mut f32, key: &'static str) {
        self.groups.push((*top, key));
        *top += 30.0;
    }

    fn add_row(
        &mut self,
        top: &mut f32,
        text: &mut TextEngine,
        target: Target,
        icon: &'static str,
        title: &'static str,
        description: String,
        control: Control,
    ) -> Result<()> {
        let x = self.left() + 24.0;
        let width = (self.size.0 - x - 24.0).max(1.0);
        let mut row = Row {
            target,
            bounds: rect(x, *top, width, 68.0),
            icon,
            title,
            description,
            title_height: 0.0,
            description_height: 0.0,
            control,
        };
        let stacked = width < 360.0;
        let room = (width
            - 56.0
            - if stacked {
                16.0
            } else {
                self.control_width(&row) + 32.0
            })
        .max(24.0);
        row.title_height = text
            .layout(crate::i18n::tr(title), label(14.0, 400).with_wrap(), room)?
            .height;
        if !row.description.is_empty() {
            row.description_height = text
                .layout(&row.description, label(12.0, 400).with_wrap(), room)?
                .height;
        }
        let height = (32.0
            + row.title_height
            + if row.description_height > 0.0 {
                4.0 + row.description_height
            } else {
                0.0
            })
        .max(68.0)
            + if stacked { 44.0 } else { 0.0 };
        row.bounds.bottom = row.bounds.top + height;
        *top += height + 6.0;
        self.rows.push(row);
        Ok(())
    }

    fn notice(&mut self, top: &mut f32, text: &mut TextEngine, value: String) -> Result<()> {
        if value.is_empty() {
            return Ok(());
        }
        let x = self.left() + 40.0;
        let width = (self.size.0 - x - 40.0).max(1.0);
        let h = text
            .layout(&value, label(12.0, 400).with_wrap(), width)?
            .height;
        self.notices.push((rect(x, *top + 6.0, width, h), value));
        *top += h + 18.0;
        Ok(())
    }

    pub fn layout(&mut self, size: (f32, f32), text: &mut TextEngine) -> Result<()> {
        self.size = size;
        self.rows.clear();
        self.groups.clear();
        self.notices.clear();
        let mut top = 76.0;
        if self.integrations {
            self.group(&mut top, "settings.group.tools");
            self.add_row(
                &mut top,
                text,
                Target::ExpandZoxide,
                "\u{e756}",
                "settings.zoxide.title",
                self.zoxide_status(),
                Control::Expand,
            )?;
            if self.expanded {
                if self.install_visible() {
                    let title = if self.zoxide_install == InstallState::Installing {
                        "settings.zoxide.installing"
                    } else if self.zoxide_winget {
                        "settings.zoxide.install"
                    } else {
                        "settings.zoxide.install.help"
                    };
                    self.add_row(
                        &mut top,
                        text,
                        Target::ZoxideInstall,
                        "\u{e896}",
                        title,
                        String::new(),
                        Control::Button,
                    )?;
                }
                for (index, setting) in [ZoxideSetting::Integration, ZoxideSetting::Record]
                    .into_iter()
                    .enumerate()
                {
                    let (title, description) = zoxide_labels(setting);
                    self.add_row(
                        &mut top,
                        text,
                        Target::Zoxide(setting),
                        if index == 0 { "\u{e721}" } else { "\u{e81c}" },
                        title,
                        crate::i18n::tr(description).into(),
                        Control::Toggle(index + 1),
                    )?;
                }
            }
        } else {
            self.group(&mut top, "settings.group.startup");
            self.add_row(
                &mut top,
                text,
                Target::Startup,
                "\u{e7e8}",
                "settings.startup.title",
                crate::i18n::tr("settings.startup.description").into(),
                Control::Dropdown,
            )?;
            self.notice(&mut top, text, self.startup_error.clone())?;
            top += 22.0;
            self.group(&mut top, "settings.group.opening");
            for source in [Source::External, Source::WinE] {
                let (title, description) = opening_labels(source);
                self.add_row(
                    &mut top,
                    text,
                    Target::Open(source),
                    if source == Source::External {
                        "\u{e8b7}"
                    } else {
                        "\u{e765}"
                    },
                    title,
                    crate::i18n::tr(description).into(),
                    Control::Dropdown,
                )?;
            }
            self.notice(&mut top, text, self.opening_error.clone())?;
            top += 22.0;
            self.group(&mut top, "settings.group.system");
            self.add_row(
                &mut top,
                text,
                Target::Toggle,
                "\u{e8b7}",
                "settings.default.title",
                crate::i18n::tr("settings.default.description").into(),
                Control::Toggle(0),
            )?;
            self.notice(&mut top, text, self.error.clone())?;
            if self.recovery && !self.enabled {
                self.add_row(
                    &mut top,
                    text,
                    Target::Restore,
                    "\u{e777}",
                    "settings.default.restore",
                    String::new(),
                    Control::Button,
                )?;
            }
        }
        self.maximum = (top + 20.0 - self.size.1).max(0.0);
        self.offset = self.offset.clamp(0.0, self.maximum);
        for row in &mut self.rows {
            row.bounds.top -= self.offset;
            row.bounds.bottom -= self.offset;
        }
        for (y, _) in &mut self.groups {
            *y -= self.offset;
        }
        for (r, _) in &mut self.notices {
            r.top -= self.offset;
            r.bottom -= self.offset;
        }
        Ok(())
    }

    pub(super) fn controls(&self) -> Vec<Target> {
        [Target::Section, Target::Integrations]
            .into_iter()
            .chain(
                self.rows
                    .iter()
                    .map(|row| row.target)
                    .filter(|target| !matches!(target, Target::Zoxide(_)) || self.zoxide.available)
                    .filter(|target| *target != Target::ZoxideInstall || self.install_enabled()),
            )
            .collect()
    }
}

fn value(target: Target) -> &'static str {
    match target {
        Target::StartupChoice(Startup::NewTab) => "settings.startup.new-tab",
        Target::StartupChoice(Startup::Continue) => "settings.startup.continue",
        Target::OpenIn(_, Destination::NewTab) => "settings.open.new-tab",
        Target::OpenIn(_, Destination::NewWindow) => "settings.open.new-window",
        _ => "settings.action.open",
    }
}

fn glyph(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    x: f32,
    y: f32,
    size: f32,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    text.draw(
        canvas,
        value,
        TextStyle::new(Family::Icons, size, 400),
        (x, y),
        color,
    )
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
    if left >= 100.0 {
        paragraph(
            canvas,
            text,
            crate::i18n::tr("settings.title"),
            label(20.0, 600),
            rect(20.0, 24.0, left - 28.0, 28.0),
            &theme.text,
        )?;
    } else {
        glyph(canvas, text, "\u{e713}", 20.0, 24.0, 20.0, &theme.text)?;
    }
    for (target, key, icon) in [
        (Target::Section, "settings.general", "\u{e713}"),
        (Target::Integrations, "settings.integrations", "\u{e774}"),
    ] {
        let r = state.bounds(target);
        let selected = (target == Target::Integrations) == state.integrations;
        if selected || state.hover == Some(target) {
            rounded(canvas, r, 4.0, &theme.subtle_hover);
        }
        if selected {
            rounded(
                canvas,
                rect(r.left + 2.0, r.top + 10.0, 3.0, 16.0),
                1.5,
                &theme.accent,
            );
        }
        glyph(
            canvas,
            text,
            icon,
            r.left + 14.0,
            r.top + 10.0,
            16.0,
            if selected { &theme.accent } else { &theme.text },
        )?;
        if left >= 100.0 {
            paragraph(
                canvas,
                text,
                crate::i18n::tr(key),
                label(14.0, 400),
                rect(r.left + 44.0, r.top + 9.0, r.right - r.left - 48.0, 20.0),
                &theme.text,
            )?;
        }
    }
    paragraph(
        canvas,
        text,
        crate::i18n::tr(if state.integrations {
            "settings.integrations"
        } else {
            "settings.general"
        }),
        label(20.0, 600),
        rect(left + 24.0, 24.0, state.size.0 - left - 48.0, 30.0),
        &theme.text,
    )?;
    unsafe {
        canvas.context.PushAxisAlignedClip(
            &rect(
                left,
                64.0,
                (state.size.0 - left).max(1.0),
                (state.size.1 - 64.0).max(1.0),
            ),
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
    }
    let result = (|| -> Result<()> {
        for &(y, key) in &state.groups {
            paragraph(
                canvas,
                text,
                crate::i18n::tr(key),
                label(14.0, 600),
                rect(left + 24.0, y, state.size.0 - left - 48.0, 22.0),
                &theme.text,
            )?;
        }
        for row in &state.rows {
            let r = row.bounds;
            if r.bottom < 64.0 || r.top >= state.size.1 {
                continue;
            }
            rounded(canvas, r, 4.0, &theme.card_border);
            rounded(
                canvas,
                rect(
                    r.left + 1.0,
                    r.top + 1.0,
                    r.right - r.left - 2.0,
                    r.bottom - r.top - 2.0,
                ),
                3.0,
                &theme.settings_card,
            );
            glyph(
                canvas,
                text,
                row.icon,
                r.left + 18.0,
                if r.right - r.left < 360.0 {
                    r.top + 18.0
                } else {
                    (r.top + r.bottom) / 2.0 - 10.0
                },
                20.0,
                &theme.text,
            )?;
            let stacked = r.right - r.left < 360.0;
            let room = (r.right
                - r.left
                - 56.0
                - if stacked {
                    16.0
                } else {
                    state.control_width(row) + 32.0
                })
            .max(24.0);
            let block = row.title_height
                + if row.description_height > 0.0 {
                    4.0 + row.description_height
                } else {
                    0.0
                };
            let y = if stacked {
                r.top + 16.0
            } else {
                (r.top + r.bottom - block) / 2.0
            };
            paragraph(
                canvas,
                text,
                crate::i18n::tr(row.title),
                label(14.0, 400),
                rect(r.left + 56.0, y, room, row.title_height),
                &theme.text,
            )?;
            if row.description_height > 0.0 {
                paragraph(
                    canvas,
                    text,
                    &row.description,
                    label(12.0, 400),
                    rect(
                        r.left + 56.0,
                        y + row.title_height + 4.0,
                        room,
                        row.description_height,
                    ),
                    &theme.text_secondary,
                )?;
            }
            let control = state.bounds(row.target);
            match row.control {
                Control::Toggle(index) => {
                    let on = match index {
                        0 => state.enabled,
                        1 => state.zoxide.enabled,
                        _ => state.zoxide.record,
                    };
                    let available = index == 0 || state.zoxide.available;
                    let caption = crate::i18n::tr(if on {
                        "settings.state.on"
                    } else {
                        "settings.state.off"
                    });
                    let width = text.layout(caption, label(14.0, 400), 4096.0)?.advance;
                    let switch = rect(control.right - 40.0, control.top, 40.0, 20.0);
                    paragraph(
                        canvas,
                        text,
                        caption,
                        label(14.0, 400),
                        rect(switch.left - 12.0 - width, control.top, width + 1.0, 20.0),
                        if available {
                            &theme.text
                        } else {
                            &theme.text_disabled
                        },
                    )?;
                    super::super::toggle::draw(
                        canvas,
                        switch,
                        state.toggles[index].frame,
                        available,
                        theme,
                    );
                }
                Control::Expand => {
                    glyph(
                        canvas,
                        text,
                        if state.expanded {
                            "\u{e70e}"
                        } else {
                            "\u{e70d}"
                        },
                        control.left + 10.0,
                        control.top + 11.0,
                        12.0,
                        &theme.text,
                    )?;
                }
                Control::Dropdown | Control::Button => {
                    rounded(canvas, control, 4.0, &theme.control_border);
                    rounded(
                        canvas,
                        rect(
                            control.left + 1.0,
                            control.top + 1.0,
                            control.right - control.left - 2.0,
                            30.0,
                        ),
                        3.0,
                        if state.pressed == Some(row.target) {
                            &theme.control_pressed
                        } else if state.hover == Some(row.target) {
                            &theme.control_hover
                        } else {
                            &theme.control
                        },
                    );
                    let caption = match row.target {
                        Target::Startup => value(Target::StartupChoice(state.startup)),
                        Target::Open(source) => {
                            value(Target::OpenIn(source, state.opening.destination(source)))
                        }
                        Target::Restore => "settings.action.restore",
                        Target::ZoxideInstall if state.zoxide_winget => "settings.zoxide.install",
                        _ => "settings.action.open",
                    };
                    super::super::details::draw_line(
                        canvas,
                        text,
                        crate::i18n::tr(caption),
                        label(14.0, 400),
                        rect(
                            control.left + 12.0,
                            control.top,
                            control.right
                                - control.left
                                - if matches!(row.control, Control::Dropdown) {
                                    40.0
                                } else {
                                    24.0
                                },
                            32.0,
                        ),
                        0.0,
                        &theme.text,
                    )?;
                    if matches!(row.control, Control::Dropdown) {
                        glyph(
                            canvas,
                            text,
                            "\u{e70d}",
                            control.right - 22.0,
                            control.top + 11.0,
                            10.0,
                            &theme.text_secondary,
                        )?;
                    }
                }
            }
        }
        for (r, value) in &state.notices {
            paragraph(
                canvas,
                text,
                value,
                label(12.0, 400),
                *r,
                &theme.settings_error,
            )?;
        }
        if cues {
            if let Focus::Settings(target) = focus {
                let r = state.bounds(target);
                super::super::chrome::focus_ring_radius(
                    canvas,
                    rect(
                        r.left - 2.0,
                        r.top - 2.0,
                        r.right - r.left + 4.0,
                        r.bottom - r.top + 4.0,
                    ),
                    theme,
                    4.0,
                );
            }
        }
        Ok(())
    })();
    unsafe {
        canvas.context.PopAxisAlignedClip();
    }
    result?;
    if state.maximum > 0.0 {
        let height =
            ((state.size.1 - 76.0).powi(2) / (state.size.1 - 76.0 + state.maximum)).max(24.0);
        let y = 70.0 + (state.size.1 - 82.0 - height) * state.offset / state.maximum;
        rounded(
            canvas,
            rect(state.size.0 - 7.0, y, 3.0, height),
            1.5,
            &theme.xaml_thumb,
        );
    }
    if state.dropdown.is_some() {
        let r = state.dropdown_bounds();
        let mut shadow = theme.flyout_shadow;
        shadow.a *= 0.5;
        rounded(
            canvas,
            rect(
                r.left - 3.0,
                r.top + 2.0,
                r.right - r.left + 6.0,
                r.bottom - r.top + 4.0,
            ),
            9.0,
            &shadow,
        );
        rounded(canvas, r, 8.0, &theme.flyout_border);
        rounded(
            canvas,
            rect(
                r.left + 1.0,
                r.top + 1.0,
                r.right - r.left - 2.0,
                r.bottom - r.top - 2.0,
            ),
            7.0,
            &theme.flyout,
        );
        for (index, target) in state.choices().into_iter().enumerate() {
            let b = state.bounds(target);
            if state.choice == index || state.hover == Some(target) {
                rounded(canvas, b, 4.0, &theme.subtle_hover);
            }
            let selected = match target {
                Target::StartupChoice(v) => state.startup == v,
                Target::OpenIn(source, v) => state.opening.destination(source) == v,
                _ => false,
            };
            if selected {
                rounded(
                    canvas,
                    rect(b.left + 2.0, b.top + 10.0, 3.0, 16.0),
                    1.5,
                    &theme.accent,
                );
            }
            super::super::details::draw_line(
                canvas,
                text,
                crate::i18n::tr(value(target)),
                label(14.0, 400),
                rect(b.left + 12.0, b.top, b.right - b.left - 24.0, 36.0),
                0.0,
                &theme.text,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_cards_keep_wrapped_labels_clear_of_controls_and_clip_popup_to_viewport() {
        let mut text = TextEngine::new(96).unwrap();
        for width in [400.0, 640.0, 1100.0] {
            let mut state = State::default();
            state.recovery = true;
            state.error = "Long recoverable association error. ".repeat(8);
            state.layout((width, 420.0), &mut text).unwrap();
            assert!(state
                .rows
                .windows(2)
                .all(|rows| rows[0].bounds.bottom < rows[1].bounds.top));
            for row in &state.rows {
                let control = state.bounds(row.target);
                assert!(control.left >= row.bounds.left + 16.0);
                assert!(control.right <= row.bounds.right - 15.0);
                assert!(control.top >= row.bounds.top);
                assert!(control.bottom <= row.bounds.bottom);
                if row.bounds.right - row.bounds.left < 360.0 {
                    assert!(
                        row.bounds.top + 16.0 + row.title_height + row.description_height + 4.0
                            <= control.top
                    );
                } else {
                    assert!(row.bounds.left + 56.0 < control.left - 16.0);
                }
            }
            state.dropdown = Some(Target::Startup);
            let popup = state.dropdown_bounds();
            assert!(popup.top >= 64.0 && popup.bottom <= state.size.1);
            let first = state.bounds(Target::StartupChoice(Startup::NewTab));
            assert_eq!(
                state.hit((first.left + 10.0, first.top + 98.0)),
                Some(Target::StartupChoice(Startup::NewTab))
            );
            state.integrations = true;
            state.expanded = true;
            state.zoxide.available = false;
            state.layout((width, 420.0), &mut text).unwrap();
            assert!(!state
                .controls()
                .iter()
                .any(|t| matches!(t, Target::Zoxide(_))));
            state.zoxide.available = true;
            state.layout((width, 420.0), &mut text).unwrap();
            assert_eq!(
                state
                    .controls()
                    .iter()
                    .filter(|t| matches!(t, Target::Zoxide(_)))
                    .count(),
                2
            );
        }
    }
}
