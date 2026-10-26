//! System image list icons and shell thumbnails as straight-alpha RGBA buffers. An `icon`
//! packs the system image list index in the low 24 bits and the overlay index (e.g. the
//! shortcut arrow) in the high 8.

use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IShellFolder, IShellIcon, SHGetFileInfoW, SHGetImageList, SHMapPIDLToSystemImageListIndex,
    SHFILEINFOW, SHGFI_ICON, SHGFI_OVERLAYINDEX, SHGFI_PIDL, SHGFI_SMALLICON, SHGFI_SYSICONINDEX,
    SHIL_EXTRALARGE, SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use super::com_init;

const GIL_FORSHELL: u32 = 0x0002;

/// Icon of a child item, using the folder's fast `IShellIcon` path when it can answer
/// without extracting per-item icons. Shortcuts also carry their overlay.
pub unsafe fn icon_of(
    folder: &IShellFolder,
    child: *const ITEMIDLIST,
    abs: *const ITEMIDLIST,
    is_link: bool,
) -> u32 {
    if is_link {
        return link_icon_of_pidl(abs);
    }
    if let Ok(icon) = folder.cast::<IShellIcon>() {
        if let Ok(index) = icon.GetIconOf(child, GIL_FORSHELL) {
            return index as u32 & 0x00FF_FFFF;
        }
    }
    let index = SHMapPIDLToSystemImageListIndex(folder, child, None);
    if index >= 0 {
        index as u32 & 0x00FF_FFFF
    } else {
        0
    }
}

pub unsafe fn icon_of_pidl(abs: *const ITEMIDLIST) -> u32 {
    let mut info = SHFILEINFOW::default();
    let res = SHGetFileInfoW(
        PCWSTR(abs as *const u16),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_PIDL | SHGFI_SYSICONINDEX | SHGFI_OVERLAYINDEX,
    );
    if res == 0 {
        0
    } else {
        info.iIcon as u32
    }
}

/// A shortcut's icon with its arrow overlay: SHGetFileInfo only fills the overlay index when
/// it also makes the icon.
unsafe fn link_icon_of_pidl(abs: *const ITEMIDLIST) -> u32 {
    let mut info = SHFILEINFOW::default();
    let res = SHGetFileInfoW(
        PCWSTR(abs as *const u16),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_PIDL | SHGFI_SYSICONINDEX | SHGFI_OVERLAYINDEX | SHGFI_ICON | SHGFI_SMALLICON,
    );
    if !info.hIcon.is_invalid() {
        let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(info.hIcon);
    }
    if res == 0 {
        0
    } else {
        info.iIcon as u32
    }
}

/// The system's "find" icon (a magnifier), which a search results tab shows.
pub fn find_icon() -> u32 {
    use windows::Win32::UI::Shell::{
        SHGetStockIconInfo, SHGSI_SYSICONINDEX, SHSTOCKICONINFO, SIID_FIND,
    };
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        SHGetStockIconInfo(SIID_FIND, SHGSI_SYSICONINDEX, &mut info)
            .map(|_| info.iSysImageIndex as u32)
            .unwrap_or(0)
    }
}

/// The system's stack of documents (SIID_STACK), which the details pane shows for several
/// items.
pub fn stack_icon() -> u32 {
    use windows::Win32::UI::Shell::{
        SHGetStockIconInfo, SHGSI_SYSICONINDEX, SHSTOCKICONINFO, SIID_STACK,
    };
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        SHGetStockIconInfo(SIID_STACK, SHGSI_SYSICONINDEX, &mut info)
            .map(|_| info.iSysImageIndex as u32)
            .unwrap_or(0)
    }
}

/// The plain folder icon.
pub fn folder_icon() -> u32 {
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows::Win32::UI::Shell::SHGFI_USEFILEATTRIBUTES;
    let name = super::wide("folder");
    let mut info = SHFILEINFOW::default();
    let res = unsafe {
        SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            FILE_ATTRIBUTE_DIRECTORY,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if res == 0 {
        0
    } else {
        info.iIcon as u32 & 0x00FF_FFFF
    }
}

/// File Explorer's own icon (explorer.exe's), which "Abrir" shows on folders.
pub fn explorer_icon() -> u32 {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let name = super::wide(&format!(r"{windir}\explorer.exe"));
    let mut info = SHFILEINFOW::default();
    let res = unsafe {
        SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX,
        )
    };
    if res == 0 {
        0
    } else {
        info.iIcon as u32 & 0x00FF_FFFF
    }
}

