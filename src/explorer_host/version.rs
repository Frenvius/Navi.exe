use serde::{Deserialize, Serialize};
use std::{io, path::Path, sync::OnceLock};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub number: u32,
    pub revision: u32,
    pub explorer_frame: String,
}

pub(crate) fn digest(path: &Path) -> io::Result<String> {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let bytes = std::fs::read(path)?;
    let mut result = [0; 32];
    unsafe {
        BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, &bytes, &mut result)
            .ok()
            .map_err(io::Error::other)?;
    }
    Ok(result.iter().map(|byte| format!("{byte:02X}")).collect())
}

impl Build {
    pub fn current() -> io::Result<Self> {
        let key = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")?;
        Ok(Self {
            number: key
                .get_value::<String, _>("CurrentBuildNumber")?
                .parse()
                .map_err(io::Error::other)?,
            revision: key.get_value("UBR")?,
            explorer_frame: digest(
                &super::security::system_directory()?.join("ExplorerFrame.dll"),
            )?,
        })
    }
    pub fn require(&self) -> io::Result<()> {
        super::abi::check(self.number)
    }
}

pub const MEASURED_BUILDS: &[(u32, u32)] = &[(26300, 9711)];

pub fn compatibility() -> Result<(), String> {
    static CHECK: OnceLock<Result<(), String>> = OnceLock::new();
    CHECK
        .get_or_init(|| {
            let result = (|| {
                let key = RegKey::predef(HKEY_LOCAL_MACHINE)
                    .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")?;
                let build = key
                    .get_value::<String, _>("CurrentBuildNumber")?
                    .parse()
                    .map_err(io::Error::other)?;
                super::abi::check(build)
            })();
            result.map_err(|error: io::Error| error.to_string())
        })
        .clone()
}

pub fn verified() -> bool {
    compatibility().is_ok()
}
