//! UI-thread image cache: system image list icons and Explorer's SVG command icons.

use std::collections::HashMap;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::shell::icons::{self, Rgba};

const ASSETS: &str = r"SystemApps\MicrosoftWindows.Client.FileExp_cw5n1h2txyewy\FileExplorerExtensions\Assets\images\contrast-standard";

#[derive(Default)]
pub struct Images {
    icons: HashMap<(u32, u32), Image>,
    svgs: HashMap<String, Image>,
    thumbs: HashMap<(String, u32), Image>,
    /// The command icons come from the dark theme's folder.
    pub dark: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshed_shell_pixels_replace_stale_overlay_without_replacing_identical_images() {
        let mut images=Images::default();
        let mut rgba=Rgba{width:1,height:1,pixels:vec![0,0,255,255]};
        images.insert(42,16,&rgba);
        rgba.pixels=vec![255,0,0,255];
        images.insert(42,16,&rgba);
        let refreshed=images.icon(42,16);
        assert_eq!(refreshed.to_rgba8().unwrap().as_bytes(),rgba.pixels);
        images.insert(42,16,&rgba);
        assert_eq!(refreshed,images.icon(42,16));
    }
}

pub fn to_image(rgba: &Rgba) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba.pixels, rgba.width, rgba.height);
    Image::from_rgba8(buffer)
}

impl Images {
    /// A system image list icon (index + overlay) at `size` pixels.
    pub fn icon(&mut self, icon: u32, size: u32) -> Image {
        if let Some(hit) = self.icons.get(&(icon, size)) {
            return hit.clone();
        }
        let image = icons::icon(icon, size).map(|rgba| to_image(&rgba)).unwrap_or_default();
        self.icons.insert((icon, size), image.clone());
        image
    }

    /// The icon at `size` if already rendered, else the largest size of it that is (drawn
    /// scaled until the real one comes in).
    pub fn icon_cached(&self, icon: u32, size: u32) -> Option<Image> {
        self.icons.get(&(icon, size)).or_else(|| self.icons.iter().filter(|((i, _), _)| *i == icon).max_by_key(|((_, s), _)| *s).map(|(_, image)| image)).cloned()
    }

    /// Stores icons rendered on a worker thread.
    pub fn insert(&mut self, icon: u32, size: u32, rgba: &Rgba) {
        // Shell overlays can finish loading after an index was first rendered. A fresh
        // worker result must replace stale pixels even when the packed index is unchanged.
        // Keep the image identity when pixels match, so unchanged rows need no repaint.
        let unchanged = self.icons.get(&(icon,size)).and_then(Image::to_rgba8).is_some_and(|old| {
            old.width()==rgba.width && old.height()==rgba.height && old.as_bytes()==rgba.pixels
        });
        if !unchanged { self.icons.insert((icon,size),to_image(rgba)); }
    }

    pub fn has(&self, icon: u32, size: u32) -> bool {
        self.icons.contains_key(&(icon, size))
    }

    pub fn has_thumb(&self, path: &str, size: u32) -> bool {
        self.thumbs.contains_key(&(path.to_string(), size))
    }

    pub fn clear_thumbs(&mut self) { self.thumbs.clear(); }

    pub fn thumb(&self, path: &str, size: u32) -> Option<Image> {
        self.thumbs.get(&(path.to_string(), size)).cloned()
    }

    /// Drops every cached thumbnail of a file whose pixels changed.
    pub fn forget_thumbs(&mut self, path: &str) {
        self.thumbs.retain(|(p, _), _| p != path);
    }

    pub fn insert_thumb(&mut self, path: &str, size: u32, image: Image) {
        if self.thumbs.len() > 5000 {
            self.thumbs.clear();
        }
        self.thumbs.insert((path.to_string(), size), image);
    }

    /// Switches the command icons to the theme's set.
    pub fn set_dark(&mut self, dark: bool) {
        if self.dark != dark {
            self.dark = dark;
            self.svgs.clear();
        }
    }

    /// One of File Explorer's own command icons (e.g. `windows.cut`).
    pub fn svg(&mut self, name: &str) -> Image {
        if let Some(hit) = self.svgs.get(name) {
            return hit.clone();
        }
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let theme = if self.dark { "theme-dark" } else { "theme-light" };
        let path = std::path::Path::new(&windir).join(ASSETS).join(theme).join(format!("{name}.svg"));
        let image = Image::load_from_path(&path).unwrap_or_default();
        self.svgs.insert(name.to_string(), image.clone());
        image
    }
}
