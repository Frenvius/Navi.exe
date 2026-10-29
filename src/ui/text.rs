use super::render::Canvas;
#[path = "classic_text.rs"]
mod classic;
pub(super) use classic::{Background as ClassicBackground, Cache as ClassicCache};
use std::collections::HashMap;
use std::rc::Rc;
use windows::core::{Interface, Result, PCWSTR};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const FORMAT_CAP: usize = 64;
const LAYOUT_CAP: usize = 256;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    DirectUi,
    Segoe,
    SegoeXaml,
    Variable,
    Small,
    Text,
    Display,
    Icons,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextStyle {
    family: Family,
    size: u32,
    weight: u16,
    kerning: bool,
    rounded_advances: bool,
    floor_baseline: bool,
    wrap: bool,
}

impl TextStyle {
    pub const fn new(family: Family, size: f32, weight: u16) -> Self {
        Self {
            family,
            size: size.to_bits(),
            weight,
            kerning: !matches!(family, Family::DirectUi | Family::SegoeXaml),
            floor_baseline: false,
            wrap: false,
            rounded_advances: matches!(
                family,
                Family::Segoe | Family::Variable | Family::Text | Family::Display
            ),
        }
    }

    pub const fn with_wrap(self) -> Self {
        Self { wrap: true, ..self }
    }

    pub const fn with_floor_baseline(self) -> Self {
        Self {
            floor_baseline: true,
            ..self
        }
    }

    pub const fn with_rounded_advances(self) -> Self {
        Self {
            rounded_advances: true,
            ..self
        }
    }

    pub const fn with_ideal_advances(self) -> Self {
        Self {
            rounded_advances: false,
            ..self
        }
    }
}

pub const DIRECTUI: TextStyle = TextStyle::new(Family::DirectUi, 12.0, 400);
pub const DIRECTUI_KERNED: TextStyle = TextStyle {
    kerning: true,
    ..DIRECTUI
};
pub const GROUP_HEADER: TextStyle = TextStyle {
    size: 15.0f32.to_bits(),
    ..DIRECTUI_KERNED
};
pub const SEARCH_NAME: TextStyle = TextStyle::new(Family::DirectUi, 15.0, 400);
pub const HEADER: TextStyle = DIRECTUI_KERNED;
pub const TAB_INACTIVE: TextStyle = TextStyle::new(Family::Variable, 12.0, 400);
pub const TAB_TITLE: TextStyle = TextStyle::new(Family::Variable, 12.0, 600);
pub const CAPTION_GLYPH: TextStyle = TextStyle::new(Family::Icons, 10.0, 400);
pub const TAB_GLYPH: TextStyle = TextStyle::new(Family::Icons, 12.0, 400);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    format: TextStyle,
    width: u32,
}

pub struct Layout {
    native: IDWriteTextLayout,
    pub width: f32,
    pub advance: f32,
    pub height: f32,
    runs: Option<super::text_run::Glyphs>,
}

pub struct TextEngine {
    factory: IDWriteFactory,
    formats: HashMap<TextStyle, IDWriteTextFormat>,
    layouts: HashMap<LayoutKey, Rc<Layout>>,
    direct_params: IDWriteRenderingParams,
    xaml_params: IDWriteRenderingParams,
    caption_params: IDWriteRenderingParams,
    direct_aa: D2D1_TEXT_ANTIALIAS_MODE,
    dpi: u32,
    locale: Vec<u16>,
    pub format_misses: usize,
    pub layout_misses: usize,
}

impl Layout {
    pub fn caret(&self, position: u32) -> Result<(f32, f32, f32)> {
        let (mut x, mut y) = (0.0, 0.0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            self.native
                .HitTestTextPosition(position, false, &mut x, &mut y, &mut metrics)?;
        }
        Ok((x, y, metrics.height))
    }

    pub fn hit(&self, x: f32) -> Result<u32> {
        let mut trailing = windows::core::BOOL::default();
        let mut inside = windows::core::BOOL::default();
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            self.native.HitTestPoint(
                x,
                self.height / 2.0,
                &mut trailing,
                &mut inside,
                &mut metrics,
            )?;
        }
        Ok(metrics.textPosition
            + if trailing.as_bool() {
                metrics.length
            } else {
                0
            })
    }

    pub fn selection(&self, start: u32, length: u32) -> Result<Vec<DWRITE_HIT_TEST_METRICS>> {
        let mut count = 0;
        let first = unsafe {
            self.native
                .HitTestTextRange(start, length, 0.0, 0.0, None, &mut count)
        };
        if count == 0 {
            first?;
            return Ok(Vec::new());
        }
        let mut metrics = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        unsafe {
            self.native.HitTestTextRange(
                start,
                length,
                0.0,
                0.0,
                Some(&mut metrics),
                &mut count,
            )?;
        }
        metrics.truncate(count as usize);
        Ok(metrics)
    }
}

