use super::icons::Rgba;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

const LIMIT: u64 = 128 * 1024 * 1024;
const ENTRIES: usize = 4096;
const MAGIC: &[u8; 8] = b"NVTH0001";

struct Store {
    root: PathBuf,
    lease: Option<std::fs::File>,
    files: HashMap<PathBuf, (u64, SystemTime)>,
    bytes: u64,
}

fn hash(value: impl Hash) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut state);
    state.finish()
}

fn store() -> Option<&'static Mutex<Store>> {
    static STORE: OnceLock<Option<Mutex<Store>>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
                .join("Navi")
                .join("Thumbnails-v1");
            std::fs::create_dir_all(&root).ok()?;
            let lease = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(root.join("cache.lock"))
                .ok();
            let mut state = Store {
                root,
                lease,
                files: HashMap::new(),
                bytes: 0,
            };
            for file in std::fs::read_dir(&state.root).ok()?.flatten() {
                let path = file.path();
                if state.lease.is_none() {
                    continue;
                }
                if temporary(&path) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if !owned(&path) {
                    continue;
                }
                let Ok(metadata) = file.metadata() else {
                    continue;
                };
                state.bytes += metadata.len();
                state.files.insert(
                    path,
                    (
                        metadata.len(),
                        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    ),
                );
            }
            if state.lease.is_some() {
                state.trim(0);
            }
            Some(Mutex::new(state))
        })
        .as_ref()
}

fn owned(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "rgba")
        && path
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.len() == 16 && name.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn temporary(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "tmp")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.rsplit_once('.'))
            .is_some_and(|(key, process)| {
                key.len() == 16
                    && key.bytes().all(|c| c.is_ascii_hexdigit())
                    && process.parse::<u32>().is_ok()
            })
}

impl Store {
    fn trim(&mut self, reserve: u64) -> bool {
        if reserve > LIMIT {
            return false;
        }
        if self.bytes.saturating_add(reserve) > LIMIT || self.files.len() >= ENTRIES {
            for (path, (_, at)) in &mut self.files {
                if let Ok(modified) =
                    std::fs::metadata(path).and_then(|metadata| metadata.modified())
                {
                    *at = modified;
                }
            }
        }
        while self.bytes.saturating_add(reserve) > LIMIT || self.files.len() >= ENTRIES {
            let Some(path) = self
                .files
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(path, _)| path.clone())
            else {
                return false;
            };
            if std::fs::remove_file(&path).is_err() {
                return false;
            }
            if let Some((bytes, _)) = self.files.remove(&path) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        }
        true
    }
}

pub fn eligible(path: &str, size: u32) -> bool {
    (48..=256).contains(&size)
        && Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                [
                    "png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "heic", "avif",
                ]
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
            })
}

fn identity(path: &str, size: u32) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(format!(
        "{size}|{}|{}|{}|{}",
        metadata.len(),
        metadata.last_write_time(),
        metadata.file_attributes(),
        path
    ))
}

pub fn read(path: &str, size: u32) -> Option<Rgba> {
    let started = std::time::Instant::now();
    let key = identity(path, size)?;
    let mut state = store()?.lock().ok()?;
    let target = state.root.join(format!("{:016x}.rgba", hash(&key)));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&target)
        .ok()?;
    let length = file.metadata().ok()?.len();
    if length > 2 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.read_to_end(&mut bytes).ok()?;
    let image = decode(&bytes, &key, size)?;
    let now = SystemTime::now();
    let _ = file.set_modified(now);
    if state.lease.is_some() {
        state.files.insert(target, (length, now));
    }
    if super::icons::thumb_probe() {
        eprintln!(
            "thumb-disk|size={size}|total_us={}|path={path}",
            started.elapsed().as_micros()
        );
    }
    Some(image)
}

