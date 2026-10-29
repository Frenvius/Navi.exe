use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use windows::core::{implement, IUnknown, Ref, Result, BOOL};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

struct Run {
    face: IDWriteFontFace,
    size: f32,
    origin: (f32, f32),
    indices: Vec<u16>,
    advances: Vec<f32>,
    offsets: Vec<DWRITE_GLYPH_OFFSET>,
    sideways: BOOL,
    bidi: u32,
}

pub struct Glyphs(Vec<Run>);

fn coordinate(value: f32) -> f32 {
    (value * 300.0).round() / 300.0
}

pub fn collect(layout: &IDWriteTextLayout) -> Result<Glyphs> {
    collect_coordinates(layout, coordinate)
}

pub fn collect_ideal(layout: &IDWriteTextLayout) -> Result<Glyphs> {
    collect_coordinates(layout, |value| value)
}

fn collect_coordinates(layout: &IDWriteTextLayout, coordinate: fn(f32) -> f32) -> Result<Glyphs> {
    let runs = Rc::new(RefCell::new(Vec::new()));
    let renderer: IDWriteTextRenderer = Collector {
        runs: runs.clone(),
        coordinate,
    }
    .into();
    unsafe {
        layout.Draw(None, &renderer, 0.0, 0.0)?;
    }
    let result = std::mem::take(&mut *runs.borrow_mut());
    Ok(Glyphs(result))
}

impl Glyphs {
    pub fn draw(&self, context: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, dpi: u32) {
        self.draw_at(context, brush, dpi, f32::round);
    }

    pub fn draw_floor(&self, context: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, dpi: u32) {
        self.draw_at(context, brush, dpi, f32::floor);
    }

    fn draw_at(
        &self,
        context: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        dpi: u32,
        baseline: fn(f32) -> f32,
    ) {
        unsafe {
            let mut transform = Default::default();
            context.GetTransform(&mut transform);
            let scale = dpi as f32 / 96.0;
            for run in &self.0 {
                let mut origin = D2D1_ARC_SEGMENT::default().point;
                origin.X = run.origin.0;
                origin.Y = run.origin.1;
                if transform.M12 == 0.0 && transform.M21 == 0.0 && transform.M22 != 0.0 {
                    origin.Y = (baseline((transform.M22 * origin.Y + transform.M32) * scale)
                        / scale
                        - transform.M32)
                        / transform.M22;
                }
                let mut native = DWRITE_GLYPH_RUN {
                    fontFace: ManuallyDrop::new(Some(run.face.clone())),
                    fontEmSize: run.size,
                    glyphCount: run.indices.len() as u32,
                    glyphIndices: run.indices.as_ptr(),
                    glyphAdvances: run.advances.as_ptr(),
                    glyphOffsets: run.offsets.as_ptr(),
                    isSideways: run.sideways,
                    bidiLevel: run.bidi,
                };
                context.DrawGlyphRun(origin, &native, brush, DWRITE_MEASURING_MODE_NATURAL);
                ManuallyDrop::drop(&mut native.fontFace);
            }
        }
    }
}

#[implement(IDWriteTextRenderer)]
struct Collector {
    coordinate: fn(f32) -> f32,
    runs: Rc<RefCell<Vec<Run>>>,
}

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for Collector_Impl {
    fn IsPixelSnappingDisabled(&self, _context: *const c_void) -> Result<BOOL> {
        Ok(true.into())
    }
    fn GetPixelsPerDip(&self, _context: *const c_void) -> Result<f32> {
        Ok(1.0)
    }
    fn GetCurrentTransform(
        &self,
        _context: *const c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> Result<()> {
        if transform.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m22: 1.0,
                ..Default::default()
            };
        }
        Ok(())
    }
}

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for Collector_Impl {
    fn DrawGlyphRun(
        &self,
        _context: *const c_void,
        x: f32,
        y: f32,
        _mode: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _description: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _effect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        let coordinate = self.coordinate;
        let run = unsafe { run.as_ref() }.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
        if run.glyphCount == 0 {
            return Ok(());
        }
        if run.glyphIndices.is_null() || run.glyphAdvances.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let face = run
            .fontFace
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(E_POINTER))?
            .clone();
        let count = run.glyphCount as usize;
        let indices = unsafe { std::slice::from_raw_parts(run.glyphIndices, count) }.to_vec();
        let advances = unsafe { std::slice::from_raw_parts(run.glyphAdvances, count) }
            .iter()
            .map(|&value| coordinate(value))
            .collect();
        let offsets = if run.glyphOffsets.is_null() {
            vec![DWRITE_GLYPH_OFFSET::default(); count]
        } else {
            unsafe { std::slice::from_raw_parts(run.glyphOffsets, count) }
                .iter()
                .map(|offset| DWRITE_GLYPH_OFFSET {
                    advanceOffset: coordinate(offset.advanceOffset),
                    ascenderOffset: coordinate(offset.ascenderOffset),
                })
                .collect()
        };
        self.runs.borrow_mut().push(Run {
            face,
            size: run.fontEmSize,
            origin: (coordinate(x), coordinate(y)),
            indices,
            advances,
            offsets,
            sideways: run.isSideways,
            bidi: run.bidiLevel,
        });
        Ok(())
    }
    fn DrawUnderline(
        &self,
        _context: *const c_void,
        _x: f32,
        _y: f32,
        _underline: *const DWRITE_UNDERLINE,
        _effect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn DrawStrikethrough(
        &self,
        _context: *const c_void,
        _x: f32,
        _y: f32,
        _strike: *const DWRITE_STRIKETHROUGH,
        _effect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn DrawInlineObject(
        &self,
        _context: *const c_void,
        _x: f32,
        _y: f32,
        _object: Ref<'_, IDWriteInlineObject>,
        _sideways: BOOL,
        _rtl: BOOL,
        _effect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;

    #[test]
    fn collected_runs_retain_surrogate_combining_and_rtl_shaping() -> Result<()> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let format = factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("pt-BR"),
            )?;
            let text: Vec<u16> = "A\u{1f4c1} e\u{301} \u{644}\u{627}"
                .encode_utf16()
                .collect();
            let layout = factory.CreateTextLayout(&text, &format, 4096.0, 56.0)?;
            let glyphs = collect(&layout)?;
            assert!(glyphs.0.iter().any(|run| run.bidi & 1 == 1));
            assert!(glyphs.0.iter().map(|run| run.indices.len()).sum::<usize>() < text.len());
            assert!(glyphs.0.iter().all(|run| !run.indices.is_empty()
                && run.indices.len() == run.advances.len()
                && run.indices.len() == run.offsets.len()));
            assert!(glyphs
                .0
                .iter()
                .flat_map(|run| run.advances.iter())
                .all(|advance| advance.is_finite() && *advance >= 0.0));
        }
        Ok(())
    }
}
