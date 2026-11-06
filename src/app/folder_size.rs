use std::io;
use std::sync::Mutex;

use winreg::{enums::HKEY_CURRENT_USER, RegKey};

use crate::shell::folder_size::{Drive, Drives};

const ROOT: &str = r"Software\Navi";
const ENABLED: &str = "FolderSizes";

fn value(drive: Drive) -> &'static str {
    match drive {
        Drive::Ssd => "FolderSizesSsd",
        Drive::Hdd => "FolderSizesHdd",
        Drive::Network => "FolderSizesNetwork",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub enabled: bool,
    pub drives: Drives,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            drives: Drives {
                ssd: true,
                hdd: false,
                network: false,
            },
        }
    }
}

static CURRENT: Mutex<Option<Preferences>> = Mutex::new(None);

pub fn current() -> Preferences {
    let mut current = CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *current.get_or_insert_with(|| {
        Preferences::read().unwrap_or_else(|error| {
            eprintln!("folder size preferences: {error}");
            Preferences::default()
        })
    })
}

fn remember(preferences: Preferences) {
    *CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(preferences);
}

impl Preferences {
    pub fn active(self) -> Drives {
        if self.enabled {
            self.drives
        } else {
            Drives::default()
        }
    }

    pub fn allows(self, drive: Drive) -> bool {
        self.drives.allows(drive)
    }

    pub fn toggled(self, drive: Option<Drive>) -> Self {
        let mut next = self;
        match drive {
            None => next.enabled = !self.enabled,
            Some(Drive::Ssd) => next.drives.ssd = !self.drives.ssd,
            Some(Drive::Hdd) => next.drives.hdd = !self.drives.hdd,
            Some(Drive::Network) => next.drives.network = !self.drives.network,
        }
        next
    }

    pub fn read() -> io::Result<Self> {
        match RegKey::predef(HKEY_CURRENT_USER).open_subkey(ROOT) {
            Ok(key) => Self::read_from(&key),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    fn read_from(key: &RegKey) -> io::Result<Self> {
        let flag = |name: &str, default: bool| match key.get_value::<u32, _>(name) {
            Ok(0) => Ok(false),
            Ok(1) => Ok(true),
            Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidData, name.to_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
            Err(error) => Err(error),
        };
        let defaults = Self::default();
        Ok(Self {
            enabled: flag(ENABLED, defaults.enabled)?,
            drives: Drives {
                ssd: flag(value(Drive::Ssd), defaults.drives.ssd)?,
                hdd: flag(value(Drive::Hdd), defaults.drives.hdd)?,
                network: flag(value(Drive::Network), defaults.drives.network)?,
            },
        })
    }

    pub fn save(self) -> io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(ROOT)?;
        self.write_to(&key)?;
        remember(self);
        Ok(())
    }

    fn write_to(self, key: &RegKey) -> io::Result<()> {
        key.set_value(ENABLED, &u32::from(self.enabled))?;
        for drive in [Drive::Ssd, Drive::Hdd, Drive::Network] {
            key.set_value(value(drive), &u32::from(self.allows(drive)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_default_off_with_ssd_selected_and_round_trip() {
        let path = format!(r"Software\NaviFolderSizeTest-{}", std::process::id());
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = root.create_subkey(&path).unwrap();
        let defaults = Preferences::read_from(&key).unwrap();
        assert_eq!(defaults, Preferences::default());
        assert!(!defaults.active().any());
        let enabled = defaults.toggled(None).toggled(Some(Drive::Network));
        enabled.write_to(&key).unwrap();
        let read = Preferences::read_from(&key).unwrap();
        assert_eq!(read, enabled);
        assert_eq!(
            read.active(),
            Drives {
                ssd: true,
                hdd: false,
                network: true
            }
        );
        key.set_value(ENABLED, &7u32).unwrap();
        assert!(Preferences::read_from(&key).is_err());
        drop(key);
        root.delete_subkey(&path).unwrap();
    }
}
