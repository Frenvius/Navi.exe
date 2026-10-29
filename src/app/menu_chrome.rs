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
        for channel in 0..3 {
            pixel[channel] = ((pixel[channel] as u32 * alpha
                + backdrop[source + channel] as u32 * (255 - alpha)
                + 127)
                / 255
                + u32::from(dark))
            .min(255) as u8;
        }
        pixel[3] = 255;
    }
    Ok(())
}
