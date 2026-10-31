use windows::core::{Interface, Result};
use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use super::theme::Theme;

pub struct Canvas {
    pub context: ID2D1DeviceContext,
    pub brush: ID2D1SolidColorBrush,
    icons: std::cell::RefCell<std::collections::HashMap<u64, std::rc::Weak<ID2D1Bitmap1>>>,
}

impl Canvas {
    pub fn shared_icon(
        &self,
        source: &crate::shell::icons::Rgba,
    ) -> Result<std::rc::Rc<ID2D1Bitmap1>> {
        let key = super::icons::key(source);
        if let Some(image) = self
            .icons
            .borrow()
            .get(&key)
            .and_then(std::rc::Weak::upgrade)
        {
            return Ok(image);
        }
        let image = std::rc::Rc::new(self.system_icon(source)?);
        let mut icons = self.icons.borrow_mut();
        if icons.len() >= 512 {
            icons.retain(|_, image| image.strong_count() > 0);
        }
        if icons.len() >= 512 {
            icons.clear();
        }
        icons.insert(key, std::rc::Rc::downgrade(&image));
        Ok(image)
    }
    pub fn system_icon(&self, source: &crate::shell::icons::Rgba) -> Result<ID2D1Bitmap1> {
        let started = std::time::Instant::now();
        let mut pixels = Vec::with_capacity(source.pixels.len());
        for rgba in source.pixels.chunks_exact(4) {
            let alpha = rgba[3] as u32;
            for channel in [2, 1, 0] {
                pixels.push(((rgba[channel] as u32 * alpha + 254) / 255) as u8);
            }
            pixels.push(rgba[3]);
        }
        let result = unsafe {
            self.context.CreateBitmap(
                D2D_SIZE_U {
                    width: source.width,
                    height: source.height,
                },
                Some(pixels.as_ptr() as _),
                source.width * 4,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    ..Default::default()
                },
            )
        };
        if crate::shell::icons::thumb_probe() {
            eprintln!(
                "thumb-upload|width={}|height={}|total_us={}",
                source.width,
                source.height,
                started.elapsed().as_micros()
            );
        }
        result
    }
}

pub struct Renderer {
    pub(super) canvas: Canvas,
    pub(super) composition: IDCompositionDevice,
    _device: ID3D11Device,
}

#[derive(Clone, Copy, Default)]
pub struct Damage {
    pub pane: bool,
    pub header: bool,
    pub title: bool,
    pub commands: bool,
    pub address: bool,
    pub navigation: bool,
    pub status: bool,
    pub all: bool,
    pub view: bool,
    pub rows: Option<(usize, usize)>,
}

