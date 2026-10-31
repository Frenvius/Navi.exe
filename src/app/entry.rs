use super::ids;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use windows::{
    core::{w, BOOL, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM},
        Globalization::{CompareStringOrdinal, CSTR_EQUAL},
        Storage::FileSystem::{GetFullPathNameW, GetLongPathNameW},
        System::{
            DataExchange::COPYDATASTRUCT, Environment::ExpandEnvironmentStringsW, Threading::*,
        },
        UI::{
            Input::KeyboardAndMouse::IsWindowEnabled,
            Shell::{SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH},
            WindowsAndMessaging::*,
        },
    },
};
const DATA: usize = 0x5357454c;
const LAUNCH_MARK: usize = 0x53574c;
pub const USAGE:&str="navi [--new-window | --reuse-window | --new-tab] [--] <folder>\nnavi [/select,<item> | --select <item>]\nnavi --restore-session <id>\nnavi --unregister";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Default,
    NewWindow,
    Reuse,
    NewTab,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub mode: Mode,
    pub shell_open: bool,
    pub path: Option<String>,
    pub select: Option<String>,
    pub restore: Option<String>,
    pub startup: bool,
}
impl Launch {
    fn delivery_mode(&self, preferences: super::launch_preferences::Preferences) -> Mode {
        use super::launch_preferences::{Destination, Source};
        if self.mode != Mode::Default && !(self.shell_open && self.mode == Mode::Reuse) {
            return self.mode;
        }
        let source = if self.shell_open && self.path.is_none() {
            Source::WinE
        } else {
            Source::External
        };
        match preferences.destination(source) {
            Destination::NewTab => Mode::NewTab,
            Destination::NewWindow => Mode::NewWindow,
        }
    }
}
thread_local! {static OPTIONS:RefCell<Launch>=RefCell::new(Launch::default());static STARTUP:RefCell<Option<Lock>>=const{RefCell::new(None)};}
pub fn options() -> Launch {
    OPTIONS.with(|o| o.borrow().clone())
}
fn full_path(path: &str) -> Result<String, String> {
    unsafe {
        let w = crate::shell::wide(path);
        let mut out = vec![0u16; 32768];
        let n = GetFullPathNameW(PCWSTR(w.as_ptr()), Some(&mut out), None) as usize;
        if n == 0 || n >= out.len() {
            return Err(format!("Invalid folder path: {path}"));
        }
        Ok(String::from_utf16_lossy(&out[..n]))
    }
}
pub fn normalize(path: &str) -> Result<String, String> {
    if ids::is_settings(path) {
        return Ok(ids::SETTINGS.into());
    }
    if path.is_empty() || path.contains('\0') || path.encode_utf16().count() > 32767 {
        return Err("Empty, NUL-containing or overlong path".into());
    }
    let mut repaired = path.to_string();
    if repaired.ends_with('"') && !repaired[..repaired.len() - 1].contains('"') {
        repaired.pop();
        if repaired.len() == 2 && repaired.as_bytes()[1] == b':' {
            repaired.push('\\');
        }
    }
    let expanded = unsafe {
        let input = crate::shell::wide(&repaired);
        let mut out = vec![0u16; 32768];
        let n = ExpandEnvironmentStringsW(PCWSTR(input.as_ptr()), Some(&mut out)) as usize;
        if n == 0 || n > out.len() {
            return Err("Environment-expanded path is too long".into());
        }
        String::from_utf16_lossy(&out[..n - 1])
    };
    let lower = expanded.to_ascii_lowercase();
    if expanded.starts_with("::") || lower.starts_with("shell:") || lower.starts_with("file:") {
        let id = ids::canonical(&expanded);
        crate::shell::com_init();
        if let Ok(p) = crate::shell::Pidl::parse(&id) {
            if p.is_desktop() {
                return Ok(String::new());
            }
            let fs = p.name(SIGDN_FILESYSPATH);
            if !fs.is_empty() {
                return full_path(&fs);
            }
            let parsed = p.name(SIGDN_DESKTOPABSOLUTEPARSING);
            return Ok(ids::canonical(if parsed.is_empty() {
                &id
            } else {
                &parsed
            }));
        }
        return Ok(id);
    }
    let path = full_path(&expanded)?;
    if path.contains('~') {
        unsafe {
            let input = crate::shell::wide(&path);
            let mut out = vec![0u16; 32768];
            let n = GetLongPathNameW(PCWSTR(input.as_ptr()), Some(&mut out)) as usize;
            if n > 0 && n < out.len() {
                return Ok(String::from_utf16_lossy(&out[..n]));
            }
        }
    }
    Ok(path)
}
fn key(path: &str) -> String {
    if path.is_empty() {
        return "shell:DesktopRoot".into();
    }
    let path = if path.starts_with("::")
        || path.to_ascii_lowercase().starts_with("shell:")
        || ids::is_search(path)
    {
        ids::canonical(path)
    } else {
        full_path(path).unwrap_or_else(|_| path.into())
    };
    path.trim_end_matches(['\\', '/']).to_string()
}
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (key(a), key(b));
    a == b
        || unsafe {
            CompareStringOrdinal(
                &a.encode_utf16().collect::<Vec<_>>(),
                &b.encode_utf16().collect::<Vec<_>>(),
                true,
            ) == CSTR_EQUAL
        }
}

