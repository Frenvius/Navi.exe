use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::{implement, IUnknown, Interface, Ref, Result, BOOL};
use windows::Win32::Foundation::{COLORREF, E_FAIL, E_NOTIMPL};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;

use super::{Canvas, TextEngine, TextStyle};
use crate::shell::icons::Rgba;

pub struct Background<'a> {
    pub color: D2D1_COLOR_F,
    pub plate: Option<&'a Rgba>,
    pub origin: (f32, f32),
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    value: String,
    style: TextStyle,
    size: (u32, u32),
    fraction: (u32, u32),
    color: [u8; 3],
    background: [u8; 3],
    plate: (usize, i32, i32),
}

#[derive(Default)]
pub struct Cache {
    generation: Option<(usize, u32)>,
    target: Option<IDWriteBitmapRenderTarget>,
    size: (u32, u32),
    images: HashMap<Key, D2D_RECT_F>,
    atlas: Option<ID2D1Bitmap1>,
    atlas_size: (u32, u32),
    cursor: (u32, u32, u32),
}

fn rgb(color: D2D1_COLOR_F) -> [u8; 3] {
    [color.r, color.g, color.b].map(|channel| (channel * 255.0).round().clamp(0.0, 255.0) as u8)
}

impl Cache {
    pub fn draw(
        &mut self,
        canvas: &Canvas,
        text: &mut TextEngine,
        value: &str,
        style: TextStyle,
        position: (f32, f32),
        foreground: D2D1_COLOR_F,
        background: Background<'_>,
        bounds: D2D_RECT_F,
    ) -> Result<()> {
        let generation = (text.direct_params.as_raw() as usize, text.dpi);
        if self.generation != Some(generation) {
            self.images.clear();
            self.atlas = None;
            self.cursor = (0, 0, 0);
            self.target = None;
            self.size = (0, 0);
            self.generation = Some(generation);
        }
        let scale = text.dpi as f32 / 96.0;
        let layout = text.layout(value, style, 4096.0)?;
        let start = (
            (position.0 * scale).floor() as i32 - 2,
            (position.1 * scale).floor() as i32 - 2,
        );
        let fraction = ((position.0 * scale).fract(), (position.1 * scale).fract());
        let dimensions = (
            ((layout.advance.min((bounds.right - position.0).max(0.0)) * scale).ceil() as u32 + 4)
                .max(4),
            (layout.height * scale).ceil() as u32 + 4,
        );
        let origin = (
            (background.origin.0 * scale).round() as i32,
            (background.origin.1 * scale).round() as i32,
        );
        let key = Key {
            value: value.into(),
            style,
            size: dimensions,
            fraction: (fraction.0.to_bits(), fraction.1.to_bits()),
            color: rgb(foreground),
            background: rgb(background.color),
            plate: (
                background
                    .plate
                    .map_or(0, |plate| plate.pixels.as_ptr() as usize),
                start.0 - origin.0,
                start.1 - origin.1,
            ),
        };
        if !self.images.contains_key(&key) {
            let size = (self.size.0.max(dimensions.0), self.size.1.max(dimensions.1));
            unsafe {
                if let Some(target) = &self.target {
                    if self.size != size {
                        target.Resize(size.0, size.1)?;
                    }
                } else {
                    self.target = Some(
                        text.factory
                            .GetGdiInterop()?
                            .CreateBitmapRenderTarget(None, size.0, size.1)?,
                    );
                }
                self.size = size;
                let target = self.target.as_ref().unwrap();
                target.SetPixelsPerDip(scale)?;
                let target1: IDWriteBitmapRenderTarget1 = target.cast()?;
                target1.SetTextAntialiasMode(
                    if text.direct_aa == D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE {
                        DWRITE_TEXT_ANTIALIAS_MODE_CLEARTYPE
                    } else {
                        DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE
                    },
                )?;
                let dc = target.GetMemoryDC();
                let bitmap = GetCurrentObject(dc, OBJ_BITMAP);
                let mut dib = DIBSECTION::default();
                if GetObjectW(
                    bitmap,
                    std::mem::size_of::<DIBSECTION>() as i32,
                    Some(&mut dib as *mut _ as _),
                ) == 0
                    || dib.dsBm.bmBits.is_null()
                    || dib.dsBmih.biBitCount != 32
                {
                    return Err(E_FAIL.into());
                }
                let height = dib.dsBm.bmHeight.unsigned_abs();
                let stride = dib.dsBm.bmWidthBytes as usize;
                if height < dimensions.1 || stride < dimensions.0 as usize * 4 {
                    return Err(E_FAIL.into());
                }
                {
                    let pixels = std::slice::from_raw_parts_mut(
                        dib.dsBm.bmBits.cast::<u8>(),
                        stride * height as usize,
                    );
                    for y in 0..dimensions.1 {
                        let row = y;
                        for x in 0..dimensions.0 {
                            let mut color = key.background;
                            if let Some(plate) = background.plate {
                                let source = (key.plate.1 + x as i32, key.plate.2 + y as i32);
                                if source.0 >= 0
                                    && source.1 >= 0
                                    && source.0 < plate.width as i32
                                    && source.1 < plate.height as i32
                                {
                                    let at = (source.1 as usize * plate.width as usize
                                        + source.0 as usize)
                                        * 4;
                                    let alpha = plate.pixels[at + 3] as u32;
                                    for channel in 0..3 {
                                        color[channel] = (((plate.pixels[at + channel] as u32
                                            * alpha
                                            + 254)
                                            / 255)
                                            as f32
                                            + color[channel] as f32 * (255 - alpha) as f32 / 255.0)
                                            .round()
                                            .min(255.0)
                                            as u8;
                                    }
                                }
                            }
                            let at = row as usize * stride + x as usize * 4;
                            pixels[at..at + 4]
                                .copy_from_slice(&[color[2], color[1], color[0], 255]);
                        }
                    }
                }
                let renderer: IDWriteTextRenderer = GlyphRenderer {
                    target: target.clone(),
                    params: text.direct_params.clone(),
                    color: COLORREF(
                        key.color[0] as u32
                            | (key.color[1] as u32) << 8
                            | (key.color[2] as u32) << 16,
                    ),
                    scale,
                }
                .into();
                layout.native.Draw(
                    None,
                    &renderer,
                    (2.0 + fraction.0) / scale,
                    (2.0 + fraction.1) / scale,
                )?;
                GdiFlush().ok()?;
                let pixels = std::slice::from_raw_parts(
                    dib.dsBm.bmBits.cast::<u8>(),
                    stride * height as usize,
                );
                let mut rgba = Vec::with_capacity((dimensions.0 * dimensions.1 * 4) as usize);
                for y in 0..dimensions.1 {
                    let row = y;
                    for x in 0..dimensions.0 {
                        let at = row as usize * stride + x as usize * 4;
                        rgba.extend_from_slice(&[pixels[at + 2], pixels[at + 1], pixels[at], 255]);
                    }
                }
                let capacity = (
                    dimensions.0.max(512).next_power_of_two(),
                    dimensions.1.max(256).next_power_of_two(),
                );
                if self.atlas.is_none()
                    || self.atlas_size.0 < capacity.0
                    || self.atlas_size.1 < capacity.1
                {
                    self.atlas = Some(canvas.system_icon(&Rgba {
                        width: capacity.0,
                        height: capacity.1,
                        pixels: vec![0; (capacity.0 * capacity.1 * 4) as usize],
                    })?);
                    self.atlas_size = capacity;
                    self.images.clear();
                    self.cursor = (0, 0, 0);
                }
                if self.cursor.0 + dimensions.0 > self.atlas_size.0 {
                    self.cursor = (0, self.cursor.1 + self.cursor.2, 0);
                }
                if self.images.len() >= 128 || self.cursor.1 + dimensions.1 > self.atlas_size.1 {
                    self.images.clear();
                    self.cursor = (0, 0, 0);
                }
                let (x, y, _) = self.cursor;
                let mut bgra = Vec::with_capacity(rgba.len());
                for pixel in rgba.chunks_exact(4) {
                    bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
                let destination = D2D_RECT_U {
                    left: x,
                    top: y,
                    right: x + dimensions.0,
                    bottom: y + dimensions.1,
                };
                self.atlas.as_ref().unwrap().CopyFromMemory(
                    Some(&destination),
                    bgra.as_ptr().cast(),
                    dimensions.0 * 4,
                )?;
                self.images.insert(
                    key.clone(),
                    D2D_RECT_F {
                        left: x as f32,
                        top: y as f32,
                        right: (x + dimensions.0) as f32,
                        bottom: (y + dimensions.1) as f32,
                    },
                );
                self.cursor.0 += dimensions.0;
                self.cursor.2 = self.cursor.2.max(dimensions.1);
            }
        }
        let source = self.images.get(&key).unwrap();
        unsafe {
            canvas.context.DrawBitmap(
                self.atlas.as_ref().unwrap(),
                Some(&D2D_RECT_F {
                    left: start.0 as f32 / scale,
                    top: start.1 as f32 / scale,
                    right: (start.0 as f32 + dimensions.0 as f32) / scale,
                    bottom: (start.1 as f32 + dimensions.1 as f32) / scale,
                }),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(source),
                None,
            );
        }
        Ok(())
    }
}

#[implement(IDWriteTextRenderer)]
struct GlyphRenderer {
    target: IDWriteBitmapRenderTarget,
    params: IDWriteRenderingParams,
    color: COLORREF,
    scale: f32,
}

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for GlyphRenderer_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> Result<BOOL> {
        Ok(false.into())
    }
    fn GetPixelsPerDip(&self, _: *const c_void) -> Result<f32> {
        Ok(self.scale)
    }
    fn GetCurrentTransform(&self, _: *const c_void, transform: *mut DWRITE_MATRIX) -> Result<()> {
        if transform.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
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
impl IDWriteTextRenderer_Impl for GlyphRenderer_Impl {
    fn DrawGlyphRun(
        &self,
        _: *const c_void,
        x: f32,
        y: f32,
        mode: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        if run.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }
        unsafe {
            self.target
                .DrawGlyphRun(x, y, mode, run, &self.params, self.color, None)
        }
    }
    fn DrawUnderline(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn DrawStrikethrough(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn DrawInlineObject(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: Ref<'_, IDWriteInlineObject>,
        _: BOOL,
        _: BOOL,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}
