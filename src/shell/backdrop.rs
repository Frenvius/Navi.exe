//! A popup's stationary backdrop, sampled while the popup is cloaked. This keeps
//! text behind an alpha menu blurred without DWM's incompatible corner/shadow preset.
use windows::Win32::Graphics::Gdi::*;

pub fn blurred(x: i32, y: i32, width: usize, height: usize, scale: f32) -> Option<Vec<u8>> {
    let radius = (30.0 * scale).round().max(1.0) as usize;
    let pad = radius * 3;
    let (w, h) = (width + pad * 2, height + pad * 2);
    let mut pixels = vec![0u8; w * h * 4];
    unsafe {
        let dc = GetDC(None);
        let mem = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, w as i32, h as i32);
        let old = SelectObject(mem, bitmap.into());
        let _ = PatBlt(mem, 0, 0, w as i32, h as i32, BLACKNESS);
        let copied = BitBlt(
            mem,
            0,
            0,
            w as i32,
            h as i32,
            Some(dc),
            x - pad as i32,
            y - pad as i32,
            SRCCOPY,
        );
        SelectObject(mem, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let read = GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, dc);
        if copied.is_err() || read != h as i32 {
            return None;
        }
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        let (left, top) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        let (right, bottom) = (
            left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        );
        let (sx, sy) = (x - pad as i32, y - pad as i32);
        for yy in 0..h {
            for xx in 0..w {
                let (ax, ay) = (sx + xx as i32, sy + yy as i32);
                if ax < left || ax >= right || ay < top || ay >= bottom {
                    let bx = (sx + xx as i32).clamp(left, right - 1) - sx;
                    let by = (sy + yy as i32).clamp(top, bottom - 1) - sy;
                    let from = (by.clamp(0, h as i32 - 1) as usize * w
                        + bx.clamp(0, w as i32 - 1) as usize)
                        * 4;
                    let rgb = [pixels[from], pixels[from + 1], pixels[from + 2]];
                    pixels[(yy * w + xx) * 4..(yy * w + xx) * 4 + 3].copy_from_slice(&rgb);
                }
            }
        }
    }
    // Three separable box passes approximate the 30px acrylic blur. Sliding sums
    // make the work linear in area, independent of the blur radius.
    let mut scratch = vec![0u8; pixels.len()];
    for _ in 0..3 {
        pass(&pixels, &mut scratch, w, h, radius, false);
        pass(&scratch, &mut pixels, w, h, radius, true);
    }
    let mut out = vec![0u8; width * height * 3];
    for yy in 0..height {
        for xx in 0..width {
            let s = ((yy + pad) * w + xx + pad) * 4;
            let d = (yy * width + xx) * 3;
            out[d..d + 3].copy_from_slice(&[pixels[s + 2], pixels[s + 1], pixels[s]]);
        }
    }
    Some(out)
}

fn pass(src: &[u8], dest: &mut [u8], w: usize, h: usize, r: usize, vertical: bool) {
    let (lines, len, stride) = if vertical { (w, h, w) } else { (h, w, 1) };
    let count = (2 * r + 1) as u32;
    for line in 0..lines {
        let base = if vertical { line } else { line * w };
        for c in 0..3 {
            let at = |i: usize| src[(base + i.min(len - 1) * stride) * 4 + c] as u32;
            let mut sum = at(0) * (r + 1) as u32 + (1..=r).map(at).sum::<u32>();
            for i in 0..len {
                dest[(base + i * stride) * 4 + c] = ((sum + count / 2) / count) as u8;
                sum = sum + at(i + r + 1) - at(i.saturating_sub(r));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn blur_preserves_a_flat_surface_including_its_edges() {
        let src = [37u8; 9 * 7 * 4];
        let mut dest = [0u8; 9 * 7 * 4];
        super::pass(&src, &mut dest, 9, 7, 12, false);
        assert!(dest.chunks_exact(4).all(|p| p[..3] == [37; 3]));
        let mut vertical = [0u8; 9 * 7 * 4];
        super::pass(&dest, &mut vertical, 9, 7, 12, true);
        assert!(vertical.chunks_exact(4).all(|p| p[..3] == [37; 3]));
    }
}
