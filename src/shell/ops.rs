use windows::core::{Interface, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::{
    CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
};
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_STREAM};
use windows::Win32::UI::Shell::{
    IContextMenu, IShellLinkW, SHChangeNotify, SHGetFileInfoW, SHOpenWithDialog, ShellExecuteExW,
    ShellLink, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, OAIF_ALLOW_REGISTRATION, OAIF_EXEC,
    OAIF_REGISTER_EXT, OPENASINFO, SEE_MASK_INVOKEIDLIST, SHCNE_CREATE, SHCNE_MKDIR, SHCNF_PATHW,
    SHELLEXECUTEINFOW, SHFILEINFOW, SHGDN_INFOLDER, SHGFI_ATTRIBUTES, SHGFI_PIDL,
    SIGDN_DESKTOPABSOLUTEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, SW_SHOWNORMAL};

use super::{com_init, wide, ItemSet, Pidl};

pub const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
const CF_HDROP: u32 = 15;

unsafe fn is_folder_pidl(pidl: &Pidl) -> bool {
    let mut info = SHFILEINFOW {
        dwAttributes: SFGAO_FOLDER.0 | SFGAO_STREAM.0,
        ..Default::default()
    };
    let ok = SHGetFileInfoW(
        PCWSTR(pidl.ptr() as *const u16),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_PIDL | SHGFI_ATTRIBUTES,
    );
    ok != 0 && info.dwAttributes & SFGAO_FOLDER.0 != 0 && info.dwAttributes & SFGAO_STREAM.0 == 0
}

unsafe fn link_target(path: &str) -> Option<Pidl> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
    let w = wide(path);
    link.cast::<IPersistFile>()
        .ok()?
        .Load(PCWSTR(w.as_ptr()), STGM_READ)
        .ok()?;
    let pidl = link.GetIDList().ok()?;
    if pidl.is_null() {
        None
    } else {
        Some(Pidl(pidl))
    }
}

/// The file system path a shortcut points at, if it points at one.
pub fn link_file(path: &str) -> Option<String> {
    com_init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let w = wide(path);
        link.cast::<IPersistFile>()
            .ok()?
            .Load(PCWSTR(w.as_ptr()), STGM_READ)
            .ok()?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
        (n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
    }
}

/// The shell drop target of a child folder, or of the folder background when `item` is None.
pub fn drop_target(
    hwnd: HWND,
    folder: &str,
    item: Option<&str>,
) -> Result<windows::Win32::System::Ole::IDropTarget, String> {
    use windows::Win32::System::Ole::IDropTarget;
    com_init();
    unsafe {
        match item {
            Some(path) => {
                let set = ItemSet::new(&[path.to_string()])?;
                set.folder
                    .GetUIObjectOf::<IDropTarget>(hwnd, &set.children(), None)
                    .map_err(|e| e.message())
            }
            None => Pidl::parse(folder)?
                .folder()?
                .CreateViewObject::<IDropTarget>(hwnd)
                .map_err(|e| e.message()),
        }
    }
}

/// The shell data object for items sharing one parent (what Explorer drags and copies).
pub fn data_object(
    hwnd: HWND,
    paths: &[String],
) -> Result<windows::Win32::System::Com::IDataObject, String> {
    com_init();
    let set = ItemSet::new(paths)?;
    unsafe {
        set.folder
            .GetUIObjectOf(hwnd, &set.children(), None)
            .map_err(|e| e.message())
    }
}

pub fn is_folder(path: &str) -> bool {
    com_init();
    match Pidl::parse(path) {
        Ok(pidl) => unsafe { is_folder_pidl(&pidl) },
        Err(_) => false,
    }
}

