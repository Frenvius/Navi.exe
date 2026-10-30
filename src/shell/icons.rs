use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IShellFolder, IShellIcon, IShellIconOverlay, SHGetFileInfoW, SHGetImageList,
    SHMapPIDLToSystemImageListIndex, SHFILEINFOW, SHGFI_ICON, SHGFI_OVERLAYINDEX, SHGFI_PIDL,
    SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHIL_EXTRALARGE, SHIL_JUMBO,
    SHIL_LARGE, SHIL_SMALL,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use super::com_init;

const GIL_FORSHELL: u32 = 0x0002;
const GIL_CHECKSHIELD: u32 = 0x0200;

pub fn command_svg_path(name: &str) -> std::path::PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let theme = if super::theme::dark() {
        "theme-dark"
    } else {
        "theme-light"
    };
    std::path::Path::new(&windir)
        .join(r"SystemApps\MicrosoftWindows.Client.FileExp_cw5n1h2txyewy\FileExplorerExtensions\Assets\images\contrast-standard")
        .join(theme)
        .join(format!("{name}.svg"))
}

pub unsafe fn icon_of(
    folder: &IShellFolder,
    child: *const ITEMIDLIST,
    abs: *const ITEMIDLIST,
    is_link: bool,
) -> u32 {
    if is_link {
        return link_icon_of_pidl(abs);
    }
    let index = folder
        .cast::<IShellIcon>()
        .ok()
        .and_then(|icon| icon.GetIconOf(child, GIL_FORSHELL | GIL_CHECKSHIELD).ok())
        .unwrap_or_else(|| SHMapPIDLToSystemImageListIndex(folder, child, None));
    let base = if index >= 0 {
        index as u32 & 0x00ff_ffff
    } else {
        0
    };
    if let Ok(provider) = folder.cast::<IShellIconOverlay>() {
        let mut overlay = 0;
        if provider.GetOverlayIndex(child, &mut overlay).is_ok() && (1..=15).contains(&overlay) {
            return base | ((overlay as u32) << 24);
        }
    }
    base
}

pub fn generic_icon(path: &str) -> u32 {
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy())
        .unwrap_or_default();
    let wide = super::wide(&format!(".{ext}"));
    let mut info = SHFILEINFOW::default();
    let res = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX,
        )
    };
    if res == 0 {
        0
    } else {
        info.iIcon as u32 & 0x00FF_FFFF
    }
}

pub fn file_icon(path: &str) -> u32 {
    com_init();
    let Ok(pidl) = super::Pidl::parse(path) else {
        return 0;
    };
    let Some(parent) = pidl.parent() else {
        return 0;
    };
    let Ok(folder) = parent.folder() else {
        return 0;
    };
    unsafe { icon_of(&folder, pidl.last_id(), pidl.ptr(), false) }
}

