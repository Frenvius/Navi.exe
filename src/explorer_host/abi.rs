use std::{ffi::c_void, io, mem::size_of};
use windows::{
    core::{GUID, PCSTR, PCWSTR},
    Win32::{
        Foundation::*,
        System::{
            Diagnostics::Debug::ReadProcessMemory, LibraryLoader::*, Threading::GetCurrentProcess,
        },
    },
};
use winreg::{enums::*, RegKey};

const HOST: &str = "{75DFF2B7-6936-4C06-A8BB-676A7B00B24B}";
const NATIVE: &str = "{D7DFB84A-C031-49BC-AD7C-5064810A2E74}";
const APP: &str = "{CDCBCFCA-3CDC-436f-A4E2-0E02075250C2}";
const PROXY: &str = "{C90250F3-4D7D-4991-9B69-A5C5BC1C2AE6}";
const IIDS: [GUID; 2] = [
    GUID::from_u128(0xff9ce68e_060d_4120_81f9_f3061c703225),
    GUID::from_u128(0x7080b61c_7b9a_4910_9281_8f4b980ce39f),
];
const MINIMUM_BUILD: u32 = 22000;
fn unavailable(message: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "Explorer host unavailable on this Windows version: {}",
            message.into()
        ),
    )
}
fn same(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[derive(Clone)]
struct Registration {
    proxy: [String; 2],
    methods: [Option<u32>; 2],
    dll_valid: bool,
    threading: String,
    appid: String,
    flags: u32,
    runas: String,
    factory_valid: bool,
    treat_as: bool,
}
impl Registration {
    fn require(&self) -> io::Result<()> {
        if self.proxy.iter().any(|value| !same(value, PROXY))
            || !self.dll_valid
            || !same(&self.threading, "Both")
        {
            return Err(unavailable(
                "IExplorerHost/Host2 proxy registration changed",
            ));
        }
        if self
            .methods
            .iter()
            .zip([5, 4])
            .any(|(value, expected)| value.is_some_and(|value| value != expected))
        {
            return Err(unavailable("registered COM method count changed"));
        }
        if !same(&self.appid, APP)
            || self.flags != 1
            || !same(&self.runas, "Interactive User")
            || !self.factory_valid
            || self.treat_as
        {
            return Err(unavailable(
                "factory CLSID/AppID or forwarding layout changed",
            ));
        }
        Ok(())
    }
}
fn native(command: &str, view: u32, guid: &str) -> io::Result<bool> {
    let windows = super::security::system_directory()?
        .parent()
        .unwrap()
        .to_path_buf();
    let paths = if view == KEY_WOW64_64KEY {
        vec![
            windows.join("explorer.exe").to_string_lossy().into_owned(),
            r"%SystemRoot%\explorer.exe".into(),
        ]
    } else {
        vec![
            windows
                .join("SysWow64")
                .join("explorer.exe")
                .to_string_lossy()
                .into_owned(),
            r"%SystemRoot%\SysWow64\explorer.exe".into(),
        ]
    };
    Ok(paths
        .iter()
        .any(|path| same(command, &format!("{path} /factory,{guid}"))))
}
fn registration(view: u32) -> io::Result<Registration> {
    let root = RegKey::predef(HKEY_CLASSES_ROOT);
    let open = |path: &str| {
        root.open_subkey_with_flags(path, KEY_READ | view)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "HKCR\\{path} ({}-bit): {error}",
                        if view == KEY_WOW64_64KEY { 64 } else { 32 }
                    ),
                )
            })
    };
    let mut proxy = [String::new(), String::new()];
    let mut methods = [None, None];
    for (index, iid) in IIDS.iter().enumerate() {
        proxy[index] = open(&format!(r"Interface\{{{iid:?}}}\ProxyStubClsid32"))?.get_value("")?;
        match open(&format!(r"Interface\{{{iid:?}}}\NumMethods")) {
            Ok(key) => {
                methods[index] = Some(
                    key.get_value::<String, _>("")
                        .and_then(|value| value.parse().map_err(io::Error::other))
                        .or_else(|_| key.get_value::<u32, _>(""))?,
                )
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let server = open(&format!(r"CLSID\{PROXY}\InprocServer32"))?;
    let path: String = server.get_value("")?;
    let system = super::security::system_directory()?;
    let dll_valid = same(&path, &system.join("ActXPrxy.dll").to_string_lossy())
        || (view == KEY_WOW64_32KEY
            && same(
                &path,
                &system
                    .parent()
                    .unwrap()
                    .join("SysWOW64")
                    .join("ActXPrxy.dll")
                    .to_string_lossy(),
            ));
    let factory = open(&format!(r"CLSID\{HOST}"))?;
    let app = open(&format!(r"AppID\{APP}"))?;
    let command: String = open(&format!(r"CLSID\{HOST}\LocalServer32"))?.get_value("")?;
    let installed = super::install::broker_path()
        .is_ok_and(|path| same(&command, &format!("\"{}\" --serve", path.display())));
    let factory_valid = if installed {
        let alias = open(&format!(r"CLSID\{NATIVE}"))?;
        let command: String = open(&format!(r"CLSID\{NATIVE}\LocalServer32"))?.get_value("")?;
        same(&alias.get_value::<String, _>("AppID")?, APP) && native(&command, view, NATIVE)?
    } else {
        native(&command, view, HOST)?
    };
    Ok(Registration {
        proxy,
        methods,
        dll_valid,
        threading: server.get_value("ThreadingModel")?,
        appid: factory.get_value("AppID")?,
        flags: app.get_value("AppIDFlags")?,
        runas: app.get_value("RunAs")?,
        factory_valid,
        treat_as: open(&format!(r"CLSID\{HOST}\TreatAs")).is_ok(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Scalar(u8),
    Longs(u16),
    Interface(GUID),
    Marshal(u16, u16),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Parameter {
    attributes: u16,
    offset: u16,
    kind: Kind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Method {
    slot: u16,
    stack: u16,
    parameters: Vec<Parameter>,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct ProxyFile {
    proxies: usize,
    stubs: usize,
    names: usize,
    delegated: usize,
    lookup: usize,
    count: u16,
    version: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Stub {
    iid: usize,
    server: usize,
    count: u32,
    dispatch: usize,
}
fn read<const N: usize>(address: usize) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            bytes.as_mut_ptr().cast(),
            N,
            None,
        )
        .map_err(io::Error::other)?;
    }
    Ok(bytes)
}
fn value<T: Copy>(address: usize) -> io::Result<T> {
    let mut result = std::mem::MaybeUninit::<T>::uninit();
    unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            result.as_mut_ptr().cast(),
            size_of::<T>(),
            None,
        )
        .map_err(io::Error::other)?;
        Ok(result.assume_init())
    }
}
fn word(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn kind(types: usize, attributes: u16, code: u16) -> io::Result<Kind> {
    if attributes & 0x40 != 0 {
        return Ok(Kind::Scalar(
            code.try_into()
                .map_err(|_| unavailable("NDR scalar changed"))?,
        ));
    }
    let bytes = read::<24>(types + usize::from(code))?;
    match bytes[0] {
        0xB4 => Ok(Kind::Marshal(word(&bytes, 2), word(&bytes, 4))),
        0x2F if bytes[1] == 0x5A => Ok(Kind::Interface(unsafe {
            std::ptr::read_unaligned(bytes[2..].as_ptr().cast())
        })),
        0x15 if bytes[1] == 3 => {
            let size = word(&bytes, 2);
            if !matches!(size, 8 | 16)
                || bytes[4..4 + usize::from(size / 4)]
                    .iter()
                    .any(|code| *code != 8)
            {
                return Err(unavailable("POINT/RECT members changed"));
            }
            Ok(Kind::Longs(size))
        }
        _ => Err(unavailable("NDR argument type changed")),
    }
}
fn method(server: usize, slot: u16) -> io::Result<Method> {
    let descriptor: usize = value(server)?;
    let types: usize = value(descriptor + 8 * size_of::<usize>())?;
    let format: usize = value(server + 2 * size_of::<usize>())?;
    let offsets: usize = value(server + 3 * size_of::<usize>())?;
    let offset: u16 = value(offsets + usize::from(slot) * 2)?;
    let bytes = read::<96>(format + usize::from(offset))?;
    if bytes[0] != 0x33
        || bytes[1] != 0x6C
        || bytes[15] > 8
        || !(2..=16).contains(&bytes[16])
        || word(&bytes, 6) != slot
    {
        return Err(unavailable("NDR procedure header changed"));
    }
    let start = 16 + usize::from(bytes[16]);
    let mut parameters = Vec::new();
    for index in 0..usize::from(bytes[15]) {
        let offset = start + index * 6;
        let attributes = word(&bytes, offset);
        parameters.push(Parameter {
            attributes,
            offset: word(&bytes, offset + 2),
            kind: kind(types, attributes, word(&bytes, offset + 4))?,
        });
    }
    Ok(Method {
        slot,
        stack: word(&bytes, 8),
        parameters,
    })
}
fn expected(pidl: Kind, hwnd: Kind) -> [Vec<Method>; 2] {
    let unknown = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
    let handshake = GUID::from_u128(0x6aca2690_0de1_46c9_a1c5_9338a22bf12b);
    let array = GUID::from_u128(0xb63ea76d_1f85_456f_a19c_48159efa858b);
    let make = |slot, stack, parameters: Vec<(u16, Kind)>| Method {
        slot,
        stack,
        parameters: parameters
            .into_iter()
            .enumerate()
            .map(|(index, (attributes, kind))| Parameter {
                attributes,
                offset: ((index + 1) * 8) as u16,
                kind,
            })
            .collect(),
    };
    [
        vec![
            make(
                3,
                72,
                vec![
                    (0x8B, pidl.clone()),
                    (0x48, Kind::Scalar(14)),
                    (0x8A, Kind::Longs(8)),
                    (0x48, Kind::Scalar(8)),
                    (0x8B, hwnd),
                    (0xB, Kind::Interface(unknown)),
                    (0xB, Kind::Interface(handshake)),
                    (0x70, Kind::Scalar(8)),
                ],
            ),
            make(
                4,
                48,
                vec![
                    (0x8B, pidl.clone()),
                    (0x48, Kind::Scalar(14)),
                    (0x10A, Kind::Longs(16)),
                    (0x48, Kind::Scalar(8)),
                    (0x70, Kind::Scalar(8)),
                ],
            ),
        ],
        vec![make(
            3,
            56,
            vec![
                (0x8B, pidl),
                (0xB, Kind::Interface(array)),
                (0x10A, Kind::Longs(16)),
                (0x48, Kind::Scalar(8)),
                (0x48, Kind::Scalar(8)),
                (0x70, Kind::Scalar(8)),
            ],
        )],
    ]
}
fn validate(methods: &[Vec<Method>; 2]) -> io::Result<()> {
    let show = methods[0]
        .first()
        .ok_or_else(|| unavailable("IExplorerHost methods absent"))?;
    let pidl = show
        .parameters
        .first()
        .map(|p| p.kind.clone())
        .ok_or_else(|| unavailable("PIDL absent"))?;
    let hwnd = show
        .parameters
        .get(4)
        .map(|p| p.kind.clone())
        .ok_or_else(|| unavailable("HWND absent"))?;
    if !matches!(pidl, Kind::Marshal(_, 8))
        || !matches!(hwnd, Kind::Marshal(_, 8))
        || pidl == hwnd
        || *methods != expected(pidl, hwnd)
    {
        return Err(unavailable(
            "Host/Host2 argument, stack or method shape changed",
        ));
    }
    Ok(())
}
fn proxy_methods() -> io::Result<[Vec<Method>; 2]> {
    struct Module(HMODULE);
    impl Drop for Module {
        fn drop(&mut self) {
            unsafe {
                let _ = FreeLibrary(self.0);
            }
        }
    }
    let dll = super::security::system_directory()?.join("ActXPrxy.dll");
    let path = super::security::wide(&dll.to_string_lossy());
    unsafe {
        let module = Module(
            LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .map_err(io::Error::other)?,
        );
        let address = GetProcAddress(module.0, PCSTR(b"GetProxyDllInfo\0".as_ptr()))
            .ok_or_else(|| unavailable("proxy metadata export absent"))?;
        let get: unsafe extern "system" fn(*mut usize, *mut usize) = std::mem::transmute(address);
        let (mut files, mut clsid) = (0, 0);
        get(&mut files, &mut clsid);
        let mut found: [Option<Vec<Method>>; 2] = [None, None];
        for file_index in 0..64 {
            let pointer: usize = value(files + file_index * size_of::<usize>())?;
            if pointer == 0 {
                break;
            }
            let file: ProxyFile = value(pointer)?;
            if file.count > 16384 {
                return Err(unavailable("proxy table header changed"));
            }
            let mut candidates = Vec::new();
            if file.lookup != 0 && file.lookup != usize::MAX {
                let lookup: unsafe extern "system" fn(*const GUID, *mut i32) -> i32 =
                    std::mem::transmute(file.lookup);
                for iid in IIDS {
                    let mut index = -1;
                    if lookup(&iid, &mut index) != 0 {
                        if index < 0 || index >= i32::from(file.count) {
                            return Err(unavailable("proxy IID lookup index changed"));
                        }
                        candidates.push(index as usize);
                    }
                }
            } else {
                candidates.extend(0..usize::from(file.count));
            }
            for index in candidates {
                let pointer: usize = value(file.stubs + index * size_of::<usize>())?;
                let stub: Stub = value(pointer)?;
                let iid: GUID = value(stub.iid)?;
                if let Some(index) = IIDS.iter().position(|expected| *expected == iid) {
                    if stub.count != [5, 4][index] {
                        return Err(unavailable("proxy dispatch method count changed"));
                    }
                    found[index] = Some(
                        (3..stub.count)
                            .map(|slot| method(stub.server, slot as u16))
                            .collect::<io::Result<_>>()?,
                    );
                }
            }
            if found.iter().all(Option::is_some) {
                break;
            }
        }
        Ok([
            found[0]
                .take()
                .ok_or_else(|| unavailable("IExplorerHost IID absent from proxy"))?,
            found[1]
                .take()
                .ok_or_else(|| unavailable("IExplorerHost2 IID absent from proxy"))?,
        ])
    }
}
fn platform(x64: bool, build: u32) -> io::Result<()> {
    if !x64 || build < MINIMUM_BUILD {
        return Err(unavailable("requires x86_64 Windows 11 or later"));
    }
    Ok(())
}
pub(super) fn check(build: u32) -> io::Result<()> {
    platform(cfg!(target_arch = "x86_64"), build)?;
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        registration(view)
            .and_then(|r| r.require())
            .map_err(|error| unavailable(error.to_string()))?;
    }
    validate(&proxy_methods()?).map_err(|error| unavailable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registration() -> Registration {
        Registration {
            proxy: [PROXY.into(), PROXY.into()],
            methods: [None, None],
            dll_valid: true,
            threading: "Both".into(),
            appid: APP.into(),
            flags: 1,
            runas: "Interactive User".into(),
            factory_valid: true,
            treat_as: false,
        }
    }
    #[test]
    fn monthly_updates_are_not_an_allow_list() {
        for build in [22000, 22631, 26100, 26300, 30000] {
            assert!(platform(true, build).is_ok());
        }
        assert!(platform(false, 26300).is_err());
        assert!(platform(true, 21999).is_err());
    }
    #[test]
    fn proxy_registration_accepts_absent_num_methods_and_rejects_layout_changes() {
        assert!(registration().require().is_ok());
        let mut good = registration();
        good.methods = [Some(5), Some(4)];
        assert!(good.require().is_ok());
        let mut cases = Vec::new();
        let mut r = registration();
        r.proxy[1] = "foreign".into();
        cases.push(r);
        let mut r = registration();
        r.methods[0] = Some(6);
        cases.push(r);
        let mut r = registration();
        r.dll_valid = false;
        cases.push(r);
        let mut r = registration();
        r.appid = "foreign".into();
        cases.push(r);
        let mut r = registration();
        r.runas = "Administrator".into();
        cases.push(r);
        let mut r = registration();
        r.factory_valid = false;
        cases.push(r);
        let mut r = registration();
        r.treat_as = true;
        cases.push(r);
        for r in cases {
            assert!(r.require().is_err());
        }
    }
    #[test]
    fn method_layout_checks_include_parameters_stack_and_interface_identity() {
        let methods = expected(Kind::Marshal(4, 8), Kind::Marshal(1, 8));
        assert!(validate(&methods).is_ok());
        let mut cases = Vec::new();
        let mut m = methods.clone();
        m[0][0].stack = 64;
        cases.push(m);
        let mut m = methods.clone();
        m[0][0].parameters[2].kind = Kind::Longs(16);
        cases.push(m);
        let mut m = methods.clone();
        m[1][0].parameters[1].kind = Kind::Interface(GUID::zeroed());
        cases.push(m);
        let mut m = methods.clone();
        m[0][1].parameters[2].attributes = 0xA;
        cases.push(m);
        let mut m = methods;
        m[1].clear();
        cases.push(m);
        for m in cases {
            assert!(validate(&m).is_err());
        }
    }
}