/// Runs a command line or URL typed into the address bar, as Explorer does: the program and
/// its arguments apart ("cmd /k dir"), started in the folder shown (`dir`).
pub fn shell_execute(hwnd: HWND, text: &str, dir: Option<&str>) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    let text = text.trim();
    let (program, args) = match text.strip_prefix('"') {
        Some(rest) => match rest.find('"') {
            Some(end) => (&rest[..end], rest[end + 1..].trim()),
            None => (rest, ""),
        },
        None => match text.find(char::is_whitespace) {
            Some(at) => (&text[..at], text[at..].trim()),
            None => (text, ""),
        },
    };
    // Some registered commands name an absolute .exe without its extension (rundll32).
    // ShellExecuteW does not resolve that spelling as CreateProcess does.
    let executable = std::path::Path::new(program);
    let with_exe = executable.with_extension("exe");
    let resolved = if executable.is_absolute()
        && executable.extension().is_none()
        && !executable.exists()
        && with_exe.is_file()
    {
        with_exe.to_string_lossy().into_owned()
    } else {
        program.to_string()
    };
    let (file, verb) = (wide(&resolved), wide("open"));
    let args = (!args.is_empty()).then(|| wide(args));
    let dir = dir
        .filter(|d| !d.is_empty() && std::path::Path::new(d).is_dir())
        .map(wide);
    let r = unsafe {
        ShellExecuteW(
            Some(hwnd),
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            args.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr())),
            dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute failed ({})", r.0 as isize))
    }
}

/// Opens an item the way a double click in Explorer does. When the item is a folder, or a
/// shortcut to one, nothing is launched and the folder to navigate into is returned instead.
pub fn open(hwnd: HWND, path: &str) -> Result<Option<String>, String> {
    com_init();
    unsafe {
        let pidl = Pidl::parse(path)?;
        if is_folder_pidl(&pidl) {
            return Ok(Some(path.to_string()));
        }
        if path.to_lowercase().ends_with(".lnk") {
            if let Some(target) = link_target(path) {
                if is_folder_pidl(&target) {
                    return Ok(Some(target.name(SIGDN_DESKTOPABSOLUTEPARSING)));
                }
            }
        }
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST,
            hwnd,
            lpIDList: pidl.ptr() as *mut _,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).map_err(|e| e.message())?;
        Ok(None)
    }
}

pub unsafe fn invoke(
    cm: &IContextMenu,
    hwnd: HWND,
    verb: Option<&str>,
    id: usize,
    shift: bool,
    pt: Option<POINT>,
) -> Result<(), String> {
    let verb_a = verb.map(|v| std::ffi::CString::new(v).unwrap());
    let verb_w = verb.map(wide);
    let mut mask = CMIC_MASK_UNICODE;
    if shift {
        mask |= CMIC_MASK_SHIFT_DOWN;
    }
    if pt.is_some() {
        mask |= CMIC_MASK_PTINVOKE;
    }
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: mask,
        hwnd,
        lpVerb: match &verb_a {
            Some(v) => PCSTR(v.as_ptr() as *const u8),
            None => PCSTR(id as *const u8),
        },
        lpVerbW: match &verb_w {
            Some(v) => PCWSTR(v.as_ptr()),
            None => PCWSTR(id as *const u16),
        },
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: pt.unwrap_or_default(),
        ..Default::default()
    };
    cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
        .map_err(|e| e.message())
}

/// Context menu of a set of items (sharing one parent) or, with no items, of the folder background.
pub unsafe fn context_menu(
    hwnd: HWND,
    folder: &str,
    paths: &[String],
) -> Result<IContextMenu, String> {
    if paths.is_empty() {
        let pidl = Pidl::parse(folder)?;
        let sf = pidl.folder()?;
        sf.CreateViewObject::<IContextMenu>(hwnd)
            .map_err(|e| e.message())
    } else {
        let set = ItemSet::new(paths)?;
        set.folder
            .GetUIObjectOf::<IContextMenu>(hwnd, &set.children(), None)
            .map_err(|e| e.message())
    }
}

/// Runs a canonical shell verb ("copy", "cut", "paste", "delete", "properties", "pintohome",
/// "openas", ...) exactly as Explorer would, so clipboard, recycle bin and dialogs are native.
pub fn invoke_verb(
    hwnd: HWND,
    folder: &str,
    paths: &[String],
    verb: &str,
    shift: bool,
) -> Result<(), String> {
    com_init();
    unsafe {
        let cm = context_menu(hwnd, folder, paths)?;
        let menu = CreatePopupMenu().map_err(|e| e.message())?;
        let flags = CMF_NORMAL | if shift { CMF_EXTENDEDVERBS } else { 0 };
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, flags);
        let res = invoke(&cm, hwnd, Some(verb), 0, shift, None);
        let _ = DestroyMenu(menu);
        res
    }
}

