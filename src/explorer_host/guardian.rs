use super::{install, security, transport::Owned};
use notify::Watcher;
use std::{
    fs, io,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::Command,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::*,
        System::{
            Registry::{
                RegNotifyChangeKeyValue, HKEY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME,
                REG_NOTIFY_CHANGE_SECURITY,
            },
            Threading::*,
        },
    },
};
use winreg::{enums::*, RegKey};

const TASK: &str = "NaviExplorerHostRecovery";
const STOP: &str = r"Global\NaviExplorerHost.Guardian.Stop.v1";

enum Wake {
    Changed,
    Rearm,
    Stop,
}

fn scheduler(arguments: &[&str]) -> io::Result<std::process::Output> {
    let system = security::known_folder(&windows::Win32::UI::Shell::FOLDERID_System)?;
    Command::new(system.join("schtasks.exe"))
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
}

fn checked(arguments: &[&str]) -> io::Result<()> {
    let output = scheduler(arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Recovery task: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

pub(crate) fn require_absent() -> io::Result<()> {
    if scheduler(&["/Query", "/TN", TASK, "/XML"])?
        .status
        .success()
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Recovery task already exists; nothing was replaced",
        ));
    }
    Ok(())
}

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(crate) fn register() -> io::Result<()> {
    require_absent()?;
    let executable = install::directory()?.join("navi-explorer-guard.exe");
    let command = escaped(&executable.to_string_lossy());
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><RegistrationInfo><Description>Navi Explorer host native recovery</Description><SecurityDescriptor>D:P(A;;GA;;;SY)(A;;GA;;;BA)</SecurityDescriptor></RegistrationInfo><Triggers><BootTrigger><Enabled>true</Enabled></BootTrigger></Triggers><Principals><Principal id=\"Recovery\"><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure></Settings><Actions Context=\"Recovery\"><Exec><Command>{command}</Command><Arguments>--watch</Arguments></Exec></Actions></Task>"
    );
    let bytes = [0xff, 0xfe]
        .into_iter()
        .chain(xml.encode_utf16().flat_map(u16::to_le_bytes))
        .collect::<Vec<_>>();
    let path = install::data_directory()?.join("recovery-task.xml");
    if path.exists() {
        security::verify_directory(&path, false)?;
        if fs::read(&path)? != bytes {
            return Err(io::Error::other("Recovery task definition changed"));
        }
    } else {
        let mut file = security::new_file(&path, false)?;
        use io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    checked(&["/Create", "/TN", TASK, "/XML", &path.to_string_lossy()])?;
    start_task()
}

pub(crate) fn ensure_running() -> io::Result<()> {
    let task = scheduler(&["/Query", "/TN", TASK, "/XML"])?;
    if !task.status.success() {
        return register();
    }
    start_task()
}

fn start_task() -> io::Result<()> {
    let task = scheduler(&["/Query", "/TN", TASK, "/XML"])?;
    if !task.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Recovery task missing",
        ));
    }
    let executable = install::directory()?.join("navi-explorer-guard.exe");
    if !owns_task(&task.stdout, &executable) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Recovery task changed; nothing was started",
        ));
    }
    let ready = install::data_directory()?.join("guardian-ready.json");
    if ready_guardian(&ready, &executable, 0) {
        return Ok(());
    }
    let started = super::clock();
    checked(&["/Run", "/TN", TASK])?;
    while super::clock() < started + 3000 {
        if ready_guardian(&ready, &executable, started) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Recovery task did not start",
    ))
}

fn ready_guardian(path: &std::path::Path, executable: &std::path::Path, since: u64) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| {
            let Some(pid) = value["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
            else {
                return false;
            };
            let Some(created) = value["created"].as_u64() else {
                return false;
            };
            value["tick"].as_u64().is_some_and(|tick| tick >= since)
                && super::transport::alive(super::transport::ProcessStamp { pid, created })
                    == Some(true)
                && super::transport::image(pid).is_ok_and(|path| path == executable)
                && super::transport::identity(pid)
                    .is_ok_and(|identity| identity.sid == "S-1-5-18" && identity.session == 0)
        })
}

