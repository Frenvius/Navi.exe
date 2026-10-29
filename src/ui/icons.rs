use crate::shell::icons::Rgba;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub fn key(source: &Rgba) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    source.width.hash(&mut hash);
    source.height.hash(&mut hash);
    source.pixels.hash(&mut hash);
    hash.finish()
}

pub fn shared(source: Rgba) -> Result<Arc<Rgba>, String> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Weak<Rgba>>>> = OnceLock::new();
    let key = key(&source);
    let mut entries = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|error| format!("native shared picture cache: {error}"))?;
    if let Some(image) = entries.get(&key).and_then(Weak::upgrade).filter(|image| {
        image.width == source.width
            && image.height == source.height
            && image.pixels == source.pixels
    }) {
        return Ok(image);
    }
    if entries.len() >= 512 {
        entries.retain(|_, image| image.strong_count() > 0);
    }
    if entries.len() >= 512 {
        entries.clear();
    }
    let source = Arc::new(source);
    entries.insert(key, Arc::downgrade(&source));
    Ok(source)
}

pub fn small(index: u32) -> Result<Arc<Rgba>, String> {
    static CACHE: OnceLock<Mutex<HashMap<u32, Weak<Rgba>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let lock = || {
        cache
            .lock()
            .map_err(|error| format!("native shared icon cache: {error}"))
    };
    if let Some(image) = lock()?.get(&index).and_then(Weak::upgrade) {
        return Ok(image);
    }
    let image = Arc::new(crate::shell::icons::icon(index, 16)?);
    let mut entries = lock()?;
    if let Some(existing) = entries.get(&index).and_then(Weak::upgrade) {
        return Ok(existing);
    }
    if entries.len() >= 512 {
        entries.retain(|_, image| image.strong_count() > 0);
    }
    if entries.len() >= 512 {
        entries.clear();
    }
    entries.insert(index, Arc::downgrade(&image));
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_active_pictures_share_pixels_without_retaining_closed_views() {
        let pixels = || Rgba {
            width: 1,
            height: 1,
            pixels: vec![42, 91, 163, 255],
        };
        let first = shared(pixels()).unwrap();
        let second = shared(pixels()).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let mut refreshed = pixels();
        refreshed.pixels[0] = 255;
        let refreshed = shared(refreshed).unwrap();
        assert!(!Arc::ptr_eq(&first, &refreshed));
        assert_eq!(refreshed.pixels[0], 255);
        assert_eq!(first.pixels[0], 42);
        let weak = Arc::downgrade(&first);
        drop(first);
        drop(second);
        assert!(weak.upgrade().is_none());
    }
}
