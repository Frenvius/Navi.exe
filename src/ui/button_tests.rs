use super::*;
use crate::ui::render::{test_pixels, Renderer};

pub(crate) fn capture(
    name: &str,
    size: (u32, u32),
    dark: bool,
    paint: impl FnOnce(&Canvas, &mut TextEngine, &Theme) -> Result<()>,
) -> Result<Vec<u8>> {
    crate::shell::com_init();
    let renderer = Renderer::new()?;
    let mut text = TextEngine::new(96)?;
    let mut theme = Theme::new(dark);
    let accent = crate::shell::nav::accent_fill(dark);
    theme.accent = D2D1_COLOR_F {
        r: ((accent >> 16) & 255) as f32 / 255.0,
        g: ((accent >> 8) & 255) as f32 / 255.0,
        b: (accent & 255) as f32 / 255.0,
        a: 1.0,
    };
    let pixels = test_pixels(&renderer.canvas, size, || {
        crate::ui::details::fill(
            &renderer.canvas,
            rect(0.0, 0.0, size.0 as f32, size.1 as f32),
            &theme.settings_card,
        );
        paint(&renderer.canvas, &mut text, &theme)
    })?;
    if let Some(directory) = std::env::var_os("NAVI_BUTTON_CAPTURE_DIR") {
        let path = std::path::Path::new(&directory).join(format!("{name}.ppm"));
        let mut image = format!("P6\n{} {}\n255\n", size.0, size.1).into_bytes();
        image.extend_from_slice(&pixels);
        std::fs::write(path, image).map_err(|error| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
        })?;
    }
    Ok(pixels)
}

pub(crate) fn states() -> [(&'static str, State); 5] {
    [
        ("rest", State::default()),
        (
            "hover",
            State {
                hovered: true,
                ..Default::default()
            },
        ),
        (
            "pressed",
            State {
                hovered: true,
                pressed: true,
                ..Default::default()
            },
        ),
        (
            "disabled",
            State {
                enabled: false,
                ..Default::default()
            },
        ),
        (
            "focus",
            State {
                focused: true,
                ..Default::default()
            },
        ),
    ]
}

#[test]
fn rendered_states_keep_long_text_centered_with_padding_in_both_themes() -> Result<()> {
    for dark in [true, false] {
        let pixels = capture(
            if dark {
                "button-states-dark"
            } else {
                "button-states-light"
            },
            (680, 320),
            dark,
            |canvas, text, theme| {
                for (column, variant) in
                    [Variant::Standard, Variant::Accent].into_iter().enumerate()
                {
                    for (row, (name, state)) in states().into_iter().enumerate() {
                        let x = 20.0 + column as f32 * 330.0;
                        let y = 40.0 + row as f32 * 54.0;
                        text.draw(canvas, name, TEXT, (x, y - 24.0), &theme.text_secondary)?;
                        let mut button = Button::new("Restaurar associações originais");
                        button.variant = variant;
                        button.state = state;
                        assert!(button.width(text)? <= 300.0);
                        button.draw(canvas, text, theme, rect(x, y, 300.0, HEIGHT))?;
                    }
                }
                Ok(())
            },
        )?;
        let ink: Vec<_> = (46usize..66)
            .flat_map(|y| (24usize..316).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let pixel = &pixels[(y * 680 + x) * 3..][..3];
                if dark {
                    pixel.iter().all(|v| *v > 180)
                } else {
                    pixel.iter().all(|v| *v < 110)
                }
            })
            .map(|(x, _)| x)
            .collect();
        let left = *ink.iter().min().unwrap();
        let right = *ink.iter().max().unwrap() + 1;
        assert!(left >= 32 && right <= 308);
        assert!((left as i32 + right as i32 - 340).abs() <= 4);
        let fill = &pixels[(56 * 680 + 30) * 3..][..3];
        let bottom = &pixels[(71 * 680 + 30) * 3..][..3];
        assert!(bottom.iter().zip(fill).all(|(edge, body)| edge < body));
        if !dark {
            assert_eq!(bottom, &[214, 214, 214]);
        }
    }
    Ok(())
}

#[test]
fn disabled_ignores_hover_press_and_keyboard_focus_for_both_variants() -> Result<()> {
    let pack = |c: D2D1_COLOR_F| [c.r, c.g, c.b, c.a];
    for dark in [true, false] {
        for variant in [Variant::Standard, Variant::Accent] {
            let theme = Theme::new(dark);
            let disabled = State {
                enabled: false,
                ..Default::default()
            };
            let a = colors(&theme, variant, disabled);
            let b = colors(
                &theme,
                variant,
                State {
                    hovered: true,
                    pressed: true,
                    focused: true,
                    ..disabled
                },
            );
            assert_eq!(
                (pack(a.fill), pack(a.text), pack(a.border)),
                (pack(b.fill), pack(b.text), pack(b.border))
            );
            let rest = capture("disabled-rest", (180, 48), dark, |canvas, text, theme| {
                let mut button = Button::new("Instalando…");
                button.variant = variant;
                button.state = disabled;
                button.draw(canvas, text, theme, rect(8.0, 8.0, 164.0, HEIGHT))
            })?;
            let held = capture("disabled-held", (180, 48), dark, |canvas, text, theme| {
                let mut button = Button::new("Instalando…");
                button.variant = variant;
                button.state = State {
                    hovered: true,
                    pressed: true,
                    focused: true,
                    ..disabled
                };
                button.draw(canvas, text, theme, rect(8.0, 8.0, 164.0, HEIGHT))
            })?;
            assert_eq!(rest, held);
        }
    }
    Ok(())
}

#[test]
fn narrow_buttons_clip_overflow_inside_the_padding() -> Result<()> {
    for dark in [true, false] {
        let pixels = capture("narrow-button", (100, 48), dark, |canvas, text, theme| {
            Button::new("Restaurar associações originais").draw(
                canvas,
                text,
                theme,
                rect(10.0, 8.0, 80.0, HEIGHT),
            )
        })?;
        let background = &pixels[(24 * 100 + 16) * 3..][..3];
        for y in 14..34 {
            for x in 13..21 {
                assert_eq!(&pixels[(y * 100 + x) * 3..][..3], background);
            }
        }
    }
    Ok(())
}
