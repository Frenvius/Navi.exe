use std::sync::OnceLock;

use windows::core::GUID;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER};
use windows::Win32::UI::Shell::{
    IExplorerCommand, IShellItemArray, ECF_HASSUBCOMMANDS, ECS_DISABLED, ECS_HIDDEN,
};

use super::icons::{small_icon_at, Rgba};
use super::{com_init, ops, take_pwstr};

struct Verb {
    clsid: GUID,
    name: String,
    group: Option<String>,
    /// Lowercase item types.
    types: Vec<String>,
    /// The handler's DLL in its package: packaged COM classes are only registered for
    /// Explorer (and the package itself), so we load it like Explorer's surrogate does.
    dll: Option<std::path::PathBuf>,
    /// Index into `FALLBACKS` when the package is one we can stand in for.
    fallback: Option<usize>,
    /// The stand-in's program, found next to the package for apps installed outside it.
    program: std::path::PathBuf,
    /// Where Explorer lists it among the app commands.
    rank: usize,
}

/// Apps whose handlers we can't load (inbox apps live in WindowsApps, which an unpackaged
/// process can't read; VS Code ships a 32-bit handler that only Explorer's surrogate runs):
/// (package family prefix, title, app id for the icon or "" for the program's, program
/// run once per item).
const FALLBACKS: &[(&str, &str, &str, &str)] = &[
    (
        "Microsoft.WindowsTerminal_",
        "Abrir no Terminal",
        "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App",
        "wt.exe",
    ),
    (
        "Microsoft.WindowsNotepad_",
        "Editar no Bloco de Notas",
        "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App",
        "notepad.exe",
    ),
    (
        "Microsoft.Paint_",
        "Editar com o Paint",
        "Microsoft.Paint_8wekyb3d8bbwe!App",
        "mspaint.exe",
    ),
    // No program: the app is activated for the files.
    (
        "Microsoft.Windows.Photos_",
        "Editar com Fotos",
        "Microsoft.Windows.Photos_8wekyb3d8bbwe!App",
        "",
    ),
    (
        "Clipchamp.Clipchamp_",
        "Edite com o Clipchamp",
        "Clipchamp.Clipchamp_yxz26nhyzhsrt!App",
        "",
    ),
    (
        "Microsoft.VisualStudioCode_",
        "Abrir com Code",
        "",
        "Code.exe",
    ),
];

/// A stand-in's program: on the PATH, or (VS Code) in a folder above the package root.
fn fallback_program(program: &str, root: &std::path::Path) -> std::path::PathBuf {
    root.ancestors()
        .take(4)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
        .unwrap_or_else(|| program.into())
}

#[derive(Clone)]
pub struct Command {
    pub clsid: GUID,
    pub title: String,
    pub access_key: String,
    pub icon: Option<Rgba>,
    pub enabled: bool,
    /// One level of subcommands (OneDrive's), invoked by index.
    pub subs: Vec<Command>,
    /// Native EnumSubCommands index; grouped package verbs keep their own CLSID.
    pub subcommand: Option<usize>,
}

const PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

