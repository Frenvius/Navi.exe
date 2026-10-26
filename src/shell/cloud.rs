//! Registered cloud-provider status. COM objects and command execution stay on one STA.
//! Merely reading status never invokes a command or hydrates a file.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
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
}
pub fn contains(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches(['\\', '/']).to_lowercase();
    let path = path.replace('/', "\\").to_lowercase();
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('\\'))
}

fn registration(path: &str) -> Option<(String, GUID, String)> {
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
        .map(|(_, id, clsid, label)| {
            // The navigation namespace supplies the localized account caption (not
            // the registration's fallback "OneDrive - Personal"). Never parse account files.
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
            (id, clsid, label)
        })
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

pub fn start(publish: impl Fn(Snapshot) + Send + 'static) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || unsafe {
        if RoInitialize(RO_INIT_SINGLETHREADED).is_err() {
            return;
        }
        let dirty = Arc::new(AtomicBool::new(true));
        let mut current = String::new();
        let mut source: Option<(IStorageProviderStatusUISource, i64, String)> = None;
        let mut commands: Vec<Option<IStorageProviderUICommand>> = Vec::new();
        let mut last = Snapshot::default();
        let mut revision = 0;
        let mut refreshed = Instant::now() - Duration::from_secs(10);
        loop {
            let first = match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(r) => Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            for request in first.into_iter().chain(rx.try_iter()) {
                match request {
                    Request::Path(path) => {
                        pending_path(&mut current, &mut source, &mut commands, &dirty, path)
                    }
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
            if dirty.swap(false, Ordering::Relaxed) || refreshed.elapsed() >= Duration::from_secs(2)
            {
                refreshed = Instant::now();
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
        RoUninitialize();
    });
    tx
}

// Path changes use the same STA for factory creation, subscriptions and release.
unsafe fn pending_path(
    current: &mut String,
    source: &mut Option<(IStorageProviderStatusUISource, i64, String)>,
    commands: &mut Vec<Option<IStorageProviderUICommand>>,
    dirty: &Arc<AtomicBool>,
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
                let handler = TypedEventHandler::new(move |_, _| {
                    changed.store(true, Ordering::Relaxed);
                    Ok(())
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
