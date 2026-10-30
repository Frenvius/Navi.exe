//! Per-user associations only. Raw registry bytes preserve type, presence and even
//! noncanonical string encodings. The durable journal precedes every registry write.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
use winreg::{enums::*, RegKey, RegValue};

const WIN_E: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Value {
    kind: u32,
    bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    key: String,
    name: String,
    before: Option<Value>,
    after: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    version: u32,
    records: Vec<Record>,
    missing: Vec<String>,
}

fn issue(key: &str) -> io::Error {
    io::Error::other(crate::i18n::tr(key).to_owned())
}

fn kind(n: u32) -> io::Result<RegType> {
    [
        REG_NONE,
        REG_SZ,
        REG_EXPAND_SZ,
        REG_BINARY,
        REG_DWORD,
        REG_DWORD_BIG_ENDIAN,
        REG_LINK,
        REG_MULTI_SZ,
        REG_RESOURCE_LIST,
        REG_FULL_RESOURCE_DESCRIPTOR,
        REG_RESOURCE_REQUIREMENTS_LIST,
        REG_QWORD,
    ]
    .into_iter()
    .find(|k| k.clone() as u32 == n)
    .ok_or_else(|| issue("settings.default.invalid-backup"))
}
fn string(s: &str) -> Value {
    Value {
        kind: REG_SZ as u32,
        bytes: s
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect(),
    }
}
fn targets(exe: &Path) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for class in ["Directory", "Drive", "Folder"] {
        let root = format!(r"Software\Classes\{class}\shell");
        out.push((root.clone(), String::new(), string("open")));
        for verb in ["open", "explore"] {
            let key = format!(r"{root}\{verb}\command");
            let path = if class == "Folder" { "%1" } else { r"%1\." };
            out.push((
                key.clone(),
                String::new(),
                string(&format!(
                    r#""{}" --shell-open --reuse-window -- "{path}""#,
                    exe.display()
                )),
            ));
            out.push((key, "DelegateExecute".into(), string("")));
        }
    }
    out.push((
        WIN_E.into(),
        String::new(),
        string(&format!(
            r#""{}" --shell-open --reuse-window"#,
            exe.display()
        )),
    ));
    out.push((WIN_E.into(), "DelegateExecute".into(), string("")));
    out
}

