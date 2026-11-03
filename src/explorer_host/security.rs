use super::transport::Owned;
use std::{
    io,
    os::windows::io::FromRawHandle,
    path::{Path, PathBuf},
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{Com::CoTaskMemFree, Threading::*},
        UI::Shell::*,
    },
};

pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
pub(crate) fn error(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}

pub(crate) fn known_folder(id: &windows::core::GUID) -> io::Result<PathBuf> {
    unsafe {
        let text = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).map_err(error)?;
        let path = text.to_string();
        CoTaskMemFree(Some(text.0.cast()));
        Ok(PathBuf::from(path.map_err(io::Error::other)?))
    }
}
pub(crate) fn system_directory() -> io::Result<PathBuf> {
    let mut text = vec![0u16; 32768];
    let count =
        unsafe { windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut text)) }
            as usize;
    if count == 0 || count >= text.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&text[..count])))
}

pub(crate) struct Privileges {
    token: Owned,
    previous: Vec<TOKEN_PRIVILEGES>,
}
impl Privileges {
    pub fn acquire() -> io::Result<Self> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
            .map_err(error)?;
            let mut result = Self {
                token: Owned(token),
                previous: Vec::new(),
            };
            for name in [
                "SeBackupPrivilege",
                "SeRestorePrivilege",
                "SeTakeOwnershipPrivilege",
                "SeSecurityPrivilege",
                "SeDebugPrivilege",
            ] {
                let mut luid = LUID::default();
                LookupPrivilegeValueW(None, PCWSTR(wide(name).as_ptr()), &mut luid)
                    .map_err(error)?;
                let state = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES {
                        Luid: luid,
                        Attributes: SE_PRIVILEGE_ENABLED,
                    }],
                };
                let mut previous = TOKEN_PRIVILEGES::default();
                let mut size = 0;
                SetLastError(ERROR_SUCCESS);
                AdjustTokenPrivileges(
                    result.token.0,
                    false,
                    Some(&state),
                    std::mem::size_of_val(&previous) as u32,
                    Some(&mut previous),
                    Some(&mut size),
                )
                .map_err(error)?;
                if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, name));
                }
                result.previous.push(previous);
            }
            Ok(result)
        }
    }
}
impl Drop for Privileges {
    fn drop(&mut self) {
        unsafe {
            for previous in self.previous.iter().rev() {
                let _ = AdjustTokenPrivileges(self.token.0, false, Some(previous), 0, None, None);
            }
        }
    }
}

pub struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    pub fn new(sddl: &str) -> io::Result<Self> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide(sddl).as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(error)?;
        }
        Ok(Self(descriptor))
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0 .0,
            bInheritHandle: false.into(),
        }
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

pub(crate) fn no_reparse(path: &Path) -> io::Result<()> {
    for part in path.ancestors() {
        if part.exists()
            && std::fs::symlink_metadata(part)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0
                != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Reparse point in installation path",
            ));
        }
    }
    Ok(())
}
use std::os::windows::fs::MetadataExt;

pub(crate) fn create_directory(path: &Path, public: bool) -> io::Result<()> {
    no_reparse(path)?;
    let sddl = if public {
        "O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)"
    } else {
        "O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
    };
    let descriptor = Descriptor::new(sddl)?;
    if !path.exists() {
        unsafe {
            CreateDirectoryW(
                PCWSTR(wide(&path.to_string_lossy()).as_ptr()),
                Some(&descriptor.attributes()),
            )
            .map_err(error)?;
        }
    }
    verify_directory(path, public)
}

pub(crate) fn new_file(path: &Path, public: bool) -> io::Result<std::fs::File> {
    no_reparse(path)?;
    let sddl = if public {
        "O:BAG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;BU)"
    } else {
        "O:BAG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)"
    };
    let descriptor = Descriptor::new(sddl)?;
    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide(&path.to_string_lossy()).as_ptr()),
            0xc0000000,
            FILE_SHARE_READ,
            Some(&descriptor.attributes()),
            CREATE_NEW,
            FILE_FLAG_WRITE_THROUGH,
            None,
        )
        .map_err(error)?;
        Ok(std::fs::File::from_raw_handle(handle.0))
    }
}

pub(crate) fn verify_directory(path: &Path, public: bool) -> io::Result<()> {
    no_reparse(path)?;
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        GetNamedSecurityInfoW(
            PCWSTR(wide(&path.to_string_lossy()).as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            &mut descriptor,
        )
        .ok()
        .map_err(error)?;
        let descriptor = Descriptor(descriptor);
        let mut text = PWSTR::null();
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            None,
        )
        .map_err(error)?;
        let value = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        let value = value.map_err(io::Error::other)?;
        if !(value.starts_with("O:BA") || value.starts_with("O:SY")) || !value.contains("D:P") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unprotected installation directory",
            ));
        }
        for ace in value.split('(').skip(1) {
            let ace = ace.split(')').next().unwrap_or("");
            let fields: Vec<_> = ace.split(';').collect();
            if fields.len() != 6
                || fields[0] != "A"
                || !matches!(fields[5], "BA" | "SY")
                    && !(public
                        && fields[5] == "BU"
                        && matches!(fields[2], "GRGX" | "FRFX" | "0x1200a9"))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Foreign directory access rule",
                ));
            }
        }
        Ok(())
    }
}
