use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use windows::{
    core::{GUID, HSTRING},
    Foundation::TypedEventHandler,
    Storage::Provider::*,
    Win32::{
        System::{Com::*, WinRT::*},
        UI::WindowsAndMessaging::*,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Command {
    pub label: String,
    pub description: String,
    pub icon: String,
    pub state: i32,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub path: String,
    pub revision: u64,
    pub label: String,
    pub icon: String,
    pub account: String,
    pub quota: String,
    pub used: u64,
    pub total: u64,
    pub color: [u8; 4],
    pub more: String,
    pub commands: Vec<Command>,
}
pub enum Request {
    Path(String),
    Invoke(u64, usize),
    Stop,
}

struct Signal(windows::Win32::Foundation::HANDLE);

unsafe impl Send for Signal {}
unsafe impl Sync for Signal {}

impl Drop for Signal {
    fn drop(&mut self) {
        if let Err(error) = unsafe { windows::Win32::Foundation::CloseHandle(self.0) } {
            eprintln!("cloud wake release: {error}");
        }
    }
}

struct Channel {
    requests: mpsc::Sender<Request>,
    signal: Arc<Signal>,
}

impl Drop for Channel {
    fn drop(&mut self) {
        if self.requests.send(Request::Stop).is_ok() {
            if let Err(error) =
                unsafe { windows::Win32::System::Threading::SetEvent(self.signal.0) }
            {
                eprintln!("cloud stop wake: {error}");
            }
        }
    }
}

#[derive(Clone)]
pub struct Sender(Arc<Channel>);

impl Sender {
    pub fn send(&self, request: Request) -> Result<(), String> {
        self.0
            .requests
            .send(request)
            .map_err(|error| error.to_string())?;
        unsafe { windows::Win32::System::Threading::SetEvent(self.0.signal.0) }
            .map_err(|error| error.to_string())
    }
}

pub fn icon_path(uri: &str) -> Option<std::path::PathBuf> {
    let uri = windows::Foundation::Uri::CreateUri(&HSTRING::from(uri)).ok()?;
    if !uri.SchemeName().is_ok_and(|scheme| scheme == "file") {
        return None;
    }
    let path = uri.Path().ok()?;
    let path = windows::Foundation::Uri::UnescapeComponent(&path)
        .unwrap_or(path)
        .to_string();
    Some(std::path::PathBuf::from(path.trim_start_matches('/')))
}
pub fn contains(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches(['\\', '/']).to_lowercase();
    let path = path.replace('/', "\\").to_lowercase();
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('\\'))
}

fn registered_source(path: &str) -> Option<(String, GUID, String)> {
    use winreg::{enums::*, RegKey};
    let roots = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager")
        .ok()?;
    let mut matches = Vec::new();
    for id in roots.enum_keys().flatten() {
        let Ok(key) = roots.open_subkey(&id) else {
            continue;
        };
        let Ok(factory) = key.get_value::<String, _>("StorageProviderStatusUISourceFactory") else {
            continue;
        };
        let Ok(users) = key.open_subkey("UserSyncRoots") else {
            continue;
        };
        for (_, value) in users.enum_values().flatten() {
            let root: String = value.to_string();
            if contains(&root, path) {
                if let Ok(clsid) = GUID::try_from(factory.trim_matches(['{', '}'])) {
                    matches.push((
                        root.len(),
                        id.clone(),
                        clsid,
                        key.get_value("DisplayNameResource").unwrap_or_default(),
                    ));
                }
            }
        }
    }
    matches
        .into_iter()
        .max_by_key(|m| m.0)
        .map(|(_, id, clsid, label)| (id, clsid, label))
}

pub fn supports(path: &str) -> bool {
    registered_source(path).is_some()
}

fn registration(path: &str) -> Option<(String, GUID, String)> {
    let (id, clsid, label) = registered_source(path)?;
    let label = super::nav::children("")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| {
            let p = super::Pidl::parse(&n.id).ok()?;
            let root = p.name(windows::Win32::UI::Shell::SIGDN_FILESYSPATH);
            (!root.is_empty() && contains(&root, path)).then_some((root.len(), n.name))
        })
        .max_by_key(|n| n.0)
        .map(|n| n.1)
        .unwrap_or(label);
    Some((id, clsid, label))
}

fn command(c: Option<&IStorageProviderUICommand>) -> Command {
    c.map(|c| Command {
        label: c.Label().unwrap_or_default().to_string(),
        description: c.Description().unwrap_or_default().to_string(),
        icon: c
            .Icon()
            .and_then(|u| u.AbsoluteUri())
            .unwrap_or_default()
            .to_string(),
        state: c.State().map(|s| s.0).unwrap_or(2),
    })
    .unwrap_or(Command {
        state: 2,
        ..Default::default()
    })
}