trait Registry {
    fn read(&self, key: &str, name: &str) -> io::Result<Option<Value>>;
    fn exists(&self, key: &str) -> io::Result<bool>;
    fn write(&mut self, key: &str, name: &str, value: Option<&Value>) -> io::Result<()>;
    fn remove_empty(&mut self, key: &str) -> io::Result<()>;
}
struct UserRegistry;
impl Registry for UserRegistry {
    fn read(&self, key: &str, name: &str) -> io::Result<Option<Value>> {
        let k = match RegKey::predef(HKEY_CURRENT_USER).open_subkey(key) {
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        match k.get_raw_value(name) {
            Ok(v) => Ok(Some(Value {
                kind: v.vtype as u32,
                bytes: v.bytes,
            })),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn exists(&self, key: &str) -> io::Result<bool> {
        match RegKey::predef(HKEY_CURRENT_USER).open_subkey(key) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
    fn write(&mut self, key: &str, name: &str, value: Option<&Value>) -> io::Result<()> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        if let Some(v) = value {
            root.create_subkey(key)?.0.set_raw_value(
                name,
                &RegValue {
                    vtype: kind(v.kind)?,
                    bytes: v.bytes.clone(),
                },
            )
        } else {
            match root
                .open_subkey_with_flags(key, KEY_SET_VALUE)
                .and_then(|k| k.delete_value(name))
            {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }
    }
    fn remove_empty(&mut self, key: &str) -> io::Result<()> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        match root.open_subkey(key) {
            Ok(k) => {
                let info = k.query_info()?;
                if info.sub_keys == 0 && info.values == 0 {
                    drop(k);
                    root.delete_subkey(key)?;
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}
fn snapshot(reg: &impl Registry, exe: &Path) -> io::Result<Backup> {
    let mut records = Vec::new();
    let mut missing = BTreeSet::new();
    for (key, name, after) in targets(exe) {
        let mut part = key.as_str();
        while part != r"Software\Classes" && !reg.exists(part)? {
            missing.insert(part.to_string());
            part = part
                .rsplit_once('\\')
                .ok_or_else(|| issue("settings.default.invalid-backup"))?
                .0;
        }
        records.push(Record {
            before: reg.read(&key, &name)?,
            key,
            name,
            after,
        });
    }
    let mut missing: Vec<_> = missing.into_iter().collect();
    missing.sort_by_key(|k| std::cmp::Reverse(k.len()));
    Ok(Backup {
        version: 1,
        records,
        missing,
    })
}
fn validate(b: &Backup) -> io::Result<()> {
    let allowed = targets(Path::new("validation.exe"));
    if b.version != 1 || b.records.len() != allowed.len() || b.missing.len() > 60 {
        return Err(issue("settings.default.invalid-backup"));
    }
    for (r, (key, name, _)) in b.records.iter().zip(&allowed) {
        if &r.key != key
            || &r.name != name
            || r.after.bytes.len() > 131072
            || r.before.as_ref().is_some_and(|v| v.bytes.len() > 1048576)
        {
            return Err(issue("settings.default.invalid-backup"));
        }
        kind(r.after.kind)?;
        if let Some(v) = &r.before {
            kind(v.kind)?;
        }
    }
    for k in &b.missing {
        if !k.starts_with(r"Software\Classes\")
            || !allowed
                .iter()
                .any(|(a, _, _)| a == k || a.starts_with(&format!("{k}\\")))
        {
            return Err(issue("settings.default.invalid-backup"));
        }
    }
    Ok(())
}
fn restore(reg: &mut impl Registry, b: &Backup) -> io::Result<()> {
    validate(b)?;
    // Fail before changing anything if another manager changed a touched value.
    for r in &b.records {
        let now = reg.read(&r.key, &r.name)?;
        if now != Some(r.after.clone()) && now != r.before {
            return Err(issue("settings.default.conflict"));
        }
    }
    for r in b.records.iter().rev() {
        reg.write(&r.key, &r.name, r.before.as_ref())?;
    }
    let mut missing = b.missing.clone();
    missing.sort_by_key(|key| std::cmp::Reverse(key.len()));
    for key in missing {
        reg.remove_empty(&key)?;
    }
    Ok(())
}
fn apply(reg: &mut impl Registry, b: &Backup) -> io::Result<()> {
    validate(b)?;
    for r in &b.records {
        if reg.read(&r.key, &r.name)? != r.before {
            return Err(issue("settings.default.conflict"));
        }
    }
    for r in &b.records {
        reg.write(&r.key, &r.name, Some(&r.after))?;
    }
    Ok(())
}
fn backup_path() -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| issue("settings.default.backup-unavailable"))?,
    )
    .join("Navi/default-file-manager/active.json"))
}
fn load(path: &Path) -> io::Result<Option<Backup>> {
    match fs::File::open(path) {
        Ok(f) => {
            if f.metadata()?.len() > 4_194_304 {
                return Err(issue("settings.default.invalid-backup"));
            }
            let b: Backup = serde_json::from_reader(f)?;
            validate(&b)?;
            Ok(Some(b))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn save(path: &Path, b: &Backup) -> io::Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| issue("settings.default.backup-unavailable"))?,
    )?;
    let bytes = serde_json::to_vec_pretty(b)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);
    // The per-user mutex makes the existence check + rename exclusive to this app.
    if path.exists() {
        return Err(issue("settings.default.active-backup"));
    }
    fs::rename(temp, path)
}
fn archive(path: &Path) -> io::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    fs::rename(path, path.with_file_name(format!("restored-{stamp}.json")))
}
fn notify() {
    unsafe {
        windows::Win32::UI::Shell::SHChangeNotify(
            windows::Win32::UI::Shell::SHCNE_ASSOCCHANGED,
            windows::Win32::UI::Shell::SHCNF_IDLIST,
            None,
            None,
        );
    }
}

struct Lock(windows::Win32::Foundation::HANDLE);
impl Lock {
    fn acquire() -> io::Result<Self> {
        use std::hash::{Hash, Hasher};
        use windows::{
            core::PCWSTR,
            Win32::{Foundation::CloseHandle, System::Threading::*},
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::env::var_os("USERPROFILE").hash(&mut h);
        let name = crate::shell::wide(&format!(r"Local\NaviDefaultManager-{:x}", h.finish()));
        unsafe {
            let handle =
                CreateMutexW(None, false, PCWSTR(name.as_ptr())).map_err(io::Error::other)?;
            if !matches!(WaitForSingleObject(handle, 2000).0, 0 | 0x80) {
                let _ = CloseHandle(handle);
                return Err(issue("settings.default.busy"));
            }
            Ok(Self(handle))
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(self.0);
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub struct Status {
    pub enabled: bool,
    pub recovery: bool,
}
pub fn status() -> io::Result<Status> {
    let _lock = Lock::acquire()?;
    let exe = std::env::current_exe()?;
    let enabled = targets(&exe)
        .into_iter()
        .map(|(k, n, v)| UserRegistry.read(&k, &n).map(|actual| actual == Some(v)))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .all(|v| v);
    Ok(Status {
        enabled,
        recovery: backup_path()?.is_file(),
    })
}
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    let _lock = Lock::acquire()?;
    let path = backup_path()?;
    let mut reg = UserRegistry;
    let existing = load(&path)?;
    if !enabled {
        if let Some(b) = existing {
            restore(&mut reg, &b)?;
            notify();
            archive(&path)?;
        } else if targets(&std::env::current_exe()?).iter().any(|(k, n, v)| {
            k.ends_with(r"\command")
                && n.is_empty()
                && reg.read(k, n).ok().flatten().as_ref() == Some(v)
        }) {
            return Err(issue("settings.default.missing-backup"));
        }
        return Ok(());
    }
    if existing.is_some() {
        return Err(issue("settings.default.active-backup"));
    }
    if targets(&std::env::current_exe()?)
        .iter()
        .any(|(key, name, value)| {
            key.ends_with(r"\command")
                && name.is_empty()
                && reg.read(key, name).ok().flatten().as_ref() == Some(value)
        })
    {
        return Err(issue("settings.default.missing-backup"));
    }
    // Refuse arrangements this feature does not own; never delete DDE or handlers.
    let classes = RegKey::predef(HKEY_CLASSES_ROOT);
    for (key, _, _) in targets(&std::env::current_exe()?) {
        if !key.ends_with(r"\command") {
            continue;
        }
        let parent = key
            .strip_prefix(r"Software\Classes\")
            .unwrap()
            .trim_end_matches(r"\command");
        if classes.open_subkey(format!(r"{parent}\ddeexec")).is_ok()
            || classes
                .open_subkey(parent)
                .is_ok_and(|k| k.get_raw_value("ExplorerCommandHandler").is_ok())
        {
            return Err(issue("settings.default.unsupported-handler"));
        }
    }
    let b = snapshot(&reg, &std::env::current_exe()?)?;
    save(&path, &b)?;
    if let Err(error) = apply(&mut reg, &b) {
        if restore(&mut reg, &b).is_ok() {
            notify();
            archive(&path)?;
        }
        return Err(error);
    }
    notify();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Memory {
        values: BTreeMap<(String, String), Value>,
        keys: BTreeSet<String>,
        fail_after: Option<usize>,
    }
    impl Registry for Memory {
        fn read(&self, k: &str, n: &str) -> io::Result<Option<Value>> {
            Ok(self.values.get(&(k.into(), n.into())).cloned())
        }
        fn exists(&self, k: &str) -> io::Result<bool> {
            Ok(self.keys.contains(k))
        }
        fn write(&mut self, k: &str, n: &str, v: Option<&Value>) -> io::Result<()> {
            if let Some(left) = self.fail_after.as_mut() {
                if *left == 0 {
                    self.fail_after = None;
                    return Err(io::Error::other("Injected failure"));
                }
                *left -= 1;
            }
            if let Some(v) = v {
                let mut p = k;
                while let Some((parent, _)) = p.rsplit_once('\\') {
                    self.keys.insert(p.into());
                    p = parent;
                }
                self.values.insert((k.into(), n.into()), v.clone());
            } else {
                self.values.remove(&(k.into(), n.into()));
            }
            Ok(())
        }
        fn remove_empty(&mut self, k: &str) -> io::Result<()> {
            if !self.values.keys().any(|(p, _)| p == k)
                && !self.keys.iter().any(|p| p.starts_with(&format!("{k}\\")))
            {
                self.keys.remove(k);
            }
            Ok(())
        }
    }
    #[test]
    fn raw_types_missing_values_and_created_keys_roundtrip() {
        let mut reg = Memory::default();
        reg.keys
            .extend(["Software".into(), r"Software\Classes".into()]);
        let all = targets(Path::new(r"C:\Program Files\Navi\navi.exe"));
        for (i, (k, n, _)) in all.iter().take(12).enumerate() {
            reg.write(
                k,
                n,
                Some(&Value {
                    kind: i as u32,
                    bytes: vec![0, 255, i as u8, 0],
                }),
            )
            .unwrap();
        }
        let before = reg.values.clone();
        let keys = reg.keys.clone();
        let b = snapshot(&reg, Path::new(r"C:\Program Files\Navi\navi.exe")).unwrap();
        let b: Backup = serde_json::from_slice(&serde_json::to_vec(&b).unwrap()).unwrap();
        apply(&mut reg, &b).unwrap();
        restore(&mut reg, &b).unwrap();
        assert_eq!(reg.values, before);
        assert_eq!(reg.keys, keys);
    }
    #[test]
    fn partial_write_recovers_and_external_edits_are_not_clobbered() {
        let mut reg = Memory::default();
        reg.keys
            .extend(["Software".into(), r"Software\Classes".into()]);
        let b = snapshot(&reg, Path::new(r"C:\Navi\navi.exe")).unwrap();
        reg.fail_after = Some(7);
        assert!(apply(&mut reg, &b).is_err());
        restore(&mut reg, &b).unwrap();
        assert!(reg.values.is_empty());
        apply(&mut reg, &b).unwrap();
        let first = &b.records[0];
        reg.write(&first.key, &first.name, Some(&string("other-manager")))
            .unwrap();
        let before = reg.values.clone();
        assert!(restore(&mut reg, &b).is_err());
        assert_eq!(reg.values, before);
    }
    #[test]
    fn unrelated_values_survive_and_backup_cannot_escape_scope() {
        let mut reg = Memory::default();
        reg.keys
            .extend(["Software".into(), r"Software\Classes".into()]);
        let mut b = snapshot(&reg, Path::new(r"C:\Navi\navi.exe")).unwrap();
        apply(&mut reg, &b).unwrap();
        reg.write(&b.records[0].key, "Unrelated", Some(&string("keep")))
            .unwrap();
        restore(&mut reg, &b).unwrap();
        assert_eq!(reg.values.len(), 1);
        b.missing.push(r"Software\Microsoft".into());
        assert!(validate(&b).is_err());
        b.missing.clear();
        b.records[0].key = r"Software\Microsoft".into();
        assert!(restore(&mut reg, &b).is_err());
    }
}