/// Puts items deleted to the Recycle Bin back where they were (its "undelete" verb): each by
/// its original path, the latest deleted when a name repeats. Returns how many it found.
pub fn restore_deleted(hwnd: HWND, originals: &[String]) -> Result<usize, String> {
    use windows::Win32::System::Com::{CoTaskMemFree, StructuredStorage::PropVariantToStringAlloc};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
    use windows::Win32::UI::Shell::{
        IEnumIDList, IShellItem2, SHCreateItemWithParent, SHCONTF_FOLDERS, SHCONTF_NONFOLDERS,
        SIGDN_NORMALDISPLAY,
    };
    com_init();
    let key = |name: &str| {
        let w = wide(name);
        let mut k = windows::Win32::Foundation::PROPERTYKEY::default();
        unsafe { PSGetPropertyKeyFromName(PCWSTR(w.as_ptr()), &mut k).map(|_| k) }
    };
    let (from_key, date_key) = (
        key("System.Recycle.DeletedFrom").map_err(|e| e.message())?,
        key("System.Recycle.DateDeleted").map_err(|e| e.message())?,
    );
    unsafe {
        let bin = Pidl::parse("shell:RecycleBinFolder")?;
        let folder = bin.folder()?;
        let mut list: Option<IEnumIDList> = None;
        let _ = folder
            .EnumObjects(
                HWND::default(),
                (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32,
                &mut list,
            )
            .ok();
        let Some(list) = list else { return Ok(0) };
        // The latest deleted item for each original path: its date and ID.
        let mut found: Vec<(String, u64, Pidl)> = Vec::new();
        loop {
            let mut child: [*mut ITEMIDLIST; 1] = [std::ptr::null_mut()];
            let mut got = 0u32;
            if list.Next(&mut child, Some(&mut got)).is_err() || got == 0 {
                break;
            }
            let child = Pidl(child[0]);
            let Ok(item) = SHCreateItemWithParent::<_, IShellItem2>(None, &folder, child.ptr())
            else {
                continue;
            };
            let from = item
                .GetProperty(&from_key)
                .ok()
                .and_then(|v| PropVariantToStringAlloc(&v).ok())
                .map(|p| super::take_pwstr(p))
                .unwrap_or_default();
            let name = item
                .GetDisplayName(SIGDN_NORMALDISPLAY)
                .map(|p| super::take_pwstr(p))
                .unwrap_or_default();
            let original = std::path::Path::new(&from)
                .join(&name)
                .to_string_lossy()
                .to_string();
            if !originals.iter().any(|o| o.eq_ignore_ascii_case(&original)) {
                continue;
            }
            let date = item
                .GetFileTime(&date_key)
                .map(|t| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64)
                .unwrap_or(0);
            match found
                .iter_mut()
                .find(|(o, _, _)| o.eq_ignore_ascii_case(&original))
            {
                Some(entry) if entry.1 < date => *entry = (original, date, child),
                Some(_) => {}
                None => found.push((original, date, child)),
            }
        }
        let _ = CoTaskMemFree;
        if found.is_empty() {
            return Ok(0);
        }
        let ids: Vec<*const ITEMIDLIST> = found.iter().map(|(_, _, p)| p.ptr()).collect();
        let cm: IContextMenu = folder
            .GetUIObjectOf(hwnd, &ids, None)
            .map_err(|e| e.message())?;
        let menu = CreatePopupMenu().map_err(|e| e.message())?;
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL);
        let res = invoke(&cm, hwnd, Some("undelete"), 0, false, None);
        let _ = DestroyMenu(menu);
        res.map(|_| found.len())
    }
}

pub fn rename(hwnd: HWND, path: &str, new_name: &str) -> Result<String, String> {
    com_init();
    unsafe {
        let set = ItemSet::new(&[path.to_string()])?;
        let w = wide(new_name);
        let mut out = std::ptr::null_mut();
        set.folder
            .SetNameOf(
                Some(hwnd),
                set.items[0].last_id(),
                PCWSTR(w.as_ptr()),
                SHGDN_INFOLDER,
                Some(&mut out),
            )
            .map_err(|e| e.message())?;
        let child = Pidl(out);
        let abs = windows::Win32::UI::Shell::ILCombine(Some(set.parent.ptr()), Some(child.ptr()));
        let abs = Pidl(abs);
        Ok(abs.name(SIGDN_DESKTOPABSOLUTEPARSING))
    }
}

pub fn unique_child_path(folder: &str, base: &str, ext: &str) -> std::path::PathBuf {
    unique_child(folder, base, ext)
}