pub fn start(publish: impl Fn(Snapshot) + Send + 'static) -> Result<Sender, String> {
    let (tx, rx) = mpsc::channel();
    let signal = Arc::new(Signal(
        unsafe { windows::Win32::System::Threading::CreateEventW(None, true, false, None) }
            .map_err(|error| error.to_string())?,
    ));
    let channel = Sender(Arc::new(Channel {
        requests: tx,
        signal: signal.clone(),
    }));
    std::thread::spawn(move || unsafe {
        if let Err(error) = RoInitialize(RO_INIT_SINGLETHREADED) {
            eprintln!("cloud apartment: {error}");
            return;
        }
        let dirty = Arc::new(AtomicBool::new(true));
        let mut current = String::new();
        let mut source: Option<(IStorageProviderStatusUISource, i64, String)> = None;
        let mut commands: Vec<Option<IStorageProviderUICommand>> = Vec::new();
        let mut last = Snapshot::default();
        let mut revision = 0;
        'events: loop {
            if MsgWaitForMultipleObjectsEx(
                Some(&[signal.0]),
                u32::MAX,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            ) == windows::Win32::Foundation::WAIT_FAILED
            {
                eprintln!("cloud wait: {}", windows::core::Error::from_win32());
                break;
            }
            if let Err(error) = windows::Win32::System::Threading::ResetEvent(signal.0) {
                eprintln!("cloud reset: {error}");
                break;
            }
            for request in rx.try_iter() {
                match request {
                    Request::Stop => break 'events,
                    Request::Path(path) => pending_path(
                        &mut current,
                        &mut source,
                        &mut commands,
                        &dirty,
                        &signal,
                        path,
                    ),
                    Request::Invoke(version, index) => {
                        if version == revision {
                            if let Some(Some(c)) = commands.get(index) {
                                if c.State()
                                    .is_ok_and(|s| s == StorageProviderUICommandState::Enabled)
                                {
                                    let _ = c.Invoke();
                                    dirty.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }
            }
            if dirty.swap(false, Ordering::Relaxed) {
                let mut next = Snapshot {
                    path: current.clone(),
                    ..Default::default()
                };
                let mut next_commands = Vec::new();
                if let Some((provider, _, account)) = &source {
                    if let Ok(ui) = provider.GetStatusUI() {
                        next.label = ui.ProviderStateLabel().unwrap_or_default().to_string();
                        next.icon = ui
                            .ProviderStateIcon()
                            .and_then(|u| u.AbsoluteUri())
                            .unwrap_or_default()
                            .to_string();
                        next.account = account.clone();
                        next_commands.push(ui.SyncStatusCommand().ok());
                        next_commands.push(ui.ProviderPrimaryCommand().ok());
                        let more = ui.MoreInfoUI().ok();
                        next.more = more
                            .as_ref()
                            .and_then(|m| m.Message().ok())
                            .unwrap_or_default()
                            .to_string();
                        next_commands.push(more.and_then(|m| m.Command().ok()));
                        if let Ok(v) = ui.ProviderSecondaryCommands() {
                            for i in 0..v.Size().unwrap_or(0) {
                                next_commands.push(v.GetAt(i).ok());
                            }
                        }
                        next.commands = next_commands.iter().map(|c| command(c.as_ref())).collect();
                        if let Ok(q) = ui.QuotaUI() {
                            next.quota = q.QuotaUsedLabel().unwrap_or_default().to_string();
                            next.used = q.QuotaUsedInBytes().unwrap_or(0);
                            next.total = q.QuotaTotalInBytes().unwrap_or(0);
                            next.color = q
                                .QuotaUsedColor()
                                .and_then(|c| c.Value())
                                .map(|c| [c.A, c.R, c.G, c.B])
                                .unwrap_or([255, 96, 205, 255]);
                        }
                    }
                }
                next.revision = last.revision;
                if next != last {
                    revision = revision.wrapping_add(1);
                    next.revision = revision;
                    last = next.clone();
                    commands = next_commands;
                    publish(next);
                }
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some((old, token, _)) = source.take() {
            let _ = old.RemoveStatusUIChanged(token);
        }
        commands.clear();
        RoUninitialize();
    });
    Ok(channel)
}

unsafe fn pending_path(
    current: &mut String,
    source: &mut Option<(IStorageProviderStatusUISource, i64, String)>,
    commands: &mut Vec<Option<IStorageProviderUICommand>>,
    dirty: &Arc<AtomicBool>,
    signal: &Arc<Signal>,
    path: String,
) {
    if *current == path {
        return;
    }
    *current = path;
    if let Some((old, token, _)) = source.take() {
        let _ = old.RemoveStatusUIChanged(token);
    }
    commands.clear();
    if let Some((id, clsid, label)) = registration(current) {
        if let Ok(factory) = CoCreateInstance::<_, IStorageProviderStatusUISourceFactory>(
            &clsid,
            None,
            CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER,
        ) {
            if let Ok(next) = factory.GetStatusUISource(&HSTRING::from(id)) {
                let changed = dirty.clone();
                let signal = signal.clone();
                let handler = TypedEventHandler::new(move |_, _| {
                    changed.store(true, Ordering::Relaxed);
                    windows::Win32::System::Threading::SetEvent(signal.0)
                });
                let token = next.StatusUIChanged(&handler).unwrap_or(0);
                *source = Some((next, token, label));
            }
        }
    }
    dirty.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn last_sender_drop_wakes_and_releases_an_idle_provider_apartment() {
        let (published, received) = mpsc::channel();
        let sender = start(move |snapshot| {
            let _ = published.send(snapshot.path);
        })
        .unwrap();
        let signal = Arc::downgrade(&sender.0.signal);
        sender
            .send(Request::Path(r"C:\sw-native-cloud-idle-test".into()))
            .unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            r"C:\sw-native-cloud-idle-test"
        );
        let clone = sender.clone();
        drop(sender);
        assert!(signal.upgrade().is_some());
        drop(clone);
        let deadline = Instant::now() + Duration::from_secs(5);
        while signal.upgrade().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(signal.upgrade().is_none());
    }
    #[test]
    fn root_boundary_does_not_expose_status_in_sibling_or_virtual_folder() {
        assert!(contains(
            r"C:\Users\A\OneDrive",
            r"c:\users\a\onedrive\Docs"
        ));
        assert!(contains(r"C:\Users\A\OneDrive\", r"C:\Users\A\OneDrive"));
        assert!(!contains(
            r"C:\Users\A\OneDrive",
            r"C:\Users\A\OneDrive-old"
        ));
        assert!(!contains(r"C:\Users\A\OneDrive", "shell:Home"));
    }
}
