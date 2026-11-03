use super::{
    journal::*,
    security::{self, error, wide},
    version::{self, Build},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::*,
        Security::*,
        Storage::FileSystem::*,
        System::Registry::{
            RegCreateKeyExW, RegFlushKey, RegGetKeySecurity, HKEY, REG_OPEN_CREATE_OPTIONS,
            REG_SAM_FLAGS,
        },
    },
};
use winreg::{enums::*, RegKey, RegValue};

const CLASS_PARENT: &str = r"Software\Classes\CLSID";
const COMPONENT: &str = "NaviExplorerHost";
pub const RECOVER_ARGUMENT: &str = "--explorer-host-recover-native";
pub fn stop_event(sid: &str, session: u32) -> String {
    format!(r"Global\NaviExplorerHost.Stop.v1.{sid}.{session}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tree {
    values: Vec<(String, Value)>,
    children: Vec<(String, Tree)>,
    security: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    view: View,
    root: String,
    before: Option<Tree>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installation {
    version: u32,
    build: Build,
    journal: MachineJournal,
    snapshots: Vec<Snapshot>,
    missing: Vec<Location>,
    files: Vec<(String, String)>,
    #[serde(default)]
    hives: Vec<(View, String, String)>,
}

pub fn directory() -> io::Result<PathBuf> {
    Ok(security::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles)?.join(COMPONENT))
}
pub fn data_directory() -> io::Result<PathBuf> {
    Ok(security::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramData)?.join(COMPONENT))
}
pub fn broker_path() -> io::Result<PathBuf> {
    Ok(directory()?.join("navi-explorer-host.exe"))
}
pub fn navi_path() -> io::Result<PathBuf> {
    Ok(directory()?.join("navi.exe"))
}

