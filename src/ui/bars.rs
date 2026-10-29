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
                    let geometry = factory.CreatePathGeometry()?;
                    let sink = geometry.Open()?;
                    for (index, &(x, y)) in vertices.iter().enumerate() {
                        let mut point = D2D1_ARC_SEGMENT::default().point;
                        let (width, height) = if direction < 2 {
                            (8.0, 6.0)
                        } else {
                            (6.0, 8.0)
                        };
                        point.X =
                            if direction == 2 { 8.0 } else { 9.0 } + (x - width / 2.0) * scale;
                        point.Y = 9.0 + (y - height / 2.0) * scale;
                        if index == 0 {
                            sink.BeginFigure(point, D2D1_FIGURE_BEGIN_FILLED);
                        } else {
                            sink.AddLine(point);
                        }
                    }
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    sink.Close()?;
                    self.arrows.push(geometry);
                }
            }
        }
        Ok(())
    }
}

pub fn draw(
    canvas: &Canvas,
    assets: &mut Assets,
    theme: &Theme,
    state: &State,
    bar: Bar,
) -> Result<()> {
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