fn decode(bytes: &[u8], key: &str, size: u32) -> Option<Rgba> {
    if bytes.get(..8)? != MAGIC {
        return None;
    }
    let word = |offset| {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let key_len = word(8)? as usize;
    let width = word(12)?;
    let height = word(16)?;
    if width != size
        || height != size
        || key_len != key.len()
        || bytes.get(28..28 + key_len)? != key.as_bytes()
    {
        return None;
    }
    let pixels = bytes.get(28 + key_len..)?;
    if pixels.len() != width as usize * height as usize * 4 {
        return None;
    }
    let checksum = u64::from_le_bytes(bytes.get(20..28)?.try_into().ok()?);
    if checksum != hash(pixels) {
        return None;
    }
    Some(Rgba {
        width,
        height,
        pixels: pixels.to_vec(),
    })
}

pub fn key(path: &str, size: u32) -> Option<String> {
    identity(path, size)
}

pub fn write_if_current(path: &str, size: u32, key: Option<String>, image: &Rgba) {
    if let Some(key) = key.filter(|key| identity(path, size).as_ref() == Some(key)) {
        write_key(&key, image);
    }
}

fn write_key(key: &str, image: &Rgba) {
    let Some(cache) = store() else {
        return;
    };
    let Ok(mut state) = cache.lock() else {
        return;
    };
    if state.lease.is_none() {
        return;
    }
    let target = state.root.join(format!("{:016x}.rgba", hash(key)));
    let mut bytes = Vec::with_capacity(28 + key.len() + image.pixels.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&image.width.to_le_bytes());
    bytes.extend_from_slice(&image.height.to_le_bytes());
    bytes.extend_from_slice(&hash(&image.pixels[..]).to_le_bytes());
    bytes.extend_from_slice(key.as_bytes());
    bytes.extend_from_slice(&image.pixels);
    let previous = state.files.get(&target).map_or(0, |entry| entry.0);
    if !state.trim((bytes.len() as u64).saturating_sub(previous)) {
        return;
    }
    let temporary = target.with_extension(format!("{}.tmp", std::process::id()));
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
    else {
        return;
    };
    let saved = file.write_all(&bytes);
    drop(file);
    if saved.is_ok() && std::fs::rename(&temporary, &target).is_ok() {
        if let Some((old, _)) = state.files.remove(&target) {
            state.bytes = state.bytes.saturating_sub(old);
        }
        state.bytes += bytes.len() as u64;
        state
            .files
            .insert(target, (bytes.len() as u64, SystemTime::now()));
    } else {
        let _ = std::fs::remove_file(&temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_stale_identity_truncation_and_corrupt_pixels() {
        let key = "48|23|456|32|C:\\dummy.png";
        let pixels = vec![42u8; 48 * 48 * 4];
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&48u32.to_le_bytes());
        bytes.extend_from_slice(&48u32.to_le_bytes());
        bytes.extend_from_slice(&hash(&pixels[..]).to_le_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(&pixels);
        assert_eq!(decode(&bytes, key, 48).unwrap().pixels, pixels);
        assert!(decode(&bytes, "48|24|456|32|C:\\dummy.png", 48).is_none());
        assert!(decode(&bytes, key, 96).is_none());
        assert!(decode(&bytes[..bytes.len() - 1], key, 48).is_none());
        *bytes.last_mut().unwrap() ^= 1;
        assert!(decode(&bytes, key, 48).is_none());
    }

    #[test]
    fn source_changes_and_lru_limit_use_only_dummy_files() {
        let root = std::env::temp_dir().join(format!(
            "navi-thumb-test-{}-{}",
            std::process::id(),
            hash(SystemTime::now())
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("dummy.png");
        std::fs::write(&source, [1, 2, 3]).unwrap();
        let name = source.to_str().unwrap();
        let before = identity(name, 48).unwrap();
        assert_ne!(before, identity(name, 96).unwrap());
        std::fs::write(&source, [1, 2, 3, 4]).unwrap();
        let resized = identity(name, 48).unwrap();
        assert_ne!(before, resized);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap();
        file.set_modified(SystemTime::now() + std::time::Duration::from_secs(10))
            .unwrap();
        drop(file);
        assert_ne!(resized, identity(name, 48).unwrap());
        let older = root.join("0000000000000001.rgba");
        let newer = root.join("0000000000000002.rgba");
        std::fs::write(&older, MAGIC).unwrap();
        std::fs::write(&newer, MAGIC).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let mut state = Store {
            root: root.clone(),
            lease: None,
            files: HashMap::from([
                (older.clone(), (LIMIT / 2, SystemTime::UNIX_EPOCH)),
                (newer.clone(), (LIMIT / 2, SystemTime::now())),
            ]),
            bytes: LIMIT,
        };
        assert!(state.trim(1));
        assert!(!older.exists());
        assert!(newer.exists());
        assert_eq!(state.bytes, LIMIT / 2);
        assert!(!state.trim(LIMIT + 1));
        std::fs::remove_file(newer).unwrap();
        std::fs::remove_file(source).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn only_one_writer_can_own_the_disk_budget_and_cleanup_is_narrow() {
        let root = std::env::temp_dir().join(format!(
            "navi-thumb-lease-{}-{}",
            std::process::id(),
            hash(SystemTime::now())
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("cache.lock");
        let open = || {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&path)
        };
        let first = open().unwrap();
        assert!(open().is_err());
        drop(first);
        drop(open().unwrap());
        assert!(temporary(Path::new("0123456789abcdef.123.tmp")));
        assert!(!temporary(Path::new("user-picture.tmp")));
        assert!(!temporary(Path::new("0123456789abcdef.not-a-pid.tmp")));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