impl TextEngine {
    pub fn new(dpi: u32) -> Result<Self> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let defaults = factory.CreateRenderingParams()?;
            let direct_params = defaults.clone();
            let mut engine = Self {
                factory,
                formats: HashMap::new(),
                layouts: HashMap::new(),
                direct_params,
                xaml_params: defaults.clone(),
                caption_params: defaults,
                direct_aa: D2D1_TEXT_ANTIALIAS_MODE_DEFAULT,
                dpi,
                locale: crate::shell::wide(if crate::i18n::language() == "en" {
                    "en-US"
                } else {
                    crate::i18n::language()
                }),
                format_misses: 0,
                layout_misses: 0,
            };
            engine.refresh(dpi)?;
            Ok(engine)
        }
    }

    pub fn refresh(&mut self, dpi: u32) -> Result<()> {
        unsafe {
            let get = |action| -> Result<u32> {
                let mut value = 0u32;
                SystemParametersInfoW(
                    action,
                    0,
                    Some(&mut value as *mut _ as _),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )?;
                Ok(value)
            };
            let smooth = get(SPI_GETFONTSMOOTHING)? != 0;
            let clear = get(SPI_GETFONTSMOOTHINGTYPE)? == 2;
            let orientation = get(SPI_GETFONTSMOOTHINGORIENTATION)?;
            let gamma = (get(SPI_GETFONTSMOOTHINGCONTRAST)? as f32 / 1000.0).clamp(1.0, 2.2);
            let defaults = self.factory.CreateRenderingParams()?;
            self.direct_aa = if !smooth {
                D2D1_TEXT_ANTIALIAS_MODE_ALIASED
            } else if clear {
                D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE
            } else {
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE
            };
            let geometry = if smooth && clear {
                if orientation == 0 {
                    DWRITE_PIXEL_GEOMETRY_BGR
                } else {
                    DWRITE_PIXEL_GEOMETRY_RGB
                }
            } else {
                DWRITE_PIXEL_GEOMETRY_FLAT
            };
            self.direct_params = self.factory.CreateCustomRenderingParams(
                gamma,
                defaults.GetEnhancedContrast(),
                if clear { 1.0 } else { 0.0 },
                geometry,
                if smooth {
                    DWRITE_RENDERING_MODE_GDI_CLASSIC
                } else {
                    DWRITE_RENDERING_MODE_ALIASED
                },
            )?;
            self.xaml_params = self.factory.CreateCustomRenderingParams(
                defaults.GetGamma(),
                defaults.GetEnhancedContrast(),
                0.0,
                DWRITE_PIXEL_GEOMETRY_FLAT,
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
            )?;
            self.caption_params = self.factory.CreateCustomRenderingParams(
                defaults.GetGamma(),
                defaults.GetEnhancedContrast(),
                0.0,
                DWRITE_PIXEL_GEOMETRY_FLAT,
                DWRITE_RENDERING_MODE_NATURAL,
            )?;
        }
        self.dpi = dpi;
        self.formats.clear();
        self.layouts.clear();
        Ok(())
    }

    pub(super) fn release_layouts(&mut self) {
        self.layouts = HashMap::new();
    }

    pub fn layout(&mut self, text: &str, style: TextStyle, width: f32) -> Result<Rc<Layout>> {
        let family = style.family;
        let size = f32::from_bits(style.size);
        let weight = style.weight;
        if !size.is_finite() || size <= 0.0 || !width.is_finite() || width <= 0.0 {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        let opsz: f32 = match family {
            Family::DirectUi | Family::Segoe | Family::SegoeXaml | Family::Icons => 0.0,
            Family::Variable => size * 0.75,
            Family::Small => 8.0,
            Family::Text => 10.5,
            Family::Display => 36.0,
        };
        let format_key = style;
        let key = LayoutKey {
            text: text.to_string(),
            format: format_key,
            width: width.to_bits(),
        };
        if let Some(layout) = self.layouts.get(&key) {
            return Ok(layout.clone());
        }
        unsafe {
            let format = if let Some(format) = self.formats.get(&format_key) {
                format.clone()
            } else {
                let name = match family {
                    Family::DirectUi | Family::Segoe | Family::SegoeXaml => "Segoe UI",
                    Family::Variable => "Segoe UI Variable",
                    Family::Small => "Segoe UI Variable Small",
                    Family::Text => "Segoe UI Variable Text",
                    Family::Display => "Segoe UI Variable Display",
                    Family::Icons => "Segoe Fluent Icons",
                };
                let name = crate::shell::wide(name);
                let format = self.factory.CreateTextFormat(
                    PCWSTR(name.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    PCWSTR(self.locale.as_ptr()),
                )?;
                format.SetWordWrapping(if style.wrap {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })?;
                if !matches!(
                    family,
                    Family::DirectUi | Family::Segoe | Family::SegoeXaml | Family::Icons
                ) {
                    let variable: IDWriteTextFormat3 = format.cast()?;
                    variable.SetAutomaticFontAxes(DWRITE_AUTOMATIC_FONT_AXES_NONE)?;
                    variable.SetFontAxisValues(&[
                        DWRITE_FONT_AXIS_VALUE {
                            axisTag: DWRITE_FONT_AXIS_TAG_OPTICAL_SIZE,
                            value: opsz,
                        },
                        DWRITE_FONT_AXIS_VALUE {
                            axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
                            value: weight as f32,
                        },
                    ])?;
                }
                if self.formats.len() >= FORMAT_CAP {
                    self.formats.clear();
                }
                self.formats.insert(format_key, format.clone());
                self.format_misses += 1;
                format
            };
            let wide: Vec<u16> = text.encode_utf16().collect();
            let native = if family == Family::DirectUi {
                self.factory.CreateGdiCompatibleTextLayout(
                    &wide,
                    &format,
                    width,
                    if style.wrap { 4096.0 } else { size * 4.0 },
                    self.dpi as f32 / 96.0,
                    None,
                    false,
                )?
            } else {
                self.factory.CreateTextLayout(
                    &wide,
                    &format,
                    width,
                    if style.wrap { 4096.0 } else { size * 4.0 },
                )?
            };
            if family == Family::DirectUi && !style.kerning {
                let typography = self.factory.CreateTypography()?;
                typography.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: DWRITE_FONT_FEATURE_TAG_KERNING,
                    parameter: 0,
                })?;
                native.SetTypography(
                    &typography,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: wide.len() as u32,
                    },
                )?;
            }
            let mut metrics = DWRITE_TEXT_METRICS::default();
            native.GetMetrics(&mut metrics)?;
            let pixels = metrics.widthIncludingTrailingWhitespace * self.dpi as f32 / 96.0;
            let width = if family == Family::DirectUi {
                pixels.round()
            } else {
                (pixels + 0.001).floor()
            };
            let runs = if style.floor_baseline {
                Some(super::text_run::collect_ideal(&native)?)
            } else if style.rounded_advances {
                Some(super::text_run::collect(&native)?)
            } else {
                None
            };
            let layout = Rc::new(Layout {
                native,
                width,
                advance: metrics.widthIncludingTrailingWhitespace,
                height: metrics.height,
                runs,
            });
            if self.layouts.len() >= LAYOUT_CAP {
                self.layouts.clear();
            }
            self.layouts.insert(key, layout.clone());
            self.layout_misses += 1;
            Ok(layout)
        }
    }

    pub(super) fn cache_counts(&self) -> (usize, usize) {
        (self.formats.len(), self.layouts.len())
    }

    pub fn draw(
        &mut self,
        canvas: &Canvas,
        text: &str,
        style: TextStyle,
        position: (f32, f32),
        color: &D2D1_COLOR_F,
    ) -> Result<()> {
        let layout = self.layout(text, style, 4096.0)?;
        self.draw_layout(canvas, &layout, style, position, color)
    }

    pub fn draw_wrapped(
        &mut self,
        canvas: &Canvas,
        value: &str,
        style: TextStyle,
        bounds: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
        color: &D2D1_COLOR_F,
    ) -> Result<()> {
        let style = style.with_wrap();
        let layout = self.layout(value, style, (bounds.right - bounds.left).max(1.0))?;
        self.draw_layout(canvas, &layout, style, (bounds.left, bounds.top), color)
    }

    fn draw_layout(
        &self,
        canvas: &Canvas,
        layout: &Layout,
        style: TextStyle,
        position: (f32, f32),
        color: &D2D1_COLOR_F,
    ) -> Result<()> {
        let family = style.family;
        let context = &canvas.context;
        unsafe {
            context.SetTextAntialiasMode(if family == Family::DirectUi {
                self.direct_aa
            } else {
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE
            });
            context.SetTextRenderingParams(if family == Family::DirectUi {
                &self.direct_params
            } else if family == Family::Segoe {
                &self.caption_params
            } else {
                &self.xaml_params
            });
            let brush = &canvas.brush;
            brush.SetColor(color);
            let mut original = Default::default();
            context.GetTransform(&mut original);
            let mut translated = original;
            translated.M31 += position.0;
            translated.M32 += position.1;
            context.SetTransform(&translated);
            if let Some(runs) = &layout.runs {
                if style.floor_baseline {
                    runs.draw_floor(context, brush, self.dpi);
                } else {
                    runs.draw(context, brush, self.dpi);
                }
            } else {
                context.DrawTextLayout(
                    Default::default(),
                    &layout.native,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            context.SetTransform(&original);
        }
        Ok(())
    }

    pub fn elide<'a>(
        &mut self,
        value: &'a str,
        style: TextStyle,
        width: f32,
    ) -> Result<std::borrow::Cow<'a, str>> {
        let ellipsis = if style.family == Family::DirectUi {
            "..."
        } else {
            "…"
        };
        let width = width * self.dpi as f32 / 96.0;
        let advance = self.layout(value, style, 4096.0)?.width;
        if advance < width || advance == width && style.family != Family::DirectUi {
            return Ok(std::borrow::Cow::Borrowed(value));
        }
        if self.layout(ellipsis, style, 4096.0)?.width >= width {
            return Ok(std::borrow::Cow::Borrowed(""));
        }
        let ends: Vec<_> = value
            .char_indices()
            .map(|(i, c)| i + c.len_utf8())
            .collect();
        let (mut lo, mut hi) = (0, ends.len());
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let candidate = format!("{}{ellipsis}", &value[..ends[mid - 1]]);
            if self.layout(&candidate, style, 4096.0)?.width < width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Ok(std::borrow::Cow::Owned(format!(
            "{}{ellipsis}",
            if lo == 0 { "" } else { &value[..ends[lo - 1]] }
        )))
    }
}