/// The icon the shell uses for a file type, e.g. ".txt".
pub fn icon_for_extension(ext: &str) -> u32 {
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::UI::Shell::SHGFI_USEFILEATTRIBUTES;
    let name = super::wide(&format!("file{ext}"));
    let mut info = SHFILEINFOW::default();
    let res = unsafe {
        SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if res == 0 {
        0
    } else {
        info.iIcon as u32 & 0x00FF_FFFF
    }
}

fn list_for(size: u32) -> i32 {
    match size {
        0..=16 => SHIL_SMALL as i32,
        17..=32 => SHIL_LARGE as i32,
        33..=48 => SHIL_EXTRALARGE as i32,
        _ => SHIL_JUMBO as i32,
    }
}

/// Straight (not premultiplied) RGBA pixels.
#[derive(Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Renders an icon from the system image list that best fits `size` pixels.
pub fn icon(icon: u32, size: u32) -> Result<Rgba, String> {
    com_init();
    unsafe {
        let index = (icon & 0x00FF_FFFF) as i32;
        let overlay = icon >> 24;
        let images: IImageList = SHGetImageList(list_for(size)).map_err(|e| e.message())?;
        // INDEXTOOVERLAYMASK(i) == i << 8
        let flags = ILD_TRANSPARENT.0 as u32 | (overlay << 8);
        let hicon = images.GetIcon(index, flags).map_err(|e| e.message())?;
        let out = hicon_to_rgba(hicon);
        let _ = DestroyIcon(hicon);
        out
    }
}

unsafe fn bitmap_pixels(bitmap: HBITMAP) -> (i32, i32, Vec<u32>) {
    let mut bmp = BITMAP::default();
    GetObjectW(
        HGDIOBJ::from(bitmap),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut _ as *mut _),
    );
    let (w, h) = (bmp.bmWidth, bmp.bmHeight.abs());
    let mut buf: Vec<u32> = vec![0; (w * h).max(0) as usize];
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = GetDC(None);
    GetDIBits(
        dc,
        bitmap,
        0,
        h as u32,
        Some(buf.as_mut_ptr() as *mut _),
        &mut bi,
        DIB_RGB_COLORS,
    );
    ReleaseDC(None, dc);
    (w, h, buf)
}

pub unsafe fn hicon_to_rgba(hicon: HICON) -> Result<Rgba, String> {
    let mut info = ICONINFO::default();
    GetIconInfo(hicon, &mut info).map_err(|e| e.message())?;
    let (w, h, color) = bitmap_pixels(info.hbmColor);
    let has_alpha = color.iter().any(|px| px >> 24 != 0);
    let mask = if has_alpha {
        None
    } else {
        Some(bitmap_pixels(info.hbmMask).2)
    };
    let _ = DeleteObject(HGDIOBJ::from(info.hbmColor));
    let _ = DeleteObject(HGDIOBJ::from(info.hbmMask));

    let mut rgba = Vec::with_capacity(color.len() * 4);
    for (i, px) in color.iter().enumerate() {
        let [b, g, r, a] = px.to_le_bytes();
        let a = match &mask {
            Some(m) => {
                if m.get(i).copied().unwrap_or(0) & 0x00FF_FFFF == 0 {
                    255
                } else {
                    0
                }
            }
            None => a,
        };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Ok(Rgba {
        width: w as u32,
        height: h as u32,
        pixels: rgba,
    })
}

/// Thumbnail (or icon, when the item has none) exactly as Explorer draws it in icon views.
/// An icon without a large size comes back at its own size, centered in a frame of the
/// requested one (a faint 1px border): Explorer shows it scaled up instead. Works on
/// premultiplied BGRA.
fn unframe(w: i32, h: i32, pixels: &[u32]) -> Option<Vec<u32>> {
    let (w, h) = (w as usize, h as usize);
    let alpha = |x: usize, y: usize| (pixels[y * w + x] >> 24) as u8;
    if w != h || w < 64 || !(1..=60).contains(&alpha(0, 0)) || alpha(w / 2, 2) != 0 {
        return None;
    }
    // The drawn icon, inside the faint frame.
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            if alpha(x, y) > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 {
        return None;
    }
    let extent = (x1 - x0 + 1).max(y1 - y0 + 1);
    let n = [16usize, 20, 24, 32, 40, 48, 64, 72, 96, 128]
        .into_iter()
        .find(|&s| s >= extent)?;
    if n * 3 / 2 > w {
        return None;
    }
    let origin = (w - n) / 2;
    // Bilinear from the n×n icon at the center to the whole w×w.
    let scale = n as f32 / w as f32;
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let sx = ((x as f32 + 0.5) * scale - 0.5).clamp(0.0, n as f32 - 1.0);
            let sy = ((y as f32 + 0.5) * scale - 0.5).clamp(0.0, n as f32 - 1.0);
            let (ix, iy) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = (sx - ix as f32, sy - iy as f32);
            let at = |dx: usize, dy: usize| {
                pixels[(origin + (iy + dy).min(n - 1)) * w + origin + (ix + dx).min(n - 1)]
            };
            let mut px = 0u32;
            for shift in [0u32, 8, 16, 24] {
                let c = |p: u32| ((p >> shift) & 0xff) as f32;
                let top = c(at(0, 0)) * (1.0 - fx) + c(at(1, 0)) * fx;
                let bottom = c(at(0, 1)) * (1.0 - fx) + c(at(1, 1)) * fx;
                px |= ((top * (1.0 - fy) + bottom * fy).round() as u32).min(255) << shift;
            }
            out[y * w + x] = px;
        }
    }
    Some(out)
}

