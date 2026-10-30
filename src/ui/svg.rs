use super::render::Canvas;
use super::theme::Theme;
use windows::core::{w, Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

const NAMES: [&str; 14] = [
    "windows.newitem",
    "windows.cut",
    "windows.copy",
    "windows.paste",
    "windows.rename",
    "Windows.ModernShare",
    "windows.ribbondelete",
    "SortBy",
    "windows.iconsize.details",
    "windows.previewpane",
    "addressbar.desktop",
    "addressbar.home",
    "addressbar.gallery",
    "addressbar.network",
];

pub fn names(mode: i32, collection: Option<&'static str>) -> Vec<&'static str> {
    let mut names = NAMES.to_vec();
    names[8] = crate::app::VIEW_MODES
        .iter()
        .find(|item| item.3 == mode)
        .map_or(NAMES[8], |item| item.2);
    if let Some(collection) = collection {
        names.push(collection);
    }
    names
}

#[derive(Default)]
pub struct Assets {
    atlas: Option<Atlas>,
}

struct Atlas {
    bitmap: ID2D1Bitmap1,
    light: bool,
    dpi: u32,
    cell: usize,
    padding: usize,
    names: Vec<&'static str>,
}

struct PaintedPath {
    geometry: ID2D1Geometry,
    color: D2D1_COLOR_F,
}

fn paths(canvas: &Canvas, node: &ID2D1SvgElement, result: &mut Vec<PaintedPath>) -> Result<()> {
    unsafe {
        let mut tag = vec![0u16; node.GetTagNameLength() as usize + 1];
        node.GetTagName(&mut tag)?;
        match String::from_utf16_lossy(&tag[..tag.len() - 1]).as_str() {
            "defs" => return Ok(()),
            "svg" | "g" => {}
            "path" => {
                let data: ID2D1SvgPathData = node.GetAttributeValue(w!("d"))?;
                let mut mode = D2D1_FILL_MODE_WINDING;
                if node.IsAttributeSpecified(w!("fill-rule"), None).as_bool() {
                    node.GetAttributeValue2(
                        w!("fill-rule"),
                        D2D1_SVG_ATTRIBUTE_POD_TYPE_FILL_MODE,
                        (&mut mode as *mut D2D1_FILL_MODE).cast(),
                        4,
                    )?;
                }
                let geometry = data.CreatePathGeometry(mode)?;
                if node.IsAttributeSpecified(w!("fill"), None).as_bool() {
                    let fill: ID2D1SvgPaint = node.GetAttributeValue(w!("fill"))?;
                    if fill.GetPaintType() == D2D1_SVG_PAINT_TYPE_COLOR {
                        let mut color = fill.GetColor();
                        if node
                            .IsAttributeSpecified(w!("fill-opacity"), None)
                            .as_bool()
                        {
                            let mut opacity = 1.0f32;
                            node.GetAttributeValue2(
                                w!("fill-opacity"),
                                D2D1_SVG_ATTRIBUTE_POD_TYPE_FLOAT,
                                (&mut opacity as *mut f32).cast(),
                                4,
                            )?;
                            color.a *= opacity;
                        }
                        result.push(PaintedPath {
                            geometry: geometry.cast()?,
                            color,
                        });
                    } else if fill.GetPaintType() != D2D1_SVG_PAINT_TYPE_NONE {
                        return Err(windows::core::Error::new(
                            windows::Win32::Foundation::E_NOTIMPL,
                            "Command SVG fill is not a solid color",
                        ));
                    }
                }
                if node.IsAttributeSpecified(w!("stroke"), None).as_bool() {
                    let stroke: ID2D1SvgPaint = node.GetAttributeValue(w!("stroke"))?;
                    if stroke.GetPaintType() != D2D1_SVG_PAINT_TYPE_COLOR {
                        return Err(windows::core::Error::new(
                            windows::Win32::Foundation::E_NOTIMPL,
                            "Command SVG stroke is not a solid color",
                        ));
                    }
                    let mut cap = D2D1_SVG_LINE_CAP_BUTT;
                    if node
                        .IsAttributeSpecified(w!("stroke-linecap"), None)
                        .as_bool()
                    {
                        node.GetAttributeValue2(
                            w!("stroke-linecap"),
                            D2D1_SVG_ATTRIBUTE_POD_TYPE_LINE_CAP,
                            (&mut cap as *mut D2D1_SVG_LINE_CAP).cast(),
                            4,
                        )?;
                    }
                    let end = if cap == D2D1_SVG_LINE_CAP_ROUND {
                        D2D1_CAP_STYLE_ROUND
                    } else {
                        D2D1_CAP_STYLE_FLAT
                    };
                    let style = canvas.context.GetFactory()?.CreateStrokeStyle(
                        &D2D1_STROKE_STYLE_PROPERTIES {
                            startCap: end,
                            endCap: end,
                            dashCap: end,
                            miterLimit: 4.0,
                            ..Default::default()
                        },
                        None,
                    )?;
                    let outline = canvas.context.GetFactory()?.CreatePathGeometry()?;
                    let sink = outline.Open()?;
                    let mut width = D2D1_SVG_LENGTH {
                        value: 1.0,
                        units: D2D1_SVG_LENGTH_UNITS_NUMBER,
                    };
                    if node
                        .IsAttributeSpecified(w!("stroke-width"), None)
                        .as_bool()
                    {
                        node.GetAttributeValue2(
                            w!("stroke-width"),
                            D2D1_SVG_ATTRIBUTE_POD_TYPE_LENGTH,
                            (&mut width as *mut D2D1_SVG_LENGTH).cast(),
                            std::mem::size_of::<D2D1_SVG_LENGTH>() as u32,
                        )?;
                    }
                    geometry.Widen(width.value, &style, None, 0.25, &sink)?;
                    sink.Close()?;
                    result.push(PaintedPath {
                        geometry: outline.cast()?,
                        color: stroke.GetColor(),
                    });
                }
                return Ok(());
            }
            _ => {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_NOTIMPL,
                    "Command SVG contains an unsupported shape",
                ))
            }
        }
        if node.HasChildren().as_bool() {
            let mut child = node.GetFirstChild()?;
            let last = node.GetLastChild()?;
            loop {
                paths(canvas, &child, result)?;
                if child == last {
                    break;
                }
                child = node.GetNextChild(&child)?;
            }
        }
        Ok(())
    }
}