/// Compresses items sharing a folder into a new archive beside them, named after the first
/// one, by dropping them on an empty .zip (the compressed folder handler copies them in).
pub fn compress_to_zip(hwnd: HWND, paths: &[String]) -> Result<String, String> {
    use windows::Win32::Foundation::POINTL;
    use windows::Win32::System::Ole::DROPEFFECT_COPY;
    use windows::Win32::System::SystemServices::MK_LBUTTON;
    let first = std::path::Path::new(&paths[0]);
    let folder = first
        .parent()
        .ok_or("sem pasta")?
        .to_string_lossy()
        .to_string();
    let base = if is_folder(&paths[0]) {
        first.file_name()
    } else {
        first.file_stem()
    };
    let base = base
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let zip = unique_child(&folder, &base, ".zip");
    // An empty archive: just the end of central directory record.
    let mut empty = vec![b'P', b'K', 5, 6];
    empty.resize(22, 0);
    std::fs::write(&zip, &empty).map_err(|e| e.to_string())?;
    notify_created(&zip);
    let zip = zip.to_string_lossy().to_string();
    // The archive as a folder takes the drop (the item's own drop target is not implemented).
    let target = drop_target(hwnd, &zip, None)?;
    let data = data_object(hwnd, paths)?;
    let mut effect = DROPEFFECT_COPY;
    unsafe {
        target
            .DragEnter(&data, MK_LBUTTON, POINTL::default(), &mut effect)
            .map_err(|e| e.message())?;
        target
            .Drop(&data, MK_LBUTTON, POINTL::default(), &mut effect)
            .map_err(|e| e.message())?;
    }
    Ok(zip)
}

/// The items as the shell item array that `IExplorerCommand`s take.
pub fn item_array(paths: &[String]) -> Result<windows::Win32::UI::Shell::IShellItemArray, String> {
    use windows::Win32::UI::Shell::SHCreateShellItemArrayFromIDLists;
    let pidls = paths
        .iter()
        .map(|p| Pidl::parse(p))
        .collect::<Result<Vec<_>, _>>()?;
    let ids: Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> =
        pidls.iter().map(|p| p.ptr()).collect();
    unsafe { SHCreateShellItemArrayFromIDLists(&ids).map_err(|e| e.message()) }
}

/// Runs one of Explorer's own command-store commands (an `ExplorerCommandHandler` CLSID,
/// e.g. `Windows.CompressTo.7Z`) on items, as its context menu entry does.
pub fn explorer_command(clsid: &str, paths: &[String]) -> Result<(), String> {
    use windows::core::GUID;
    use windows::Win32::UI::Shell::IExplorerCommand;
    com_init();
    let clsid =
        GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')).map_err(|e| e.message())?;
    let items = item_array(paths)?;
    unsafe {
        let command: IExplorerCommand =
            CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        command.Invoke(&items, None).map_err(|e| e.message())
    }
}

