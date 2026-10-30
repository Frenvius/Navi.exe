use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

use super::details::fill;
use super::render::Canvas;
use super::scroll::{Axis, Bar, Hit, Part, State};
use super::theme::Theme;

#[derive(Default)]
pub struct Assets {
    arrows: Vec<ID2D1PathGeometry>,
    overlay: Vec<ID2D1PathGeometry>,
}

impl Assets {
    fn initialize(&mut self, canvas: &Canvas) -> Result<()> {
        if !self.arrows.is_empty() {
            return Ok(());
        }
        unsafe {
            let factory = canvas.context.GetFactory()?;
            for direction in 0..4 {
                for scale in [1.0, 1.1, 0.9] {
                    let vertices: &[(f32, f32)] = match direction {
                        0 => &[
                            (3.3, 0.0),
                            (4.7, 0.0),
                            (8.0, 4.6),
                            (8.0, 6.0),
                            (0.0, 6.0),
                            (0.0, 4.6),
                        ],
                        1 => &[
                            (0.0, 0.0),
                            (8.0, 0.0),
                            (8.0, 1.4),
                            (4.7, 6.0),
                            (3.3, 6.0),
                            (0.0, 1.4),
                        ],
                        2 => &[
                            (0.0, 3.3),
                            (0.0, 4.7),
                            (4.6, 8.0),
                            (6.0, 8.0),
                            (6.0, 0.0),
                            (4.6, 0.0),
                        ],
                        _ => &[
                            (0.0, 0.0),
                            (1.4, 0.0),
                            (6.0, 3.3),
                            (6.0, 4.7),
                            (1.4, 8.0),
                            (0.0, 8.0),
                        ],
                    };
                    let (width, height) = if direction < 2 {
                        (8.0, 6.0)
                    } else {
                        (6.0, 8.0)
                    };
                    let points = vertices.iter().map(|&(x, y)| {
                        (
                            if direction == 2 { 8.0 } else { 9.0 } + (x - width / 2.0) * scale,
                            9.0 + (y - height / 2.0) * scale,
                        )
                    });
                    self.arrows.push(polygon(&factory, points)?);
                }
            }
            for vertices in OVERLAY_ARROWS {
                let points = vertices.iter().map(|&(x, y)| (x + 2.0, y + 6.0));
                self.overlay.push(polygon(&factory, points)?);
            }
        }
        Ok(())
    }
}

fn polygon(
    factory: &ID2D1Factory,
    points: impl Iterator<Item = (f32, f32)>,
) -> Result<ID2D1PathGeometry> {
    let mut points = points.map(|(x, y)| {
        let mut point = D2D1_ARC_SEGMENT::default().point;
        point.X = x;
        point.Y = y;
        point
    });
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        if let Some(first) = points.next() {
            sink.BeginFigure(first, D2D1_FIGURE_BEGIN_FILLED);
            points.for_each(|point| sink.AddLine(point));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        sink.Close()?;
        Ok(geometry)
    }
}

const OVERLAY_ARROWS: [[(f32, f32); 6]; 2] = [
    [
        (3.05, 0.0),
        (4.95, 0.0),
        (7.89, 4.3),
        (7.89, 6.0),
        (0.11, 6.0),
        (0.11, 4.3),
    ],
    [
        (0.11, 0.0),
        (7.89, 0.0),
        (7.89, 1.7),
        (4.95, 6.0),
        (3.05, 6.0),
        (0.11, 1.7),
    ],
];

fn rounded(canvas: &Canvas, rect: D2D_RECT_F, radius: (f32, f32), color: &D2D1_COLOR_F) {
    unsafe {
        canvas.brush.SetColor(color);
        canvas.context.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect,
                radiusX: radius.0,
                radiusY: radius.1,
            },
            &canvas.brush,
        );
    }
}

fn overlay_thumb(theme: &Theme) -> D2D1_COLOR_F {
    if theme.content.r < 0.5 {
        D2D1_COLOR_F { a: 0.5442, ..WHITE }
    } else {
        D2D1_COLOR_F { a: 0.4458, ..BLACK }
    }
}

fn draw_indicator(canvas: &Canvas, theme: &Theme, bar: Bar, opacity: f32) {
    let rect = D2D_RECT_F {
        left: 7.0,
        top: bar.thumb_pos,
        right: 9.0,
        bottom: bar.thumb_pos + bar.thumb_size,
    };
    let color = overlay_thumb(theme);
    let color = D2D1_COLOR_F {
        a: color.a * opacity,
        ..color
    };
    rounded(canvas, rect, (1.0, 3.0), &color);
}