impl Assets {
    pub fn prepare(
        &mut self,
        canvas: &Canvas,
        theme: &Theme,
        dpi: u32,
        names: &[&'static str],
    ) -> Result<()> {
        let light = theme.content.r > 0.5;
        if self
            .atlas
            .as_ref()
            .is_some_and(|atlas| atlas.light == light && atlas.dpi == dpi && atlas.names == names)
        {
            return Ok(());
        }
        super::memory::checkpoint("command-cpu-atlas-before");
        let scale = dpi as f64 / 96.0;
        let cell = (20.0 * scale).ceil() as usize;
        let padding = (2.0 * scale).ceil() as usize;
        let width = cell * names.len();
        let height = cell * 2;
        let mut rgba = vec![[0.0f64; 4]; width * height];
        unsafe {
            let context: ID2D1DeviceContext5 = canvas.context.cast()?;
            for (index, name) in names.iter().enumerate() {
                let bytes = std::fs::read(crate::shell::icons::command_svg_path(name)).map_err(
                    |error| {
                        windows::core::Error::new(
                            windows::Win32::Foundation::E_FAIL,
                            error.to_string(),
                        )
                    },
                )?;
                let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(&bytes))
                    .ok_or_else(windows::core::Error::from_win32)?;
                let document = context.CreateSvgDocument(
                    &stream,
                    D2D_SIZE_F {
                        width: 16.0,
                        height: 16.0,
                    },
                )?;
                let mut shapes = Vec::new();
                let root = document.GetRoot()?;
                let mut viewbox = D2D1_SVG_VIEWBOX {
                    x: 0.0,
                    y: 0.0,
                    width: 16.0,
                    height: 16.0,
                };
                if root.IsAttributeSpecified(w!("viewBox"), None).as_bool() {
                    root.GetAttributeValue2(
                        w!("viewBox"),
                        D2D1_SVG_ATTRIBUTE_POD_TYPE_VIEWBOX,
                        (&mut viewbox as *mut D2D1_SVG_VIEWBOX).cast(),
                        std::mem::size_of::<D2D1_SVG_VIEWBOX>() as u32,
                    )?;
                }
                if viewbox.width <= 0.0 || viewbox.height <= 0.0 {
                    return Err(windows::Win32::Foundation::E_INVALIDARG.into());
                }
                let fit = (16.0 / viewbox.width as f64).min(16.0 / viewbox.height as f64);
                let offset = [
                    (16.0 - viewbox.width as f64 * fit) / 2.0,
                    (16.0 - viewbox.height as f64 * fit) / 2.0,
                ];
                paths(canvas, &root, &mut shapes)?;
                for shape in shapes {
                    let mut coverage = vec![0.0; cell * cell];
                    for triangle in super::coverage::triangles(
                        &shape.geometry,
                        D2D1_DEFAULT_FLATTENING_TOLERANCE / (scale * fit) as f32,
                    )? {
                        super::coverage::triangle(
                            [triangle.point1, triangle.point2, triangle.point3].map(|value| {
                                [
                                    ((value.X - viewbox.x) as f64 * fit + offset[0]) * scale
                                        + padding as f64,
                                    ((value.Y - viewbox.y) as f64 * fit + offset[1]) * scale
                                        + padding as f64,
                                ]
                            }),
                            cell,
                            &mut coverage,
                        );
                    }
                    for (row, opacity) in [1.0, 0.36].into_iter().enumerate() {
                        for y in 0..cell {
                            for x in 0..cell {
                                let alpha = coverage[y * cell + x].clamp(0.0, 1.0)
                                    * shape.color.a as f64
                                    * opacity;
                                if alpha == 0.0 {
                                    continue;
                                }
                                let target = &mut rgba[(row * cell + y) * width + index * cell + x];
                                for (channel, color) in
                                    [shape.color.r, shape.color.g, shape.color.b]
                                        .into_iter()
                                        .enumerate()
                                {
                                    target[channel] =
                                        color as f64 * alpha + target[channel] * (1.0 - alpha);
                                }
                                target[3] = alpha + target[3] * (1.0 - alpha);
                            }
                        }
                    }
                }
            }
        }
        let pixels = rgba
            .iter()
            .flat_map(|pixel| {
                [pixel[2], pixel[1], pixel[0], pixel[3]]
                    .map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8)
            })
            .collect::<Vec<_>>();
        drop(rgba);
        let bitmap = unsafe {
            canvas.context.CreateBitmap(
                D2D_SIZE_U {
                    width: width as u32,
                    height: height as u32,
                },
                Some(pixels.as_ptr().cast()),
                width as u32 * 4,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    ..Default::default()
                },
            )
        }?;
        self.atlas = Some(Atlas {
            bitmap,
            light,
            dpi,
            cell,
            padding,
            names: names.to_vec(),
        });
        super::memory::checkpoint("command-cpu-atlas-ready");
        Ok(())
    }

    pub fn icon(
        &self,
        canvas: &Canvas,
        name: &'static str,
        point: (f32, f32),
        opacity: f32,
    ) -> Result<()> {
        let atlas = self.atlas.as_ref().ok_or_else(|| {
            windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "Command atlas was not prepared",
            )
        })?;
        let index = atlas
            .names
            .iter()
            .position(|value| *value == name)
            .ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Unknown command atlas icon",
                )
            })?;
        let row = match opacity {
            1.0 => 0,
            0.36 => 1,
            _ => {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Unknown command atlas opacity",
                ))
            }
        };
        let scale = atlas.dpi as f32 / 96.0;
        let padding = atlas.padding as f32 / scale;
        let side = atlas.cell as f32 / scale;
        unsafe {
            canvas.context.DrawBitmap(
                &atlas.bitmap,
                Some(&D2D_RECT_F {
                    left: point.0 - padding,
                    top: point.1 - padding,
                    right: point.0 - padding + side,
                    bottom: point.1 - padding + side,
                }),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&D2D_RECT_F {
                    left: (index * atlas.cell) as f32,
                    top: (row * atlas.cell) as f32,
                    right: ((index + 1) * atlas.cell) as f32,
                    bottom: ((row + 1) * atlas.cell) as f32,
                }),
                None,
            );
        }
        Ok(())
    }
}