/// An item's icon at `size` from the system image list: the icon of the size nearest it in
/// ratio, scaled.
fn scaled_icon(path: &str, size: u32) -> Option<Rgba> {
    let pidl = super::Pidl::parse(path).ok()?;
    let index = unsafe { icon_of_pidl(pidl.0) };
    let ratio = |s: u32| (s as f32 / size as f32).ln().abs();
    let nearest = [16u32, 32, 48, 256]
        .into_iter()
        .min_by(|&a, &b| ratio(a).total_cmp(&ratio(b)))?;
    let icon = icon(index, nearest).ok()?;
    // An icon without a 256px image sits small in the jumbo list's top-left corner: Explorer
    // shows it centred in a frame, which the image factory draws.
    if nearest == 256
        && !icon
            .pixels
            .chunks_exact(4)
            .enumerate()
            .any(|(i, p)| p[3] > 0 && (i % 256 >= 64 || i / 256 >= 64))
    {
        return None;
    }
    // Explorer grows icons pixel for pixel.
    Some(if icon.width == size && icon.height == size {
        icon
    } else if size % icon.width == 0 {
        pixel_scale(&icon, size)
    } else {
        resize(&icon, size)
    })
}

/// What an icon view draws for an item at `size`: a thumbnail fits a box inset by a 32nd
/// of the size (rounded up) and sits on its bottom, centred (Explorer's layout); an icon fills it.
pub fn view_picture(path: &str, size: u32) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    com_init();
    if super::listing::settings().icons_only {
        return image_factory(
            path,
            size,
            windows::Win32::UI::Shell::SIIGBF_ICONONLY,
            false,
        );
    }
    // A folder's preview is drawn like an icon.
    if size < 256 && !std::path::Path::new(path).is_dir() {
        let inset = size.div_ceil(32);
        if let Ok(thumb) = image_factory(path, size - 2 * inset, SIIGBF_THUMBNAILONLY, false) {
            return Ok(place_bottom(&thumb, size, inset));
        }
    }
    thumbnail(path, size)
}

/// The aggregate library view uses its 48px shell list image enlarged at 96px,
/// rather than extracting a different 96px resource from the file association.
pub fn library_picture(path: &str, size: u32) -> Result<Rgba, String> {
    if size != 96 || std::path::Path::new(path).is_dir() {return view_picture(path,size);}
    com_init();
    if !super::listing::settings().icons_only {
        let inset=size.div_ceil(32);
        if let Ok(thumb)=image_factory(path,size-2*inset,windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,false) {
            return Ok(place_bottom(&thumb,size,inset));
        }
    }
    let set=super::ItemSet::new(&[path.to_string()])?;
    let index=unsafe {icon_of(&set.folder,set.children()[0],set.items[0].ptr(),path.to_lowercase().ends_with(".lnk"))};
    Ok(resize(&icon(index,48)?,size))
}

/// A discovered device's parsing name can discard provider-specific image data.
/// Ask the image factory for the enumerated identity, as the native folder view does.
pub fn network_picture(path: &str, id: &[u8], size: u32) -> Result<Rgba, String> {
    if id.is_empty() {
        return view_picture(path, size);
    }
    com_init();
    unsafe {
        let factory = windows::Win32::UI::Shell::SHCreateItemFromIDList::<
            windows::Win32::UI::Shell::IShellItemImageFactory,
        >(id.as_ptr().cast())
        .map_err(|e| e.message())?;
        factory_picture(&factory, size, Default::default(), false)
    }
}