fn draw_overlay(canvas: &Canvas, assets: &mut Assets, theme: &Theme, bar: Bar) -> Result<()> {
    let dark = theme.content.r < 0.5;
    let thumb = overlay_thumb(theme);
    let bottom = bar.thumb_pos + bar.thumb_size;
    let track = D2D_RECT_F {
        left: 0.0,
        top: 1.0,
        right: OVERLAY_TRACK,
        bottom: bar.length - 1.0,
    };
    rounded(
        canvas,
        track,
        (OVERLAY_TRACK / 2.0, OVERLAY_TRACK / 2.0),
        if dark { &TRACK_DARK } else { &TRACK_LIGHT },
    );
    let rect = D2D_RECT_F {
        left: 3.0,
        top: bar.thumb_pos,
        right: 9.0,
        bottom,
    };
    rounded(canvas, rect, (3.0, 3.0), &thumb);
    assets.initialize(canvas)?;
    for (arrow, at) in assets.overlay.iter().zip([0.0, bar.length - 18.0]) {
        unsafe {
            let mut original = Default::default();
            canvas.context.GetTransform(&mut original);
            let mut transform = original;
            transform.M32 += at;
            canvas.context.SetTransform(&transform);
            canvas.brush.SetColor(&thumb);
            canvas.context.FillGeometry(arrow, &canvas.brush, None);
            canvas.context.SetTransform(&original);
        }
    }
    Ok(())
}

const OVERLAY_TRACK: f32 = 12.0;
const WHITE: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const BLACK: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
const TRACK_DARK: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 45.0 / 255.0,
    g: 45.0 / 255.0,
    b: 45.0 / 255.0,
    a: 1.0,
};
const TRACK_LIGHT: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 250.0 / 255.0,
    g: 250.0 / 255.0,
    b: 250.0 / 255.0,
    a: 1.0,
};

pub fn draw(
    canvas: &Canvas,
    assets: &mut Assets,
    theme: &Theme,
    state: &State,
    bar: Bar,
) -> Result<()> {
    if bar.overlay && !state.hot(bar.axis) {
        draw_indicator(canvas, theme, bar, state.overlay_opacity());
        return Ok(());
    }
    if bar.overlay {
        return draw_overlay(canvas, assets, theme, bar);
    }
    let rect = |along: f32, across: f32, length: f32, thickness: f32| match bar.axis {
        Axis::Vertical => D2D_RECT_F {
            left: across,
            top: along,
            right: across + thickness,
            bottom: along + length,
        },
        Axis::Horizontal => D2D_RECT_F {
            left: along,
            top: across,
            right: along + length,
            bottom: across + thickness,
        },
    };
    fill(canvas, rect(0.0, 0.0, bar.length, 17.0), &theme.track);
    fill(canvas, rect(0.0, 0.0, bar.length, 1.0), &theme.track_edge);
    let hot = state.hot(bar.axis);
    let thickness = if hot { 6.0 } else { 2.0 };
    let across = 9.0 - thickness / 2.0;
    if hot {
        unsafe {
            canvas.brush.SetColor(&theme.thumb);
            canvas.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(bar.thumb_pos, across, bar.thumb_size, thickness),
                    radiusX: 3.0,
                    radiusY: 3.0,
                },
                &canvas.brush,
            );
        }
    } else {
        fill(
            canvas,
            rect(bar.thumb_pos + 1.0, across, bar.thumb_size - 2.0, thickness),
            &theme.thumb,
        );
        let mut end = theme.thumb;
        end.a *= 0.873;
        for at in [bar.thumb_pos, bar.thumb_pos + bar.thumb_size - 1.0] {
            fill(canvas, rect(at, across, 1.0, thickness), &end);
        }
    }
    if !hot {
        return Ok(());
    }
    assets.initialize(canvas)?;
    for (part, at) in [(Part::Start, 0.0), (Part::End, bar.length - 17.0)] {
        let hovered = state.hover
            == Some(Hit {
                axis: bar.axis,
                part,
            });
        let pressed = hovered
            && state.pressed
                == Some(Hit {
                    axis: bar.axis,
                    part,
                });
        let direction = match (bar.axis, part) {
            (Axis::Vertical, Part::Start) => 0,
            (Axis::Vertical, _) => 1,
            (Axis::Horizontal, Part::Start) => 2,
            _ => 3,
        };
        unsafe {
            let mut original = Default::default();
            canvas.context.GetTransform(&mut original);
            let mut transform = original;
            match bar.axis {
                Axis::Vertical => transform.M32 += at,
                Axis::Horizontal => transform.M31 += at,
            };
            canvas.context.SetTransform(&transform);
            canvas.brush.SetColor(if hovered {
                &theme.caret_hot
            } else {
                &theme.caret
            });
            canvas.context.FillGeometry(
                &assets.arrows[direction * 3 + if pressed { 2 } else { usize::from(hovered) }],
                &canvas.brush,
                None,
            );
            canvas.context.SetTransform(&original);
        }
    }
    Ok(())
}
