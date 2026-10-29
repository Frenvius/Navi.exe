use windows::core::w;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_TEXT_METRICS,
};

#[derive(Clone)]
pub struct IconLabel {
    pub full: String,
    pub collapsed: String,
    pub lines: u8,
    pub width: u32,
}

struct LabelDc {
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: windows::Win32::Graphics::Gdi::HFONT,
    old: windows::Win32::Graphics::Gdi::HGDIOBJ,
    ellipsis_extra: i32,
}

impl LabelDc {
    fn new() -> Option<Self> {
        Self::with_size(12)
    }

    fn with_size(size: i32) -> Option<Self> {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let font = CreateFontW(
                -size,
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
            if font.is_invalid() {
                let _ = DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, font.into());
            let mut dots = windows::Win32::Foundation::SIZE::default();
            let mut ellipsis = windows::Win32::Foundation::SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &[46, 46, 46], &mut dots);
            let _ = GetTextExtentPoint32W(dc, &[0x2026], &mut ellipsis);
            Some(Self {
                dc,
                font,
                old,
                ellipsis_extra: ellipsis.cx - dots.cx,
            })
        }
    }

    fn wrap(&self, text: &str, width: i32) -> IconLabel {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Gdi::*;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut lines = Vec::new();
        let mut starts = Vec::new();
        let mut at = 0;
        let mut width_used = Some(0);
        while at < wide.len() {
            let mut rest = wide[at..].to_vec();
            let mut rect = RECT {
                right: width,
                bottom: 15,
                ..Default::default()
            };
            let mut params = DRAWTEXTPARAMS {
                cbSize: std::mem::size_of::<DRAWTEXTPARAMS>() as u32,
                ..Default::default()
            };
            unsafe {
                DrawTextExW(
                    self.dc,
                    &mut rest,
                    &mut rect,
                    DT_EDITCONTROL | DT_WORDBREAK | DT_NOPREFIX,
                    Some(&mut params),
                );
            }
            let fallback = if (0xD800..=0xDBFF).contains(&wide[at]) {
                2
            } else {
                1
            };
            let count = (params.uiLengthDrawn as usize)
                .max(fallback)
                .min(wide.len() - at);
            let mut extent = windows::Win32::Foundation::SIZE::default();
            if unsafe { GetTextExtentPoint32W(self.dc, &wide[at..at + count], &mut extent) }
                .as_bool()
            {
                if extent.cx > width {
                    let visible = String::from_utf16_lossy(&wide[at..at + count]);
                    let trimmed: Vec<u16> = visible.trim_end().encode_utf16().collect();
                    if !trimmed.is_empty()
                        && !unsafe { GetTextExtentPoint32W(self.dc, &trimmed, &mut extent) }
                            .as_bool()
                    {
                        eprintln!("native label visible extent unavailable");
                        width_used = None;
                    }
                }
                width_used = width_used.map(|width| width.max(extent.cx));
            } else {
                eprintln!("native label extent unavailable");
                width_used = None;
            }
            starts.push(at);
            lines.push(
                String::from_utf16_lossy(&wide[at..at + count])
                    .trim_end()
                    .to_string(),
            );
            at += count;
        }
        let full = lines.join("\n");
        let collapsed = if lines.len() <= 4 {
            full.clone()
        } else {
            let tail = &wide[starts[3]..];
            let mut buffer = vec![0u16; tail.len() + 4];
            buffer[..tail.len()].copy_from_slice(tail);
            let mut rect = RECT {
                right: width - self.ellipsis_extra - 3,
                bottom: 15,
                ..Default::default()
            };
            unsafe {
                DrawTextExW(
                    self.dc,
                    &mut buffer[..tail.len()],
                    &mut rect,
                    DT_SINGLELINE | DT_END_ELLIPSIS | DT_MODIFYSTRING | DT_NOPREFIX,
                    None,
                );
            }
            let end = buffer.iter().position(|&c| c == 0).unwrap_or(tail.len());
            let last = String::from_utf16_lossy(&buffer[..end]);
            let last = last
                .strip_suffix("...")
                .map(|s| format!("{s}…"))
                .unwrap_or(last);
            format!("{}\n{}", lines[..3].join("\n"), last)
        };
        IconLabel {
            full,
            collapsed,
            lines: lines.len().clamp(1, 255) as u8,
            width: width_used.map_or(0, |measured| measured.max(0).min(width) as u32 + 4),
        }
    }
}