/// `src` on a transparent `size` square, centred across and `inset` above the bottom.
fn place_bottom(src: &Rgba, size: u32, inset: u32) -> Rgba {
    // The factory may hand back a bigger cached thumbnail.
    let fit = size - 2 * inset;
    let scaled;
    let src = if src.width > fit || src.height > fit {
        let k = (fit as f32 / src.width as f32).min(fit as f32 / src.height as f32);
        scaled = resize_to(
            src,
            ((src.width as f32 * k).round() as u32).max(1),
            ((src.height as f32 * k).round() as u32).max(1),
        );
        &scaled
    } else {
        src
    };
    let n = size as usize;
    let mut pixels = vec![0u8; n * n * 4];
    let (w, h) = (src.width.min(size) as usize, src.height.min(size) as usize);
    // Measured: a pixel up and left of centred-on-the-inset-box.
    let x0 = ((n - w) / 2).saturating_sub(1);
    let y0 = (n - inset as usize).saturating_sub(h + 1);
    for y in 0..h {
        let from = y * src.width as usize * 4;
        let to = ((y0 + y) * n + x0) * 4;
        pixels[to..to + w * 4].copy_from_slice(&src.pixels[from..from + w * 4]);
    }
    Rgba {
        width: size,
        height: size,
        pixels,
    }
}

/// Nearest-neighbour resize to `size` square.
fn pixel_scale(src: &Rgba, size: u32) -> Rgba {
    let (sw, sh, n) = (src.width as usize, src.height as usize, size as usize);
    let mut pixels = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let i = ((y * sh / n) * sw + x * sw / n) * 4;
            pixels.extend_from_slice(&src.pixels[i..i + 4]);
        }
    }
    Rgba {
        width: size,
        height: size,
        pixels,
    }
}

/// Bilinear resize of a straight-alpha image to `size` square, through premultiplied
/// alpha so transparent edges don't darken.
fn resize(src: &Rgba, size: u32) -> Rgba {
    resize_to(src, size, size)
}