/// The registered verbs, read from the installed packages' manifests once.
fn verbs() -> &'static [Verb] {
    static VERBS: OnceLock<Vec<Verb>> = OnceLock::new();
    VERBS.get_or_init(|| {
        use winreg::enums::HKEY_CURRENT_USER;
        let mut out: Vec<Verb> = Vec::new();
        let Ok(packages) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(PACKAGES) else {
            return out;
        };
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
        for name in packages.enum_keys().flatten() {
            let root: String = packages
                .open_subkey(&name)
                .and_then(|k| k.get_value("PackageRootFolder"))
                .unwrap_or_default();
            // Sparse packages keep their manifest under WindowsApps, not at their root.
            let manifest = [
                std::path::Path::new(&root).join("AppxManifest.xml"),
                std::path::Path::new(&program_files)
                    .join("WindowsApps")
                    .join(&name)
                    .join("AppxManifest.xml"),
            ]
            .into_iter()
            .find_map(|p| std::fs::read_to_string(p).ok());
            let Some(manifest) = manifest else { continue };
            for (kind, clsid) in manifest_verbs(&manifest) {
                let Ok(guid) = GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')) else {
                    continue;
                };
                match out.iter_mut().find(|v| v.clsid == guid) {
                    Some(v) => v.types.push(kind),
                    None => {
                        let dll =
                            class_dll(&name, &guid).map(|d| std::path::Path::new(&root).join(d));
                        let fallback = FALLBACKS.iter().position(|f| name.starts_with(f.0));
                        let program = fallback
                            .map(|f| fallback_program(FALLBACKS[f].3, std::path::Path::new(&root)))
                            .unwrap_or_default();
                        let command_name = manifest
                            .split('<')
                            .find(|tag| {
                                attr(tag, "Clsid").is_some_and(|id| id.eq_ignore_ascii_case(&clsid))
                            })
                            .and_then(|tag| attr(tag, "Id"))
                            .unwrap_or_default();
                        let group = name.starts_with("Microsoft.OneDriveSync_").then(|| {
                            manifest
                                .split("<DisplayName>")
                                .nth(1)
                                .and_then(|s| s.split('<').next())
                                .unwrap_or("OneDrive")
                                .to_string()
                        });
                        out.push(Verb {
                            clsid: guid,
                            name: command_name,
                            group,
                            types: vec![kind],
                            dll,
                            fallback,
                            program,
                            rank: rank(&name, out.len()),
                        });
                    }
                }
            }
        }
        out.sort_by_key(|v| v.rank);
        out
    })
}

/// Explorer's order for the app commands, observed on its menus (it is neither the package,
/// install nor CLSID order): these families first, in this order, then the rest as found.
const ORDER: &[&str] = &[
    "Microsoft.WindowsNotepad_",
    "Microsoft.Copilot_",
    "Microsoft.Windows.Photos_",
    "Clipchamp.Clipchamp_",
    "Microsoft.OneDriveSync_",
    "Microsoft.Paint_",
    "Microsoft.PowerToys.FileLocksmithContextMenu_",
    "Microsoft.PowerToys.ImageResizerContextMenu_",
    "Microsoft.PowerToys.PowerRenameContextMenu_",
    "Microsoft.WindowsTerminal_",
    "Microsoft.VisualStudioCode_",
    "ZedIndustries.Zed_",
];

fn rank(package: &str, found: usize) -> usize {
    ORDER
        .iter()
        .position(|p| package.starts_with(p))
        .unwrap_or(ORDER.len() + found)
}

/// A packaged class's DLL, relative to its package root.
fn class_dll(package: &str, clsid: &GUID) -> Option<String> {
    use winreg::enums::HKEY_CLASSES_ROOT;
    let key = format!(r"PackagedCom\Package\{package}\Class\{{{clsid:?}}}");
    winreg::RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(key)
        .ok()?
        .get_value("DllPath")
        .ok()
}

/// (item type, CLSID) pairs of a manifest's FileExplorerContextMenus sections.
fn manifest_verbs(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("FileExplorerContextMenus>") {
        let section = &rest[start..];
        let end = section[1..]
            .find("FileExplorerContextMenus>")
            .map_or(section.len(), |e| e + 1);
        let mut kind = String::new();
        for tag in section[..end].split('<').skip(1) {
            let tag = tag.split('>').next().unwrap_or("");
            let local = tag
                .split_whitespace()
                .next()
                .unwrap_or("")
                .rsplit(':')
                .next()
                .unwrap_or("");
            match local {
                "ItemType" => kind = attr(tag, "Type").unwrap_or_default().to_lowercase(),
                "Verb" => {
                    if let Some(clsid) = attr(tag, "Clsid") {
                        out.push((kind.clone(), clsid));
                    }
                }
                _ => {}
            }
        }
        rest = &section[end..];
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let at = tag.find(&key)? + key.len();
    Some(tag[at..].split('"').next()?.to_string())
}

enum ItemKind {
    Folder,
    File(Option<String>),
}

fn item_kinds(paths: &[String]) -> Vec<ItemKind> {
    paths
        .iter()
        .map(|p| {
            if ops::is_folder(p) {
                ItemKind::Folder
            } else {
                ItemKind::File(
                    std::path::Path::new(p)
                        .extension()
                        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase())),
                )
            }
        })
        .collect()
}

