use super::{wide, Pidl};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::HashMap, sync::mpsc};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, PROPERTYKEY, WPARAM},
        Storage::EnhancedStorage::*,
        System::{
            Com::{StructuredStorage::*, *},
            Threading::*,
            Variant::VT_LPWSTR,
        },
        UI::{
            Shell::{Common::*, PropertiesSystem::*, *},
            WindowsAndMessaging::*,
        },
    },
};
use winreg::{enums::*, RegKey};
pub const APP_ID: &str = "SlintWinExplorer.App";
const HISTORY: &str = r"Software\slint-win-explorer\JumpList";
const KEYS: [PROPERTYKEY; 4] = [
    PKEY_AppUserModel_RelaunchCommand,
    PKEY_AppUserModel_RelaunchDisplayNameResource,
    PKEY_AppUserModel_RelaunchIconResource,
    PKEY_AppUserModel_ID,
];
thread_local! {static ICONS:RefCell<HashMap<usize, (HICON,HICON)>>=RefCell::new(HashMap::new());}

/// CommandLineToArgvW quoting, including a volume root's final backslash.
pub fn quote(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.push_str(&"\\".repeat(slashes * 2 + 1));
            out.push(c);
        } else {
            out.push_str(&"\\".repeat(slashes));
            out.push(c);
        }
        slashes = 0;
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}
pub fn identity() {
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide(APP_ID).as_ptr()));
    }
}
fn explorer_icon() -> String {
    std::path::Path::new(&std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()))
        .join("explorer.exe")
        .to_string_lossy()
        .into_owned()
}
unsafe fn string(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    text: &str,
) -> windows::core::Result<()> {
    let text = SHStrDupW(PCWSTR(wide(text).as_ptr()))?;
    let mut value = PROPVARIANT::default();
    (*value.Anonymous.Anonymous).vt = VT_LPWSTR;
    // windows-rs drops PROPVARIANT through PropVariantClear. Give it an owned
    // CoTaskMem string, not a pointer into a Rust Vec, even for a synchronous call.
    (*value.Anonymous.Anonymous).Anonymous.pwszVal = text;
    store.SetValue(key, &value)
}
pub fn window(hwnd: HWND) -> windows::core::Result<()> {
    unsafe {
        let exe = std::env::current_exe().map_err(|_| windows::core::Error::from_win32())?;
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd)?;
        string(
            &store,
            &KEYS[0],
            &format!("{} --new-window", quote(&exe.to_string_lossy())),
        )?;
        string(&store, &KEYS[1], "@shell32.dll,-12352")?;
        string(&store, &KEYS[2], &format!("{},0", explorer_icon()))?;
        string(&store, &KEYS[3], APP_ID)?;
        let (mut big, mut small) = (HICON::default(), HICON::default());
        if SHDefExtractIconW(
            PCWSTR(wide(&explorer_icon()).as_ptr()),
            0,
            0,
            Some(&mut big),
            Some(&mut small),
            (16 << 16) | 32,
        )
        .is_ok()
        {
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(big.0 as _)));
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(0)),
                Some(LPARAM(small.0 as _)),
            );
            let previous =
                ICONS.with(|icons| icons.borrow_mut().insert(hwnd.0 as usize, (big, small)));
            release_icons(previous);
        }
        Ok(())
    }
}
pub fn close(hwnd: HWND) {
    unsafe {
        if let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) {
            for key in KEYS {
                let _ = store.SetValue(&key, &PROPVARIANT::default());
            }
        }
        SendMessageW(hwnd, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(0)));
        SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(0)));
        let icons = ICONS.with(|icons| icons.borrow_mut().remove(&(hwnd.0 as usize)));
        release_icons(icons);
    }
}
fn release_icons(icons: Option<(HICON, HICON)>) {
    if let Some((big, small)) = icons {
        for icon in [big, small] {
            if !icon.is_invalid() {
                if let Err(error) = unsafe { DestroyIcon(icon) } {
                    eprintln!("taskbar icon cleanup: {error}");
                }
            }
        }
    }
}
pub fn history_enabled() -> bool {
    if unsafe { SHRestricted(REST_NORECENTDOCSHISTORY) } != 0 {
        return false;
    }
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced")
        .and_then(|k| k.get_value::<u32, _>("Start_TrackDocs"))
        .unwrap_or(1)
        != 0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Visit {
    path: String,
    count: u32,
    last: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct History {
    version: u32,
    visits: Vec<Visit>,
    removed: Vec<String>,
}
impl Default for History {
    fn default() -> Self {
        Self {
            version: 1,
            visits: Vec::new(),
            removed: Vec::new(),
        }
    }
}
impl History {
    fn decode(text: &str) -> Self {
        if text.len() > 1_048_576 {
            return Self::default();
        }
        serde_json::from_str::<Self>(text)
            .ok()
            .filter(|h| {
                h.version == 1
                    && h.visits.len() <= 128
                    && h.removed.len() <= 4096
                    && h.visits
                        .iter()
                        .all(|v| v.path.len() <= 32768 && !v.path.contains('\0'))
                    && h.removed
                        .iter()
                        .all(|p| p.len() <= 32768 && !p.contains('\0'))
            })
            .unwrap_or_default()
    }
    fn suppress(&mut self, path: String) {
        self.visits.retain(|v| !v.path.eq_ignore_ascii_case(&path));
        if !self.removed.iter().any(|p| p.eq_ignore_ascii_case(&path)) {
            self.removed.push(path);
        }
    }
    fn visit(&mut self, path: &str, now: u64) {
        if self.removed.iter().any(|p| p.eq_ignore_ascii_case(path)) {
            return;
        }
        if let Some(v) = self
            .visits
            .iter_mut()
            .find(|v| v.path.eq_ignore_ascii_case(path))
        {
            v.count = v.count.saturating_add(1);
            v.last = now;
        } else {
            self.visits.push(Visit {
                path: path.to_string(),
                count: 1,
                last: now,
            });
        }
        self.visits.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(b.last.cmp(&a.last))
                .then(a.path.cmp(&b.path))
        });
        self.visits.truncate(128);
    }
}
struct Lock(HANDLE);
impl Lock {
    fn get() -> Option<Self> {
        unsafe {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::env::var_os("USERPROFILE").hash(&mut h);
            let handle = CreateMutexW(
                None,
                false,
                PCWSTR(wide(&format!(r"Local\SlintExplorerJumpList-{:x}", h.finish())).as_ptr()),
            )
            .ok()?;
            let wait = WaitForSingleObject(handle, 2000);
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
fn folder_icon(path: &str) -> (String, i32) {
    unsafe {
        let found = (|| {
            let p = Pidl::parse(path).ok()?;
            let parent = p.parent()?.folder().ok()?;
            let extract: IExtractIconW = parent
                .GetUIObjectOf(HWND::default(), &[p.last_id()], None)
                .ok()?;
            let (mut file, mut index, mut flags) = (vec![0u16; 32768], 0, 0);
            extract
                .GetIconLocation(GIL_FORSHELL, &mut file, &mut index, &mut flags)
                .ok()?;
            let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
            if len == 0 || flags & GIL_NOTFILENAME != 0 {
                return None;
            }
            Some((String::from_utf16_lossy(&file[..len]), index))
        })();
        found.unwrap_or_else(|| ("shell32.dll".into(), 3))
    }
}
pub fn link(path: Option<&str>) -> windows::core::Result<IShellLinkW> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let exe = std::env::current_exe().map_err(|_| windows::core::Error::from_win32())?;
        link.SetPath(PCWSTR(wide(&exe.to_string_lossy()).as_ptr()))?;
        link.SetArguments(PCWSTR(
            wide(&path.map(quote).unwrap_or_else(|| "--new-window".into())).as_ptr(),
        ))?;
        if let Some(parent) = exe.parent() {
            let _ = link.SetWorkingDirectory(PCWSTR(wide(&parent.to_string_lossy()).as_ptr()));
        }
        let (icon, index) = path
            .map(folder_icon)
            .unwrap_or_else(|| (explorer_icon(), 0));
        link.SetIconLocation(PCWSTR(wide(&icon).as_ptr()), index)?;
        let store: IPropertyStore = link.cast()?;
        string(&store, &PKEY_AppUserModel_ID, APP_ID)?;
        let title = path
            .map(super::listing::display_name_of)
            .unwrap_or_else(|| crate::i18n::system_string("shell32.dll", 12352));
        string(&store, &PKEY_Title, &title)?;
        store.Commit()?;
        Ok(link)
    }
}
unsafe fn removed_path(link: &IShellLinkW) -> Option<String> {
    let mut args = vec![0u16; 32768];
    link.GetArguments(&mut args).ok()?;
    let len = args.iter().position(|&c| c == 0).unwrap_or(args.len());
    let command = wide(&format!(
        "slint-win-explorer.exe {}",
        String::from_utf16_lossy(&args[..len])
    ));
    let mut count = 0;
    let argv = CommandLineToArgvW(PCWSTR(command.as_ptr()), &mut count);
    if argv.is_null() {
        return None;
    }
    let result = if count == 2 {
        (*argv.add(1)).to_string().ok()
    } else {
        None
    };
    let _ =
        windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(argv as _)));
    result
}
fn update(path: &str) -> windows::core::Result<()> {
    unsafe {
        let Some(_lock) = Lock::get() else {
            return Ok(());
        };
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(HISTORY, KEY_READ | KEY_WRITE)
            .ok();
        let mut history = History::decode(
            &key.as_ref()
                .and_then(|k| k.get_value::<String, _>("History").ok())
                .unwrap_or_default(),
        );
        let before = history.clone();
        let list: ICustomDestinationList =
            CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        list.SetAppID(PCWSTR(wide(APP_ID).as_ptr()))?;
        let mut slots = 0;
        let removed: IObjectArray = list.BeginList(&mut slots)?;
        let result = (|| {
            for i in 0..removed.GetCount()? {
                if let Ok(link) = removed.GetAt::<IShellLinkW>(i) {
                    if let Some(path) = removed_path(&link) {
                        history.suppress(path);
                    }
                }
            }
            let on = history_enabled();
            if !on {
                history.visits.clear();
            }
            if on && !path.is_empty() && std::path::Path::new(path).is_dir() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                history.visit(path, now);
            }
            if on {
                let objects: IObjectCollection =
                    CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
                let mut added = 0;
                for item in &history.visits {
                    if added >= slots.min(12) {
                        break;
                    }
                    if std::path::Path::new(&item.path).is_dir() {
                        if let Ok(item) = link(Some(&item.path)) {
                            objects.AddObject(&item)?;
                            added += 1;
                        }
                    }
                }
                if added > 0 {
                    let array: IObjectArray = objects.cast()?;
                    let label = wide(&crate::i18n::system_string("explorerframe.dll", 50198));
                    // Privacy policy can deny custom categories; still commit common commands.
                    if let Err(error) = list.AppendCategory(PCWSTR(label.as_ptr()), &array) {
                        if error.code().0 != -2147024891 {
                            return Err(error);
                        }
                    }
                }
            }
            list.CommitList()?;
            if history != before {
                if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(HISTORY) {
                    if let Ok(text) = serde_json::to_string(&history) {
                        let _ = key.set_value("History", &text);
                    }
                }
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = list.AbortList();
        }
        result
    }
}
pub fn start() -> mpsc::Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        super::com_init();
        for path in rx {
            if let Err(e) = update(&path) {
                eprintln!("jump list: {}", e.message());
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    #[test]
    fn native_window_properties_roundtrip_and_release_without_taskbar_pinning() {
        super::super::com_init();
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Slint Explorer metadata test"),
                WINDOW_STYLE(0),
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            struct Window(HWND);
            impl Drop for Window {
                fn drop(&mut self) {
                    close(self.0);
                    unsafe {
                        let _ = DestroyWindow(self.0);
                    }
                }
            }
            let _owned = Window(hwnd);
            window(hwnd).unwrap();
            let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).unwrap();
            let icon_resource = format!("{},0", explorer_icon());
            for (key, expected) in [
                (PKEY_AppUserModel_ID, APP_ID),
                (
                    PKEY_AppUserModel_RelaunchDisplayNameResource,
                    "@shell32.dll,-12352",
                ),
                (
                    PKEY_AppUserModel_RelaunchIconResource,
                    icon_resource.as_str(),
                ),
            ] {
                let value = store.GetValue(&key).unwrap();
                let text = PropVariantToStringAlloc(&value).unwrap();
                assert_eq!(super::super::take_pwstr(text), expected);
            }
            let second = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Slint Explorer second metadata test"),
                WINDOW_STYLE(0),
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let _second_owned = Window(second);
            window(second).unwrap();
            let second_icon = SendMessageW(second, WM_GETICON, Some(WPARAM(1)), None);
            assert_ne!(second_icon.0, 0);
            assert!(ICONS.with(|icons| icons.borrow().contains_key(&(second.0 as usize))));
            close(hwnd);
            assert_eq!(
                SendMessageW(second, WM_GETICON, Some(WPARAM(1)), None),
                second_icon
            );
            let second_store: IPropertyStore = SHGetPropertyStoreForWindow(second).unwrap();
            let value = second_store.GetValue(&PKEY_AppUserModel_ID).unwrap();
            assert_eq!(
                super::super::take_pwstr(PropVariantToStringAlloc(&value).unwrap()),
                APP_ID
            );
            assert!(ICONS.with(|icons| !icons.borrow().contains_key(&(hwnd.0 as usize))));
            assert!(store.GetValue(&PKEY_AppUserModel_ID).unwrap().is_empty());
        }
    }
    #[test]
    fn shell_link_preserves_own_identity_and_quoted_folder_including_volume_root() {
        super::super::com_init();
        unsafe {
            for path in [r"C:\", r"C:\two words\", r"C:\Usuários\José"] {
                let item = link(Some(path)).unwrap();
                assert_eq!(removed_path(&item).as_deref(), Some(path));
                let store: IPropertyStore = item.cast().unwrap();
                let mut v = store.GetValue(&PKEY_AppUserModel_ID).unwrap();
                let text = PropVariantToStringAlloc(&v).unwrap();
                assert_eq!(super::super::take_pwstr(text), APP_ID);
                PropVariantClear(&mut v).unwrap();
            }
        }
    }
    #[test]
    fn removed_destination_stays_suppressed_after_further_visits_and_history_reload() {
        let mut h = History::default();
        h.visit(r"C:\A", 10);
        h.visit(r"C:\B", 20);
        h.visit(r"c:\a", 30);
        assert_eq!(h.visits[0].path, r"C:\A");
        h.suppress(r"c:\a".into());
        h.visit(r"C:\A", 40);
        let restored = History::decode(&serde_json::to_string(&h).unwrap());
        assert_eq!(restored.visits.len(), 1);
        assert_eq!(restored.visits[0].path, r"C:\B");
        assert_eq!(restored.removed, [r"c:\a"]);
    }
}