/// Resize to `w` by `h`: box-averaged when shrinking, bilinear when growing.
fn resize_to(src: &Rgba, w: u32, h: u32) -> Rgba {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (nw, nh) = (w as usize, h as usize);
    let pre: Vec<[f32; 4]> = src
        .pixels
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            [
                p[0] as f32 * a,
                p[1] as f32 * a,
                p[2] as f32 * a,
                p[3] as f32,
            ]
        })
        .collect();
    let (kx, ky) = (sw as f32 / nw as f32, sh as f32 / nh as f32);
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            let mut px = [0f32; 4];
            if kx > 1.0 || ky > 1.0 {
                // Shrinking: the average of the source pixels under this one.
                let (x0, x1) = (
                    (x as f32 * kx) as usize,
                    (((x + 1) as f32 * kx).ceil() as usize).min(sw).max(1),
                );
                let (y0, y1) = (
                    (y as f32 * ky) as usize,
                    (((y + 1) as f32 * ky).ceil() as usize).min(sh).max(1),
                );
                let mut count = 0.0;
                for sy in y0.min(sh - 1)..y1 {
                    for sx in x0.min(sw - 1)..x1 {
                        for (c, v) in px.iter_mut().enumerate() {
                            *v += pre[sy * sw + sx][c];
                        }
                        count += 1.0;
                    }
                }
                for v in px.iter_mut() {
                    *v /= count;
                }
            } else {
                let sx = ((x as f32 + 0.5) * kx - 0.5).clamp(0.0, sw as f32 - 1.0);
                let sy = ((y as f32 + 0.5) * ky - 0.5).clamp(0.0, sh as f32 - 1.0);
                let (ix, iy) = (sx.floor() as usize, sy.floor() as usize);
                let (fx, fy) = (sx - ix as f32, sy - iy as f32);
                let at =
                    |dx: usize, dy: usize| pre[(iy + dy).min(sh - 1) * sw + (ix + dx).min(sw - 1)];
                for (c, v) in px.iter_mut().enumerate() {
                    let top = at(0, 0)[c] * (1.0 - fx) + at(1, 0)[c] * fx;
                    let bottom = at(0, 1)[c] * (1.0 - fx) + at(1, 1)[c] * fx;
                    *v = top * (1.0 - fy) + bottom * fy;
                }
            }
            let a = px[3] / 255.0;
            let un = |c: f32| {
                if a > 0.0 {
                    (c / a).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            out.extend_from_slice(&[
                un(px[0]),
                un(px[1]),
                un(px[2]),
                px[3].round().clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    Rgba {
        width: w,
        height: h,
        pixels: out,
    }
}

/// The item's own icon resource at `size`, through its `IExtractIcon` (Windows picks the
/// image nearest that size and scales it).
fn extracted_icon(path: &str, size: u32) -> Option<Rgba> {
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, IExtractIconW, IShellItem, SHCreateItemFromParsingName, SHDefExtractIconW,
    };
    unsafe {
        let w = super::wide(path);
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let extract: IExtractIconW = item.BindToHandler(None, &BHID_SFUIObject).ok()?;
        let mut location = [0u16; 1024];
        let (mut index, mut flags) = (0i32, 0u32);
        extract
            .GetIconLocation(GIL_FORSHELL, &mut location, &mut index, &mut flags)
            .ok()?;
        // "*": an index into the system image list, which only has its fixed sizes.
        if location[0] == u16::from(b'*') {
            return None;
        }
        let mut large = HICON::default();
        let sizes = (size & 0xffff) | (16 << 16);
        let hr = extract.Extract(
            PCWSTR(location.as_ptr()),
            index as u32,
            Some(&mut large),
            None,
            sizes,
        );
        if hr.is_err() || large.is_invalid() {
            large = HICON::default();
            if SHDefExtractIconW(
                PCWSTR(location.as_ptr()),
                index,
                0,
                Some(&mut large),
                None,
                sizes,
            )
            .is_err()
            {
                return None;
            }
        }
        if large.is_invalid() {
            return None;
        }
        let rgba = hicon_to_rgba(large).ok();
        let _ = DestroyIcon(large);
        rgba
    }
}

/// An item's picture for the icon views: its thumbnail when it has one (images, folder
/// previews), else its icon resource at that size, else the nearest system icon scaled.
pub fn thumbnail(path: &str, size: u32) -> Result<Rgba, String> {
    com_init();
    // Explorer's extra large thumbnails are its 128px ones doubled; icons are drawn at 256.
    if size >= 256 {
        use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
        if let Ok(thumb) = image_factory(path, size / 2, SIIGBF_THUMBNAILONLY, true) {
            return Ok(pixel_scale(&thumb, size));
        }
    }
    picture(path, size, false)
}

/// The details pane's thumbnail (a picture's, a video's) for a `w` x `h` area: the cached
/// thumbnail the factory hands back (768px for a 299px ask), area-averaged down to fit; one
/// that is smaller stays so (the pane grows it). A shrunk one comes centred on the whole area.
/// An error for items that only have an icon.
pub fn pane_thumbnail(path: &str, w: u32, h: u32) -> Result<Rgba, String> {
    if super::listing::settings().icons_only {
        return Err("Thumbnails disabled by Folder Options".into());
    }
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    com_init();
    let thumb = image_factory(path, w.max(h), SIIGBF_THUMBNAILONLY, false)?;
    let scale = (w as f32 / thumb.width as f32).min(h as f32 / thumb.height as f32);
    if scale >= 1.0 {
        return Ok(thumb);
    }
    let (ew, eh) = (thumb.width as f32 * scale, thumb.height as f32 * scale);
    let small = area_resize(
        &thumb,
        (ew.round() as u32).max(1),
        (eh.round() as u32).max(1),
    );
    // Placed on the whole area by its exact size, as the real centres it (864x576 is 199.33
    // tall: 10px down).
    let mut canvas = Rgba {
        width: w,
        height: h,
        pixels: vec![0; (w * h * 4) as usize],
    };
    draw_over(
        &mut canvas,
        &small,
        ((w as f32 - ew) / 2.0).round() as u32,
        ((h as f32 - eh) / 2.0).round() as u32,
    );
    Ok(canvas)
}

/// Shrinks by averaging each target pixel's source area (premultiplied).
fn area_resize(src: &Rgba, w: u32, h: u32) -> Rgba {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (fx, fy) = (sw as f32 / w as f32, sh as f32 / h as f32);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let (y0, y1) = (y as f32 * fy, (y + 1) as f32 * fy);
        for x in 0..w as usize {
            let (x0, x1) = (x as f32 * fx, (x + 1) as f32 * fx);
            let mut acc = [0f32; 4];
            let mut total = 0f32;
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(sh) {
                let wy = (y1.min(sy as f32 + 1.0) - y0.max(sy as f32)).max(0.0);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(sw) {
                    let wx = (x1.min(sx as f32 + 1.0) - x0.max(sx as f32)).max(0.0);
                    let k = wx * wy;
                    let p = &src.pixels[(sy * sw + sx) * 4..][..4];
                    let a = p[3] as f32 / 255.0;
                    acc[0] += p[0] as f32 * a * k;
                    acc[1] += p[1] as f32 * a * k;
                    acc[2] += p[2] as f32 * a * k;
                    acc[3] += a * k;
                    total += k;
                }
            }
            let a = if total > 0.0 { acc[3] / total } else { 0.0 };
            let un = |c: f32| {
                if acc[3] > 0.0 {
                    (c / acc[3]).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            out.extend_from_slice(&[
                un(acc[0]),
                un(acc[1]),
                un(acc[2]),
                (a * 255.0).round() as u8,
            ]);
        }
    }
    Rgba {
        width: w,
        height: h,
        pixels: out,
    }
}

/// The details pane's picture: a thumbnail, else the image factory's icon at the size (the
/// real scales down a big icon there, not the 48px one doubled the icon views use).
pub fn pane_picture(path: &str, size: u32) -> Result<Rgba, String> {
    if super::listing::settings().icons_only {
        com_init();
        return image_factory(path, size, windows::Win32::UI::Shell::SIIGBF_ICONONLY, true);
    }
    // XAML requests the whole pane's width, then fits the framed shell picture
    // into its 96px icon slot. Keeping the factory frame also preserves the
    // deliberately tiny association icon in compressed-folder previews.
    if super::archive::is_root(path) {
        return image_factory(path, 299, Default::default(), true);
    }
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    com_init();
    if let Ok(thumb) = image_factory(path, size, SIIGBF_THUMBNAILONLY, false) {
        return Ok(thumb);
    }
    // An icon resource at the size, as the icon views draw it, is closer to the real than the
    // factory's.
    if let Some(icon) = extracted_icon(path, size).filter(|i| i.width == size) {
        return Ok(icon);
    }
    let typed = if path.to_lowercase().ends_with(".lnk") {
        super::ops::link_file(path).unwrap_or_default()
    } else {
        path.to_string()
    };
    if let Some(Some(icon)) = type_icon(&typed, size) {
        return Ok(icon);
    }
    shell_picture(path, size)
}

/// The shell's own picture of an item at `size` (a thumbnail, or its icon scaled from the
/// next larger one): what the address bar's drag visual draws.
pub fn shell_picture(path: &str, size: u32) -> Result<Rgba, String> {
    com_init();
    image_factory(path, size, Default::default(), false)
}
/// Gallery needs a real thumbnail, never a file-type icon while a cloud/cache
/// provider is still working. Its enumerated identity carries search metadata.
pub fn gallery_picture(path:&str,id:&[u8],size:u32)->Result<Rgba,String>{
    com_init();
    if !id.is_empty(){unsafe{
        if let Ok(factory)=windows::Win32::UI::Shell::SHCreateItemFromIDList::<windows::Win32::UI::Shell::IShellItemImageFactory>(id.as_ptr().cast()){
            if let Ok(image)=factory_picture(&factory,size,windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,false){return Ok(image);}
        }
    }}
    image_factory(path,size,windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,false)
}

/// The view's shell icon, including GIL_FORSHELL (which differs from SHGetFileInfo's
/// generic .txt icon). Explorer's drag image grows its 48px view icon to 96px.
pub fn drag_picture(path: &str, size: u32) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    com_init();
    if let Ok(thumb) = image_factory(path, size, SIIGBF_THUMBNAILONLY, false) {
        return Ok(thumb);
    }
    let set = super::ItemSet::new(&[path.to_string()])?;
    let index = unsafe {
        icon_of(
            &set.folder,
            set.children()[0],
            set.items[0].ptr(),
            path.to_lowercase().ends_with(".lnk"),
        )
    };
    if std::path::Path::new(path).is_dir() {
        return Ok(resize(&icon(index, 256)?, size));
    }
    Ok(resize(&icon(index, 48)?, size))
}

/// The XAML strip samples the jumbo shell icon with nearest-neighbour filtering.
/// In particular, its .txt has thin crisp lines, unlike the list's enlarged 48px icon.
pub fn tab_drag_picture(path: &str, size: u32) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    com_init();
    if let Ok(thumb) = image_factory(path, size, SIIGBF_THUMBNAILONLY, false) {
        return Ok(thumb);
    }
    let set = super::ItemSet::new(&[path.to_string()])?;
    let index = unsafe {
        icon_of(
            &set.folder,
            set.children()[0],
            set.items[0].ptr(),
            path.to_lowercase().ends_with(".lnk"),
        )
    };
    let source = icon(index, 256)?;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let sx = (((x as f64 + 0.5) * source.width as f64 / size as f64) as u32)
                .min(source.width - 1);
            let sy = (((y as f64 + 0.5) * source.height as f64 / size as f64) as u32)
                .min(source.height - 1);
            let i = ((sy * source.width + sx) * 4) as usize;
            pixels.extend_from_slice(&source.pixels[i..i + 4]);
        }
    }
    Ok(Rgba {
        width: size,
        height: size,
        pixels,
    })
}

