use crate::app::session::{self, Window as Snapshot};
use windows::core::Result;
use windows::Win32::Foundation::{E_FAIL, HWND};

fn error(message: String) -> windows::core::Error {
    windows::core::Error::new(E_FAIL, message)
}

fn snapshot(hwnd: HWND, window: &super::Window) -> Result<Snapshot> {
    Snapshot::capture(
        hwnd,
        window
            .tabs
            .entries
            .iter()
            .map(|tab| tab.location.tab.path().to_string())
            .collect(),
        window
            .tabs
            .entries
            .iter()
            .map(|tab| tab.location.tab.library())
            .collect(),
        window.tabs.current,
    )
    .map_err(error)
}

pub fn start(hwnd: HWND, launch: &crate::app::entry::Launch) -> Result<Vec<(String, Snapshot)>> {
    let _context = super::context::enter(hwnd);
    let Some(mut window) = super::WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return Ok(Vec::new());
    };
    let result = (|| -> Result<_> {
        let claim =
            session::claim(launch, snapshot(hwnd, &window)?, hwnd.0 as usize).map_err(error)?;
        if let Some(saved) = claim.restored {
            let mut entries = Vec::with_capacity(saved.paths.len());
            for (index, path) in saved.paths.iter().enumerate() {
                let mut tab = super::tabs::Saved::new(path);
                tab.location.tab.libraries[0] = saved
                    .libraries
                    .get(index)
                    .cloned()
                    .flatten()
                    .filter(|context| {
                        crate::shell::library::is_library(&context.library)
                            && context.contains(path)
                    });
                entries.push(tab);
            }
            window.tabs = crate::app::tabs::Collection {
                entries,
                current: saved.active,
            };
            super::tabs::show(&mut window, hwnd)?;
            saved.place(hwnd).map_err(error)?;
            window.title.maximized =
                unsafe { windows::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd).as_bool() };
        }
        window.journal = Some(claim.tracker);
        super::tabs::sync(&mut window, hwnd)?;
        checkpoint_window(hwnd, &mut window)?;
        Ok(claim.others)
    })();
    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    super::invalidate_all(hwnd);
    result
}

pub fn checkpoint_window(hwnd: HWND, window: &mut super::Window) -> Result<()> {
    if window.journal.is_none() {
        return Ok(());
    }
    let saved = snapshot(hwnd, window)?;
    window
        .journal
        .as_mut()
        .unwrap()
        .checkpoint(saved)
        .map_err(error)
}

pub fn checkpoint(hwnd: HWND) {
    let Some(mut window) = super::WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let result = checkpoint_window(hwnd, &mut window);
    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if let Err(error) = result {
        eprintln!("native session checkpoint: {error}");
    }
}

pub fn ending(hwnd: HWND) {
    checkpoint(hwnd);
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            if let Some(journal) = window.journal.as_mut() {
                journal.ending();
            }
        }
    });
}

pub fn finish(hwnd: HWND) {
    let Some(mut window) = super::WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let result = (|| -> Result<()> {
        let saved = snapshot(hwnd, &window)?;
        if let Some(journal) = window.journal.as_mut() {
            journal.finish(saved).map_err(error)?;
        }
        Ok(())
    })();
    super::WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if let Err(error) = result {
        eprintln!("native session finish: {error}");
    }
}
