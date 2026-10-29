use windows::core::Result;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::{CloseThemeData, DrawThemeBackground, HTHEME};
use windows::Win32::UI::HiDpi::OpenThemeDataForDpi;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Clear,
    Selected,
}

struct Theme(HTHEME);

impl Drop for Theme {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseThemeData(self.0) } {
            eprintln!("native checkbox theme release: {error}");
        }
    }
}

use super::dib::Dib;

fn bitmap(
    class: &str,
    part: i32,
    state: i32,
    dimensions: (f32, f32),
    dpi: u32,
) -> Result<crate::shell::icons::Rgba> {
    let width = (dimensions.0 * dpi as f32 / 96.0).round().max(1.0) as i32;
    let height = (dimensions.1 * dpi as f32 / 96.0).round().max(1.0) as i32;
    let class = crate::shell::wide(class);
    let theme =
        Theme(unsafe { OpenThemeDataForDpi(None, windows::core::PCWSTR(class.as_ptr()), dpi) });
    if theme.0.is_invalid() {
        return Err(windows::core::Error::from_win32());
    }
    let dib = Dib::new(width, height)?;
    let rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let mut samples = Vec::new();
    for background in [0, 255] {
        unsafe {
            let pixels = std::slice::from_raw_parts_mut(dib.pixels, (width * height * 4) as usize);
            pixels.fill(background);
            DrawThemeBackground(theme.0, dib.dc, part, state, &rect, None)?;
            GdiFlush().ok()?;
            samples.push(pixels.to_vec());
        }
    }
    let pixels = samples[0]
        .chunks_exact(4)
        .zip(samples[1].chunks_exact(4))
        .flat_map(|(black, white)| {
            let alpha = 255u32 - white[0].saturating_sub(black[0]) as u32;
            let channel = |index| {
                if alpha == 0 {
                    0
                } else {
                    (black[index] as u32 * 255 / alpha).min(255) as u8
                }
            };
            [channel(2), channel(1), channel(0), alpha as u8]
        })
        .collect();
    Ok(crate::shell::icons::Rgba {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

pub fn image(state: State, dpi: u32) -> Result<crate::shell::icons::Rgba> {
    bitmap(
        "Button",
        3,
        match state {
            State::Clear => 1,
            State::Selected => 5,
        },
        (13.0, 13.0),
        dpi,
    )
}

pub fn group(
    state: windows::Win32::UI::Controls::GROUPHEADERSTATES,
    theme: &super::theme::Theme,
    dpi: u32,
) -> Result<crate::shell::icons::Rgba> {
    bitmap(
        if theme.content.r > 0.5 {
            "ItemsView::ListView"
        } else {
            "DarkMode_ItemsView::ListView"
        },
        6,
        state.0,
        (8.0, 8.0),
        dpi,
    )
}

#[cfg(test)]
pub fn icon_reference(index: u32) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT, IMAGELISTDRAWPARAMS};
    use windows::Win32::UI::Shell::{SHGetImageList, SHIL_SMALL};
    let dib = Dib::new(16, 16)?;
    unsafe {
        let pixels = std::slice::from_raw_parts_mut(dib.pixels, 16 * 16 * 4);
        pixels.fill(25);
        let images: IImageList = SHGetImageList(SHIL_SMALL as i32)?;
        images.Draw(&IMAGELISTDRAWPARAMS {
            cbSize: std::mem::size_of::<IMAGELISTDRAWPARAMS>() as u32,
            i: (index & 0x00ff_ffff) as i32,
            hdcDst: dib.dc,
            cx: 16,
            cy: 16,
            rgbBk: COLORREF(0xffff_ffff),
            fStyle: ILD_TRANSPARENT.0 | ((index >> 24) << 8),
            ..Default::default()
        })?;
        GdiFlush().ok()?;
        Ok(pixels
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            .collect())
    }
}

pub fn tree(
    part: i32,
    state: i32,
    dimensions: (f32, f32),
    theme: &super::theme::Theme,
    dpi: u32,
) -> Result<crate::shell::icons::Rgba> {
    bitmap(
        crate::shell::theme::tree(theme.content.r < 0.5),
        part,
        state,
        dimensions,
        dpi,
    )
}