fn picture(path: &str, size: u32, keep_frame: bool) -> Result<Rgba, String> {
    let link = path.to_lowercase().ends_with(".lnk");
    let mut picture = item_picture(path, size, keep_frame)?;
    // A shortcut's arrow, unscaled in the bottom-left corner: the 32px overlay image over
    // 48px icons (exact), the 48px one over 96px ones and the 96px one over 256px ones (the
    // real's are a little bolder).
    if link && size >= 48 && picture.width == size && picture.height == size {
        if let Some(arrow) = link_arrow(match size {
            0..=48 => 32,
            49..=128 => 48,
            _ => 96,
        }) {
            draw_over(&mut picture, &arrow, 0, size - arrow.height);
        }
    }
    Ok(picture)
}

/// The shortcut overlay image at `size` (imageres.dll's arrow, as the system image list's).
fn link_arrow(size: u32) -> Option<Rgba> {
    thread_local! {
        static ARROWS: std::cell::RefCell<std::collections::HashMap<u32, Option<Rgba>>> = std::cell::RefCell::new(Default::default());
    }
    ARROWS.with(|a| {
        a.borrow_mut()
            .entry(size)
            .or_insert_with(|| {
                let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
                icon_at(&format!(r"{windir}\System32\imageres.dll"), -163, size)
            })
            .clone()
    })
}