pub(crate) fn unregister() -> io::Result<()> {
    let task = scheduler(&["/Query", "/TN", TASK, "/XML"])?;
    if task.status.success() {
        let expected = install::directory()?.join("navi-explorer-guard.exe");
        if !owns_task(&task.stdout, &expected) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Recovery task changed; nothing was removed",
            ));
        }
        unsafe {
            if let Ok(event) = OpenEventW(
                EVENT_MODIFY_STATE,
                false,
                PCWSTR(security::wide(STOP).as_ptr()),
            ) {
                let event = Owned(event);
                SetEvent(event.0).map_err(security::error)?;
            }
        }
        checked(&["/Delete", "/TN", TASK, "/F"])?;
    }
    Ok(())
}

fn owns_task(bytes: &[u8], executable: &std::path::Path) -> bool {
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.get(1) == Some(&0) {
        let start = usize::from(bytes.starts_with(&[0xff, 0xfe])) * 2;
        String::from_utf16_lossy(
            &bytes[start..]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let expected = escaped(&executable.to_string_lossy());
    text.contains(&format!("<Command>{expected}</Command>"))
        && text.contains("<Arguments>--watch</Arguments>")
        && text.contains("<UserId>S-1-5-18</UserId>")
        && text.contains("<RunLevel>HighestAvailable</RunLevel>")
        && text.matches("<Command>").count() == 1
        && text.matches("<Arguments>").count() == 1
        && text.matches("<UserId>").count() == 1
        && text.matches("<Exec>").count() + text.matches("<Exec ").count() == 1
        && !text.contains("<ComHandler")
}

fn watch_package(
    path: &std::path::Path,
    sender: mpsc::Sender<Wake>,
) -> io::Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = sender.send(Wake::Changed);
        }
    })
    .map_err(io::Error::other)?;
    watcher
        .watch(path, notify::RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;
    Ok(watcher)
}

fn registry_watch(
    path: &str,
    view: u32,
    subtree: bool,
    sender: mpsc::Sender<Wake>,
) -> io::Result<Option<std::thread::JoinHandle<()>>> {
    let key = match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(path, KEY_NOTIFY | view)
    {
        Ok(key) => key,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(1018) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    Ok(Some(std::thread::spawn(move || loop {
        let status = unsafe {
            RegNotifyChangeKeyValue(
                HKEY(key.raw_handle() as _),
                subtree,
                REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_SECURITY,
                None,
                false,
            )
        };
        if sender.send(Wake::Changed).is_err() || status != ERROR_SUCCESS {
            break;
        }
    })))
}

pub fn run() -> io::Result<()> {
    let _privileges = security::Privileges::acquire()?;
    let descriptor = security::Descriptor::new("O:BAG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)")?;
    let event = unsafe {
        Owned(
            CreateEventW(
                Some(&descriptor.attributes()),
                true,
                false,
                PCWSTR(security::wide(STOP).as_ptr()),
            )
            .map_err(security::error)?,
        )
    };
    let (sender, receiver) = mpsc::channel();
    let stop_sender = sender.clone();
    let handle = event.0 .0 as usize;
    std::thread::spawn(move || {
        unsafe {
            WaitForSingleObject(HANDLE(handle as _), INFINITE);
        }
        let _ = stop_sender.send(Wake::Stop);
    });
    let _watcher = watch_package(&install::directory()?, sender.clone())?;
    let mut watches = Vec::new();
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        registry_watch(r"Software\Classes\CLSID", view, false, sender.clone())?
            .ok_or_else(|| io::Error::other("Class registry is unavailable"))?;
        watches.push((
            view,
            registry_watch(super::journal::HOST, view, true, sender.clone())?,
        ));
    }
    let ready = install::data_directory()?.join("guardian-ready.json");
    let stamp = super::transport::process_stamp(std::process::id()).map_err(security::error)?;
    let value = serde_json::json!({"pid":stamp.pid,"created":stamp.created,"tick":super::clock()});
    fs::write(ready, serde_json::to_vec(&value)?)?;
    let (health_sender, health_receiver) = mpsc::sync_channel(1);
    let health_retry = health_sender.clone();
    let health_signal = sender.clone();
    let failure = Arc::new(Mutex::new(None));
    let worker_failure = failure.clone();
    std::thread::spawn(move || {
        while health_receiver.recv().is_ok() {
            match install::recover_native_if_unhealthy() {
                Ok(()) => {
                    let _ = health_signal.send(Wake::Rearm);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let _ = health_retry.try_send(());
                }
                Err(error) => {
                    *worker_failure.lock().unwrap() = Some(error.to_string());
                    let _ = health_signal.send(Wake::Stop);
                    break;
                }
            }
        }
    });
    let _ = health_sender.try_send(());
    while let Ok(wake) = receiver.recv() {
        match wake {
            Wake::Changed => {
                let _ = health_sender.try_send(());
                continue;
            }
            Wake::Stop => break,
            Wake::Rearm => {}
        }
        for (view, watch) in &mut watches {
            if watch.as_ref().is_none_or(|watch| watch.is_finished()) {
                *watch = registry_watch(super::journal::HOST, *view, true, sender.clone())?;
            }
        }
    }
    let error = failure.lock().unwrap().take();
    error.map_or(Ok(()), |error| Err(io::Error::other(error)))
}