pub fn parse(args: &[String]) -> Result<Launch, String> {
    let mut result = Launch::default();
    let mut target = None;
    let mut select = false;
    let mut literal = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let lower = arg.to_ascii_lowercase();
        if !literal && arg == "--" {
            literal = true;
            i += 1;
            continue;
        }
        if !literal && lower == "--shell-open" {
            result.shell_open = true;
            i += 1;
            continue;
        }
        let mode = if !literal {
            match lower.as_str() {
                "--new-window" | "/n" => Some(Mode::NewWindow),
                "--reuse-window" => Some(Mode::Reuse),
                "--new-tab" => Some(Mode::NewTab),
                _ => None,
            }
        } else {
            None
        };
        if let Some(mode) = mode {
            if result.mode != Mode::Default && result.mode != mode {
                return Err("Conflicting launch modes".into());
            }
            result.mode = mode;
            i += 1;
            continue;
        }
        if !literal && lower == "--restore-session" {
            i += 1;
            let id = args.get(i).ok_or("Missing session id")?;
            if id.is_empty() || id.len() > 256 || id.contains('\0') {
                return Err("Invalid session id".into());
            }
            result.restore = Some(id.clone());
            i += 1;
            continue;
        }
        let mut value = arg.clone();
        if !literal && (lower == "--select" || lower == "/select" || lower.starts_with("/select,"))
        {
            select = true;
            value = if let Some((_, tail)) = arg.split_once(',') {
                tail.to_string()
            } else {
                String::new()
            };
            if value.is_empty() {
                i += 1;
                value = args.get(i).ok_or("Missing selected item")?.clone();
            }
        } else if !literal
            && (lower == "/e"
                || lower.starts_with("/e,")
                || lower.starts_with("/n,")
                || lower == "/newtab"
                || lower.starts_with("/newtab,"))
        {
            let mode = if lower.starts_with("/n,") {
                Some(Mode::NewWindow)
            } else if lower == "/newtab" || lower.starts_with("/newtab,") {
                Some(Mode::NewTab)
            } else {
                None
            };
            if let Some(mode) = mode {
                if result.mode != Mode::Default && result.mode != mode {
                    return Err("Conflicting launch modes".into());
                }
                result.mode = mode;
            }
            value = arg
                .split_once(',')
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            if value.is_empty() {
                i += 1;
                if i >= args.len() {
                    break;
                }
                value = args[i].clone();
            }
        } else if !literal && arg.starts_with('-') {
            return Err(format!("Unknown option: {arg}"));
        }
        if target.replace(value).is_some() {
            return Err("Only one folder or selected item is accepted".into());
        }
        i += 1;
    }
    if result.restore.is_some() {
        if target.is_some() || result.mode != Mode::Default || select {
            return Err("Session restore cannot be combined with folder arguments".into());
        }
        return Ok(result);
    }
    if let Some(target) = target {
        let normalized = normalize(&target)?;
        if select {
            let parent = if normalized.starts_with("::")
                || normalized.to_ascii_lowercase().starts_with("shell:")
            {
                crate::shell::Pidl::parse(&normalized)
                    .ok()
                    .and_then(|p| p.parent())
                    .map(|p| p.name(SIGDN_DESKTOPABSOLUTEPARSING))
                    .unwrap_or_default()
            } else {
                std::path::Path::new(&normalized)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ids::THIS_PC.into())
            };
            result.path = Some(parent);
            result.select = Some(normalized);
        } else {
            result.path = Some(normalized);
        }
    }
    Ok(result)
}