/// `top` composited over `base` (both straight alpha) at x, y.
fn draw_over(base: &mut Rgba, top: &Rgba, x: u32, y: u32) {
    for ty in 0..top.height {
        for tx in 0..top.width {
            let (bx, by) = (x + tx, y + ty);
            if bx >= base.width || by >= base.height {
                continue;
            }
            let t = &top.pixels[((ty * top.width + tx) * 4) as usize..][..4];
            let i = ((by * base.width + bx) * 4) as usize;
            let b = &mut base.pixels[i..i + 4];
            let (ta, ba) = (t[3] as f32 / 255.0, b[3] as f32 / 255.0);
            let a = ta + ba * (1.0 - ta);
            if a <= 0.0 {
                continue;
            }
            for c in 0..3 {
                b[c] = ((t[c] as f32 * ta + b[c] as f32 * ba * (1.0 - ta)) / a).round() as u8;
            }
            b[3] = (a * 255.0).round() as u8;
        }
    }
}

fn item_picture(path: &str, size: u32, keep_frame: bool) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    if let Ok(thumb) = image_factory(path, size, SIIGBF_THUMBNAILONLY, keep_frame) {
        return Ok(thumb);
    }
    if let Some(icon) = extracted_icon(path, size).filter(|i| i.width == size) {
        return Ok(icon);
    }
    // A shortcut to a file shows its target's type.
    let typed = if path.to_lowercase().ends_with(".lnk") {
        super::ops::link_file(path).unwrap_or_default()
    } else {
        path.to_string()
    };
    // Past the system image list's sizes (96px) Explorer draws the type's icon resource at
    // the size, as extracting it gives (a .dll, a .bat, an .exe without icons); a packaged
    // app's PNG it scales much as the image factory does.
    if size > 48 {
        match type_icon(&typed, size) {
            Some(Some(icon)) => return Ok(icon),
            Some(None) => return image_factory(path, size, Default::default(), keep_frame),
            None => {}
        }
    }
    if let Some(icon) = scaled_icon(path, size) {
        return Ok(icon);
    }
    image_factory(path, size, Default::default(), keep_frame)
}

fn image_factory(
    path: &str,
    size: u32,
    only: windows::Win32::UI::Shell::SIIGBF,
    keep_frame: bool,
) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName};

    unsafe {
        let w = super::wide(path);
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).map_err(|e| e.message())?;
        factory_picture(&factory, size, only, keep_frame)
    }
}

fn factory_picture(
    factory: &windows::Win32::UI::Shell::IShellItemImageFactory,
    size: u32,
    only: windows::Win32::UI::Shell::SIIGBF,
    keep_frame: bool,
) -> Result<Rgba, String> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::UI::Shell::{SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT};
    unsafe {
        let side = size as i32;
        let bitmap = factory
            .GetImage(
                SIZE { cx: side, cy: side },
                SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK | only,
            )
            .map_err(|e| e.message())?;
        let (w, h, pixels) = bitmap_pixels(bitmap);
        let _ = DeleteObject(HGDIOBJ::from(bitmap));
        // Extra large keeps the frame, as Explorer does.
        let (w, h, pixels) = match (!keep_frame).then(|| unframe(w, h, &pixels)).flatten() {
            Some(scaled) => (w, h, scaled),
            None => (w, h, pixels),
        };
        // A picture without alpha (a black PNG saved as RGB) comes with alpha 0 everywhere:
        // Explorer draws it opaque.
        let opaque = pixels.iter().all(|px| px >> 24 == 0);
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for px in &pixels {
            let [b, g, r, a] = px.to_le_bytes();
            if opaque {
                rgba.extend_from_slice(&[r, g, b, 255]);
                continue;
            }
            // The factory hands back premultiplied alpha.
            let un = |c: u8| {
                if a == 0 {
                    0
                } else {
                    ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
                }
            };
            rgba.extend_from_slice(&[un(r), un(g), un(b), a]);
        }
        Ok(Rgba {
            width: w as u32,
            height: h as u32,
            pixels: rgba,
        })
    }
}