impl Damage {
    pub const NONE: Self = Self {
        pane: false,
        header: false,
        title: false,
        commands: false,
        address: false,
        navigation: false,
        status: false,
        all: false,
        view: false,
        rows: None,
    };
    pub const TITLE: Self = Self {
        title: true,
        ..Self::NONE
    };
    pub const VIEW: Self = Self {
        view: true,
        ..Self::NONE
    };
    pub const ALL: Self = Self {
        pane: true,
        header: true,
        title: true,
        commands: true,
        address: true,
        navigation: true,
        status: true,
        all: true,
        view: true,
        rows: None,
    };
    pub fn merge(self, other: Self) -> Self {
        Self {
            pane: self.pane || other.pane,
            header: self.header || other.header,
            title: self.title || other.title,
            commands: self.commands || other.commands,
            address: self.address || other.address,
            navigation: self.navigation || other.navigation,
            status: self.status || other.status,
            all: self.all || other.all,
            view: self.view || other.view,
            rows: match (self.rows, other.rows) {
                (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                (a, b) => a.or(b),
            },
        }
    }
}

pub struct Surface {
    assets: super::titlebar::Assets,
    title: Layer,
    content: Layer,
    pane: Layer,
    pane_assets: super::svg::Assets,
    commands: Layer,
    address: Layer,
    caret: super::caret::Caret,
    provider_icon: super::address::ProviderIcon,
    status: Layer,
    chrome_assets: super::svg::Assets,
    list: super::list_surface::Surface,
    navigation: super::navigation_surface::Surface,
    background_scale: IDCompositionScaleTransform,
    _target: IDCompositionTarget,
    root: IDCompositionVisual,
    floating_title: bool,
}

pub(super) struct Layer {
    pub(super) surface: IDCompositionVirtualSurface,
    pub(super) visual: IDCompositionVisual,
    pub(super) size: (u32, u32),
    alpha: D2D1_ALPHA_MODE,
    pub(super) presents: usize,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            super::memory::checkpoint("before-d3d");
            let mut device = None;
            let hardware = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT
                    | D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            );
            if let Err(error) = hardware {
                eprintln!("hardware device unavailable, using WARP: {error}");
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
            }
            let device = device.ok_or_else(windows::core::Error::from_win32)?;
            super::memory::device_info(&device);
            super::memory::checkpoint("d3d-device");
            let dxgi: IDXGIDevice = device.cast()?;
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = d2d.CreateDevice(&dxgi)?;
            let context = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            super::memory::checkpoint("d2d-context");
            let brush = context.CreateSolidColorBrush(&D2D1_COLOR_F::default(), None)?;
            let composition = DCompositionCreateDevice(&dxgi)?;
            super::memory::checkpoint("composition-device");
            Ok(Self {
                canvas: Canvas {
                    context,
                    brush,
                    icons: Default::default(),
                },
                composition,
                _device: device,
            })
        }
    }

    pub fn surface(&self, hwnd: HWND, icon: &crate::shell::icons::Rgba) -> Result<Surface> {
        unsafe {
            let title = self.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            super::memory::checkpoint("title-surface-init");
            let content = self.layer(DXGI_ALPHA_MODE_IGNORE)?;
            super::memory::checkpoint("background-surface-init");
            let list = super::list_surface::Surface::new(self)?;
            let navigation = super::navigation_surface::Surface::new(self)?;
            super::memory::checkpoint("list-surface-init");
            let pane = self.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            let commands = self.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            let address = self.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            let caret = super::caret::Caret::new(self)?;
            let status = self.layer(DXGI_ALPHA_MODE_IGNORE)?;
            let root = self.composition.CreateVisual()?;
            root.AddVisual(&content.visual, true, None)?;
            root.AddVisual(&title.visual, true, &content.visual)?;
            root.AddVisual(&list.root, true, &title.visual)?;
            root.AddVisual(&navigation.root, true, &list.root)?;
            root.AddVisual(&pane.visual, true, &navigation.root)?;
            root.AddVisual(&commands.visual, true, &pane.visual)?;
            root.AddVisual(&status.visual, true, &commands.visual)?;
            root.AddVisual(&address.visual, true, &status.visual)?;
            root.AddVisual(&caret.layer.visual, true, &address.visual)?;
            let target = self.composition.CreateTargetForHwnd(hwnd, false)?;
            target.SetRoot(&root)?;
            let background_scale = self.composition.CreateScaleTransform()?;
            content.visual.SetTransform(&background_scale)?;
            self.composition.Commit()?;
            Ok(Surface {
                assets: super::titlebar::Assets::new(&self.canvas, icon)?,
                title,
                content,
                pane,
                pane_assets: Default::default(),
                commands,
                address,
                caret,
                provider_icon: Default::default(),
                status,
                chrome_assets: Default::default(),
                list,
                navigation,
                background_scale,
                _target: target,
                root,
                floating_title: false,
            })
        }
    }

    pub(super) fn layer(&self, alpha: DXGI_ALPHA_MODE) -> Result<Layer> {
        unsafe {
            let surface =
                self.composition
                    .CreateVirtualSurface(1, 1, DXGI_FORMAT_B8G8R8A8_UNORM, alpha)?;
            let visual = self.composition.CreateVisual()?;
            visual.SetContent(&surface)?;
            Ok(Layer {
                surface,
                visual,
                size: (1, 1),
                alpha: if alpha == DXGI_ALPHA_MODE_IGNORE {
                    D2D1_ALPHA_MODE_IGNORE
                } else {
                    D2D1_ALPHA_MODE_PREMULTIPLIED
                },
                presents: 0,
            })
        }
    }

    pub fn paint(
        &self,
        surface: &mut Surface,
        size: (u32, u32),
        theme: &Theme,
        dpi: u32,
        text: &mut super::text::TextEngine,
        list: &super::list::List,
        titlebar: &mut super::titlebar::TitleBar,
        chrome: &mut super::chrome::State,
        address: &mut super::address::State,
        navigation: &super::navigation::State,
        pane: &super::panes::State,
        settings: &mut super::settings::State,
        damage: Damage,
        scroll: &super::scroll::State,
    ) -> Result<()> {
        if size.0 == 0 || size.1 == 0 {
            return Ok(());
        }
        let scale = dpi as f32 / 96.0;
        let upper_shift = super::details::top() - super::details::TOP;
        unsafe {
            surface.title.visual.SetOffsetY2(upper_shift * scale)?;
        }
        let floating = titlebar.drag.is_some_and(|drag| drag.active);
        if floating != surface.floating_title {
            unsafe {
                surface.root.RemoveVisual(&surface.title.visual)?;
                surface.root.AddVisual(
                    &surface.title.visual,
                    true,
                    if floating {
                        None
                    } else {
                        Some(&surface.content.visual)
                    },
                )?;
            }
            surface.floating_title = floating;
        }
        let extra = titlebar
            .drag
            .filter(|drag| drag.active)
            .map_or(0.0, |drag| (drag.pointer.1 - drag.origin.1).max(0.0));
        let title_height = ((super::titlebar::HEIGHT + extra) * scale)
            .round()
            .min(size.1 as f32) as u32;
        let title_resized = surface.title.resize((size.0, title_height.max(1)))?;
        if upper_shift == 0.0
            && (title_resized || surface.title.presents == 0 || damage.title || damage.all)
        {
            self.update(
                &mut surface.title,
                None,
                dpi,
                &D2D1_COLOR_F::default(),
                |canvas| {
                    titlebar.draw(
                        &mut surface.assets,
                        canvas,
                        text,
                        theme,
                        size.0 as f32 / scale,
                        scale,
                    )
                },
            )?;
            if surface.title.presents == 1 {
                super::memory::checkpoint("title-painted");
            }
        }
        if surface.content.presents == 0 || damage.all {
            let content_top = (super::details::top() * scale).round().min(size.1 as f32) as u32;
            self.update(&mut surface.content, None, 96, &theme.content, |_| Ok(()))?;
            unsafe {
                surface.content.visual.SetOffsetY2(content_top as f32)?;
                surface.background_scale.SetScaleX2(size.0 as f32)?;
                surface
                    .background_scale
                    .SetScaleY2((size.1 - content_top).max(1) as f32)?;
            }
            if surface.content.presents == 1 {
                super::memory::checkpoint("background-painted");
            }
        }
        let asset_names = chrome.asset_names();
        surface
            .chrome_assets
            .prepare(&self.canvas, theme, dpi, &asset_names)?;
        surface
            .provider_icon
            .prepare(&self.canvas, address.cloud_image.as_ref(), dpi)?;
        let address_resized = surface
            .address
            .resize((size.0, (48.0 * scale).round() as u32))?;
        if upper_shift == 0.0
            && (address_resized || surface.address.presents == 0 || damage.address || damage.all)
        {
            address.layout(size.0 as f32 / scale, text)?;
            self.update(
                &mut surface.address,
                None,
                dpi,
                &D2D1_COLOR_F::default(),
                |canvas| {
                    super::address::draw(
                        address,
                        &mut surface.chrome_assets,
                        &surface.provider_icon,
                        canvas,
                        text,
                        theme,
                    )
                },
            )?;
        }
        unsafe {
            surface
                .address
                .visual
                .SetOffsetY2((40.0 + upper_shift) * scale)?;
        }
        surface.caret.update(self, address, theme, dpi)?;
        if settings.active {
            let body_size = (
                size.0 as f32 / scale,
                (size.1 as f32 / scale - 88.0).max(1.0),
            );
            settings.layout(body_size, text)?;
            let resized = surface
                .pane
                .resize((size.0, (body_size.1 * scale).ceil() as u32))?;
            unsafe {
                surface.list.root.SetClip2(&D2D_RECT_F::default())?;
                surface.navigation.root.SetClip2(&D2D_RECT_F::default())?;
                surface.commands.visual.SetClip2(&D2D_RECT_F::default())?;
                surface.status.visual.SetClip2(&D2D_RECT_F::default())?;
                surface.pane.visual.SetOffsetX2(0.0)?;
                surface.pane.visual.SetOffsetY2(88.0 * scale)?;
                surface.pane.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: size.0 as f32,
                    bottom: body_size.1 * scale,
                })?;
            }
            if resized || damage.all || damage.pane || damage.view {
                self.update(&mut surface.pane, None, dpi, &theme.settings_background, |canvas| {
                    super::settings::draw(
                        settings,
                        canvas,
                        text,
                        theme,
                        chrome.focus,
                        chrome.cues && titlebar.active,
                    )
                })?;
            }
            unsafe {
                self.composition.Commit()?;
            }
            return Ok(());
        }
        unsafe {
            surface.commands.visual.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.0 as f32,
                bottom: 48.0 * scale,
            })?;
        }
        chrome.layout((size.0 as f32 / scale, size.1 as f32 / scale), text)?;
        let first_commands = surface.commands.presents == 0;
        if first_commands {
            super::memory::checkpoint("commands-before");
        }
        surface
            .chrome_assets
            .prepare(&self.canvas, theme, dpi, &asset_names)?;
        let command_resized = surface
            .commands
            .resize((size.0, (48.0 * scale).round() as u32))?;
        if upper_shift == 0.0
            && (command_resized || surface.commands.presents == 0 || damage.commands || damage.all)
        {
            self.update(
                &mut surface.commands,
                None,
                dpi,
                &D2D1_COLOR_F::default(),
                |canvas| {
                    super::chrome::draw_commands(
                        chrome,
                        &mut surface.chrome_assets,
                        canvas,
                        text,
                        theme,
                    )
                },
            )?;
        }
        if first_commands {
            super::memory::checkpoint("commands-painted");
        }
        let first_status = surface.status.presents == 0;
        let status_resized = surface
            .status
            .resize((size.0, (24.0 * scale).round() as u32))?;
        if status_resized || surface.status.presents == 0 || damage.status || damage.all {
            self.update(&mut surface.status, None, dpi, &theme.status, |canvas| {
                super::chrome::draw_status(chrome, canvas, text, theme)
            })?;
        }
        if first_status {
            super::memory::checkpoint("status-painted");
        }
        if first_commands && super::memory::enabled() {
            eprintln!(
                "native-bars|commands={:?}|status={:?}|text={:?}|theme-bytes={}",
                surface.commands.size,
                surface.status.size,
                text.cache_counts(),
                std::mem::size_of::<Theme>()
            );
        }
        unsafe {
            surface
                .commands
                .visual
                .SetOffsetY2((88.0 + upper_shift) * scale)?;
            surface
                .status
                .visual
                .SetOffsetY2((size.1 as f32 - 24.0 * scale).max(0.0))?;
            surface.status.visual.SetClip2(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.0 as f32,
                bottom: if chrome.status_visible {
                    24.0 * scale
                } else {
                    0.0
                },
            })?;
        }
        if damage.header || damage.view || damage.all || damage.rows.is_some() {
            surface
                .list
                .paint(self, size, dpi, list, text, theme, scroll, damage)
                .map_err(|error| {
                    windows::core::Error::new(error.code(), format!("list surface: {error}"))
                })?;
        }
        if pane.kind.is_some() {
            let rect = pane.bounds;
            let resized = surface.pane.resize((
                ((rect.right - rect.left) * scale).round().max(1.0) as u32,
                ((rect.bottom - rect.top) * scale).round().max(1.0) as u32,
            ))?;
            unsafe {
                surface.pane.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: (rect.right - rect.left) * scale,
                    bottom: (rect.bottom - rect.top) * scale,
                })?;
                surface.pane.visual.SetOffsetX2(rect.left * scale)?;
                surface.pane.visual.SetOffsetY2(rect.top * scale)?;
            }
            if resized || damage.pane || damage.all {
                surface.pane_assets.prepare(
                    &self.canvas,
                    theme,
                    dpi,
                    &["Windows.ModernShare", "windows.properties"],
                )?;
                self.update(&mut surface.pane, None, dpi, &theme.content, |canvas| {
                    super::panes::draw(pane, canvas, text, theme, &surface.pane_assets)
                })?;
            }
        } else {
            unsafe {
                surface.pane.visual.SetClip2(&D2D_RECT_F::default())?;
            }
        }
        if damage.navigation || damage.all || damage.view {
            super::memory::checkpoint("navigation-paint-before");
            surface
                .navigation
                .paint(self, navigation, text, theme, dpi, damage)
                .map_err(|error| {
                    windows::core::Error::new(error.code(), format!("navigation surface: {error}"))
                })?;
            super::memory::checkpoint("navigation-paint-after");
        }
        unsafe {
            self.composition.Commit().map_err(|error| {
                windows::core::Error::new(error.code(), format!("composition commit: {error}"))
            })?;
        }
        Ok(())
    }

    pub(super) fn update(
        &self,
        layer: &mut Layer,
        dirty: Option<RECT>,
        dpi: u32,
        clear: &D2D1_COLOR_F,
        draw: impl FnOnce(&Canvas) -> Result<()>,
    ) -> Result<()> {
        let dirty = dirty.unwrap_or(RECT {
            left: 0,
            top: 0,
            right: layer.size.0 as i32,
            bottom: layer.size.1 as i32,
        });
        if dirty.right <= dirty.left || dirty.bottom <= dirty.top {
            return Ok(());
        }
        unsafe {
            let mut offset = POINT::default();
            let dxgi: IDXGISurface =
                layer
                    .surface
                    .BeginDraw(Some(&dirty), &mut offset)
                    .map_err(|error| {
                        windows::core::Error::new(
                            error.code(),
                            format!("composition draw {dirty:?} in {:?}: {error}", layer.size),
                        )
                    })?;
            let result = (|| {
                if super::memory::enabled() && layer.presents == 0 {
                    let desc = dxgi.GetDesc()?;
                    let resource = dxgi.cast::<IDXGISurface2>().and_then(|surface| {
                        let mut subresource = 0;
                        surface.GetResource::<ID3D11Texture2D>(&mut subresource)
                    });
                    match resource {
                        Ok(resource) => eprintln!(
                            "native-surface|virtual={:?}|backing={}x{}|alpha={:?}|resource={:#x}",
                            layer.size,
                            desc.Width,
                            desc.Height,
                            layer.alpha,
                            resource.as_raw() as usize
                        ),
                        Err(error) => eprintln!("native surface resource probe: {error}"),
                    }
                }
                let target = self.canvas.context.CreateBitmapFromDxgiSurface(
                    &dxgi,
                    Some(&D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: layer.alpha,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                        ..Default::default()
                    }),
                )?;
                self.canvas.context.SetTarget(&target);
                self.canvas.context.SetDpi(dpi as f32, dpi as f32);
                let scale = dpi as f32 / 96.0;
                let mut original = Default::default();
                self.canvas.context.GetTransform(&mut original);
                let mut translated = original;
                translated.M31 = (offset.x - dirty.left) as f32 / scale;
                translated.M32 = (offset.y - dirty.top) as f32 / scale;
                self.canvas.context.SetTransform(&translated);
                let rect = D2D_RECT_F {
                    left: dirty.left as f32 / scale,
                    top: dirty.top as f32 / scale,
                    right: dirty.right as f32 / scale,
                    bottom: dirty.bottom as f32 / scale,
                };
                self.canvas.context.BeginDraw();
                self.canvas
                    .context
                    .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED);
                self.canvas
                    .context
                    .SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_COPY);
                super::details::fill(&self.canvas, rect, clear);
                self.canvas
                    .context
                    .SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_SOURCE_OVER);
                let drawn = draw(&self.canvas);
                self.canvas.context.PopAxisAlignedClip();
                let ended = self.canvas.context.EndDraw(None, None);
                self.canvas.context.SetTransform(&original);
                self.canvas.context.SetTarget(None);
                self.canvas.context.SetDpi(96.0, 96.0);
                drawn?;
                ended
            })();
            let finished = layer.surface.EndDraw();
            result.map_err(|error| {
                windows::core::Error::new(
                    error.code(),
                    format!("draw {dirty:?} in {:?}: {error}", layer.size),
                )
            })?;
            finished.map_err(|error| {
                windows::core::Error::new(
                    error.code(),
                    format!("surface EndDraw {dirty:?} in {:?}: {error}", layer.size),
                )
            })?;
            layer.presents += 1;
        }
        Ok(())
    }
}