/// The native commands exposed by This PC/drive overflow menus. Merely building
/// a menu never creates or invokes these handlers. Formatting is the shell's
/// format *dialog*; no format parameters or automatic confirmation are supplied.
pub fn command_store(hwnd: HWND, name: &str, paths: &[String]) -> Result<(), String> {
    use windows::Win32::NetworkManagement::WNet::{
        WNetConnectionDialog, WNetDisconnectDialog, RESOURCETYPE_DISK,
    };
    use windows::Win32::UI::Shell::{IExplorerCommand, IInitializeWithWindow, IShellItemArray};
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    com_init();
    match name {
        // The native handler launches this GUI without parameters. Calling the handler
        // without Explorer's browser site can succeed without opening any window.
        "Windows.Defragment" => return shell_execute(hwnd, "dfrgui.exe", None),
        "Windows.AddNetworkLocation" => {
            let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            return shell_execute(hwnd, &format!("\"{windows}\\System32\\rundll32.exe\" \"{windows}\\System32\\shwebsvc.dll\",AddNetPlaceRunDll"), None);
        }
        "Windows.DiskFormat" => {
            return invoke_verb(
                hwnd,
                "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
                paths,
                "format",
                false,
            )
        }
        "Windows.MapNetworkDrive" | "Windows.DisconnectNetworkDrive" => {
            let status = unsafe {
                if name == "Windows.MapNetworkDrive" {
                    WNetConnectionDialog(hwnd, RESOURCETYPE_DISK.0)
                } else {
                    WNetDisconnectDialog(Some(hwnd), RESOURCETYPE_DISK.0)
                }
            };
            return if status.0 == 0 || status.0 == 1223 || status.0 == u32::MAX {
                Ok(())
            } else {
                Err(format!("{name}: {}", status.0))
            };
        }
        _ => (),
    }
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(format!(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\CommandStore\shell\{name}"
        ))
        .map_err(|e| e.to_string())?;
    if let Ok(handler) = key.get_value::<String, _>("ExplorerCommandHandler") {
        let clsid = windows::core::GUID::try_from(handler.trim_matches(|c| c == '{' || c == '}'))
            .map_err(|e| e.message())?;
        unsafe {
            let command: IExplorerCommand =
                CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
            if let Ok(init) = command.cast::<IInitializeWithWindow>() {
                let _ = init.Initialize(hwnd);
            }
            let selection = if paths.is_empty() {
                None
            } else {
                Some(item_array(paths)?)
            };
            command
                .Invoke(selection.as_ref().map(|s| s as &IShellItemArray), None)
                .map_err(|e| e.message())
        }
    } else if let Ok(verb) = key.get_value::<String, _>("VerbName") {
        invoke_verb(hwnd, "", paths, &verb, false)
    } else {
        let command: String = key
            .open_subkey("command")
            .and_then(|k| k.get_value(""))
            .map_err(|e| e.to_string())?;
        shell_execute(hwnd, &command, None)
    }
}

/// "Arquivo 7z" / "Arquivo TAR": the system's bsdtar writes the archive (named after the
/// first item, beside the items) and picks the format from the extension. Blocks until done.
pub fn compress_with_tar(paths: &[String], ext: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let first = std::path::Path::new(&paths[0]);
    let folder = first
        .parent()
        .ok_or("sem pasta")?
        .to_string_lossy()
        .to_string();
    let base = if is_folder(&paths[0]) {
        first.file_name()
    } else {
        first.file_stem()
    };
    let base = base
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let out = unique_child(&folder, &base, ext);
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let names = paths.iter().filter_map(|p| {
        std::path::Path::new(p)
            .file_name()
            .map(|n| n.to_os_string())
    });
    let status = std::process::Command::new(std::path::Path::new(&windir).join(r"System32	ar.exe"))
        .arg("-a")
        .arg("-c")
        .arg("-f")
        .arg(&out)
        .arg("-C")
        .arg(&folder)
        .args(names)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(format!("tar: {status}"));
    }
    notify_created(&out);
    Ok(out.to_string_lossy().to_string())
}

pub fn notify_created(path: &std::path::Path) {
    notify(SHCNE_CREATE, path);
}

/// "Novo > Atalho": an empty shortcut handed to the Create Shortcut wizard, like Explorer.
pub fn new_shortcut(hwnd: HWND, folder: &str) -> Result<(), String> {
    // shell32's "Novo(a) %s" with "Atalho", as Explorer names it.
    let name = crate::i18n::shell_string(30316, &crate::i18n::system_string("shell32.dll", 4153));
    let target = unique_child(folder, &name, ".lnk");
    std::fs::write(&target, b"").map_err(|e| e.to_string())?;
    let args = format!("appwiz.cpl,NewLinkHere {}", target.to_string_lossy());
    let file = wide("rundll32.exe");
    let params = wide(&args);
    let r = unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            Some(hwnd),
            PCWSTR::null(),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            None,
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err("could not start the shortcut wizard".into())
    }
}

