pub fn pixels(width: i32, height: i32, dark: bool, context: bool) -> crate::shell::icons::Rgba {
    let atlas: &[u8] = match (dark, context) {
        (true, false) => include_bytes!("../../ui/assets/menu-chrome-dark-view.rgba"),
        (true, true) => include_bytes!("../../ui/assets/menu-chrome-dark-context.rgba"),
        (false, false) => include_bytes!("../../ui/assets/menu-chrome-light-view.rgba"),
        (false, true) => include_bytes!("../../ui/assets/menu-chrome-light-context.rgba"),
    };
    let (width, height) = (width.max(45) as usize, height.max(45) as usize);
    let mut pixels = vec![0; width * height * 4];
    for y in 0..height {
        let sy = if y < 14 {
            y
        } else if y >= height - 30 {
            45 - (height - y)
        } else {
            14
        };
        for x in 0..width {
            let sx = if x < 22 {
                x
            } else if x >= width - 22 {
                45 - (width - x)
            } else {
                22
            };
            let source = (sy * 45 + sx) * 4;
            let target = (y * width + x) * 4;
            pixels[target..target + 4].copy_from_slice(&atlas[source..source + 4]);
        }
    }
    crate::shell::icons::Rgba {
        width: width as u32,
        height: height as u32,
        pixels,
    }
}

pub fn backdrop(
    image: &mut crate::shell::icons::Rgba,
    origin: (i32, i32),
    scale: f32,
) -> Result<(), String> {
    let width = (image.width as f32 * scale).round().max(1.0) as usize;
    let height = (image.height as f32 * scale).round().max(1.0) as usize;
    let backdrop = crate::shell::backdrop::blurred(origin.0, origin.1, width, height, scale)
        .ok_or_else(|| "Menu backdrop capture failed".to_owned())?;
    let dark = crate::shell::theme::dark();
    for (index, pixel) in image.pixels.chunks_exact_mut(4).enumerate() {
        let alpha = pixel[3] as u32;
        if alpha < if dark { 245 } else { 217 } {
            continue;
        }
        let x = (((index % image.width as usize) as f32 + 0.5) * scale - 0.5)
            .round()
            .max(0.0) as usize;
        let y = (((index / image.width as usize) as f32 + 0.5) * scale - 0.5)
            .round()
            .max(0.0) as usize;
        let source = (y.min(height - 1) * width + x.min(width - 1)) * 3;
        let behind: [f32; 3] = std::array::from_fn(|channel| backdrop[source + channel] as f32);
        let mixed: [f32; 3] = std::array::from_fn(|channel| {
            ((pixel[channel] as u32 * alpha
                + backdrop[source + channel] as u32 * (255 - alpha)
                + 127)
                / 255
                + u32::from(dark))
            .min(255) as f32
        });
        let tint_opacity = if dark {
            DARK_TINT_OPACITY
        } else {
            LIGHT_TINT_OPACITY
        };
        let color = with_chroma(mixed, behind, alpha as f32 / 255.0 - tint_opacity);
        pixel[..3].copy_from_slice(&color);
        pixel[3] = 255;
    }
    Ok(())
}

const DARK_TINT_OPACITY: f32 = 0.15;
const LIGHT_TINT_OPACITY: f32 = 0.0;

fn luminance(color: [f32; 3]) -> f32 {
    0.3 * color[0] + 0.59 * color[1] + 0.11 * color[2]
}

fn with_chroma(base: [f32; 3], behind: [f32; 3], amount: f32) -> [u8; 3] {
    let gray = luminance(behind);
    let color: [f32; 3] =
        std::array::from_fn(|channel| base[channel] + amount * (behind[channel] - gray));
    let level = luminance(color);
    let low = color.iter().copied().fold(f32::MAX, f32::min);
    let high = color.iter().copied().fold(f32::MIN, f32::max);
    let mut scale: f32 = 1.0;
    if low < 0.0 {
        scale = scale.min(level / (level - low));
    }
    if high > 255.0 {
        scale = scale.min((255.0 - level) / (high - level));
    }
    color.map(|value| (level + (value - level) * scale).round().clamp(0.0, 255.0) as u8)
}

#[cfg(test)]
mod tests {
    #[test]
    fn chroma_keeps_luminance_and_gray_backdrops_unchanged() {
        assert_eq!(
            super::with_chroma([44.0; 3], [200.0; 3], 0.81),
            [44, 44, 44]
        );
        let tinted = super::with_chroma([44.0; 3], [120.0, 40.0, 130.0], 0.81);
        let level = super::luminance(tinted.map(f32::from));
        assert!((level - 44.0).abs() < 1.0, "{tinted:?}");
        assert!(tinted[0] > tinted[1] && tinted[2] > tinted[1]);
    }

    #[test]
    fn chroma_is_clipped_toward_luminance() {
        let clipped = super::with_chroma([250.0; 3], [255.0, 0.0, 0.0], 1.0);
        assert!((super::luminance(clipped.map(f32::from)) - 250.0).abs() < 1.5);
    }
}