/// The icon of an item named in an already bound folder. Parsing a full path walks the
/// namespace from the desktop, which costs milliseconds per item under a sync root.
pub fn icon_in(folder: &IShellFolder, name: &str) -> Option<u32> {
    let mut wide = super::wide(name);
    let mut child = std::ptr::null_mut();
    unsafe {
        folder
            .ParseDisplayName(
                HWND::default(),
                None,
                PWSTR(wide.as_mut_ptr()),
                None,
                &mut child,
                std::ptr::null_mut(),
            )
            .ok()?;
        let icon = icon_of(folder, child, std::ptr::null(), false);
        CoTaskMemFree(Some(child as *const _));
        Some(icon)
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

pub fn view_picture(path: &str, size: u32) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
    com_init();
    if size > 48 && super::archive::is_root(path) {
        return image_factory(path, size, Default::default(), true);
    }
    if super::listing::settings().icons_only {
        return image_factory(
            path,
            size,
            windows::Win32::UI::Shell::SIIGBF_ICONONLY,
            false,
        );
    }
    if size < 256 && !std::path::Path::new(path).is_dir() {
        let inset = size.div_ceil(32);
        if let Ok(thumb) = image_factory(path, size - 2 * inset + 1, SIIGBF_THUMBNAILONLY, false) {
            return Ok(compressed_marker(
                place_bottom(&thumb, size, inset),
                path,
                size.div_ceil(4).saturating_sub(1),
            ));
        }
    }
    thumbnail(path, size)
}

pub fn library_picture(path: &str, size: u32) -> Result<Rgba, String> {
    if size != 96 || std::path::Path::new(path).is_dir() {
        return view_picture(path, size);
    }
    com_init();
    if !super::listing::settings().icons_only {
        let inset = size.div_ceil(32);
        if let Ok(thumb) = image_factory(
            path,
            size - 2 * inset,
            windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,
            false,
        ) {
            return Ok(place_bottom(&thumb, size, inset));
        }
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
    Ok(resize(&icon(index, 48)?, size))
}

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

pub fn icon_picture(path: &str, id: &[u8], size: u32) -> Result<Rgba, String> {
    factory_by_id(path, id, size, windows::Win32::UI::Shell::SIIGBF_ICONONLY)
}

pub fn thumbnail_or_icon(path: &str, id: &[u8], size: u32) -> Result<Rgba, String> {
    let source = size * 3 / 2;
    factory_by_id(
        path,
        id,
        source,
        windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,
    )
    .or_else(|_| overlaid_icon(path, source).map_or_else(|| icon_picture(path, id, source), Ok))
    .map(|picture| shrink_to_fit(picture, size))
}

fn overlaid_icon(path: &str, size: u32) -> Option<Rgba> {
    let packed = file_icon(path);
    if packed >> 24 == 0 {
        return None;
    }
    icon(packed, size).ok()
}

fn shrink_to_fit(picture: Rgba, size: u32) -> Rgba {
    let longest = picture.width.max(picture.height);
    if longest <= size {
        return picture;
    }
    let fit = |side: u32| ((side * size) as f32 / longest as f32).round().max(1.0) as u32;
    hamming_resize(&picture, fit(picture.width), fit(picture.height))
}

fn hamming_resize(src: &Rgba, w: u32, h: u32) -> Rgba {
    let premultiplied: Vec<[f32; 4]> = src
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
    let rows = resample_rows(&premultiplied, src.width as usize, w as usize);
    let columns = resample_rows(
        &transpose(&rows, w as usize),
        src.height as usize,
        h as usize,
    );
    let pixels = transpose(&columns, h as usize)
        .into_iter()
        .flat_map(|[r, g, b, a]| {
            let a = a.clamp(0.0, 255.0);
            let straight = |c: f32| {
                if a > 0.0 {
                    (c * 255.0 / a).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            [straight(r), straight(g), straight(b), a.round() as u8]
        })
        .collect();
    Rgba {
        width: w,
        height: h,
        pixels,
    }
}

fn resample_rows(src: &[[f32; 4]], width: usize, out: usize) -> Vec<[f32; 4]> {
    let scale = width as f32 / out as f32;
    let stretch = scale.max(1.0);
    let kernels: Vec<(usize, Vec<f32>)> = (0..out)
        .map(|x| {
            let center = (x as f32 + 0.5) * scale;
            let start = (center - stretch + 0.5).max(0.0) as usize;
            let end = ((center + stretch + 0.5) as usize).min(width);
            let weights: Vec<f32> = (start..end)
                .map(|i| hamming((i as f32 - center + 0.5) / stretch))
                .collect();
            let total: f32 = weights.iter().sum();
            (start, weights.into_iter().map(|w| w / total).collect())
        })
        .collect();
    src.chunks_exact(width)
        .flat_map(|row| {
            kernels.iter().map(move |(start, weights)| {
                let mut pixel = [0f32; 4];
                for (offset, weight) in weights.iter().enumerate() {
                    for (channel, value) in pixel.iter_mut().zip(row[start + offset]) {
                        *channel += value * weight;
                    }
                }
                pixel
            })
        })
        .collect()
}

fn transpose(src: &[[f32; 4]], width: usize) -> Vec<[f32; 4]> {
    let height = src.len() / width;
    (0..width)
        .flat_map(|x| (0..height).map(move |y| src[y * width + x]))
        .collect()
}

fn hamming(x: f32) -> f32 {
    let x = x.abs();
    if x == 0.0 {
        return 1.0;
    }
    if x >= 1.0 {
        return 0.0;
    }
    let x = x * std::f32::consts::PI;
    x.sin() / x * (0.54 + 0.46 * x.cos())
}

fn factory_by_id(
    path: &str,
    id: &[u8],
    size: u32,
    only: windows::Win32::UI::Shell::SIIGBF,
) -> Result<Rgba, String> {
    com_init();
    if id.is_empty() {
        return image_factory(path, size, only, false);
    }
    unsafe {
        let factory = windows::Win32::UI::Shell::SHCreateItemFromIDList::<
            windows::Win32::UI::Shell::IShellItemImageFactory,
        >(id.as_ptr().cast())
        .map_err(|e| e.message())?;
        factory_picture(&factory, size, only, false)
    }
}

fn place_bottom(src: &Rgba, size: u32, inset: u32) -> Rgba {
    let fit = size - 2 * inset + 1;
    let scaled;
    let src = if src.width > fit || src.height > fit {
        let k = (fit as f32 / src.width as f32).min(fit as f32 / src.height as f32);
        scaled = area_resize(
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

fn resize_to(src: &Rgba, w: u32, h: u32) -> Rgba {
    if w >= src.width && h >= src.height {
        return bilinear_resize(src, w, h);
    }
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

pub fn stack_picture() -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::{
        SHGetStockIconInfo, SHGSI_ICONLOCATION, SHSTOCKICONINFO, SIID_STACK,
    };
    com_init();
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        SHGetStockIconInfo(SIID_STACK, SHGSI_ICONLOCATION, &mut info)
            .map_err(|error| error.message())?;
    }
    let end = info
        .szPath
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szPath.len());
    let path = String::from_utf16_lossy(&info.szPath[..end]);
    icon_at(&path, info.iIcon, 96).ok_or_else(|| {
        format!(
            "Stock stack icon resource unavailable: {path},{}",
            info.iIcon
        )
    })
}

pub fn pane_icon(path: &str) -> Result<Rgba, String> {
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, SIIGBF_RESIZETOFIT,
    };
    com_init();
    unsafe {
        let path = super::wide(path);
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None)
                .map_err(|error| error.message())?;
        factory_picture_with_flags(&factory, 28, SIIGBF_ICONONLY | SIIGBF_RESIZETOFIT, true)
    }
}

pub fn preview_picture(path: &str) -> Result<Rgba, String> {
    com_init();
    let image = image_factory(
        path,
        256,
        windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,
        true,
    )?;
    let scale = (256.0 / image.width as f32)
        .min(256.0 / image.height as f32)
        .min(1.0);
    if scale == 1.0 {
        return Ok(image);
    }
    Ok(area_resize(
        &image,
        (image.width as f32 * scale).round().max(1.0) as u32,
        (image.height as f32 * scale).round().max(1.0) as u32,
    ))
}

pub fn thumbnail(path: &str, size: u32) -> Result<Rgba, String> {
    com_init();
    if size >= 256 {
        use windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY;
        if let Ok(thumb) = image_factory(path, size / 2, SIIGBF_THUMBNAILONLY, true) {
            return Ok(compressed_marker(
                pixel_scale(&thumb, size),
                path,
                size.div_ceil(4).saturating_sub(1),
            ));
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
pub fn gallery_picture(path: &str, id: &[u8], size: u32) -> Result<Rgba, String> {
    com_init();
    if !id.is_empty() {
        unsafe {
            if let Ok(factory) = windows::Win32::UI::Shell::SHCreateItemFromIDList::<
                windows::Win32::UI::Shell::IShellItemImageFactory,
            >(id.as_ptr().cast())
            {
                if let Ok(image) = factory_picture(
                    &factory,
                    size,
                    windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,
                    false,
                ) {
                    return Ok(image);
                }
            }
        }
    }
    image_factory(
        path,
        size,
        windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY,
        false,
    )
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

fn link_arrow(size: u32) -> Option<Rgba> {
    thread_local! {
        static ARROWS: std::cell::RefCell<std::collections::HashMap<u32, Option<Rgba>>> = std::cell::RefCell::new(Default::default());
    }
    ARROWS.with(|a| {
        a.borrow_mut()
            .entry(size)
            .or_insert_with(|| {
                let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
                icon_at(&format!(r"{windir}\System32\imageres.dll"), -163, size).map(straight_icon)
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
        return Ok(compressed_marker(
            thumb,
            path,
            size.div_ceil(4).saturating_sub(1),
        ));
    }
    let image = (|| {
        if let Some(icon) = extracted_icon(path, size).filter(|i| i.width == size) {
            return Ok(icon);
        }
        let typed = if path.to_lowercase().ends_with(".lnk") {
            super::ops::link_file(path).unwrap_or_default()
        } else {
            path.to_string()
        };
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
    })()?;
    Ok(compressed_marker(image, path, (size / 2).max(8)))
}

fn compressed_marker(mut image: Rgba, path: &str, size: u32) -> Rgba {
    use windows::Win32::Storage::FileSystem::{
        GetFileAttributesW, FILE_ATTRIBUTE_COMPRESSED, INVALID_FILE_ATTRIBUTES,
    };
    let attributes = unsafe { GetFileAttributesW(PCWSTR(super::wide(path).as_ptr())) };
    if attributes == INVALID_FILE_ATTRIBUTES || attributes & FILE_ATTRIBUTE_COMPRESSED.0 == 0 {
        return image;
    }
    static MARKERS: std::sync::OnceLock<Option<[Rgba; 2]>> = std::sync::OnceLock::new();
    let markers = MARKERS.get_or_init(|| {
        use windows::Win32::UI::Shell::{
            SHGetStockIconInfo, SHGSI_ICONLOCATION, SHSTOCKICONID, SHSTOCKICONINFO,
        };
        unsafe {
            let mut info = SHSTOCKICONINFO {
                cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
                ..Default::default()
            };
            if let Err(error) =
                SHGetStockIconInfo(SHSTOCKICONID(179), SHGSI_ICONLOCATION, &mut info)
            {
                eprintln!("compression icon: {error}");
                return None;
            }
            let end = info
                .szPath
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(info.szPath.len());
            let file = String::from_utf16_lossy(&info.szPath[..end]);
            Some([
                straight_icon(icon_at(&file, info.iIcon, 32)?),
                straight_icon(icon_at(&file, info.iIcon, 256)?),
            ])
        }
    });
    if let Some(markers) = markers {
        let size = size.min(image.width).min(image.height);
        if size > 0 {
            let marker = if size < 32 {
                bilinear_resize(&markers[0], size, size)
            } else {
                area_resize(&markers[1], size, size)
            };
            let x = image.width - size;
            draw_over(&mut image, &marker, x, 0);
        }
    }
    image
}

fn straight_icon(mut image: Rgba) -> Rgba {
    for pixel in image.pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    image
}

fn bilinear_resize(source: &Rgba, width: u32, height: u32) -> Rgba {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let sx = ((x as f32 + 0.5) * source.width as f32 / width as f32 - 0.5)
                .clamp(0.0, source.width as f32 - 1.0);
            let sy = ((y as f32 + 0.5) * source.height as f32 / height as f32 - 0.5)
                .clamp(0.0, source.height as f32 - 1.0);
            let ix = sx.floor() as u32;
            let iy = sy.floor() as u32;
            let fx = sx - ix as f32;
            let fy = sy - iy as f32;
            let mut value = [0.0; 4];
            for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
                    let index = (((iy + dy).min(source.height - 1) * source.width
                        + (ix + dx).min(source.width - 1))
                        * 4) as usize;
                    let pixel = &source.pixels[index..index + 4];
                    let alpha = pixel[3] as f32 / 255.0;
                    for channel in 0..3 {
                        value[channel] += pixel[channel] as f32 * alpha * wx * wy;
                    }
                    value[3] += alpha * wx * wy;
                }
            }
            for channel in 0..3 {
                pixels.push(if value[3] > 0.0 {
                    (value[channel] / value[3]).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                });
            }
            pixels.push((value[3] * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    Rgba {
        width,
        height,
        pixels,
    }
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
    use windows::Win32::UI::Shell::{SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT};
    factory_picture_with_flags(
        factory,
        size,
        SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK | only,
        keep_frame,
    )
}

fn factory_picture_with_flags(
    factory: &windows::Win32::UI::Shell::IShellItemImageFactory,
    size: u32,
    flags: windows::Win32::UI::Shell::SIIGBF,
    keep_frame: bool,
) -> Result<Rgba, String> {
    use windows::Win32::Foundation::SIZE;
    unsafe {
        let side = size as i32;
        let bitmap = factory
            .GetImage(SIZE { cx: side, cy: side }, flags)
            .map_err(|error| error.message())?;
        let (w, h, pixels) = bitmap_pixels(bitmap);
        let _ = DeleteObject(HGDIOBJ::from(bitmap));
        let (w, h, pixels) = match (!keep_frame).then(|| unframe(w, h, &pixels)).flatten() {
            Some(scaled) => (w, h, scaled),
            None => (w, h, pixels),
        };
        let opaque = pixels.iter().all(|px| px >> 24 == 0);
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for px in &pixels {
            let [b, g, r, a] = px.to_le_bytes();
            if opaque {
                rgba.extend_from_slice(&[r, g, b, 255]);
                continue;
            }
            let a = if flags.0 & windows::Win32::UI::Shell::SIIGBF_ICONONLY.0 != 0 {
                a.max(r).max(g).max(b)
            } else {
                a
            };
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
pub(super) fn expand_env(text: &str) -> String {
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
    let expanded = expand_env(location);
    let location = expanded.trim().trim_matches('"');
    if location.starts_with('@') {
        let w = super::wide(location);
        let mut buf = [0u16; 1024];
        unsafe { SHLoadIndirectString(PCWSTR(w.as_ptr()), &mut buf, None).ok()? };
        let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
        let resolved = String::from_utf16_lossy(&buf[..n]);
        let path = std::path::Path::new(&resolved);
        if let (Some(parent), Some(stem)) =
            (path.parent(), path.file_stem().and_then(|s| s.to_str()))
        {
            let mut parents = Vec::new();
            if let Some((package, resource)) = location
                .strip_prefix("@{")
                .and_then(|value| value.split_once('?'))
            {
                if let Some(relative) = resource.split("/Files/").nth(1) {
                    let key = format!(
                        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages\{package}"
                    );
                    if let Ok(root) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                        .open_subkey(key)
                        .and_then(|key| key.get_value::<String, _>("PackageRootFolder"))
                    {
                        if let Some(parent) = std::path::Path::new(&root)
                            .join(relative.trim_end_matches('}'))
                            .parent()
                        {
                            parents.push(parent.to_owned());
                        }
                    }
                }
            }
            parents.push(parent.to_owned());
            let base = stem
                .split(".scale-")
                .next()
                .unwrap_or(stem)
                .split(".targetsize-")
                .next()
                .unwrap_or(stem);
            for parent in parents {
                if let Some(icon) = qualified_bitmap(&parent.join(format!("{base}.png"))) {
                    return Some(icon);
                }
            }
        }
        return bitmap_icon(&resolved);
    }
    let (file, index) = match location.rsplit_once(',') {
        Some((file, index)) if index.trim().parse::<i32>().is_ok() => {
            (file, index.trim().parse::<i32>().unwrap())
        }
        _ => (location, 0),
    };
    let lower = file.to_lowercase();
    if [".png", ".jpg", ".jpeg", ".bmp"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return qualified_bitmap(std::path::Path::new(file)).or_else(|| bitmap_icon(file));
    }
    let w = super::wide(file);
    let mut small = HICON::default();
    unsafe {
        if SHDefExtractIconW(
            PCWSTR(w.as_ptr()),
            index,
            0,
            None,
            Some(&mut small),
            16 | (16 << 16),
        )
        .is_err()
            || small.is_invalid()
        {
            return None;
        }
        let rgba = hicon_to_rgba(small).ok();
        let _ = DestroyIcon(small);
        rgba
    }
}

fn bitmap_icon(path: &str) -> Option<Rgba> {
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    com_init();
    let result = (|| -> windows::core::Result<Rgba> {
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let filename = super::wide(path);
            let decoder = factory.CreateDecoderFromFilename(
                PCWSTR(filename.as_ptr()),
                None,
                windows::Win32::Foundation::GENERIC_READ,
                WICDecodeMetadataCacheOnLoad,
            )?;
            let frame = decoder.GetFrame(0)?;
            let (mut width, mut height) = (0, 0);
            frame.GetSize(&mut width, &mut height)?;
            if width == 0 || height == 0 {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            let mut source: IWICBitmapSource = frame.cast()?;
            let scale = (16.0 / width as f64).min(16.0 / height as f64);
            let target = (
                (width as f64 * scale).round().clamp(1.0, 16.0) as u32,
                (height as f64 * scale).round().clamp(1.0, 16.0) as u32,
            );
            if target != (width, height) {
                let scaler = factory.CreateBitmapScaler()?;
                scaler.Initialize(&source, target.0, target.1, WICBitmapInterpolationModeFant)?;
                source = scaler.cast()?;
            }
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &source,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            let mut data = vec![0; (target.0 * target.1 * 4) as usize];
            converter.CopyPixels(std::ptr::null(), target.0 * 4, &mut data)?;
            let mut pixels = vec![0; 16 * 16 * 4];
            let left = (16 - target.0) / 2;
            let top = (16 - target.1) / 2;
            for row in 0..target.1 {
                let start = (((top + row) * 16 + left) * 4) as usize;
                let source = (row * target.0 * 4) as usize;
                pixels[start..start + target.0 as usize * 4]
                    .copy_from_slice(&data[source..source + target.0 as usize * 4]);
            }
            Ok(Rgba {
                width: 16,
                height: 16,
                pixels,
            })
        }
    })();
    match result {
        Ok(image) => Some(image),
        Err(error) => {
            eprintln!("menu bitmap {path}: {error}");
            thumbnail(path, 16).ok()
        }
    }
}

pub fn small_namespace_icon(path: &str) -> Option<Rgba> {
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, IExtractIconW, IShellItem, SHCreateItemFromParsingName,
    };
    com_init();
    unsafe {
        let path = super::wide(path);
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok()?;
        let extract: IExtractIconW = item.BindToHandler(None, &BHID_SFUIObject).ok()?;
        let mut location = [0u16; 2048];
        let (mut index, mut flags) = (0, 0);
        extract
            .GetIconLocation(2, &mut location, &mut index, &mut flags)
            .ok()?;
        let length = location
            .iter()
            .position(|&value| value == 0)
            .unwrap_or(location.len());
        let name = String::from_utf16_lossy(&location[..length]);
        if let Some((file, scale)) = name.rsplit_once(".png-") {
            if scale.parse::<u32>().is_ok() {
                if let Some(icon) = qualified_bitmap(std::path::Path::new(&format!("{file}.png"))) {
                    return Some(icon);
                }
            }
        }
        let (mut large, mut small) = (HICON::default(), HICON::default());
        let result = extract.Extract(
            PCWSTR(location.as_ptr()),
            index as u32,
            Some(&mut large),
            Some(&mut small),
            16 | (16 << 16),
        );
        let icon = if small.is_invalid() { large } else { small };
        let image = if result.is_ok() && !icon.is_invalid() {
            hicon_to_rgba(icon).ok().map(straight_icon)
        } else {
            None
        };
        for handle in [
            large,
            if small == large {
                HICON::default()
            } else {
                small
            },
        ] {
            if !handle.is_invalid() {
                if let Err(error) = DestroyIcon(handle) {
                    eprintln!("namespace menu icon release: {error}");
                }
            }
        }
        image
    }
}

fn qualified_bitmap(path: &std::path::Path) -> Option<Rgba> {
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let base = stem.split(".scale-").next()?.split(".targetsize-").next()?;
    let form = if super::theme::dark() {
        "unplated"
    } else {
        "lightunplated"
    };
    for name in [
        format!("{base}.targetsize-16_altform-{form}.png"),
        format!("{base}.altform-{form}_targetsize-16.png"),
        format!("{base}.targetsize-16.png"),
    ] {
        let path = parent.join(name);
        if path.is_file() {
            return bitmap_icon(&path.to_string_lossy());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_keeps_the_aspect_and_flat_colours() {
        let picture = Rgba {
            width: 48,
            height: 24,
            pixels: [200, 40, 10, 255].repeat(48 * 24),
        };
        let small = shrink_to_fit(picture, 32);
        assert_eq!((small.width, small.height), (32, 16));
        assert!(small
            .pixels
            .chunks_exact(4)
            .all(|p| p == [200, 40, 10, 255]));
    }

    #[test]
    fn pictures_within_the_size_stay_untouched() {
        let picture = Rgba {
            width: 20,
            height: 32,
            pixels: [0, 0, 0, 0].repeat(20 * 32),
        };
        let kept = shrink_to_fit(picture, 32);
        assert_eq!((kept.width, kept.height), (20, 32));
    }
}