pub fn settings_changed(wparam: usize) -> bool {
    [
        SPI_SETFONTSMOOTHING,
        SPI_SETFONTSMOOTHINGTYPE,
        SPI_SETFONTSMOOTHINGORIENTATION,
        SPI_SETFONTSMOOTHINGCONTRAST,
    ]
    .iter()
    .any(|action| action.0 as usize == wparam)
}

#[cfg(test)]
mod tests {
    #[test]
    fn wrapping_uses_a_distinct_cached_format_without_changing_single_line_metrics() -> Result<()> {
        let mut text = TextEngine::new(96)?;
        let style = TextStyle::new(Family::Variable, 12.0, 400);
        let value = "A longer settings description that must wrap at this width";
        let single = text.layout(value, style, 80.0)?;
        let wrapped = text.layout(value, style.with_wrap(), 80.0)?;
        assert!(wrapped.height > single.height * 2.0);
        assert!(wrapped.advance <= 80.0);
        assert_eq!(text.layout(value, style, 80.0)?.height, single.height);
        Ok(())
    }

    #[test]
    fn edit_hits_preserve_surrogates_and_combining_clusters() -> windows::core::Result<()> {
        let mut engine = super::TextEngine::new(96)?;
        let value = "a\u{1f4c1}e\u{301}z";
        let layout = engine.layout(
            value,
            super::TextStyle::new(super::Family::Small, 14.0, 400),
            4096.0,
        )?;
        let mut positions = std::collections::HashSet::new();
        for pixel in 0..=(layout.advance.ceil() as usize * 8) {
            positions.insert(layout.hit(pixel as f32 / 8.0)?);
        }
        assert!(!positions.contains(&2));
        assert!(!positions.contains(&4));
        assert!(positions.contains(&1) && positions.contains(&3) && positions.contains(&5));
        let bounds = layout.selection(1, 2)?;
        assert_eq!(bounds.len(), 1);
        assert!(bounds[0].width > 0.0);
        Ok(())
    }
    #[test]
    fn header_kerning_and_strict_ellipsis_boundary() -> windows::core::Result<()> {
        let mut text = super::TextEngine::new(96)?;
        let body = text.layout("Tamanho", super::DIRECTUI, 4096.0)?.width;
        let header = text.layout("Tamanho", super::HEADER, 4096.0)?.width;
        assert!(header < body);
        let name = "AccountHealthAssets";
        let full = text.layout(name, super::DIRECTUI, 4096.0)?.width;
        assert!(text.elide(name, super::DIRECTUI, full)?.ends_with("..."));
        assert_eq!(text.elide(name, super::DIRECTUI, full + 1.0)?, name);
        assert_eq!(
            text.elide("Extensão de aplicativo", super::DIRECTUI_KERNED, 108.0)?,
            "Extensão de aplica..."
        );
        for width in [40.0, 60.0, 90.0] {
            let clipped = text.elide(name, super::DIRECTUI, width)?.into_owned();
            assert!(clipped.ends_with("..."));
            assert!(text.layout(&clipped, super::DIRECTUI, 4096.0)?.width < width);
        }
        Ok(())
    }
    use super::*;
    use windows::core::w;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::*;

