//! External folder launches. Plain/native CLI forms create windows on this reference;
//! explicit --reuse-window/--new-tab requests use a bounded same-executable broker.
use super::{ids, with_app, AfterLoad, App};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::HashSet, rc::Rc};
use windows::{
    core::{w, BOOL, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM},
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
pub const USAGE:&str="slint-win-explorer [--new-window | --reuse-window | --new-tab] [--] <folder>\nslint-win-explorer [/select,<item> | --select <item>]\nslint-win-explorer --restore-session <id>";
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
    if ids::is_settings(path) { return Ok(ids::SETTINGS.into()); }
    if path.is_empty() || path.contains('\0') || path.encode_utf16().count() > 32767 {
        return Err("Empty, NUL-containing or overlong path".into());
    }
    // Some legacy shell templates quote C:\ as "C:\", leaving C:" in argv.
    // A quote cannot occur in an ordinary Windows filename. Repair that root only.
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
        if !literal && lower == "--shell-open" {result.shell_open=true;i+=1;continue;}
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
            result.mode = Mode::NewWindow;
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
            let name = crate::shell::wide(&format!(r"Local\SlintExplorerLaunch-{:x}", h.finish()));
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
enum Action {
    Probe,
    Match,
    Navigate,
    NewTab,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    action: Action,
    path: String,
    select: Option<String>,
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
        && GetPropW(hwnd, w!("SlintExplorerMain")).0 as usize == 0x535745
        && GetPropW(hwnd, w!("SlintExplorerLaunchReady")).0 as usize == 1
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
            let _ = RemovePropW(self.0, w!("SlintExplorerLaunch"));
            let _ = DestroyWindow(self.0);
        }
    }
}
// Integration point: a future resident/pipe transport can replace this bounded CLI handoff.
// Shell window publication/selection (IShellWindows) is deliberately separate and pending.
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
        let Ok(source) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Slint Explorer launcher"),
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
        let _ = SetPropW(
            source,
            w!("SlintExplorerLaunch"),
            Some(HANDLE(LAUNCH_MARK as _)),
        );
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
            // ASFW replaces the one permitted process. Do not give away the
            // launcher's permission while merely probing a nonmatching window.
            if !probe {
                let permitted = AllowSetForegroundWindow(pid).is_ok();
                #[cfg(debug_assertions)]
                let _ = SetPropW(
                    target,
                    w!("SlintLaunchPermission"),
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
    if crate::shell::browser::handles(path) { return false; }
    if !ids::is_virtual(path) { return true; }
    [ids::THIS_PC, ids::HOME, ids::QUICK_ACCESS, ids::GALLERY, ids::RECYCLE_BIN,
     ids::NETWORK, ids::LIBRARIES].iter().any(|id| ids::same(path, id))
        || crate::shell::library::is_library(path)
}
/// Invoke the untouched native Folder/opennewwindow verb explicitly. Neither that
/// verb nor its ExecuteFolder CLSID is registered by this feature, so this cannot
/// route through our open/explore or the separate Win+E association.
fn open_original(path: &str) -> Result<(), String> {
    use winreg::{enums::HKEY_CLASSES_ROOT, RegKey};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_CLASSNAME, SEE_MASK_NOASYNC};
    let native: String = RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(r"Folder\shell\opennewwindow\command")
        .and_then(|k| k.get_value("DelegateExecute")).map_err(|e|e.to_string())?;
    if !native.eq_ignore_ascii_case("{11dbb47c-a525-400b-9e80-a54615a090c0}") {
        return Err("Native Explorer fallback association changed".into());
    }
    let target=crate::shell::wide(path);
    let mut info=SHELLEXECUTEINFOW {
        cbSize:std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask:SEE_MASK_CLASSNAME|SEE_MASK_NOASYNC,
        lpClass:w!("Folder"),lpVerb:w!("opennewwindow"),lpFile:PCWSTR(target.as_ptr()),
        nShow:SW_SHOWNORMAL.0,..Default::default()
    };
    unsafe {ShellExecuteExW(&mut info).map_err(|e|e.message())}
}
pub fn start() -> Result<bool, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{USAGE}");
        return Ok(true);
    }
    let launch = parse(&args)?;
    if launch.shell_open {
        if let Some(path)=launch.path.as_deref() {
            if !shell_supported(path) { open_original(path)?; return Ok(true); }
        }
    }
    OPTIONS.with(|o| *o.borrow_mut() = launch.clone());
    STARTUP.with(|g| *g.borrow_mut() = Lock::get());
    if dispatch(&launch) {
        STARTUP.with(|g| g.borrow_mut().take());
        return Ok(true);
    }
    Ok(false)
}
pub fn ready(app: &Rc<App>) {
    if let Some(select) = options().select {
        app.state.borrow_mut().after_load = Some(AfterLoad {
            select,
            rename: false,
        });
    }
    unsafe {
        let _ = SetPropW(
            app.hwnd.get(),
            w!("SlintExplorerLaunchReady"),
            Some(HANDLE(1 as _)),
        );
    }
    app.sync_tabs();
    if options().path.is_some() || options().mode == Mode::NewWindow {
        activate(app.hwnd.get(), 2);
    }
    STARTUP.with(|g| g.borrow_mut().take());
}
fn activate(hwnd: HWND, retries: u8) {
    unsafe {
        if GetPropW(hwnd, w!("SlintExplorerMain")).0 as usize != 0x535745
            || !super::tab_transfer::same_executable(hwnd)
        {
            return;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            // The alpha software surface may have been discarded while minimized.
            // A normal request_redraw only repaints dirty items, leaving holes.
            let handle = hwnd.0 as usize;
            slint::Timer::single_shot(std::time::Duration::from_millis(20), move || {
                with_app(|app| {
                    if app.hwnd.get().0 as usize != handle {
                        return;
                    }
                    use slint::private_unstable_api::re_exports::{LogicalRect, WindowInner};
                    use slint::ComponentHandle;
                    let ui = app.ui();
                    let window = ui.window();
                    let size = window.size().to_logical(window.scale_factor());
                    WindowInner::from_pub(window)
                        .window_adapter()
                        .renderer()
                        .mark_dirty_region(
                            LogicalRect::from_size((size.width, size.height).into()).into(),
                        );
                    window.request_redraw();
                })
            });
        }
        let activated = SetForegroundWindow(hwnd).as_bool();
        #[cfg(debug_assertions)]
        let _ = SetPropW(
            hwnd,
            w!("SlintLaunchActivated"),
            Some(HANDLE((activated as usize + 1) as _)),
        );
        if !activated && retries > 0 {
            // A foreground ShellExecute caller can still hold its launch/menu
            // lock during the handoff. Retry after it returns; Windows retains
            // control over grants and revokes them on intervening user input.
            let handle = hwnd.0 as usize;
            slint::Timer::single_shot(std::time::Duration::from_millis(100), move || {
                activate(HWND(handle as _), retries - 1)
            });
        }
    }
}
fn select(app: &Rc<App>, path: String) {
    let (loaded, row) = {
        let st = app.state.borrow();
        (
            st.loaded,
            st.order
                .iter()
                .position(|&i| same(&st.entries[i].path, &path)),
        )
    };
    if let Some(row) = row {
        let path = {
            let st = app.state.borrow();
            st.entries[st.order[row]].path.clone()
        };
        app.set_selection(
            HashSet::from([path.clone()]),
            Some(path.clone()),
            Some(path.clone()),
        );
        app.reveal(&path);
    } else {
        app.state.borrow_mut().after_load = Some(AfterLoad {
            select: path,
            rename: false,
        });
        if loaded {
            app.reload();
        }
    }
}
pub fn message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if msg != WM_COPYDATA || lparam.0 == 0 {
        return None;
    }
    let source = HWND(wparam.0 as _);
    unsafe {
        if GetPropW(source, w!("SlintExplorerLaunch")).0 as usize != LAUNCH_MARK
            || !super::tab_transfer::same_executable(source)
        {
            return None;
        }
        let packet = &*(lparam.0 as *const COPYDATASTRUCT);
        if packet.dwData != DATA {
            return None;
        }
        if packet.lpData.is_null() || packet.cbData == 0 || packet.cbData > 131072 {
            return Some(LRESULT(0));
        }
        let Some(request) = decode(std::slice::from_raw_parts(
            packet.lpData as *const u8,
            packet.cbData as usize,
        )) else {
            return Some(LRESULT(0));
        };
        let mut matched = None;
        with_app(|app| {
            matched = app
                .state
                .borrow()
                .tabs
                .iter()
                .position(|t| same(t.path(), &request.path));
        });
        if matches!(request.action, Action::Probe) {
            return Some(LRESULT(matched.is_some() as _));
        }
        if matches!(request.action, Action::Match) && matched.is_none() {
            return Some(LRESULT(0));
        }
        let queued = slint::invoke_from_event_loop(move || {
            with_app(|app| {
                match request.action {
                    Action::Probe => unreachable!("Probe is synchronous and does not activate"),
                    Action::Match => {
                        if let Some(index) = matched {
                            app.select_tab(index);
                        }
                    }
                    Action::Navigate => app.navigate(&request.path),
                    Action::NewTab => app.new_tab(&request.path),
                }
                if let Some(path) = request.select {
                    select(app, path);
                }
            })
        })
        .is_ok();
        // Activate during the synchronous handoff, while the launching process
        // and its foreground grant still exist. UI mutations stay queued.
        if queued {
            activate(hwnd, 2);
        }
        Some(LRESULT(queued as _))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
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