fn applies(types: &[String], kinds: &[ItemKind], background: bool) -> bool {
    if background {
        return types.iter().any(|t| t == r"directory\background");
    }
    kinds.iter().all(|kind| match kind {
        ItemKind::Folder => types.iter().any(|t| t == "directory"),
        ItemKind::File(ext) => types.iter().any(|t| t == "*" || Some(t) == ext.as_ref()),
    })
}

const ICON_CACHE_LIMIT: usize = 128;

thread_local! {
    static ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<Rgba>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static UNLOADABLE: std::cell::RefCell<std::collections::HashSet<GUID>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

fn cached_icon(key: &str, load: impl FnOnce() -> Option<Rgba>) -> Option<Rgba> {
    if let Some(icon) = ICONS.with(|cache| cache.borrow().get(key).cloned()) {
        return icon;
    }
    let icon = load();
    ICONS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= ICON_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key.to_string(), icon.clone());
    });
    icon
}

fn create_or_stand_in(verb: &Verb, items: &IShellItemArray, kinds: &[ItemKind]) -> Option<Command> {
    let skip = verb.fallback.is_some() && UNLOADABLE.with(|set| set.borrow().contains(&verb.clsid));
    if !skip {
        if let Some(command) = create(verb) {
            return unsafe { describe(&command, verb.clsid, items, 0) };
        }
        if verb.fallback.is_some() {
            UNLOADABLE.with(|set| set.borrow_mut().insert(verb.clsid));
        }
    }
    stand_in(verb, kinds)
}

fn create(verb: &Verb) -> Option<IExplorerCommand> {
    use windows::core::{Interface, HRESULT, PCSTR};
    use windows::Win32::System::Com::IClassFactory;
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };
    unsafe {
        if let Ok(c) = CoCreateInstance(
            &verb.clsid,
            None,
            CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER,
        ) {
            return initialize(c, &verb.name);
        }
        let dll = super::wide(&verb.dll.as_ref()?.to_string_lossy());
        // Handlers inside WindowsApps only load with package identity (access denied).
        let module = LoadLibraryExW(
            windows::core::PCWSTR(dll.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
        .ok()?;
        let entry = GetProcAddress(module, PCSTR(b"DllGetClassObject\0".as_ptr()))?;
        type GetClassObject = unsafe extern "system" fn(
            *const GUID,
            *const GUID,
            *mut *mut core::ffi::c_void,
        ) -> HRESULT;
        let get: GetClassObject = std::mem::transmute(entry);
        let mut factory = std::ptr::null_mut();
        get(&verb.clsid, &IClassFactory::IID, &mut factory)
            .ok()
            .ok()?;
        let factory = IClassFactory::from_raw(factory);
        factory
            .CreateInstance(None)
            .ok()
            .and_then(|c| initialize(c, &verb.name))
    }
}

fn initialize(command: IExplorerCommand, name: &str) -> Option<IExplorerCommand> {
    use windows::{
        core::{Interface, PCWSTR},
        Win32::UI::Shell::IInitializeCommand,
    };
    // The native share flyout dereferences its command state before Initialize.
    // CommandStore verbs require the same initialization that Explorer supplies.
    unsafe {
        if let Ok(init) = command.cast::<IInitializeCommand>() {
            init.Initialize(PCWSTR(super::wide(name).as_ptr()), None)
                .ok()?;
        }
    }
    Some(command)
}

const BUILT_INS: &[&str] = &["Windows.SendToMyPhone", "Windows.ModernShareFlyout"];
fn store_verb(name: &str) -> Option<Verb> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(format!(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\CommandStore\shell\{name}"
        ))
        .ok()?;
    let clsid: String = key.get_value("ExplorerCommandHandler").ok()?;
    Some(Verb {
        clsid: GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')).ok()?,
        name: name.into(),
        group: None,
        types: vec!["*".into()],
        dll: None,
        fallback: None,
        program: Default::default(),
        rank: 0,
    })
}

