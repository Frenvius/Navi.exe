#![windows_subsystem = "windows"]

mod app;
mod dnd;
mod frame;
mod i18n;
mod shell;
mod ui;
mod watch;

use navi::explorer_host;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    navi::crash::install();
    if std::env::args_os().skip(1).eq([std::ffi::OsString::from(
        explorer_host::install::RECOVER_ARGUMENT,
    )]) {
        explorer_host::install::recover_from_startup()?;
        return Ok(());
    }
    if let Err(error) = app::default_manager::recover_host_on_startup() {
        let message = shell::wide(&i18n::trf(
            "settings.host.recovery-failed",
            &[&error.to_string()],
        ));
        let title = shell::wide(i18n::tr("settings.host.warning.title"));
        unsafe {
            use windows::{core::PCWSTR, Win32::UI::WindowsAndMessaging::*};
            MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONWARNING,
            );
        }
    }
    ui::run()?;
    Ok(())
}