pub(crate) fn command_target(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    let path = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"')?.0
    } else {
        value.split_whitespace().next()?
    };
    let expanded_root = path.strip_prefix('%').is_some_and(|tail| {
        tail.split_once('%')
            .is_some_and(|(name, suffix)| !name.is_empty() && suffix.starts_with(['\\', '/']))
    });
    let path = PathBuf::from(path);
    (path.is_absolute() || expanded_root).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn broker_removal_notifies_the_guardian_without_a_client_or_polling_timer() {
        let root = std::env::temp_dir().join(format!(
            "NaviGuardianTests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let broker = root.join("broker.bin");
        fs::write(&broker, b"fixture").unwrap();
        let (sender, receiver) = mpsc::channel();
        let watcher = watch_package(&root, sender).unwrap();
        fs::remove_file(&broker).unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Ok(Wake::Changed)
        ));
        assert!(!broker.exists());
        drop(watcher);
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn repair_does_not_run_a_foreign_task_or_a_changed_principal() {
        let path =
            std::path::Path::new(r"C:\Program Files\NaviExplorerHost\navi-explorer-guard.exe");
        let xml = format!(
            "<Settings><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel><Exec><Command>{}</Command><Arguments>--watch</Arguments></Exec>",
            path.display()
        );
        assert!(owns_task(xml.as_bytes(), path));
        let utf16 = [0xff, 0xfe]
            .into_iter()
            .chain(xml.encode_utf16().flat_map(u16::to_le_bytes))
            .collect::<Vec<_>>();
        assert!(owns_task(&utf16, path));
        assert!(!owns_task(
            xml.replace("--watch", "--other").as_bytes(),
            path
        ));
        assert!(!owns_task(
            xml.replace("S-1-5-18", "S-1-5-32-545").as_bytes(),
            path
        ));
        assert!(!owns_task(
            xml.replace("navi-explorer-guard.exe", "foreign.exe")
                .as_bytes(),
            path
        ));
        assert!(!owns_task(
            format!("{xml}<Exec><Command>foreign.exe</Command></Exec>").as_bytes(),
            path
        ));
    }

    #[test]
    fn task_commands_escape_xml_and_malformed_targets_are_not_executable() {
        assert_eq!(
            escaped("C:\\a&b<\"'.exe"),
            "C:\\a&amp;b&lt;&quot;&apos;.exe"
        );
        for command in ["", "broken", "\"unterminated", "relative.exe --serve"] {
            assert!(command_target(command).is_none());
        }
        assert!(command_target(r#""%ProgramFiles%\OtherManager\manager.exe" --open"#).is_some());
        let current = std::env::current_exe().unwrap();
        assert_eq!(
            command_target(&format!("\"{}\" --serve", current.display())),
            Some(current)
        );
    }
}