/// Query a CommandStore command with the hosting browser as its native site.
pub fn store_query(
    name: &str,
    site: Option<windows::core::IUnknown>,
) -> Result<Vec<String>, String> {
    use windows::{core::Interface, Win32::System::Ole::IObjectWithSite};
    com_init();
    unsafe {
        let command = store_verb(name)
            .and_then(|v| create(&v))
            .ok_or("Command unavailable")?;
        if let (Ok(object), Some(site)) = (command.cast::<IObjectWithSite>(), site) {
            object.SetSite(&site).map_err(|e| e.message())?;
        }
        let mut result = vec![format!(
            "title {:?}",
            command.GetTitle(None).map(|p| take_pwstr(p))
        )];
        if let Ok(subs) = command.EnumSubCommands() {
            let mut commands = [None];
            let mut fetched = 0;
            while subs.Next(&mut commands, Some(&mut fetched)).is_ok() && fetched > 0 {
                if let Some(sub) = commands[0].take() {
                    result.push(format!(
                        "{} tooltip {:?} icon {:?}",
                        sub.GetTitle(None)
                            .map(|p| take_pwstr(p))
                            .unwrap_or_default(),
                        sub.GetToolTip(None).map(|p| take_pwstr(p)),
                        sub.GetIcon(None).map(|p| take_pwstr(p))
                    ));
                }
            }
        }
        Ok(result)
    }
}

pub fn store_subcommands(name: &str) -> Vec<(String, Option<Rgba>)> {
    com_init();
    unsafe {
        let Some(command) = store_verb(name).and_then(|v| create(&v)) else {
            return Vec::new();
        };
        let Ok(subs) = command.EnumSubCommands() else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let mut commands = [None];
        let mut fetched = 0;
        while subs.Next(&mut commands, Some(&mut fetched)).is_ok() && fetched > 0 {
            if let Some(command) = commands[0].take() {
                result.push((
                    command
                        .GetTitle(None)
                        .map(|p| take_pwstr(p))
                        .unwrap_or_default(),
                    command
                        .GetIcon(None)
                        .ok()
                        .map(|p| take_pwstr(p))
                        .and_then(|p| small_icon_at(&p)),
                ));
            }
        }
        result
    }
}
pub fn invoke_store_dialog(name: &str) -> Result<(), String> {
    com_init();
    unsafe {
        let command = store_verb(name)
            .and_then(|v| create(&v))
            .ok_or("Command unavailable")?;
        command.Invoke(None, None).map_err(|e| e.message())
    }
}

/// Native file-sharing rows before Favorites; capabilities, text and targets come
/// from CommandStore/IExplorerCommand rather than a fixed list of installed apps.
pub fn sharing_commands(paths: &[String]) -> Vec<Command> {
    com_init();
    if paths.is_empty() || paths.iter().any(|p| ops::is_folder(p)) {
        return Vec::new();
    }
    let Ok(items) = ops::item_array(paths) else {
        return Vec::new();
    };
    BUILT_INS
        .iter()
        .filter_map(|name| {
            let verb = store_verb(name)?;
            let c = create(&verb)?;
            unsafe { describe(&c, verb.clsid, &items, 0) }
        })
        .collect()
}

/// Share targets can finish discovery after the parent menu was constructed.
pub fn refreshed_sharing(clsid: &GUID, paths: &[String]) -> Option<Command> {
    let verb = BUILT_INS
        .iter()
        .filter_map(|name| store_verb(name))
        .find(|v| v.clsid == *clsid)?;
    let items = ops::item_array(paths).ok()?;
    unsafe { describe(&create(&verb)?, verb.clsid, &items, 0) }
}

/// Titles may carry Win32 menu mnemonics ("Open w&ith Z&ed"); "&&" is a literal "&".
fn strip_mnemonics(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut chars = title.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
        } else if chars.peek() == Some(&'&') {
            out.push('&');
            chars.next();
        }
    }
    out
}

