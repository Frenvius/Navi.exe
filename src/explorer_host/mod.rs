mod abi;
pub mod guardian;
pub mod install;
pub mod journal;
pub mod presentation;
pub mod protocol;
pub mod security;
pub mod transport;
pub mod version;

pub fn opted_in() -> bool {
    let Ok(key) =
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Navi")
    else {
        return false;
    };
    if let Ok(identity) = transport::identity(std::process::id()) {
        requested(
            key.get_value("ExplorerHostOptIn").ok(),
            key.get_value("ExplorerHostSession").ok(),
            identity.session,
        )
    } else {
        false
    }
}

fn requested(opt_in: Option<u32>, session: Option<u32>, current: u32) -> bool {
    current != 0 && opt_in == Some(1) && session == Some(current)
}

pub fn enabled() -> bool {
    opted_in()
        && install::broker_path().is_ok_and(|path| path.is_file())
        && install::registered()
        && version::verified()
}

pub fn exclusion_commands(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().replace('\'', "''");
    format!(
        "Add-MpPreference -ExclusionPath '{path}'\r\nAdd-MpPreference -ExclusionProcess 'navi-explorer-host.exe'"
    )
}

pub fn clock() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experimental_opt_in_is_off_unless_explicit_for_the_current_interactive_session() {
        for opt_in in [None, Some(0), Some(1), Some(2)] {
            for session in [None, Some(0), Some(2), Some(3)] {
                assert_eq!(
                    requested(opt_in, session, 2),
                    opt_in == Some(1) && session == Some(2)
                );
                assert!(!requested(opt_in, session, 0));
            }
        }
    }

    #[test]
    fn displayed_exclusions_quote_the_exact_broker_without_extra_commands() {
        let command = exclusion_commands(std::path::Path::new(
            r"C:\Users\O'Brien\Navi\navi-explorer-host.exe",
        ));
        assert_eq!(command, "Add-MpPreference -ExclusionPath 'C:\\Users\\O''Brien\\Navi\\navi-explorer-host.exe'\r\nAdd-MpPreference -ExclusionProcess 'navi-explorer-host.exe'");
        assert_eq!(command.lines().count(), 2);
    }
}
