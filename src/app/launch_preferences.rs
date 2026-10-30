use std::io;
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const ROOT: &str = r"Software\Navi";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    NewTab = 0,
    NewWindow = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    External,
    WinE,
}

impl Source {
    fn value(self) -> &'static str {
        match self {
            Self::External => "ExternalOpenDestination",
            Self::WinE => "WinEOpenDestination",
        }
    }

    pub fn save(self, destination: Destination) -> io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(ROOT)?;
        key.set_value(self.value(), &(destination as u32))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub external: Destination,
    pub win_e: Destination,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            external: Destination::NewTab,
            win_e: Destination::NewWindow,
        }
    }
}

impl Preferences {
    pub fn destination(self, source: Source) -> Destination {
        match source {
            Source::External => self.external,
            Source::WinE => self.win_e,
        }
    }

    pub fn read() -> io::Result<Self> {
        match RegKey::predef(HKEY_CURRENT_USER).open_subkey(ROOT) {
            Ok(key) => Self::read_from(&key),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    fn read_from(key: &RegKey) -> io::Result<Self> {
        let mut result = Self::default();
        for source in [Source::External, Source::WinE] {
            let destination = match key.get_value::<u32, _>(source.value()) {
                Ok(0) => Destination::NewTab,
                Ok(1) => Destination::NewWindow,
                Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, source.value())),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            match source {
                Source::External => result.external = destination,
                Source::WinE => result.win_e = destination,
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_destinations_persist_with_opposite_defaults() {
        let path = format!(r"Software\NaviLaunchPreferencesTest-{}", std::process::id());
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = root.create_subkey(&path).unwrap();
        struct Fixture(String);
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Err(error) = RegKey::predef(HKEY_CURRENT_USER).delete_subkey(&self.0) {
                    eprintln!("launch preferences test cleanup: {error}");
                }
            }
        }
        let _fixture = Fixture(path);
        assert_eq!(
            Preferences::read_from(&key).unwrap(),
            Preferences::default()
        );
        key.set_value(Source::External.value(), &1u32).unwrap();
        assert_eq!(
            Preferences::read_from(&key).unwrap(),
            Preferences {
                external: Destination::NewWindow,
                win_e: Destination::NewWindow,
            }
        );
        key.set_value(Source::WinE.value(), &0u32).unwrap();
        assert_eq!(
            Preferences::read_from(&key).unwrap(),
            Preferences {
                external: Destination::NewWindow,
                win_e: Destination::NewTab,
            }
        );
        key.set_value(Source::WinE.value(), &9u32).unwrap();
        assert!(Preferences::read_from(&key).is_err());
        drop(key);
    }
}
