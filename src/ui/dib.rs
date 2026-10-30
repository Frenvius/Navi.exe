use windows::core::Result;
use windows::Win32::Graphics::Gdi::*;

pub(super) struct Dib {
    pub dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pub pixels: *mut u8,
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            if !DeleteObject(self.bitmap.into()).as_bool() {
                eprintln!(
                    "native bitmap bitmap release: {}",
                    windows::core::Error::from_win32()
                );
            }
            if !DeleteDC(self.dc).as_bool() {
                eprintln!(
                    "native bitmap context release: {}",
                    windows::core::Error::from_win32()
                );
            }
        }
    }
}

impl Dib {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(windows::core::Error::from_win32());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = std::ptr::null_mut();
            let bitmap =
                match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut pixels, None, 0) {
                    Ok(bitmap) => bitmap,
                    Err(error) => {
                        if !DeleteDC(dc).as_bool() {
                            eprintln!("native bitmap failed-context release");
                        }
                        return Err(error);
                    }
                };
            let previous = SelectObject(dc, bitmap.into());
            if previous.is_invalid() {
                let error = windows::core::Error::from_win32();
                DeleteObject(bitmap.into()).ok()?;
                DeleteDC(dc).ok()?;
                return Err(error);
            }
            Ok(Self {
                dc,
                bitmap,
                previous,
                pixels: pixels.cast(),
            })
        }
    }
}