impl Layer {
    pub(super) fn resize(&mut self, size: (u32, u32)) -> Result<bool> {
        if self.size == size {
            return Ok(false);
        }
        unsafe {
            self.surface.Resize(size.0, size.1)?;
            self.surface.Trim(Some(&[RECT {
                left: 0,
                top: 0,
                right: size.0 as i32,
                bottom: size.1 as i32,
            }]))?;
        }
        self.size = size;
        Ok(true)
    }
}

pub fn device_lost(error: &windows::core::Error) -> bool {
    matches!(
        error.code(),
        windows::Win32::Foundation::D2DERR_RECREATE_TARGET
            | DXGI_ERROR_DEVICE_REMOVED
            | DXGI_ERROR_DEVICE_RESET
            | DXGI_ERROR_DEVICE_HUNG
    )
}

impl Surface {
    pub fn chrome_presents(&self) -> (usize, usize) {
        (self.commands.presents, self.status.presents)
    }
    pub fn list_work(&self) -> (usize, usize) {
        (self.list.rows_drawn, self.list.pixels_drawn)
    }

    pub fn navigation_work(&self) -> (usize, usize) {
        self.navigation.work()
    }
    pub fn presents(&self) -> (usize, usize) {
        (self.title.presents, self.content.presents)
    }
    pub fn list_presents(&self) -> usize {
        self.list.presents
    }
}