unsafe fn describe(
    command: &IExplorerCommand,
    clsid: GUID,
    items: &IShellItemArray,
    depth: u32,
) -> Option<Command> {
    let state = command.GetState(items, true).ok()?;
    if state & ECS_HIDDEN.0 as u32 != 0 {
        return None;
    }
    let raw_title = command
        .GetTitle(items)
        .map(|p| take_pwstr(p))
        .unwrap_or_default();
    let title = strip_mnemonics(&raw_title);
    let mut access_key = String::new();
    let mut chars = raw_title.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            if let Some(next) = chars.next() {
                if next != '&' {
                    access_key = next.to_uppercase().collect();
                }
            }
        }
    }
    if title.is_empty() {
        return None;
    }
    let icon = command
        .GetIcon(items)
        .ok()
        .map(|p| take_pwstr(p))
        .and_then(|loc| cached_icon(&loc, || small_icon_at(&loc)));
    let mut subs = Vec::new();
    let flags = command.GetFlags().map(|f| f as i32).unwrap_or(0);
    if depth == 0 && flags & ECF_HASSUBCOMMANDS.0 != 0 {
        if let Ok(list) = command.EnumSubCommands() {
            loop {
                let mut one = [None];
                let mut fetched = 0u32;
                if list.Next(&mut one, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                if let Some(sub) = one[0].take() {
                    let blank = Command {
                        clsid,
                        title: String::new(),
                        access_key: String::new(),
                        icon: None,
                        enabled: false,
                        subs: Vec::new(),
                        subcommand: None,
                    };
                    let mut child = describe(&sub, clsid, items, 1).unwrap_or(blank);
                    child.subcommand = Some(subs.len());
                    subs.push(child);
                }
            }
        }
        if subs.iter().all(|s| s.title.is_empty()) {
            return None;
        }
    }
    Some(Command {
        clsid,
        title,
        access_key,
        icon,
        enabled: state & ECS_DISABLED.0 as u32 == 0,
        subs,
        subcommand: None,
    })
}

pub fn commands(paths: &[String], background: bool) -> Vec<Command> {
    com_init();
    let Ok(items) = ops::item_array(paths) else {
        return Vec::new();
    };
    let kinds = item_kinds(paths);
    let mut out: Vec<Command> = Vec::new();
    for (verb, command) in verbs()
        .iter()
        .filter(|v| applies(&v.types, &kinds, background))
        .filter_map(|v| create_or_stand_in(v, &items, &kinds).map(|c| (v, c)))
    {
        if let Some(group) = &verb.group {
            if let Some(parent) = out
                .iter_mut()
                .find(|c| c.title == *group && c.clsid == GUID::zeroed())
            {
                parent.subs.push(command);
            } else {
                out.push(Command {
                    clsid: GUID::zeroed(),
                    title: group.clone(),
                    access_key: String::new(),
                    icon: app_icon("Microsoft.OneDriveSync_8wekyb3d8bbwe!OneDrive"),
                    enabled: true,
                    subs: vec![command],
                    subcommand: None,
                });
            }
        } else {
            out.push(command)
        }
    }
    out
}

pub fn diagnostics(paths: &[String]) -> Vec<String> {
    com_init();
    let Ok(items) = ops::item_array(paths) else {
        return Vec::new();
    };
    verbs()
        .iter()
        .filter(|v| applies(&v.types, &item_kinds(paths), false))
        .map(|v| match create(v) {
            Some(c) => unsafe {
                format!(
                    "{:?} dll={:?} title={:?} state={:?} flags={:?} icon={:?}",
                    v.clsid,
                    v.dll,
                    c.GetTitle(&items).map(|p| take_pwstr(p)),
                    c.GetState(&items, true),
                    c.GetFlags(),
                    c.GetIcon(&items).map(|path| take_pwstr(path))
                )
            },
            None => format!("{:?} dll={:?} create failed", v.clsid, v.dll),
        })
        .collect()
}

fn app_icon(app: &str) -> Option<Rgba> {
    let item = format!(r"shell:AppsFolder\{app}");
    cached_icon(&item, || {
        super::icons::small_namespace_icon(&item)
            .or_else(|| super::icons::thumbnail(&item, 16).ok())
    })
}