/// A 16px icon from a shell icon location: `file,index` (index negative for a resource id),
/// an image file, or a packaged app's `@{Package?ms-resource://...png}`.
/// The file type's DefaultIcon resource at `size`: None when the type names none, Some(None)
/// when it names one this can't extract (a packaged app's "@{...}" PNG).
fn type_icon(path: &str, size: u32) -> Option<Option<Rgba>> {
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_DEFAULTICON,
    };
    let ext = std::path::Path::new(path).extension()?;
    let w = super::wide(&format!(".{}", ext.to_string_lossy()));
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_DEFAULTICON,
            PCWSTR(w.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
    }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
    let location = expand_env(String::from_utf16_lossy(&buf[..n]).trim().trim_matches('"'));
    if location.starts_with('@') {
        return Some(None);
    }
    // "%1": the file's own icons, else the system's generic program icon.
    if location == "%1" {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        return Some(
            icon_at(path, 0, size)
                .or_else(|| icon_at(&format!(r"{windir}\System32\imageres.dll"), -15, size)),
        );
    }
    let (file, index) = match location.rsplit_once(',') {
        Some((f, i)) if i.trim().parse::<i32>().is_ok() => {
            (f.to_string(), i.trim().parse::<i32>().unwrap())
        }
        _ => (location.clone(), 0),
    };
    Some(icon_at(&file, index, size))
}

/// `%NAME%` references replaced by the environment's values.
fn expand_env(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else {
            break;
        };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() && name != "1" => out.push_str(&value),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// An icon resource (`file`, `index` as in "imageres.dll,-102") at `size`; Windows picks the
/// image nearest that size and scales it.
pub fn icon_at(file: &str, index: i32, size: u32) -> Option<Rgba> {
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    let w = super::wide(file);
    let mut large = HICON::default();
    unsafe {
        if SHDefExtractIconW(
            PCWSTR(w.as_ptr()),
            index,
            0,
            Some(&mut large),
            None,
            size & 0xffff,
        )
        .is_err()
            || large.is_invalid()
        {
            return None;
        }
        let rgba = hicon_to_rgba(large).ok();
        let _ = DestroyIcon(large);
        rgba
    }
}

pub fn small_icon_at(location: &str) -> Option<Rgba> {
    use windows::Win32::UI::Shell::{SHDefExtractIconW, SHLoadIndirectString};
    let location = location.trim().trim_matches('"');
    if location.starts_with('@') {
        let w = super::wide(location);
        let mut buf = [0u16; 1024];
        unsafe { SHLoadIndirectString(PCWSTR(w.as_ptr()), &mut buf, None).ok()? };
        let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
        let resolved = String::from_utf16_lossy(&buf[..n]);
        let path = std::path::Path::new(&resolved);
        // PRI resolution commonly picks the 44px tile's scale-100 bitmap. Modern
        // menu image sources instead select the unplated 16px target-size asset.
        if let (Some(parent), Some(stem)) =
            (path.parent(), path.file_stem().and_then(|s| s.to_str()))
        {
            let base = stem.split(".scale-").next().unwrap_or(stem);
            let light = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
                .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
                .unwrap_or(0)
                != 0;
            let form = if light { "lightunplated" } else { "unplated" };
            for name in [
                format!("{base}.targetsize-16_altform-{form}.png"),
                format!("{base}.altform-{form}_targetsize-16.png"),
                format!("{base}.targetsize-16.png"),
            ] {
                let candidate = parent.join(name);
                if candidate.is_file() {
                    return thumbnail(&candidate.to_string_lossy(), 16).ok();
                }
            }
        }
        return thumbnail(&resolved, 16).ok();
    }
    let lower = location.to_lowercase();
    if [".png", ".jpg", ".jpeg", ".bmp"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return thumbnail(location, 16).ok();
    }
    let (file, index) = match location.rsplit_once(',') {
        Some((f, i)) if i.trim().parse::<i32>().is_ok() => (f, i.trim().parse::<i32>().unwrap()),
        _ => (location, 0),
    };
    let w = super::wide(file);
    let mut small = HICON::default();
    unsafe {
        if SHDefExtractIconW(PCWSTR(w.as_ptr()), index, 0, None, Some(&mut small), 16).is_err()
            || small.is_invalid()
        {
            return None;
        }
        let rgba = hicon_to_rgba(small).ok();
        let _ = DestroyIcon(small);
        rgba
    }
}