#[cfg(test)]
pub(super) fn test_pixels(
    canvas: &Canvas,
    size: (u32, u32),
    paint: impl FnOnce() -> Result<()>,
) -> Result<Vec<u8>> {
    let context = &canvas.context;
    unsafe {
        let properties = |options| D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: options,
            ..Default::default()
        };
        let size = D2D_SIZE_U {
            width: size.0,
            height: size.1,
        };
        let target =
            context.CreateBitmap(size, None, 0, &properties(D2D1_BITMAP_OPTIONS_TARGET))?;
        context.SetTarget(&target);
        context.BeginDraw();
        context.Clear(Some(&D2D1_COLOR_F {
            r: 25.0 / 255.0,
            g: 25.0 / 255.0,
            b: 25.0 / 255.0,
            a: 1.0,
        }));
        let painted = paint();
        let finished = context.EndDraw(None, None);
        context.SetTarget(None);
        painted?;
        finished?;
        let readback = context.CreateBitmap(
            size,
            None,
            0,
            &properties(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
        )?;
        readback.CopyFromBitmap(None, &target, None)?;
        let mapped = readback.Map(D2D1_MAP_OPTIONS_READ)?;
        let mut rgb = Vec::new();
        for y in 0..size.height as usize {
            let row = std::slice::from_raw_parts(
                mapped.bits.add(y * mapped.pitch as usize),
                size.width as usize * 4,
            );
            for pixel in row.chunks_exact(4) {
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
        }
        readback.Unmap()?;
        Ok(rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_folder_icon_matches_measured_explorer_pixels() -> Result<()> {
        let renderer = Renderer::new()?;
        let index = crate::shell::icons::folder_icon();
        let reference = super::super::check::icon_reference(index)?;
        assert!(
            reference.as_slice() == include_bytes!("fixtures/folder-dark.rgb")
                || reference.as_slice() == include_bytes!("fixtures/folder-dark-current.rgb")
        );
        let source = crate::shell::icons::icon(index, 16).map_err(|error| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
        })?;
        let icon = renderer.canvas.system_icon(&source)?;
        let rgb = test_pixels(&renderer.canvas, (16, 16), || {
            unsafe {
                renderer.canvas.context.DrawBitmap(
                    &icon,
                    None,
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
            Ok(())
        })?;
        assert_eq!(rgb, reference);
        Ok(())
    }
}