struct Lock(HANDLE);
impl Lock {
    fn get() -> Option<Self> {
        unsafe {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::env::var_os("USERPROFILE").hash(&mut h);
            std::env::current_exe().ok().hash(&mut h);
            let name = crate::shell::wide(&format!(r"Local\NaviLaunch-{:x}", h.finish()));
            let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr())).ok()?;
            let wait = WaitForSingleObject(handle, 5000);
            if wait.0 != 0 && wait.0 != 0x80 {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Self(handle))
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Action {
    Probe,
    Match,
    Navigate,
    NewTab,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub version: u32,
    pub action: Action,
    pub path: String,
    pub select: Option<String>,
}
fn decode(bytes: &[u8]) -> Option<Request> {
    if bytes.is_empty() || bytes.len() > 131072 {
        return None;
    }
    let r: Request = serde_json::from_slice(bytes).ok()?;
    let valid = |s: &str| !s.contains('\0') && s.encode_utf16().count() <= 32767;
    (r.version == 1 && valid(&r.path) && r.select.as_deref().is_none_or(valid)).then_some(r)
}
unsafe extern "system" fn collect(hwnd: HWND, param: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd).as_bool()
        && IsWindowEnabled(hwnd).as_bool()
        && GetPropW(hwnd, w!("NaviMain")).0 as usize == 0x535745
        && GetPropW(hwnd, w!("NaviLaunchReady")).0 as usize == 1
        && super::tab_transfer::same_executable(hwnd)
    {
        (&mut *(param.0 as *mut Vec<HWND>)).push(hwnd);
    }
    true.into()
}
struct Requester(HWND);
impl Drop for Requester {
    fn drop(&mut self) {
        unsafe {
            let _ = RemovePropW(self.0, w!("NaviLaunch"));
            let _ = DestroyWindow(self.0);
        }
    }
}
fn dispatch(launch: &Launch) -> bool {
    unsafe {
        if !matches!(launch.mode, Mode::Reuse | Mode::NewTab) {
            return false;
        }
        let mut peers = Vec::<HWND>::new();
        let _ = EnumWindows(Some(collect), LPARAM(&mut peers as *mut _ as _));
        if peers.is_empty() {
            return false;
        }
        peers.sort_by_cached_key(|hwnd| {
            std::cmp::Reverse(GetPropW(*hwnd, w!("NaviLastActivated")).0 as usize)
        });
        let Ok(source) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Navi launcher"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        ) else {
            return false;
        };
        let _owned = Requester(source);
        let _ = SetPropW(source, w!("NaviLaunch"), Some(HANDLE(LAUNCH_MARK as _)));
        let path = launch.path.clone().unwrap_or_else(ids::start_location);
        let send = |target: HWND, action: Action| {
            let probe = matches!(action, Action::Probe);
            let request = Request {
                version: 1,
                action,
                path: path.clone(),
                select: launch.select.clone(),
            };
            let Ok(bytes) = serde_json::to_vec(&request) else {
                return false;
            };
            let packet = COPYDATASTRUCT {
                dwData: DATA,
                cbData: bytes.len() as u32,
                lpData: bytes.as_ptr() as _,
            };
            let mut pid = 0;
            GetWindowThreadProcessId(target, Some(&mut pid));
            if !probe {
                let permitted = AllowSetForegroundWindow(pid).is_ok();
                #[cfg(debug_assertions)]
                let _ = SetPropW(
                    target,
                    w!("NaviLaunchPermission"),
                    Some(HANDLE((permitted as usize + 1) as _)),
                );
            }
            let mut result = 0usize;
            let accepted = SendMessageTimeoutW(
                target,
                WM_COPYDATA,
                WPARAM(source.0 as _),
                LPARAM(&packet as *const _ as _),
                SMTO_ABORTIFHUNG,
                1500,
                Some(&mut result),
            )
            .0 != 0
                && result == 1;
            accepted
        };
        if launch.mode == Mode::Reuse {
            for &target in &peers {
                if send(target, Action::Probe) {
                    return send(target, Action::Match);
                }
            }
        }
        send(
            peers[0],
            if launch.mode == Mode::NewTab {
                Action::NewTab
            } else {
                Action::Navigate
            },
        )
    }
}
fn shell_supported(path: &str) -> bool {
    if crate::shell::browser::handles(path) {
        return false;
    }
    if !ids::is_virtual(path) {
        return true;
    }
    [
        ids::THIS_PC,
        ids::HOME,
        ids::QUICK_ACCESS,
        ids::GALLERY,
        ids::RECYCLE_BIN,
        ids::NETWORK,
        ids::LIBRARIES,
    ]
    .iter()
    .any(|id| ids::same(path, id))
        || crate::shell::library::is_library(path)
}
fn open_original(path: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_CLASSNAME, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    };
    use winreg::{enums::HKEY_CLASSES_ROOT, RegKey};
    let native: String = RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(r"Folder\shell\opennewwindow\command")
        .and_then(|k| k.get_value("DelegateExecute"))
        .map_err(|e| e.to_string())?;
    if !native.eq_ignore_ascii_case("{11dbb47c-a525-400b-9e80-a54615a090c0}") {
        return Err("Native Explorer fallback association changed".into());
    }
    let target = crate::shell::wide(path);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_CLASSNAME | SEE_MASK_NOASYNC,
        lpClass: w!("Folder"),
        lpVerb: w!("opennewwindow"),
        lpFile: PCWSTR(target.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info).map_err(|e| e.message()) }
}
pub fn start() -> Result<bool, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{USAGE}");
        return Ok(true);
    }
    if args == ["--unregister"] {
        let status = super::default_manager::status().map_err(|error| error.to_string())?;
        if status.enabled || status.recovery {
            super::default_manager::set_enabled(false).map_err(|error| error.to_string())?;
        }
        return Ok(true);
    }
    let mut launch = parse(&args)?;
    launch.startup = launch.mode == Mode::Default
        && launch.path.is_none()
        && launch.select.is_none()
        && launch.restore.is_none();
    if launch.shell_open {
        if let Some(path) = launch.path.as_deref() {
            if !shell_supported(path) {
                open_original(path)?;
                return Ok(true);
            }
        }
    }
    if launch.restore.is_none() {
        let preferences = super::launch_preferences::Preferences::read().unwrap_or_else(|error| {
            eprintln!("launch preferences: {error}");
            Default::default()
        });
        launch.mode = launch.delivery_mode(preferences);
    }
    OPTIONS.with(|o| *o.borrow_mut() = launch.clone());
    STARTUP.with(|g| *g.borrow_mut() = Lock::get());
    if dispatch(&launch) {
        STARTUP.with(|g| g.borrow_mut().take());
        return Ok(true);
    }
    Ok(false)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn external_and_win_e_destinations_are_independent_and_explicit_modes_win() {
        use super::super::launch_preferences::{Destination, Preferences};
        for external in [Destination::NewTab, Destination::NewWindow] {
            for win_e in [Destination::NewTab, Destination::NewWindow] {
                let preferences = Preferences { external, win_e };
                let mode = |destination| match destination {
                    Destination::NewTab => Mode::NewTab,
                    Destination::NewWindow => Mode::NewWindow,
                };
                for arguments in [
                    vec!["--shell-open", "--reuse-window", r"C:\Windows"],
                    vec!["--reuse-window", "--shell-open", r"C:\Windows"],
                    vec!["--shell-open", r"C:\Windows"],
                    vec![r"C:\Windows"],
                    vec![r"/e,C:\Windows"],
                    vec!["/e", r"C:\Windows"],
                ] {
                    assert_eq!(
                        parse(&args(&arguments)).unwrap().delivery_mode(preferences),
                        mode(external)
                    );
                }
                for arguments in [vec!["--shell-open", "--reuse-window"], vec!["--shell-open"]] {
                    assert_eq!(
                        parse(&args(&arguments)).unwrap().delivery_mode(preferences),
                        mode(win_e)
                    );
                }
                for (arguments, expected) in [
                    (vec!["--reuse-window", r"C:\Windows"], Mode::Reuse),
                    (vec!["--new-tab", r"C:\Windows"], Mode::NewTab),
                    (vec![r"/newtab,C:\Windows"], Mode::NewTab),
                    (vec![r"/n,C:\Windows"], Mode::NewWindow),
                    (vec!["--shell-open", "--new-tab"], Mode::NewTab),
                    (
                        vec!["--shell-open", "--new-window", r"C:\Windows"],
                        Mode::NewWindow,
                    ),
                ] {
                    assert_eq!(
                        parse(&args(&arguments)).unwrap().delivery_mode(preferences),
                        expected
                    );
                }
            }
        }
        let selected = parse(&args(&[
            "--shell-open",
            "--reuse-window",
            "--select",
            r"C:\Users\José\selected.txt",
        ]))
        .unwrap();
        assert_eq!(selected.delivery_mode(Preferences::default()), Mode::NewTab);
        assert_eq!(selected.path.as_deref(), Some(r"C:\Users\José"));
        assert_eq!(
            selected.select.as_deref(),
            Some(r"C:\Users\José\selected.txt")
        );
    }
    #[test]
    fn folder_launch_modes_root_selection_and_native_legacy_forms() {
        let p = parse(&args(&["--new-tab", r"C:\two words\José\"])).unwrap();
        assert_eq!(p.mode, Mode::NewTab);
        assert!(same(p.path.as_deref().unwrap(), r"C:\two words\José"));
        let s = parse(&args(&[r"/select,C:\two words\Alvo.txt"])).unwrap();
        assert_eq!(s.select.as_deref(), Some(r"C:\two words\Alvo.txt"));
        assert!(same(s.path.as_deref().unwrap(), r"C:\two words"));
        assert!(same(&normalize("C:\"").unwrap(), r"C:\"));
        assert_eq!(
            parse(&args(&["--new-window"])).unwrap().mode,
            Mode::NewWindow
        );
        assert!(parse(&args(&["--new-tab", "--reuse-window", r"C:\"])).is_err());
        assert!(parse(&args(&["--restore-session", "one", r"C:\"])).is_err());
    }
    #[test]
    fn windows_folder_identity_uses_unicode_case_and_separator_boundaries() {
        assert!(same(r"C:\Usuários\JOSÉ\", r"c:\usuários\josé"));
        assert!(!same(r"C:\A", r"C:\A-old"));
        assert!(same(r"C:\A\..\B", r"C:\B"));
        assert!(!same(
            "",
            &std::env::current_dir().unwrap().to_string_lossy()
        ));
    }
    #[test]
    fn broker_rejects_future_nul_truncated_and_unbounded_packets() {
        let r = Request {
            version: 1,
            action: Action::NewTab,
            path: r"C:\A".into(),
            select: None,
        };
        let data = serde_json::to_vec(&r).unwrap();
        assert!(decode(&data).is_some());
        assert!(decode(&data[..data.len() - 1]).is_none());
        assert!(decode(&vec![b'x'; 131073]).is_none());
        let mut future = r.clone();
        future.version = 2;
        assert!(decode(&serde_json::to_vec(&future).unwrap()).is_none());
        future.version = 1;
        future.path.push('\0');
        assert!(decode(&serde_json::to_vec(&future).unwrap()).is_none());
    }
}

pub(crate) fn incoming(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Result<Request, ()>> {
    if msg != WM_COPYDATA || lparam.0 == 0 {
        return None;
    }
    let source = HWND(wparam.0 as _);
    unsafe {
        if GetPropW(source, w!("NaviLaunch")).0 as usize != LAUNCH_MARK
            || !super::tab_transfer::same_executable(source)
        {
            return None;
        }
        let packet = &*(lparam.0 as *const COPYDATASTRUCT);
        if packet.dwData != DATA {
            return None;
        }
        if packet.lpData.is_null() || packet.cbData == 0 || packet.cbData > 131072 {
            return Some(Err(()));
        }
        let Some(request) = decode(std::slice::from_raw_parts(
            packet.lpData as *const u8,
            packet.cbData as usize,
        )) else {
            return Some(Err(()));
        };
        Some(Ok(request))
    }
}

pub fn ready_window(hwnd: HWND) {
    if let Err(error) = unsafe { SetPropW(hwnd, w!("NaviLaunchReady"), Some(HANDLE(1 as _))) } {
        eprintln!("launch readiness: {error}");
    }
    STARTUP.with(|guard| guard.borrow_mut().take());
}
