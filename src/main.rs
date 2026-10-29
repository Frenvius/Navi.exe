#![windows_subsystem = "windows"]

mod app;
mod dnd;
mod frame;
mod i18n;
mod shell;
mod ui;
mod watch;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ui::run()?;
    Ok(())
}
