use super::{
    address::{rect, rounded, ProviderIcon},
    menu_layout::{Hit, Layout},
    render::Canvas,
    text::{Family, TextEngine, TextStyle},
    theme::Theme,
};
use crate::{
    app::menu_model::{self, Action, Icon, Menu},
    shell::cloud::Snapshot,
};
use std::sync::Arc;
use windows::{
    core::Result,
    Win32::{Foundation::HWND, Graphics::Direct2D::Common::*},
};

pub struct State {
    pub snapshot: Snapshot,
    sources: Vec<Option<Arc<Vec<u8>>>>,
    icons: Vec<ProviderIcon>,
}
impl State {
    pub fn new(snapshot: Snapshot) -> Self {
        let sources = snapshot
            .commands
            .iter()
            .map(|command| {
                let path = crate::shell::cloud::icon_path(&command.icon)?;
                match std::fs::read(path) {
                    Ok(bytes) if bytes.len() <= 4 * 1024 * 1024 => Some(Arc::new(bytes)),
                    Ok(_) => {
                        eprintln!("cloud icon exceeds the size limit");
                        None
                    }
                    Err(error) => {
                        eprintln!("cloud command icon: {error}");
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        let icons = (0..sources.len())
            .map(|_| ProviderIcon::default())
            .collect();
        Self {
            snapshot,
            sources,
            icons,
        }
    }
    pub fn prepare(&mut self, canvas: &Canvas, dpi: u32) -> Result<()> {
        for (icon, source) in self.icons.iter_mut().zip(&self.sources) {
            icon.prepare(canvas, source.as_ref(), dpi)?;
        }
        Ok(())
    }
    pub fn menu(&self) -> Menu {
        Menu {
            owner: "cloud",
            buttons: Vec::new(),
            items: self
                .snapshot
                .commands
                .iter()
                .enumerate()
                .map(|(slot, command)| {
                    let mut item = menu_model::item(
                        &command.label,
                        Icon::None,
                        Action::CloudCommand(self.snapshot.revision, slot),
                    )
                    .disabled(command.state != 0 || slot == 2);
                    item.tip = command.description.clone();
                    item
                })
                .collect(),
        }
    }
    pub fn height(&self) -> f32 {
        202.0
            + if self.snapshot.more.is_empty() {
                0.0
            } else {
                48.0
            }
    }
    pub fn bounds(&self, slot: usize) -> D2D_RECT_F {
        if slot == 0 {
            rect(7.0, 6.0, 306.0, 37.0)
        } else {
            rect(
                if slot == 1 {
                    9.0
                } else {
                    217.0 + (slot as f32 - 3.0) * 36.0
                },
                160.0
                    + if self.snapshot.more.is_empty() {
                        0.0
                    } else {
                        48.0
                    },
                if slot == 1 { 187.0 } else { 32.0 },
                32.0,
            )
        }
    }
    pub fn layout(&self) -> Layout {
        Layout {
            width: 320.0,
            height: self.height(),
            rows: self
                .snapshot
                .commands
                .iter()
                .enumerate()
                .map(|(slot, _)| {
                    let r = self.bounds(slot);
                    (r.top, r.bottom - r.top)
                })
                .collect(),
            row: 32.0,
            padding: 0.0,
            label: 0.0,
            icon: 0.0,
            toggle_column: false,
            dropdown: true,
            overflow: false,
            context: false,
            buttons_height: 0.0,
            buttons_bottom: false,
            shortcut_width: 0.0,
        }
    }
    pub fn hit(&self, point: (f32, f32)) -> Option<Hit> {
        self.snapshot
            .commands
            .iter()
            .enumerate()
            .find_map(|(slot, command)| {
                (slot != 2
                    && command.state == 0
                    && super::address::contains(self.bounds(slot), point))
                .then_some(Hit::Row(slot))
            })
    }
    pub fn draw(
        &self,
        canvas: &Canvas,
        text: &mut TextEngine,
        theme: &Theme,
        hover: Option<Hit>,
        keyboard: bool,
    ) -> Result<()> {
        rounded(
            canvas,
            rect(0.0, 0.0, 320.0, self.height()),
            8.0,
            &theme.cloud_body,
        );
        super::details::fill(canvas, rect(1.0, 49.0, 318.0, 1.0), &theme.cloud_divider);
        super::details::fill(
            canvas,
            rect(1.0, 150.0, 318.0, self.height() - 151.0),
            &theme.cloud_footer,
        );
        let account = TextStyle::new(Family::SegoeXaml, 14.0, 600);
        super::details::draw_line(
            canvas,
            text,
            &self.snapshot.account,
            account,
            rect(15.0, 71.0, 290.0, 22.0),
            0.0,
            &theme.text,
        )?;
        super::details::fill(canvas, rect(15.0, 101.0, 290.0, 1.0), &theme.cloud_track);
        let color = self.snapshot.color;
        let color = D2D1_COLOR_F {
            r: color[1] as f32 / 255.0,
            g: color[2] as f32 / 255.0,
            b: color[3] as f32 / 255.0,
            a: color[0] as f32 / 255.0,
        };
        let fraction = if self.snapshot.total == 0 {
            0.0
        } else {
            (self.snapshot.used as f64 / self.snapshot.total as f64).clamp(0.0, 1.0) as f32
        };
        super::details::fill(canvas, rect(15.0, 101.0, 290.0 * fraction, 1.0), &color);
        super::details::draw_line(
            canvas,
            text,
            &self.snapshot.quota,
            TextStyle::new(Family::Segoe, 12.0, 400).with_ideal_advances(),
            rect(15.0, 107.0, 290.0, 20.0),
            0.0,
            &theme.text,
        )?;
        if !self.snapshot.more.is_empty() {
            text.draw_wrapped(
                canvas,
                &self.snapshot.more,
                TextStyle::new(Family::Variable, 12.0, 400),
                rect(15.0, 148.0, 290.0, 44.0),
                &theme.text,
            )?;
        }
        for (slot, command) in self.snapshot.commands.iter().enumerate() {
            if slot == 2 || command.state == 2 {
                continue;
            }
            let r = self.bounds(slot);
            let hot = hover == Some(Hit::Row(slot));
            if hot {
                rounded(canvas, r, 4.0, &theme.subtle_hover);
            } else if slot == 1 {
                rounded(canvas, r, 4.0, &theme.cloud_manage);
            }
            let color = if command.state == 1 {
                &theme.text_disabled
            } else {
                &theme.text
            };
            if slot == 1 {
                super::details::draw_line(
                    canvas,
                    text,
                    &command.label,
                    TextStyle::new(Family::Text, 14.0, 400),
                    rect(r.left + 12.0, r.top - 1.0, 163.0, 32.0),
                    0.0,
                    color,
                )?;
            } else {
                self.icons[slot].draw(
                    canvas,
                    rect(
                        r.left + if slot == 0 { 6.0 } else { 8.0 },
                        r.top + if slot == 0 { 10.0 } else { 8.0 },
                        16.0,
                        16.0,
                    ),
                    if command.state == 1 { 0.4 } else { 1.0 },
                );
                if slot == 0 {
                    super::details::draw_line(
                        canvas,
                        text,
                        &command.label,
                        TextStyle::new(Family::SegoeXaml, 14.0, 400),
                        rect(r.left + 37.0, r.top + 7.0, 242.0, 20.0),
                        0.0,
                        color,
                    )?;
                    text.draw(
                        canvas,
                        "\u{e974}",
                        TextStyle::new(Family::Icons, 16.0, 400),
                        (286.0, 17.0),
                        color,
                    )?;
                }
            }
            if hot && keyboard {
                super::chrome::focus_ring_radius(
                    canvas,
                    rect(
                        r.left - 3.0,
                        r.top - 3.0,
                        r.right - r.left + 6.0,
                        r.bottom - r.top + 6.0,
                    ),
                    theme,
                    4.0,
                );
            }
        }
        Ok(())
    }
}
pub fn show(hwnd: HWND) -> Result<()> {
    let state = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| {
            Some((
                window.address.cloud.clone()?,
                window.address.bounds(super::address::Target::Cloud)?,
                window.chrome.cues,
            ))
        })
    });
    let Some((snapshot, bounds, keyboard)) = state else {
        return Ok(());
    };
    let Some(context) = super::menu_actions::context(hwnd) else {
        return Ok(());
    };
    let model = State::new(snapshot);
    super::menu::show(
        hwnd,
        context,
        model.menu(),
        super::menu::Anchor::Point(bounds.left, bounds.bottom + 50.0),
        if keyboard {
            super::menu::Origin::Keyboard
        } else {
            super::menu::Origin::Pointer
        },
    )
}
pub fn invoke(revision: u64, slot: usize) -> Result<()> {
    let sender = super::WINDOW.with(|state| {
        state.borrow().as_ref().and_then(|window| {
            let snapshot = window.address.cloud.as_ref()?;
            if snapshot.path != window.tabs.active().location.tab.path()
                || snapshot.revision != revision
                || !snapshot
                    .commands
                    .get(slot)
                    .is_some_and(|command| command.state == 0)
            {
                return None;
            }
            window.address_worker.cloud_sender()
        })
    });
    if let Some(sender) = sender {
        sender
            .send(crate::shell::cloud::Request::Invoke(revision, slot))
            .map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_slots_keep_disabled_hidden_and_extended_footer_geometry() {
        let mut snapshot = Snapshot {
            revision: 7,
            commands: (0..6)
                .map(|slot| crate::shell::cloud::Command {
                    label: slot.to_string(),
                    state: if slot == 4 { 1 } else { 0 },
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        snapshot.commands[5].state = 2;
        let mut state = State::new(snapshot);
        assert_eq!(state.height(), 202.0);
        assert_eq!(state.hit((20.0, 20.0)), Some(Hit::Row(0)));
        assert_eq!(state.hit((20.0, 175.0)), Some(Hit::Row(1)));
        assert_eq!(state.hit((225.0, 175.0)), Some(Hit::Row(3)));
        assert_eq!(state.hit((261.0, 175.0)), None);
        assert_eq!(state.hit((298.0, 175.0)), None);
        assert!(state.menu().items[2].entry.disabled);
        state.snapshot.more = "Provider notice".into();
        assert_eq!(state.height(), 250.0);
        assert_eq!(state.hit((20.0, 175.0)), None);
        assert_eq!(state.hit((20.0, 223.0)), Some(Hit::Row(1)));
        assert!(matches!(
            state.menu().items[1].action,
            Action::CloudCommand(7, 1)
        ));
    }
}