    #[test]
    fn system32_gdi_classic_widths() -> Result<()> {
        let mut engine = TextEngine::new(96)?;
        unsafe {
            let dc = CreateCompatibleDC(None);
            assert!(!dc.is_invalid());
            let font = CreateFontW(
                -12,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            assert!(!font.is_invalid());
            let old = SelectObject(dc, font.into());
            let mut misses = Vec::new();
            let mut count = 0;
            for entry in std::fs::read_dir(r"C:\Windows\System32").unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                let wide: Vec<u16> = name.encode_utf16().collect();
                let mut size = SIZE::default();
                assert!(GetTextExtentPoint32W(dc, &wide, &mut size).as_bool());
                let width = engine.layout(&name, DIRECTUI, 4096.0)?.width as i32;
                if width != size.cx {
                    misses.push((name, size.cx, width));
                }
                count += 1;
            }
            SelectObject(dc, old);
            assert!(DeleteObject(font.into()).as_bool());
            assert!(DeleteDC(dc).as_bool());
            println!(
                "System32 widths: {}/{} exact; misses: {:?}",
                count - misses.len(),
                count,
                misses
            );
            assert!(misses.is_empty());
            assert!(engine.layouts.len() <= LAYOUT_CAP);
        }
        Ok(())
    }

    #[test]
    fn dpi_refresh_bold_widths_and_cache_invalidation() -> Result<()> {
        let mut engine = TextEngine::new(96)?;
        unsafe {
            let dc = CreateCompatibleDC(None);
            assert!(!dc.is_invalid());
            for dpi in [96, 120, 144, 192] {
                engine.refresh(dpi)?;
                assert!(engine.formats.is_empty());
                assert!(engine.layouts.is_empty());
                for weight in [400, 700] {
                    let font = CreateFontW(
                        -(12 * dpi as i32 / 96),
                        0,
                        0,
                        0,
                        weight,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY,
                        0,
                        w!("Segoe UI"),
                    );
                    assert!(!font.is_invalid());
                    let old = SelectObject(dc, font.into());
                    for name in [
                        "AccountHealthAssets",
                        "Taskmgr.exe",
                        "KBDTAT.DLL",
                        "@AppHelpToast.png",
                    ] {
                        let wide: Vec<u16> = name.encode_utf16().collect();
                        let mut size = SIZE::default();
                        assert!(GetTextExtentPoint32W(dc, &wide, &mut size).as_bool());
                        let layout = engine.layout(
                            name,
                            TextStyle::new(Family::DirectUi, 12.0, weight as u16),
                            4096.0,
                        )?;
                        assert_eq!(layout.width as i32, size.cx, "{dpi} dpi, {weight}, {name}");
                    }
                    SelectObject(dc, old);
                    assert!(DeleteObject(font.into()).as_bool());
                }
            }
            assert!(DeleteDC(dc).as_bool());
        }
        Ok(())
    }

    #[test]
    fn variable_widths_and_cached_second_pass() -> Result<()> {
        let mut engine = TextEngine::new(96)?;
        let labels = ["Novo", "Classificar", "Visualizar", "Filtrar", "Detalhes"];
        for family in [
            Family::Variable,
            Family::Small,
            Family::Text,
            Family::Display,
        ] {
            for size in [12.0, 14.0] {
                let widths = labels
                    .iter()
                    .map(|label| {
                        engine
                            .layout(label, TextStyle::new(family, size, 400), 4096.0)
                            .map(|layout| layout.width)
                    })
                    .collect::<Result<Vec<_>>>()?;
                println!("{family:?} {size}: {widths:?}");
            }
        }
        let misses = engine.layout_misses;
        for family in [
            Family::Variable,
            Family::Small,
            Family::Text,
            Family::Display,
        ] {
            for size in [12.0, 14.0] {
                for label in labels {
                    engine.layout(label, TextStyle::new(family, size, 400), 4096.0)?;
                }
            }
        }
        assert_eq!(engine.layout_misses, misses);
        Ok(())
    }
}