fn stand_in(verb: &Verb, kinds: &[ItemKind]) -> Option<Command> {
    let (_, title, app, program) = FALLBACKS[verb.fallback?];
    if program == "notepad.exe" && kinds.iter().any(|k| matches!(k, ItemKind::Folder)) {
        return None;
    }
    let icon = if app.is_empty() {
        let program = verb.program.to_string_lossy();
        cached_icon(&program, || small_icon_at(&program))
    } else {
        app_icon(app)
    };
    Some(Command {
        clsid: verb.clsid,
        title: title.to_string(),
        access_key: if program == "wt.exe" {
            "T".into()
        } else {
            String::new()
        },
        icon,
        enabled: true,
        subs: Vec::new(),
        subcommand: None,
    })
}

/// Runs a command (or its `sub`th subcommand) on the items.
pub fn invoke(clsid: &GUID, sub: Option<usize>, paths: &[String]) -> Result<(), String> {
    com_init();
    let items = ops::item_array(paths)?;
    let store = BUILT_INS
        .iter()
        .filter_map(|name| store_verb(name))
        .find(|v| v.clsid == *clsid);
    let verb = verbs()
        .iter()
        .find(|v| v.clsid == *clsid)
        .or(store.as_ref())
        .ok_or("comando indisponível")?;
    let Some(command) = create(verb) else {
        let (_, _, app, program) = FALLBACKS[verb.fallback.ok_or("comando indisponível")?];
        if program.is_empty() {
            return activate_for_files(app, &items);
        }
        for p in paths {
            let mut run = std::process::Command::new(&verb.program);
            if program == "wt.exe" {
                run.arg("-d");
            }
            run.arg(p).spawn().map_err(|e| e.to_string())?;
        }
        return Ok(());
    };
    unsafe {
        let command = match sub {
            None => command,
            Some(index) => {
                let list = command.EnumSubCommands().map_err(|e| e.message())?;
                let mut found = None;
                for _ in 0..=index {
                    let mut one = [None];
                    let mut fetched = 0u32;
                    if list.Next(&mut one, Some(&mut fetched)).is_err() || fetched == 0 {
                        break;
                    }
                    found = one[0].take();
                }
                found.ok_or("subcomando indisponível")?
            }
        };
        command.Invoke(&items, None).map_err(|e| e.message())
    }
}

/// Opens the files in a packaged app, as its file type association would.
fn activate_for_files(
    app: &str,
    items: &windows::Win32::UI::Shell::IShellItemArray,
) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER};
    use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager};
    unsafe {
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .map_err(|e| e.message())?;
        let app = super::wide(app);
        manager
            .ActivateForFile(PCWSTR(app.as_ptr()), items, PCWSTR::null())
            .map(|_| ())
            .map_err(|e| e.message())
    }
}

/// Probe aid: each registered verb's package, item types and CLSID, in discovery order.
pub fn verb_table() -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let mut out = Vec::new();
    let Ok(packages) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(PACKAGES) else {
        return out;
    };
    let program_files =
        std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    for name in packages.enum_keys().flatten() {
        let root: String = packages
            .open_subkey(&name)
            .and_then(|k| k.get_value("PackageRootFolder"))
            .unwrap_or_default();
        let manifest = [
            std::path::Path::new(&root).join("AppxManifest.xml"),
            std::path::Path::new(&program_files)
                .join("WindowsApps")
                .join(&name)
                .join("AppxManifest.xml"),
        ]
        .into_iter()
        .find_map(|p| std::fs::read_to_string(p).ok());
        let Some(manifest) = manifest else { continue };
        for tag in manifest
            .split('<')
            .map(|t| t.split('>').next().unwrap_or(""))
        {
            if tag
                .split_whitespace()
                .next()
                .unwrap_or("")
                .ends_with(":Verb")
            {
                out.push(format!(
                    "{name} | {} | {}",
                    attr(tag, "Id").unwrap_or_default(),
                    attr(tag, "Clsid").unwrap_or_default()
                ));
            }
        }
    }
    out
}
