use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, GetDriveTypeW, GetFileAttributesW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FIND_FIRST_EX_LARGE_FETCH, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING, WIN32_FIND_DATAW,
};
use windows::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, DEVICE_SEEK_PENALTY_DESCRIPTOR,
    IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
};
use windows::Win32::System::IO::DeviceIoControl;

const PROGRESS: Duration = Duration::from_millis(250);
const STALE: Duration = Duration::from_secs(120);
const CAPACITY: usize = 4096;
const CANCEL_CHECK: u32 = 4096;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_RAMDISK: u32 = 6;
const NAME_SURROGATE: u32 = 0x2000_0000;
const UNPOPULATED: u32 = FILE_ATTRIBUTE_OFFLINE.0
    | FILE_ATTRIBUTE_RECALL_ON_OPEN.0
    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderSize {
    pub bytes: u64,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Drive {
    Ssd,
    Hdd,
    Network,
}

impl Drive {
    fn concurrency(self) -> usize {
        match self {
            Self::Ssd => 2,
            Self::Hdd | Self::Network => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drives {
    pub ssd: bool,
    pub hdd: bool,
    pub network: bool,
}

impl Drives {
    pub fn allows(self, drive: Drive) -> bool {
        match drive {
            Drive::Ssd => self.ssd,
            Drive::Hdd => self.hdd,
            Drive::Network => self.network,
        }
    }

    pub fn any(self) -> bool {
        self.ssd || self.hdd || self.network
    }
}

pub fn scannable(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix(r"\\") {
        let mut parts = rest.split('\\').filter(|part| !part.is_empty());
        return !rest.starts_with(['?', '.']) && parts.next().is_some() && parts.next().is_some();
    }
    let bytes = path.as_bytes();
    bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn key(path: &str) -> String {
    path.trim_end_matches('\\').to_lowercase()
}

fn drives() -> &'static Mutex<HashMap<u8, Option<Drive>>> {
    static DRIVES: OnceLock<Mutex<HashMap<u8, Option<Drive>>>> = OnceLock::new();
    DRIVES.get_or_init(Default::default)
}

pub fn drive(path: &str) -> Option<Drive> {
    if path.starts_with(r"\\") {
        return Some(Drive::Network);
    }
    let letter = path.as_bytes().first()?.to_ascii_uppercase();
    if !letter.is_ascii_uppercase() {
        return None;
    }
    if let Some(&known) = lock(drives()).get(&letter) {
        return known;
    }
    let classified = classify(letter);
    lock(drives()).insert(letter, classified);
    classified
}

fn classify(letter: u8) -> Option<Drive> {
    let root = super::wide(&format!("{}:\\", letter as char));
    match unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } {
        DRIVE_REMOTE => Some(Drive::Network),
        DRIVE_RAMDISK => Some(Drive::Ssd),
        DRIVE_FIXED | DRIVE_REMOVABLE => Some(match seek_penalty(letter) {
            Ok(false) => Drive::Ssd,
            Ok(true) => Drive::Hdd,
            Err(error) => {
                eprintln!("folder size drive {}: {error}", letter as char);
                Drive::Hdd
            }
        }),
        _ => None,
    }
}

fn seek_penalty(letter: u8) -> windows::core::Result<bool> {
    let device = super::wide(&format!(r"\\.\{}:", letter as char));
    let handle = unsafe {
        CreateFileW(
            PCWSTR(device.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )?
    };
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut descriptor = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
    let mut returned = 0u32;
    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as _),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(&mut descriptor as *mut _ as _),
            std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            Some(&mut returned),
            None,
        )
    };
    if let Err(error) = unsafe { CloseHandle(handle) } {
        eprintln!("folder size device close: {error}");
    }
    result.map(|()| descriptor.IncursSeekPenalty)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Walk {
    Done(u64),
    Failed,
    Cancelled,
}

fn long_path(path: &str) -> Vec<u16> {
    let path = path.trim_end_matches('\\');
    let prefixed = match path.strip_prefix(r"\\") {
        Some(rest) => format!(r"\\?\UNC\{rest}"),
        None => format!(r"\\?\{path}"),
    };
    prefixed.encode_utf16().collect()
}

fn terminated(path: &[u16], suffix: &str) -> Vec<u16> {
    let mut wide = Vec::with_capacity(path.len() + suffix.len() + 1);
    wide.extend_from_slice(path);
    wide.extend(suffix.encode_utf16());
    wide.push(0);
    wide
}

pub fn walk(root: &str, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Walk {
    let root = long_path(root);
    let attributes = unsafe { GetFileAttributesW(PCWSTR(terminated(&root, "").as_ptr())) };
    if attributes == INVALID_FILE_ATTRIBUTES
        || attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
        || attributes & UNPOPULATED != 0
    {
        return Walk::Failed;
    }
    let mut stack = vec![root];
    let mut total = 0u64;
    let mut first = true;
    let mut reported = Instant::now();
    let mut seen = 0u32;
    while let Some(directory) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Walk::Cancelled;
        }
        let pattern = terminated(&directory, "\\*");
        let mut data = WIN32_FIND_DATAW::default();
        let handle = unsafe {
            FindFirstFileExW(
                PCWSTR(pattern.as_ptr()),
                FindExInfoBasic,
                &mut data as *mut _ as _,
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        let handle = match handle {
            Ok(handle) => handle,
            Err(_) if first => return Walk::Failed,
            Err(_) => continue,
        };
        first = false;
        loop {
            let length = data
                .cFileName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(data.cFileName.len());
            let name = &data.cFileName[..length];
            let dots = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
            if !dots {
                let attributes = data.dwFileAttributes;
                if attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
                    total += (u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow);
                } else if attributes & UNPOPULATED == 0
                    && (attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
                        || data.dwReserved0 & NAME_SURROGATE == 0)
                {
                    let mut child = Vec::with_capacity(directory.len() + 1 + length);
                    child.extend_from_slice(&directory);
                    child.push(b'\\' as u16);
                    child.extend_from_slice(name);
                    stack.push(child);
                }
            }
            seen += 1;
            if seen % CANCEL_CHECK == 0 && cancel.load(Ordering::Relaxed) {
                close(handle);
                return Walk::Cancelled;
            }
            if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
                break;
            }
        }
        close(handle);
        if reported.elapsed() >= PROGRESS {
            progress(total);
            reported = Instant::now();
        }
    }
    Walk::Done(total)
}

fn close(handle: windows::Win32::Foundation::HANDLE) {
    if let Err(error) = unsafe { FindClose(handle) } {
        eprintln!("folder size find close: {error}");
    }
}

type Notify = Arc<dyn Fn() + Send + Sync>;

struct Want {
    key: String,
    path: String,
    drive: Drive,
}

struct Subscriber {
    wants: Vec<Want>,
    inbox: Vec<(String, FolderSize)>,
    notify: Notify,
}

struct Cached {
    bytes: Option<u64>,
    at: Instant,
    stale: bool,
}

impl Cached {
    fn fresh(&self) -> bool {
        !self.stale && self.at.elapsed() < STALE
    }
}

struct Running {
    cancel: Arc<AtomicBool>,
    drive: Drive,
    shown: bool,
}

struct Task {
    key: String,
    path: String,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Service {
    next: u64,
    subscribers: HashMap<u64, Subscriber>,
    cache: HashMap<String, Cached>,
    running: HashMap<String, Running>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn service() -> MutexGuard<'static, Service> {
    static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new();
    lock(SERVICE.get_or_init(Default::default))
}

impl Service {
    fn wanted(&self, key: &str) -> bool {
        self.subscribers
            .values()
            .any(|subscriber| subscriber.wants.iter().any(|want| want.key == key))
    }

    fn cancel_unwanted(&self) {
        for (key, running) in &self.running {
            if !self.wanted(key) {
                running.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    fn claim(&mut self) -> Option<Task> {
        let mut active = HashMap::<Drive, usize>::new();
        for running in self.running.values() {
            *active.entry(running.drive).or_default() += 1;
        }
        let mut ids = self.subscribers.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        let (key, path, drive) = ids.iter().find_map(|id| {
            self.subscribers[id].wants.iter().find_map(|want| {
                let busy = active.get(&want.drive).copied().unwrap_or(0);
                (busy < want.drive.concurrency()
                    && !self.running.contains_key(&want.key)
                    && !self.cache.get(&want.key).is_some_and(Cached::fresh))
                .then(|| (want.key.clone(), want.path.clone(), want.drive))
            })
        })?;
        let cancel = Arc::new(AtomicBool::new(false));
        let shown = self
            .cache
            .get(&key)
            .is_some_and(|cached| cached.bytes.is_some());
        self.running.insert(
            key.clone(),
            Running {
                cancel: cancel.clone(),
                drive,
                shown,
            },
        );
        Some(Task { key, path, cancel })
    }

    fn deliver(&mut self, key: &str, size: FolderSize) -> Vec<Notify> {
        let mut notify = Vec::new();
        for subscriber in self.subscribers.values_mut() {
            let Some(want) = subscriber.wants.iter().find(|want| want.key == key) else {
                continue;
            };
            if subscriber.inbox.is_empty() {
                notify.push(subscriber.notify.clone());
            }
            match subscriber
                .inbox
                .iter_mut()
                .find(|(path, _)| *path == want.path)
            {
                Some(slot) => slot.1 = size,
                None => subscriber.inbox.push((want.path.clone(), size)),
            }
        }
        notify
    }

    fn mark_stale(&mut self, path: &str) {
        let changed = key(path);
        for (key, cached) in self.cache.iter_mut() {
            if changed == *key
                || changed
                    .strip_prefix(key.as_str())
                    .is_some_and(|rest| rest.starts_with('\\'))
            {
                cached.stale = true;
            }
        }
    }

    fn store(&mut self, key: String, bytes: Option<u64>) {
        if self.cache.len() >= CAPACITY && !self.cache.contains_key(&key) {
            if let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, cached)| cached.at)
                .map(|(key, _)| key.clone())
            {
                self.cache.remove(&oldest);
            }
        }
        self.cache.insert(
            key,
            Cached {
                bytes,
                at: Instant::now(),
                stale: false,
            },
        );
    }
}

fn start(task: Task) {
    let spawned = std::thread::Builder::new()
        .name("folder-size".into())
        .spawn(move || {
            if let Err(error) =
                unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }
            {
                eprintln!("folder size background mode: {error}");
            }
            let mut task = Some(task);
            while let Some(current) = task.take() {
                task = run(current);
            }
        });
    if let Err(error) = spawned {
        eprintln!("folder size thread: {error}");
    }
}

fn run(task: Task) -> Option<Task> {
    let result = walk(&task.path, &task.cancel, |bytes| {
        let notify = {
            let mut service = service();
            if service
                .running
                .get(&task.key)
                .is_none_or(|running| running.shown)
            {
                return;
            }
            service.deliver(
                &task.key,
                FolderSize {
                    bytes,
                    complete: false,
                },
            )
        };
        notify.iter().for_each(|notify| notify());
    });
    let (notify, next) = {
        let mut service = service();
        service.running.remove(&task.key);
        let notify = match result {
            Walk::Done(bytes) => {
                service.store(task.key.clone(), Some(bytes));
                service.deliver(
                    &task.key,
                    FolderSize {
                        bytes,
                        complete: true,
                    },
                )
            }
            Walk::Failed => {
                service.store(task.key.clone(), None);
                Vec::new()
            }
            Walk::Cancelled => Vec::new(),
        };
        (notify, service.claim())
    };
    notify.iter().for_each(|notify| notify());
    next
}

pub fn invalidate(path: &str) {
    service().mark_stale(path);
}

pub struct Client {
    id: u64,
}

impl Client {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let mut service = service();
        service.next += 1;
        let id = service.next;
        service.subscribers.insert(
            id,
            Subscriber {
                wants: Vec::new(),
                inbox: Vec::new(),
                notify: Arc::new(notify),
            },
        );
        Self { id }
    }

    pub fn want(&self, paths: Vec<String>, allowed: Drives) -> Vec<(String, FolderSize)> {
        let wants = paths
            .into_iter()
            .filter_map(|path| {
                let drive = drive(&path).filter(|&drive| allowed.allows(drive))?;
                Some(Want {
                    key: key(&path),
                    path,
                    drive,
                })
            })
            .collect::<Vec<_>>();
        let mut tasks = Vec::new();
        let ready = {
            let mut service = service();
            let ready = wants
                .iter()
                .filter_map(|want| {
                    let bytes = service.cache.get(&want.key)?.bytes?;
                    Some((
                        want.path.clone(),
                        FolderSize {
                            bytes,
                            complete: true,
                        },
                    ))
                })
                .collect();
            if let Some(subscriber) = service.subscribers.get_mut(&self.id) {
                subscriber
                    .inbox
                    .retain(|(path, _)| wants.iter().any(|want| want.path == *path));
                subscriber.wants = wants;
            }
            service.cancel_unwanted();
            while let Some(task) = service.claim() {
                tasks.push(task);
            }
            ready
        };
        tasks.into_iter().for_each(start);
        ready
    }

    pub fn take(&self) -> Vec<(String, FolderSize)> {
        service()
            .subscribers
            .get_mut(&self.id)
            .map(|subscriber| std::mem::take(&mut subscriber.inbox))
            .unwrap_or_default()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let mut service = service();
        service.subscribers.remove(&self.id);
        service.cancel_unwanted();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("navi-folder-size-{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::write(root.join("one.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("a").join("two.bin"), vec![0u8; 2000]).unwrap();
        std::fs::write(root.join("a").join("b").join("three.bin"), vec![0u8; 3000]).unwrap();
        root
    }

    #[test]
    fn walk_sums_nested_files_and_reports_missing_roots() {
        let root = fixture("walk");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            walk(root.to_str().unwrap(), &cancel, |_| {}),
            Walk::Done(6000)
        );
        assert_eq!(
            walk(root.join("missing").to_str().unwrap(), &cancel, |_| {}),
            Walk::Failed
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            walk(root.to_str().unwrap(), &cancel, |_| {}),
            Walk::Cancelled
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn walk_skips_directory_junctions() {
        let root = fixture("junction");
        let link = root.join("loop");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&root)
            .output()
            .unwrap();
        assert!(status.status.success());
        let cancel = AtomicBool::new(false);
        assert_eq!(
            walk(root.to_str().unwrap(), &cancel, |_| {}),
            Walk::Done(6000)
        );
        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scannable_accepts_folders_but_not_drive_roots_or_servers() {
        assert!(scannable(r"C:\Windows"));
        assert!(scannable(r"\\server\share"));
        assert!(!scannable(r"C:\"));
        assert!(!scannable(r"\\server"));
        assert!(!scannable(r"\\?\C:\Windows"));
        assert!(!scannable("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
    }

    #[test]
    fn invalidation_marks_the_path_and_its_ancestors_only() {
        let mut service = Service::default();
        for path in [r"c:\a", r"c:\a\b", r"c:\ab", r"c:\a\b\c"] {
            service.store(path.into(), Some(1));
        }
        service.mark_stale(r"C:\A\B");
        let stale = |path: &str| service.cache[path].stale;
        assert!(stale(r"c:\a"));
        assert!(stale(r"c:\a\b"));
        assert!(!stale(r"c:\ab"));
        assert!(!stale(r"c:\a\b\c"));
    }

    #[test]
    fn client_scans_wanted_folder_and_serves_cache_afterwards() {
        let root = fixture("client");
        let path = root.join("a").to_string_lossy().into_owned();
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = Mutex::new(sender);
        let client = Client::new(move || {
            lock(&sender).send(()).unwrap();
        });
        let all = Drives {
            ssd: true,
            hdd: true,
            network: false,
        };
        assert!(client.want(vec![path.clone()], all).is_empty());
        let mut last = None;
        while last.is_none_or(|size: FolderSize| !size.complete) {
            receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            last = client
                .take()
                .into_iter()
                .map(|(_, size)| size)
                .last()
                .or(last);
        }
        assert_eq!(last.unwrap().bytes, 5000);
        assert_eq!(
            client.want(vec![path.clone()], all),
            vec![(
                path,
                FolderSize {
                    bytes: 5000,
                    complete: true
                }
            )]
        );
        drop(client);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