fn unique_child(folder: &str, base: &str, ext: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(folder);
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{base}{ext}")
        } else {
            format!("{base} ({n}){ext}")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn notify(event: windows::Win32::UI::Shell::SHCNE_ID, path: &std::path::Path) {
    let w = wide(&path.to_string_lossy());
    unsafe { SHChangeNotify(event, SHCNF_PATHW, Some(w.as_ptr() as *const _), None) };
}

pub fn new_folder(folder: &str) -> Result<String, String> {
    let target = unique_child(folder, &crate::i18n::shell_string(30396, ""), "");
    std::fs::create_dir(&target).map_err(|e| e.to_string())?;
    notify(SHCNE_MKDIR, &target);
    Ok(target.to_string_lossy().into_owned())
}

pub fn new_file(folder: &str, base: &str, ext: &str) -> Result<String, String> {
    let target = unique_child(folder, base, ext);
    std::fs::File::create(&target).map_err(|e| e.to_string())?;
    notify(SHCNE_CREATE, &target);
    Ok(target.to_string_lossy().into_owned())
}

pub fn can_paste() -> bool {
    unsafe {
        let idlist = RegisterClipboardFormatW(windows::core::w!("Shell IDList Array"));
        IsClipboardFormatAvailable(CF_HDROP).is_ok() || IsClipboardFormatAvailable(idlist).is_ok()
    }
}

pub fn open_with(hwnd: HWND, path: &str) -> Result<(), String> {
    com_init();
    let w = wide(path);
    let info = OPENASINFO {
        pcszFile: PCWSTR(w.as_ptr()),
        oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        ..Default::default()
    };
    unsafe { SHOpenWithDialog(Some(hwnd), &info).map_err(|e| e.message()) }
}

/// The file system paths a data object carries (CF_HDROP).
pub fn dropped_files(data: &windows::Win32::System::Com::IDataObject) -> Vec<String> {
    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::ReleaseStgMedium;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    unsafe {
        let fmt = FORMATETC {
            cfFormat: CF_HDROP as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let Ok(mut medium) = data.GetData(&fmt) else {
            return Vec::new();
        };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let files = (0..count)
            .map(|i| {
                let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                let n = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                String::from_utf16_lossy(&buf[..n])
            })
            .collect();
        ReleaseStgMedium(&mut medium);
        files
    }
}

/// Drag identities, including namespace folders that have no CF_HDROP paths.
pub fn drag_items(data: &windows::Win32::System::Com::IDataObject) -> Vec<String> {
    let files = dropped_files(data);
    if !files.is_empty() {
        return files;
    }
    unsafe {
        use windows::Win32::UI::Shell::{
            IShellItemArray, SHCreateShellItemArrayFromDataObject, SIGDN_DESKTOPABSOLUTEPARSING,
        };
        let Ok(items) = SHCreateShellItemArrayFromDataObject::<_, IShellItemArray>(data) else {
            return Vec::new();
        };
        (0..items.GetCount().unwrap_or(0))
            .filter_map(|i| {
                items
                    .GetItemAt(i)
                    .ok()?
                    .GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)
                    .ok()
                    .map(|p| super::take_pwstr(p))
            })
            .collect()
    }
}

/// Asks a data object for a synchronous transfer, so that the shell's copy or move is done
/// when the drop (or paste) returns and what it made can be told.
pub fn transfer_now(data: &windows::Win32::System::Com::IDataObject) {
    use windows::Win32::UI::Shell::IDataObjectAsyncCapability;
    if let Ok(asynchronous) = data.cast::<IDataObjectAsyncCapability>() {
        unsafe {
            let _ = asynchronous.SetAsyncMode(false);
        }
    }
}

/// The clipboard's files, with the transfer made synchronous (see `transfer_now`).
pub fn clipboard_files() -> Vec<String> {
    use windows::Win32::System::Ole::OleGetClipboard;
    let Ok(data) = (unsafe { OleGetClipboard() }) else {
        return Vec::new();
    };
    transfer_now(&data);
    dropped_files(&data)
}

/// A folder's names, before a copy or move into it.
pub fn snapshot(dir: &str) -> Option<std::collections::HashSet<String>> {
    let read = std::fs::read_dir(dir).ok()?;
    Some(
        read.filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
    )
}

/// What a copy or move of files into a folder made: each source and the item it became.
#[derive(Default, Debug)]
pub struct Transfer {
    pub moved: Vec<(String, String)>,
    pub copied: Vec<(String, String)>,
}

/// Matches the items that appeared in `dir` since `before` to the sources: the same name, or
/// one built on it ("a - Copia.txt", "a (2).txt"); a source that's gone was moved.
pub fn transfer(
    sources: &[String],
    dir: &str,
    before: &std::collections::HashSet<String>,
) -> Transfer {
    let mut fresh: Vec<String> = snapshot(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| !before.contains(n))
        .collect();
    fresh.sort();
    let mut out = Transfer::default();
    for source in sources {
        let path = std::path::Path::new(source);
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let is_dir =
            path.is_dir() || !path.exists() && std::path::Path::new(dir).join(&name).is_dir();
        let (stem, ext) = match (is_dir, name.rfind('.')) {
            (false, Some(dot)) if dot > 0 => (name[..dot].to_string(), name[dot..].to_string()),
            _ => (name.clone(), String::new()),
        };
        let at = fresh
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&name))
            .or_else(|| {
                fresh
                    .iter()
                    .position(|n| n.len() > stem.len() && n.starts_with(&stem) && n.ends_with(&ext))
            });
        let Some(at) = at else { continue };
        let made = std::path::Path::new(dir)
            .join(fresh.remove(at))
            .to_string_lossy()
            .to_string();
        if path.exists() {
            out.copied.push((source.clone(), made));
        } else {
            out.moved.push((source.clone(), made));
        }
    }
    out
}

