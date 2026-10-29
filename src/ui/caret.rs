use super::render::{Layer, Renderer};
use windows::core::Result;
use windows::Win32::Graphics::DirectComposition::IDCompositionEffectGroup;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED;

pub struct Caret {
    pub layer: Layer,
    opacity: IDCompositionEffectGroup,
    revision: Option<(u8, u64)>,
    light: Option<bool>,
}

impl Caret {
    pub fn new(renderer: &Renderer) -> Result<Self> {
        let layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
        let opacity = unsafe { renderer.composition.CreateEffectGroup()? };
        unsafe {
            layer.visual.SetEffect(&opacity)?;
            opacity.SetOpacity2(0.0)?;
        }
        Ok(Self {
            layer,
            opacity,
            revision: None,
            light: None,
        })
    }

    pub fn update(
        &mut self,
        renderer: &Renderer,
        address: &super::address::State,
        theme: &super::theme::Theme,
        dpi: u32,
    ) -> Result<()> {
        let source = if address.editing {
            Some((0, &address.address_edit))
        } else if address.search_focus {
            Some((1, &address.search_edit))
        } else {
            None
        };
        let source = source
            .and_then(|(search, edit)| {
                edit.caret
                    .map(|bounds| (search, edit.revision, bounds, None))
            })
            .or_else(|| {
                super::rename::caret()
                    .map(|(revision, bounds, image)| (2, revision, bounds, Some(image)))
            });
        let Some((search, revision, bounds, image)) = source else {
            unsafe {
                self.layer
                    .visual
                    .SetClip2(&super::address::rect(0.0, 0.0, 0.0, 0.0))?;
            }
            if self.revision.take().is_some() {
                unsafe {
                    self.opacity.SetOpacity2(0.0)?;
                    self.layer.surface.Trim(Some(&[]))?;
                }
                self.light = None;
            }
            return Ok(());
        };
        let scale = dpi as f32 / 96.0;
        let light = theme.content.r > 0.5;
        let resized = self.layer.resize((
            scale.round().max(1.0) as u32,
            ((bounds.bottom - bounds.top) * scale).round().max(1.0) as u32,
        ))?;
        if resized
            || self.layer.presents == 0
            || self.light != Some(light)
            || image.is_some() && self.revision != Some((search, revision))
        {
            renderer.update(&mut self.layer, None, 96, &theme.text, |canvas| {
                if let Some(image) = &image {
                    let bitmap = canvas.system_icon(image)?;
                    unsafe { canvas.context.DrawBitmap(&bitmap, None, 1.0, windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None); }
                }
                Ok(())
            })?;
            self.light = Some(light);
        }
        unsafe {
            self.layer.visual.SetClip2(&super::address::rect(
                0.0,
                0.0,
                self.layer.size.0 as f32,
                self.layer.size.1 as f32,
            ))?;
            self.layer
                .visual
                .SetOffsetX2((bounds.left * scale).round())?;
            self.layer
                .visual
                .SetOffsetY2(((40.0 + bounds.top) * scale).round())?;
            if self.revision != Some((search, revision)) {
                let blink = windows::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime();
                if blink == u32::MAX || blink == 0 {
                    self.opacity.SetOpacity2(1.0)?;
                } else {
                    let half = blink as f64 / 1000.0;
                    let animation = renderer.composition.CreateAnimation()?;
                    animation.AddCubic(0.0, 1.0, 0.0, 0.0, 0.0)?;
                    animation.AddCubic(half, 0.0, 0.0, 0.0, 0.0)?;
                    animation.AddRepeat(half * 2.0, half * 2.0)?;
                    self.opacity.SetOpacity(&animation)?;
                }
                self.revision = Some((search, revision));
            }
        }
        Ok(())
    }
}