impl Drop for LabelDc {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.font.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

thread_local! { static LABEL_DC: Option<LabelDc> = LabelDc::new(); }
thread_local! { static CONTENT_DC: Option<LabelDc> = LabelDc::with_size(15); }

/// GDI advances of the already wrapped DirectUI label lines, for its 1px underline.
pub fn underline_widths(text: &str) -> Vec<f32> {
    underline_widths_at(text, 12, f32::MAX)
}

pub fn underline_widths_at(text: &str, size: i32, limit: f32) -> Vec<f32> {
    let measure = |dc: &Option<LabelDc>| {
        text.split('\n')
            .map(|line| {
                let Some(dc) = dc else { return 0.0 };
                let extent = |s: &str| {
                    let mut measured = windows::Win32::Foundation::SIZE::default();
                    unsafe {
                        let _ = windows::Win32::Graphics::Gdi::GetTextExtentPoint32W(
                            dc.dc,
                            &s.encode_utf16().collect::<Vec<_>>(),
                            &mut measured,
                        );
                    }
                    measured.cx as f32
                };
                let width = extent(line);
                if width <= limit {
                    return width;
                }
                let mut prefix = String::new();
                let mut fit = extent("…").min(limit);
                for ch in line.chars() {
                    prefix.push(ch);
                    let width = extent(&format!("{prefix}…"));
                    if width >= limit {
                        break;
                    }
                    fit = width;
                }
                fit
            })
            .collect()
    };
    if size == 15 {
        CONTENT_DC.with(measure)
    } else {
        LABEL_DC.with(measure)
    }
}

#[cfg(test)]
mod native_label_tests {
    #[test]
    fn measured_name_boxes_preserve_spaces_only_while_they_fit() {
        assert_eq!(super::icon_label("Alpha folder", 61).width, 38);
        assert_eq!(
            super::icon_label("Beta folder with a long name", 61).width,
            64
        );
        assert_eq!(
            super::icon_label("Beta folder with a long name", 92).width,
            96
        );
        assert_eq!(super::icon_label("braille-tables.txt", 61).width, 61);
    }
}

pub fn tile_label(text: &str, width: i32) -> String {
    let label = icon_label(text, width);
    let mut lines = label.full.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    if lines.next().is_none() {
        return text.to_owned();
    }
    let tail = text.get(first.len()..).unwrap_or_default();
    format!(
        "{first}{}\n{}",
        if tail.starts_with(' ') { " " } else { "" },
        tail.trim_start()
    )
}

pub fn icon_label(text: &str, width: i32) -> IconLabel {
    LABEL_DC.with(|dc| {
        dc.as_ref()
            .map(|dc| dc.wrap(text, width))
            .unwrap_or_else(|| IconLabel {
                full: text.to_string(),
                collapsed: text.to_string(),
                lines: 1,
                width: 0,
            })
    })
}

#[cfg(test)]
mod icon_label_tests {
    use super::icon_label;

    #[test]
    fn native_icon_word_breaks_and_pixel_boundaries() {
        assert_eq!(icon_label("000000000T", 61).full, "000000000T");
        assert_eq!(icon_label("000000000A", 61).full, "000000000\nA");
        assert_eq!(icon_label("braille-tables", 61).full, "braille-tabl\nes");
        assert_eq!(
            icon_label("alpha-beta-gamma-delta", 61).full,
            "alpha-beta\n-gamma-d\nelta"
        );
        assert_eq!(icon_label("000000000AAAAL", 92).lines, 1);
        assert_eq!(icon_label("000000000AAAAB", 92).lines, 2);
        let prefix = "0".repeat(38);
        assert_eq!(icon_label(&format!("{prefix}AAAL"), 258).lines, 1);
        assert_eq!(icon_label(&format!("{prefix}AAAB"), 258).lines, 2);
        let long = icon_label(
            "00000 alpha beta gamma delta epsilon zeta eta theta iota kappa lambda",
            61,
        );
        assert!(long.lines > 4);
        assert_eq!(long.collapsed.lines().count(), 4);
        assert!(long.collapsed.ends_with("delta eps…"));
        assert!(long.full.ends_with("lambda"));
    }
}

thread_local! {
    static FORMAT: Option<IDWriteTextFormat> = unsafe {
        DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok().and_then(|factory| {
            factory
                .CreateTextFormat(w!("Segoe UI"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 12.0, w!(""))
                .ok()
        })
    };
}

/// The width of a line of Segoe UI at 12px (DirectUI's text), in pixels.
pub fn ui_width(text: &str) -> f32 {
    FORMAT.with(|format| {
        let Some(format) = format else { return 0.0 };
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(factory) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)
            else {
                return 0.0;
            };
            let Ok(layout) = factory.CreateTextLayout(&wide, format, f32::MAX, f32::MAX) else {
                return 0.0;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return 0.0;
            }
            metrics.widthIncludingTrailingWhitespace
        }
    })
}

/// The lines DirectWrite breaks a text into at a width, breaking only between words as
/// XAML's WrapWholeWords does (a word wider than the line keeps a line of its own).
pub fn wrap_whole_words(
    text: &str,
    family: &str,
    size: f32,
    semibold: bool,
    width: f32,
) -> Vec<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_LINE_METRICS, DWRITE_WORD_WRAPPING_WHOLE_WORD,
    };
    let fallback = || vec![text.to_string()];
    let wide: Vec<u16> = text.encode_utf16().collect();
    let family: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let Ok(factory) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else {
            return fallback();
        };
        let weight = if semibold {
            DWRITE_FONT_WEIGHT_SEMI_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        let Ok(format) = factory.CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!(""),
        ) else {
            return fallback();
        };
        let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_WHOLE_WORD);
        let Ok(layout) = factory.CreateTextLayout(&wide, &format, width, f32::MAX) else {
            return fallback();
        };
        let mut count = 0;
        let _ = layout.GetLineMetrics(None, &mut count);
        let mut metrics = vec![DWRITE_LINE_METRICS::default(); count as usize];
        if layout
            .GetLineMetrics(Some(&mut metrics), &mut count)
            .is_err()
        {
            return fallback();
        }
        let mut at = 0usize;
        let mut lines = Vec::new();
        for m in &metrics[..count as usize] {
            let end = (at + m.length as usize).min(wide.len());
            let shown = end
                .saturating_sub(m.trailingWhitespaceLength as usize)
                .max(at);
            let line = String::from_utf16_lossy(&wide[at..shown]);
            if !(line.is_empty() && m.newlineLength == 0 && end == wide.len() && !lines.is_empty())
            {
                lines.push(line);
            }
            at = end;
        }
        if lines.is_empty() {
            fallback()
        } else {
            lines
        }
    }
}