fn pin_broker(path: &Path) -> io::Result<fs::File> {
    security::no_reparse(path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(issue("Explorer host broker is not a file"));
    }
    Ok(file)
}
pub fn draining() -> bool {
    directory()
        .and_then(|path| path.join("draining").try_exists())
        .unwrap_or(true)
}
fn journal_path() -> io::Result<PathBuf> {
    Ok(data_directory()?.join("active.json"))
}
fn flags(view: View) -> u32 {
    match view {
        View::Registry64 => KEY_WOW64_64KEY,
        View::Registry32 => KEY_WOW64_32KEY,
    }
}
fn issue(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct Machine;
impl Machine {
    fn open(&self, view: View, key: &str) -> io::Result<RegKey> {
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(key, KEY_READ | flags(view))
    }
    fn writable(&self, view: View, key: &str) -> io::Result<RegKey> {
        if ![HOST, NATIVE]
            .iter()
            .any(|root| key == *root || key == format!(r"{root}\LocalServer32"))
        {
            return Err(issue("Machine write outside Explorer host scope"));
        }
        if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(key, KEY_READ | KEY_WRITE | DELETE.0 | flags(view))
        {
            return Ok(key);
        }
        let parent = self.open(view, CLASS_PARENT)?;
        let suffix = key
            .strip_prefix(&format!(r"{CLASS_PARENT}\"))
            .ok_or_else(|| issue("Machine class path"))?;
        let mut handle = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY(parent.raw_handle() as _),
                PCWSTR(wide(suffix).as_ptr()),
                None,
                None,
                REG_OPEN_CREATE_OPTIONS(REG_OPTION_BACKUP_RESTORE),
                REG_SAM_FLAGS(0),
                None,
                &mut handle,
                None,
            )
            .ok()
            .map_err(error)?;
            Ok(RegKey::predef(handle.0 as _))
        }
    }
    fn tree(&self, view: View, path: &str) -> io::Result<Option<Tree>> {
        let key = match RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(path, KEY_READ | flags(view) | 0x01000000)
        {
            Ok(key) => key,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut values = key
            .enum_values()
            .map(|item| {
                item.map(|(name, value)| {
                    (
                        name,
                        Value {
                            kind: value.vtype as u32,
                            bytes: value.bytes,
                        },
                    )
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        values.sort_by(|left, right| left.0.cmp(&right.0));
        let mut names = key.enum_keys().collect::<io::Result<Vec<_>>>()?;
        names.sort();
        let children = names
            .into_iter()
            .map(|name| {
                self.tree(view, &format!(r"{path}\{name}"))
                    .map(|tree| (name, tree.unwrap()))
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Some(Tree {
            values,
            children,
            security: key_security(&key)?,
        }))
    }
    fn remove_empty(&self, location: &Location) -> io::Result<()> {
        let key = match self.open(location.view, &location.key) {
            Ok(key) => key,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let info = key.query_info()?;
        if info.values != 0 || info.sub_keys != 0 {
            return Err(issue("Foreign values in created Explorer class"));
        }
        drop(key);
        let key = self.writable(location.view, &location.key)?;
        unsafe { NtDeleteKey(key.raw_handle() as _).ok().map_err(error) }
    }
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtDeleteKey(key: *mut std::ffi::c_void) -> NTSTATUS;
}

fn key_security(key: &RegKey) -> io::Result<Vec<u8>> {
    unsafe {
        let mut size = 0;
        let result = RegGetKeySecurity(
            HKEY(key.raw_handle() as _),
            OWNER_SECURITY_INFORMATION
                | GROUP_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | SACL_SECURITY_INFORMATION,
            None,
            &mut size,
        );
        if result != ERROR_INSUFFICIENT_BUFFER && result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result.0 as _));
        }
        let mut bytes = vec![0; size as usize];
        RegGetKeySecurity(
            HKEY(key.raw_handle() as _),
            OWNER_SECURITY_INFORMATION
                | GROUP_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | SACL_SECURITY_INFORMATION,
            Some(PSECURITY_DESCRIPTOR(bytes.as_mut_ptr().cast())),
            &mut size,
        )
        .ok()
        .map_err(error)?;
        Ok(bytes)
    }
}
impl Registry for Machine {
    fn read(&self, location: &Location) -> io::Result<Option<Value>> {
        let key = match self.open(location.view, &location.key) {
            Ok(key) => key,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        match key.get_raw_value(&location.name) {
            Ok(value) => Ok(Some(Value {
                kind: value.vtype as u32,
                bytes: value.bytes,
            })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn write(&mut self, location: &Location, value: Option<&Value>) -> io::Result<()> {
        let key = self.writable(location.view, &location.key)?;
        if let Some(value) = value {
            let kind = (0..=11)
                .find(|kind| *kind == value.kind)
                .ok_or_else(|| issue("Registry type"))?;
            let vtype = match kind {
                0 => REG_NONE,
                1 => REG_SZ,
                2 => REG_EXPAND_SZ,
                3 => REG_BINARY,
                4 => REG_DWORD,
                5 => REG_DWORD_BIG_ENDIAN,
                6 => REG_LINK,
                7 => REG_MULTI_SZ,
                8 => REG_RESOURCE_LIST,
                9 => REG_FULL_RESOURCE_DESCRIPTOR,
                10 => REG_RESOURCE_REQUIREMENTS_LIST,
                _ => REG_QWORD,
            };
            key.set_raw_value(
                &location.name,
                &RegValue {
                    vtype,
                    bytes: value.bytes.clone(),
                },
            )?;
        } else {
            match key.delete_value(&location.name) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(()) => {}
            }
        }
        unsafe { RegFlushKey(HKEY(key.raw_handle() as _)).ok().map_err(error) }
    }
}
fn string(value: &str) -> Value {
    Value {
        kind: 1,
        bytes: value
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect(),
    }
}
fn text(value: &Value) -> io::Result<String> {
    if !matches!(value.kind, 1 | 2) || value.bytes.len() % 2 != 0 {
        return Err(issue("Invalid native command type"));
    }
    let units = value
        .bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units)
        .map(|text| text.trim_end_matches('\0').to_owned())
        .map_err(io::Error::other)
}
fn native_command(value: &Value) -> io::Result<Value> {
    let original = text(value)?;
    let windows = security::system_directory()?
        .parent()
        .ok_or_else(|| issue("Windows directory"))?
        .to_path_buf();
    let candidates = [
        windows.join("explorer.exe").to_string_lossy().into_owned(),
        windows
            .join("SysWow64")
            .join("explorer.exe")
            .to_string_lossy()
            .into_owned(),
        r"%SystemRoot%\explorer.exe".into(),
        r"%SystemRoot%\SysWow64\explorer.exe".into(),
    ];
    let guid = "{75dff2b7-6936-4c06-a8bb-676a7b00b24b}";
    if !candidates.iter().any(|path| {
        original.eq_ignore_ascii_case(&format!("{path} /factory,{guid}"))
            && (!path.starts_with('%') || value.kind == 2)
    }) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Another manager owns Explorer registration",
        ));
    }
    let offset = original
        .to_ascii_lowercase()
        .find(guid)
        .ok_or_else(|| issue("Native factory GUID"))?;
    let mut result = string(&format!(
        "{}{{D7DFB84A-C031-49BC-AD7C-5064810A2E74}}{}",
        &original[..offset],
        &original[offset + guid.len()..]
    ));
    result.kind = value.kind;
    Ok(result)
}
fn command() -> io::Result<String> {
    Ok(format!("\"{}\" --serve", broker_path()?.display()))
}
pub fn registered() -> bool {
    let Ok(expected) = command() else {
        return false;
    };
    [View::Registry64, View::Registry32]
        .into_iter()
        .all(|view| {
            Machine
                .read(&Location {
                    view,
                    key: format!(r"{HOST}\LocalServer32"),
                    name: String::new(),
                })
                .ok()
                .flatten()
                == Some(string(&expected))
        })
}
pub fn startup_recovery_needed() -> io::Result<bool> {
    if !super::opted_in() {
        return Ok(false);
    }
    needs_startup_recovery(true, &broker_path()?, &Machine)
}
fn needs_startup_recovery(
    opted_in: bool,
    broker: &Path,
    registry: &impl Registry,
) -> io::Result<bool> {
    if !opted_in {
        return Ok(false);
    }
    match fs::metadata(broker) {
        Ok(metadata) if metadata.is_file() => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let expected = string(&format!("\"{}\" --serve", broker.display()));
    for view in [View::Registry64, View::Registry32] {
        if registry.read(&Location {
            view,
            key: format!(r"{HOST}\LocalServer32"),
            name: String::new(),
        })? == Some(expected.clone())
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn recover_from_startup() -> io::Result<()> {
    let _privileges = security::Privileges::acquire()?;
    recover_native_if_unhealthy()
}
fn save(path: &Path, installation: &Installation) -> io::Result<()> {
    let root = data_directory()?;
    security::verify_directory(&root, false)?;
    if path.parent() != Some(root.as_path()) {
        return Err(issue("Journal path escaped"));
    }
    let bytes = serde_json::to_vec_pretty(installation)?;
    let temp = root.join(format!("journal-{}.tmp", std::process::id()));
    let mut file = security::new_file(&temp, false)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    unsafe {
        MoveFileExW(
            PCWSTR(wide(&temp.to_string_lossy()).as_ptr()),
            PCWSTR(wide(&path.to_string_lossy()).as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(error)?;
    }
    Ok(())
}
fn load() -> io::Result<Installation> {
    let root = data_directory()?;
    security::verify_directory(&root, false)?;
    let path = journal_path()?;
    security::no_reparse(&path)?;
    security::verify_directory(&path, false)?;
    if fs::metadata(&path)?.len() > 8_388_608 {
        return Err(issue("Oversized journal"));
    }
    let installation: Installation = serde_json::from_slice(&fs::read(path)?)?;
    installation.validate()?;
    Ok(installation)
}
impl Installation {
    fn validate(&self) -> io::Result<()> {
        self.journal.validate()?;
        if !matches!(self.version, 2 | 3 | 4) || self.snapshots.len() != 4 {
            return Err(issue("Invalid protected journal"));
        }
        for (index, view) in [View::Registry64, View::Registry32].into_iter().enumerate() {
            for (offset, root) in [HOST, NATIVE].into_iter().enumerate() {
                let snapshot = &self.snapshots[index * 2 + offset];
                if snapshot.view != view || snapshot.root != root {
                    return Err(issue("Invalid snapshot scope"));
                }
            }
            if self.journal.records[index * 3].after != string(&command()?) {
                return Err(issue("Foreign broker command"));
            }
        }
        for location in &self.missing {
            if ![HOST, NATIVE].iter().any(|root| {
                location.key == *root || location.key == format!(r"{root}\LocalServer32")
            }) {
                return Err(issue("Created key outside scope"));
            }
        }
        let expected = if self.version >= 3 {
            vec![
                "navi-explorer-host.exe",
                "navi.exe",
                "navi-explorer-guard.exe",
            ]
        } else {
            vec!["navi-explorer-host.exe", "navi.exe"]
        };
        if self
            .files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            != expected
        {
            return Err(issue("Invalid package file list"));
        }
        if self.version == 4 {
            if self.hives.len() != 2 {
                return Err(issue("Missing original registry hives"));
            }
            for ((view, name, hash), expected) in
                self.hives.iter().zip([View::Registry64, View::Registry32])
            {
                if !valid_hive_identity(*view, name, hash, expected) {
                    return Err(issue("Invalid original hive identity"));
                }
            }
        } else if !self.hives.is_empty() {
            return Err(issue("Unexpected hive journal"));
        }
        Ok(())
    }
    fn verify_security(&self) -> io::Result<()> {
        for snapshot in &self.snapshots {
            if let Some(before) = &snapshot.before {
                verify_tree_security(snapshot.view, &snapshot.root, before)?;
            }
        }
        Ok(())
    }
    fn preflight(&self) -> io::Result<()> {
        self.verify_security()?;
        for snapshot in &self.snapshots {
            let current = Machine.tree(snapshot.view, &snapshot.root)?;
            if !owned_tree(snapshot, current, &self.journal.records) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Foreign registry tree changes; nothing was overwritten",
                ));
            }
        }
        Ok(())
    }
}
fn owned_tree(snapshot: &Snapshot, current: Option<Tree>, records: &[Record]) -> bool {
    let Some(mut current) = current else {
        return snapshot.before.is_none();
    };
    let relevant = records
        .iter()
        .filter(|record| {
            record.location.view == snapshot.view
                && (record.location.key == snapshot.root
                    || record
                        .location
                        .key
                        .starts_with(&format!("{}\\", snapshot.root)))
        })
        .collect::<Vec<_>>();
    if let Some(before) = &snapshot.before {
        for record in relevant {
            let mut node = &mut current;
            let relative = record
                .location
                .key
                .strip_prefix(&snapshot.root)
                .unwrap()
                .trim_start_matches('\\');
            for part in relative.split('\\').filter(|part| !part.is_empty()) {
                let Some((_, child)) = node
                    .children
                    .iter_mut()
                    .find(|(name, _)| name.eq_ignore_ascii_case(part))
                else {
                    return false;
                };
                node = child;
            }
            node.values
                .retain(|(name, _)| !name.eq_ignore_ascii_case(&record.location.name));
            if let Some(value) = &record.before {
                node.values
                    .push((record.location.name.clone(), value.clone()));
            }
            node.values.sort_by(|left, right| left.0.cmp(&right.0));
        }
        current == *before
    } else {
        for (name, value) in &current.values {
            if !relevant.iter().any(|record| {
                record.location.key == snapshot.root
                    && record.location.name == *name
                    && record.after == *value
            }) {
                return false;
            }
        }
        current.children.iter().all(|(name, child)| {
            name == "LocalServer32"
                && child.children.is_empty()
                && child.values.iter().all(|(name, value)| {
                    name.is_empty()
                        && relevant.iter().any(|record| {
                            record.location.key.ends_with(r"\LocalServer32")
                                && record.after == *value
                        })
                })
        })
    }
}
fn verify_tree_security(view: View, path: &str, before: &Tree) -> io::Result<()> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(path, KEY_READ | flags(view) | 0x01000000)?;
    if key_security(&key)? != before.security {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Registry owner or ACL changed externally",
        ));
    }
    for (name, child) in &before.children {
        verify_tree_security(view, &format!(r"{path}\{name}"), child)?;
    }
    Ok(())
}
pub fn install(source: &Path) -> io::Result<()> {
    if journal_path()?.exists() {
        return repair();
    }
    let build = Build::current()?;
    build.require()?;
    let _privileges = security::Privileges::acquire()?;
    let _lock = InstallationLock::acquire()?;
    security::no_reparse(source)?;
    if version::digest(&source.join("navi-explorer-host.exe"))?
        != version::digest(&std::env::current_exe()?)?
    {
        return Err(issue("Helper/source binary identity mismatch"));
    }
    if journal_path()?.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Use explicit repair for the existing journal",
        ));
    }
    super::guardian::require_absent()?;
    let verified_files = [
        "navi-explorer-host.exe",
        "navi.exe",
        "navi-explorer-guard.exe",
    ]
    .into_iter()
    .map(|name| {
        let path = source.join(name);
        security::no_reparse(&path)?;
        version::digest(&path).map(|hash| (name, hash))
    })
    .collect::<io::Result<Vec<_>>>()?;
    let mut records = Vec::new();
    let mut snapshots = Vec::new();
    let mut original_appid = Vec::new();
    let mut missing = Vec::new();
    for view in [View::Registry64, View::Registry32] {
        for root in [HOST, NATIVE] {
            snapshots.push(Snapshot {
                view,
                root: root.into(),
                before: Machine.tree(view, root)?,
            });
        }
        if snapshots.last().unwrap().before.is_some() {
            return Err(issue("Native alias already exists"));
        }
        if Machine.open(view, &format!(r"{HOST}\TreatAs")).is_ok() {
            return Err(issue("Foreign TreatAs registration"));
        }
        let app = Machine
            .read(&Location {
                view,
                key: HOST.into(),
                name: "AppID".into(),
            })?
            .ok_or_else(|| issue("Missing native AppID"))?;
        original_appid.push(app.clone());
        let main = Location {
            view,
            key: format!(r"{HOST}\LocalServer32"),
            name: String::new(),
        };
        let before = Machine
            .read(&main)?
            .ok_or_else(|| issue("Missing native command"))?;
        let alias_command = native_command(&before)?;
        for (location, after) in [
            (main, string(&command()?)),
            (
                Location {
                    view,
                    key: format!(r"{NATIVE}\LocalServer32"),
                    name: String::new(),
                },
                alias_command,
            ),
            (
                Location {
                    view,
                    key: NATIVE.into(),
                    name: "AppID".into(),
                },
                app,
            ),
        ] {
            records.push(Record {
                before: Machine.read(&location)?,
                location,
                after,
            });
        }
        missing.extend([
            Location {
                view,
                key: format!(r"{NATIVE}\LocalServer32"),
                name: String::new(),
            },
            Location {
                view,
                key: NATIVE.into(),
                name: String::new(),
            },
        ]);
    }
    let package = directory()?;
    let data = data_directory()?;
    security::create_directory(&package, true)?;
    security::create_directory(&data, false)?;
    let files = verified_files
        .into_iter()
        .map(|(name, hash)| {
            let src = source.join(name);
            security::no_reparse(&src)?;
            let dest = package.join(name);
            if dest.exists() {
                security::verify_directory(&dest, true)?;
                if version::digest(&dest)? != hash {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Existing component binary",
                    ));
                }
            } else {
                let mut output = security::new_file(&dest, true)?;
                std::io::copy(&mut fs::File::open(&src)?, &mut output)?;
                output.sync_all()?;
                drop(output);
            }
            if version::digest(&dest)? != hash {
                return Err(issue("Copied binary changed"));
            }
            Ok((name.into(), hash))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let _broker = pin_broker(&broker_path()?)?;
    let hives = save_original_hives()?;
    let installation = Installation {
        version: 4,
        build,
        journal: MachineJournal {
            version: 1,
            original_appid: [original_appid.remove(0), original_appid.remove(0)],
            records,
        },
        snapshots,
        missing,
        files,
        hives,
    };
    installation.validate()?;
    installation.preflight()?;
    save(&journal_path()?, &installation)?;
    reset_stop()?;
    if let Err(error) = super::guardian::register()
        .and_then(|_| installation.journal.apply(&mut Machine))
        .and_then(|_| installation.verify_security())
    {
        let _ = super::guardian::unregister();
        let _ = restore(&installation);
        return Err(error);
    }
    Ok(())
}
pub fn repair() -> io::Result<()> {
    Build::current()?.require()?;
    let _privileges = security::Privileges::acquire()?;
    let _lock = InstallationLock::acquire()?;
    let installation = load()?;
    installation.preflight()?;
    for (name, hash) in &installation.files {
        if version::digest(&directory()?.join(name))? != *hash {
            return Err(issue("Installed binary missing or changed"));
        }
    }
    let _broker = pin_broker(&broker_path()?)?;
    super::guardian::ensure_running()?;
    installation.journal.repair(&mut Machine)?;
    installation.verify_security()?;
    let barrier = directory()?.join("draining");
    if barrier.exists() {
        fs::remove_file(barrier)?;
    }
    reset_stop()
}
fn restore(installation: &Installation) -> io::Result<()> {
    installation.preflight()?;
    installation.journal.restore(&mut Machine)?;
    let mut missing = installation.missing.iter().collect::<Vec<_>>();
    missing.sort_by_key(|location| std::cmp::Reverse(location.key.len()));
    for location in missing {
        Machine.remove_empty(location)?;
    }
    for snapshot in &installation.snapshots {
        if Machine.tree(snapshot.view, &snapshot.root)? != snapshot.before {
            return Err(issue(
                "Complete registry tree/security differs after uninstall",
            ));
        }
    }
    Ok(())
}
fn valid_hive_identity(view: View, name: &str, hash: &str, expected: View) -> bool {
    view == expected
        && name.starts_with("host-original-")
        && name.ends_with(".hiv")
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.'))
        && hash.len() == 64
        && hash.bytes().all(|c| c.is_ascii_hexdigit())
}
fn save_original_hives() -> io::Result<Vec<(View, String, String)>> {
    use windows::Win32::System::Registry::{RegSaveKeyExW, REG_LATEST_FORMAT};
    let descriptor = security::Descriptor::new("O:BAG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)")?;
    let mut hives = Vec::new();
    for (view, suffix) in [(View::Registry64, "64"), (View::Registry32, "32")] {
        let name = format!("host-original-{}-{suffix}.hiv", super::clock());
        let path = data_directory()?.join(&name);
        security::no_reparse(&path)?;
        if path.exists() {
            return Err(issue("Original hive backup already exists"));
        }
        let key = Machine.writable(view, HOST)?;
        unsafe {
            RegSaveKeyExW(
                HKEY(key.raw_handle() as _),
                PCWSTR(wide(&path.to_string_lossy()).as_ptr()),
                Some(&descriptor.attributes()),
                REG_LATEST_FORMAT,
            )
            .ok()
            .map_err(error)?;
        }
        security::verify_directory(&path, false)?;
        hives.push((view, name, version::digest(&path)?));
    }
    Ok(hives)
}
fn restore_original_hive(installation: &Installation, view: View) -> io::Result<()> {
    use windows::Win32::System::Registry::{RegRestoreKeyW, REG_FORCE_RESTORE};
    let (_, name, expected) = installation
        .hives
        .iter()
        .find(|(saved, _, _)| *saved == view)
        .ok_or_else(|| issue("Original hive backup unavailable; manual repair required"))?;
    let path = data_directory()?.join(name);
    security::no_reparse(&path)?;
    security::verify_directory(&path, false)?;
    if version::digest(&path)? != *expected {
        return Err(issue("Original hive backup changed"));
    }
    let key = Machine.writable(view, HOST)?;
    unsafe {
        RegRestoreKeyW(
            HKEY(key.raw_handle() as _),
            PCWSTR(wide(&path.to_string_lossy()).as_ptr()),
            REG_FORCE_RESTORE,
        )
        .ok()
        .map_err(error)?;
    }
    let before = &installation
        .snapshots
        .iter()
        .find(|snapshot| snapshot.view == view && snapshot.root == HOST)
        .ok_or_else(|| issue("Missing original host snapshot"))?
        .before;
    if Machine.tree(view, HOST)? != *before {
        return Err(issue("Original hive restoration differs"));
    }
    Ok(())
}
pub(crate) fn recover_native_if_unhealthy() -> io::Result<()> {
    let _lock = InstallationLock::acquire()?;
    let native_only = draining();
    let installation = load()?;
    let binary_ok = installation
        .files
        .iter()
        .filter(|(name, _)| name == "navi-explorer-host.exe")
        .all(|(name, digest)| {
            directory()
                .and_then(|path| version::digest(&path.join(name)))
                .is_ok_and(|actual| actual == *digest)
        });
    let mut restore_host = Vec::new();
    for record in installation
        .journal
        .records
        .iter()
        .filter(|record| record.location.key.starts_with(HOST))
    {
        let current = Machine.read(&record.location)?;
        if current == record.before {
            continue;
        }
        if current.as_ref() == Some(&record.after) && binary_ok && !native_only {
            continue;
        }
        if recovery_owns_value(current.as_ref(), &record.after) {
            restore_host.push(record);
        }
    }
    if restore_host.is_empty() {
        return Ok(());
    }
    for record in &restore_host {
        let snapshot = installation
            .snapshots
            .iter()
            .find(|snapshot| snapshot.view == record.location.view && snapshot.root == HOST)
            .ok_or_else(|| issue("Missing host snapshot"))?;
        let current = Machine.tree(snapshot.view, HOST)?;
        let mut adjusted = current
            .clone()
            .or_else(|| snapshot.before.clone())
            .ok_or_else(|| issue("Missing original host class"))?;
        if !adjusted
            .children
            .iter()
            .any(|(name, _)| name == "LocalServer32")
        {
            let before = snapshot
                .before
                .as_ref()
                .and_then(|tree| {
                    tree.children
                        .iter()
                        .find(|(name, _)| name == "LocalServer32")
                })
                .ok_or_else(|| issue("Missing native command snapshot"))?;
            adjusted.children.push(before.clone());
            adjusted
                .children
                .sort_by(|left, right| left.0.cmp(&right.0));
        }
        if !owned_tree(snapshot, Some(adjusted), &installation.journal.records) {
            return Err(issue(
                "Foreign class edits; recovery did not overwrite them",
            ));
        }
    }
    let barrier = directory()?.join("draining");
    if !barrier.exists() {
        security::new_file(&barrier, true)?.sync_all()?;
    }
    for record in restore_host {
        if Machine.tree(record.location.view, HOST)?.is_none()
            || Machine
                .tree(record.location.view, &record.location.key)?
                .is_none()
        {
            restore_original_hive(&installation, record.location.view)?;
            continue;
        }
        Machine.write(&record.location, record.before.as_ref())?;
        let snapshot = installation
            .snapshots
            .iter()
            .find(|snapshot| snapshot.view == record.location.view && snapshot.root == HOST)
            .unwrap();
        let before = &snapshot
            .before
            .as_ref()
            .unwrap()
            .children
            .iter()
            .find(|(name, _)| name == "LocalServer32")
            .unwrap()
            .1;
        if !before.children.is_empty() || before.values.len() != 1 {
            return Err(issue("Unexpected original native command layout"));
        }
        verify_tree_security(record.location.view, &record.location.key, before)?;
    }
    signal_stop()
}
fn recovery_owns_value(current: Option<&Value>, installed: &Value) -> bool {
    current == Some(installed)
        || current
            .and_then(|value| text(value).ok())
            .and_then(|command| super::guardian::command_target(&command))
            .is_none()
}
fn active_brokers() -> io::Result<bool> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let executable = broker_path()?;
    unsafe {
        let snapshot = super::transport::Owned(
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(error)?,
        );
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as _,
            ..Default::default()
        };
        let mut next = Process32FirstW(snapshot.0, &mut entry);
        while next.is_ok() {
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if entry.th32ProcessID != std::process::id()
                && String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("navi-explorer-host.exe")
            {
                match super::transport::image(entry.th32ProcessID) {
                    Ok(path) if path == executable => return Ok(true),
                    Err(cause) if cause.code() == E_ACCESSDENIED => return Ok(true),
                    _ => {}
                }
            }
            next = Process32NextW(snapshot.0, &mut entry);
        }
    }
    Ok(false)
}
fn archive_runtime(path: &Path, name: &str, digest: &str) -> io::Result<()> {
    let root = data_directory()?;
    security::verify_directory(&root, false)?;
    let stamp = format!("{}-{}", super::clock(), std::process::id());
    let destination = root.join(format!("runtime-retired-{stamp}-{name}.bin"));
    security::no_reparse(&destination)?;
    if destination.exists() {
        return Err(issue("Retired runtime destination already exists"));
    }
    let record =
        serde_json::json!({"version":1,"source":path,"destination":destination,"sha256":digest});
    let mut journal =
        security::new_file(&root.join(format!("runtime-retired-{stamp}.json")), false)?;
    journal.write_all(&serde_json::to_vec_pretty(&record)?)?;
    journal.sync_all()?;
    fs::rename(path, &destination)?;
    if version::digest(&destination)? != digest {
        return Err(issue("Retired runtime differs from its protected journal"));
    }
    Ok(())
}
pub fn uninstall() -> io::Result<()> {
    let _privileges = security::Privileges::acquire()?;
    let _lock = InstallationLock::acquire()?;
    let installation = load()?;
    let package = directory()?;
    security::no_reparse(&package)?;
    installation.preflight()?;
    let barrier = package.join("draining");
    if !barrier.exists() {
        security::new_file(&barrier, true)?.sync_all()?;
    }
    for record in installation
        .journal
        .records
        .iter()
        .filter(|record| record.location.key.starts_with(HOST))
    {
        Machine.write(&record.location, record.before.as_ref())?;
    }
    signal_stop()?;
    let deadline = super::clock() + 3000;
    while super::clock() < deadline && active_brokers()? {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if active_brokers()? {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Explorer host requests are still draining; native routing is restored, retry uninstall",
        ));
    }
    restore(&installation)?;
    if installation.version >= 3 {
        super::guardian::unregister()?;
        let guard = directory()?.join("navi-explorer-guard.exe");
        let deadline = super::clock() + 500;
        while guard.exists()
            && fs::OpenOptions::new().write(true).open(&guard).is_err()
            && super::clock() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let root = data_directory()?;
    fs::rename(
        journal_path()?,
        root.join(format!("restored-{}.json", super::clock())),
    )?;
    fs::remove_file(barrier)?;
    for (name, hash) in &installation.files {
        let path = package.join(name);
        if path.exists() && version::digest(&path)? == *hash {
            match fs::remove_file(&path) {
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    archive_runtime(&path, name, hash)?;
                }
                result => result?,
            }
        }
    }
    match fs::read_dir(&package) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                fs::remove_dir(package)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_recovery_requires_opt_in_missing_broker_and_an_owned_view() {
        struct Probe {
            values: [Option<Value>; 2],
            reads: std::cell::Cell<usize>,
            fail: bool,
        }
        impl Registry for Probe {
            fn read(&self, location: &Location) -> io::Result<Option<Value>> {
                self.reads.set(self.reads.get() + 1);
                assert_eq!(location.key, format!(r"{HOST}\LocalServer32"));
                assert!(location.name.is_empty());
                if self.fail {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                Ok(self.values[usize::from(location.view == View::Registry32)].clone())
            }
            fn write(&mut self, _: &Location, _: Option<&Value>) -> io::Result<()> {
                panic!("Startup detection must never write registry values")
            }
        }
        let root = std::env::temp_dir().join(format!(
            "NaviStartupRecoveryTests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let broker = root.join("broker.bin");
        let ours = Some(string(&format!("\"{}\" --serve", broker.display())));
        let foreign = Some(string(r#""C:\OtherManager\manager.exe" --serve"#));
        for present in [true, false] {
            if present {
                fs::write(&broker, b"fixture").unwrap();
            } else {
                fs::remove_file(&broker).unwrap();
            }
            for opted_in in [false, true] {
                for values in [
                    [None, None],
                    [foreign.clone(), foreign.clone()],
                    [ours.clone(), foreign.clone()],
                    [None, ours.clone()],
                    [ours.clone(), ours.clone()],
                ] {
                    let claimed = values.iter().any(|value| value == &ours);
                    let registry = Probe {
                        values,
                        reads: std::cell::Cell::new(0),
                        fail: false,
                    };
                    assert_eq!(
                        needs_startup_recovery(opted_in, &broker, &registry).unwrap(),
                        opted_in && !present && claimed
                    );
                    if !opted_in || present {
                        assert_eq!(registry.reads.get(), 0);
                    }
                }
            }
        }
        let denied = Probe {
            values: [ours.clone(), ours],
            reads: std::cell::Cell::new(0),
            fail: true,
        };
        assert_eq!(
            needs_startup_recovery(true, &broker, &denied)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn absent_broker_is_rejected_and_present_broker_cannot_disappear_while_arming() {
        let root = std::env::temp_dir().join(format!(
            "NaviBrokerPinTests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("broker.bin");
        assert_eq!(
            pin_broker(&path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(pin_broker(&root).is_err());
        fs::write(&path, b"fixture").unwrap();
        let pin = pin_broker(&path).unwrap();
        let renamed = root.join("renamed.bin");
        assert!(fs::rename(&path, &renamed).is_err());
        assert!(fs::remove_file(&path).is_err());
        assert!(path.is_file());
        drop(pin);
        fs::rename(&path, &renamed).unwrap();
        fs::remove_file(&renamed).unwrap();
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn hive_journal_rejects_path_escape_wrong_view_and_invalid_digest() {
        let hash = "A1".repeat(32);
        let view = View::Registry64;
        assert!(valid_hive_identity(
            view,
            "host-original-123-64.hiv",
            &hash,
            view
        ));
        for name in [
            "../host-original-123.hiv",
            "host-original-../bad.hiv",
            "C:\\host-original.hiv",
            "host-original-123.hiv:stream",
        ] {
            assert!(!valid_hive_identity(view, name, &hash, view));
        }
        assert!(!valid_hive_identity(
            view,
            "host-original-123-64.hiv",
            &hash,
            View::Registry32
        ));
        assert!(!valid_hive_identity(
            view,
            "host-original-123-64.hiv",
            "bad",
            view
        ));
    }
    #[test]
    fn recovery_restores_only_owned_missing_or_malformed_commands() {
        let ours = string(r#""C:\Program Files\NaviExplorerHost\navi-explorer-host.exe" --serve"#);
        assert!(recovery_owns_value(Some(&ours), &ours));
        assert!(recovery_owns_value(None, &ours));
        assert!(recovery_owns_value(Some(&string("broken")), &ours));
        assert!(!recovery_owns_value(
            Some(&string(r#""C:\OtherManager\manager.exe" --open"#)),
            &ours,
        ));
    }
    #[test]
    fn unrelated_tree_edits_are_detected_before_any_registry_mutation() {
        let original = Tree {
            values: vec![("AppID".into(), string("native"))],
            children: vec![(
                "LocalServer32".into(),
                Tree {
                    values: vec![("".into(), string("original"))],
                    children: vec![],
                    security: vec![2],
                },
            )],
            security: vec![1],
        };
        let record = Record {
            location: Location {
                view: View::Registry64,
                key: format!(r"{HOST}\LocalServer32"),
                name: "".into(),
            },
            before: Some(string("original")),
            after: string("ours"),
        };
        let snapshot = Snapshot {
            view: View::Registry64,
            root: HOST.into(),
            before: Some(original.clone()),
        };
        let mut current = original.clone();
        current.children[0].1.values[0].1 = string("ours");
        assert!(owned_tree(
            &snapshot,
            Some(current.clone()),
            &[record.clone()]
        ));
        current.values.push(("OtherManager".into(), string("keep")));
        assert!(!owned_tree(&snapshot, Some(current), &[record]));
    }
    #[test]
    fn created_alias_rejects_foreign_children_or_values() {
        let snapshot = Snapshot {
            view: View::Registry64,
            root: NATIVE.into(),
            before: None,
        };
        let record = Record {
            location: Location {
                view: View::Registry64,
                key: NATIVE.into(),
                name: "AppID".into(),
            },
            before: None,
            after: string("native"),
        };
        let mut current = Tree {
            values: vec![("AppID".into(), string("native"))],
            children: vec![],
            security: vec![1],
        };
        assert!(owned_tree(
            &snapshot,
            Some(current.clone()),
            &[record.clone()]
        ));
        current.children.push(("foreign".into(), current.clone()));
        assert!(!owned_tree(&snapshot, Some(current), &[record]));
    }
    #[test]
    fn native_alias_preserves_expand_strings_and_refuses_foreign_managers() {
        for path in [
            r"%SystemRoot%\explorer.exe",
            r"%SystemRoot%\SysWow64\explorer.exe",
        ] {
            let mut before = string(&format!(
                "{path} /factory,{{75dff2b7-6936-4c06-a8bb-676a7b00b24b}}"
            ));
            before.kind = 2;
            let after = native_command(&before).unwrap();
            assert_eq!(after.kind, 2);
            assert!(text(&after).unwrap().starts_with(path));
        }
        assert!(native_command(&string(r"C:\OtherManager\manager.exe")).is_err());
    }
}

struct InstallationLock(super::transport::Owned);
impl InstallationLock {
    fn acquire() -> io::Result<Self> {
        unsafe {
            use windows::Win32::System::Threading::*;
            let descriptor = security::Descriptor::new("O:BAG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)")?;
            let handle = super::transport::Owned(
                CreateMutexW(
                    Some(&descriptor.attributes()),
                    false,
                    PCWSTR(wide(r"Global\NaviExplorerHost.Install.v1").as_ptr()),
                )
                .map_err(error)?,
            );
            if !matches!(
                WaitForSingleObject(handle.0, 5000),
                WAIT_OBJECT_0 | WAIT_ABANDONED
            ) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Another installation is active",
                ));
            }
            Ok(Self(handle))
        }
    }
}
impl Drop for InstallationLock {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(self.0 .0);
        }
    }
}
fn reset_stop() -> io::Result<()> {
    unsafe {
        use windows::Win32::System::Threading::*;
        let identity = super::transport::identity(std::process::id()).map_err(error)?;
        if let Ok(event) = OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            PCWSTR(wide(&stop_event(&identity.sid, identity.session)).as_ptr()),
        ) {
            let event = super::transport::Owned(event);
            ResetEvent(event.0).map_err(error)?;
        }
        Ok(())
    }
}
fn signal_stop() -> io::Result<()> {
    unsafe {
        use windows::Win32::System::Threading::*;
        let identity = super::transport::identity(std::process::id()).map_err(error)?;
        let mut targets = vec![(identity.sid, identity.session)];
        for sid in RegKey::predef(HKEY_USERS)
            .enum_keys()
            .flatten()
            .filter(|sid| sid.starts_with("S-1-5-21-") && !sid.ends_with("_Classes"))
        {
            if let Ok(key) = RegKey::predef(HKEY_USERS).open_subkey(format!(r"{sid}\Software\Navi"))
            {
                if let Ok(session) = key.get_value::<u32, _>("ExplorerHostSession") {
                    targets.push((sid, session));
                }
            }
        }
        for (sid, session) in targets {
            if let Ok(event) = OpenEventW(
                EVENT_MODIFY_STATE,
                false,
                PCWSTR(wide(&stop_event(&sid, session)).as_ptr()),
            ) {
                let event = super::transport::Owned(event);
                SetEvent(event.0).map_err(error)?;
            }
        }
    }
    Ok(())
}