/// Moves (or copies) each item to the path given, through the shell's copy engine with its
/// progress and conflict prompts ("Desfazer Mover", "Refazer Copiar").
pub fn transfer_items(hwnd: HWND, pairs: &[(String, String)], copy: bool) -> Result<(), String> {
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName, FOF_ALLOWUNDO,
        FOF_NOCONFIRMMKDIR,
    };
    com_init();
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.message())?;
        op.SetOwnerWindow(hwnd).map_err(|e| e.message())?;
        op.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR)
            .map_err(|e| e.message())?;
        for (from, to) in pairs {
            let to = std::path::Path::new(to);
            let (Some(folder), Some(name)) = (to.parent(), to.file_name()) else {
                continue;
            };
            let from_w = wide(from);
            let folder_w = wide(&folder.to_string_lossy());
            let name_w = wide(&name.to_string_lossy());
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(from_w.as_ptr()), None)
                .map_err(|e| e.message())?;
            let dest: IShellItem = SHCreateItemFromParsingName(PCWSTR(folder_w.as_ptr()), None)
                .map_err(|e| e.message())?;
            if copy {
                op.CopyItem(&item, &dest, PCWSTR(name_w.as_ptr()), None)
                    .map_err(|e| e.message())?;
            } else {
                op.MoveItem(&item, &dest, PCWSTR(name_w.as_ptr()), None)
                    .map_err(|e| e.message())?;
            }
        }
        op.PerformOperations().map_err(|e| e.message())
    }
}

/// Deletes items for good, without asking: Explorer's "Desfazer Copiar" (the Recycle Bin
/// doesn't get the copies).
pub fn delete_permanently(hwnd: HWND, paths: &[String]) -> Result<(), String> {
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    };
    if paths.is_empty() {
        return Ok(());
    }
    com_init();
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.message())?;
        op.SetOwnerWindow(hwnd).map_err(|e| e.message())?;
        op.SetOperationFlags(FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT)
            .map_err(|e| e.message())?;
        let items = item_array(paths)?;
        op.DeleteItems(&items).map_err(|e| e.message())?;
        op.PerformOperations().map_err(|e| e.message())
    }
}

/// "Esvaziar Lixeira", with the shell's own confirmation and progress.
pub fn empty_recycle_bin(hwnd: HWND) {
    use windows::Win32::UI::Shell::SHEmptyRecycleBinW;
    unsafe {
        let _ = SHEmptyRecycleBinW(Some(hwnd), PCWSTR::null(), 0);
    }
}

/// "Restaurar todos os itens" / "Restaurar os itens selecionados": the Recycle Bin's items,
/// given by their IDs in it, put back where they were (their "undelete" verb).
pub fn restore_from_recycle_bin(hwnd: HWND, children: &[Vec<u8>]) -> Result<(), String> {
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    com_init();
    let ids: Vec<*const ITEMIDLIST> = children
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| c.as_ptr() as *const ITEMIDLIST)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    unsafe {
        let folder = Pidl::parse("shell:RecycleBinFolder")?.folder()?;
        let cm: IContextMenu = folder
            .GetUIObjectOf(hwnd, &ids, None)
            .map_err(|e| e.message())?;
        let menu = CreatePopupMenu().map_err(|e| e.message())?;
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL);
        let res = invoke(&cm, hwnd, Some("undelete"), 0, false, None);
        let _ = DestroyMenu(menu);
        res
    }
}

/// A Yes/No question in a classic warning message box; true for Yes.
pub fn confirm(hwnd: HWND, caption: &str, text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO};
    let (caption, text) = (wide(caption), wide(text));
    unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_YESNO | MB_ICONWARNING,
        ) == IDYES
    }
}
