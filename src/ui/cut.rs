use std::collections::HashSet;
use windows::core::{w, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium};

pub fn paths() -> Result<HashSet<String>> {
    unsafe {
        let format = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        if format == 0 {
            return Err(windows::core::Error::from_win32());
        }
        if IsClipboardFormatAvailable(format).is_err() {
            return Ok(HashSet::new());
        }
        let data = OleGetClipboard()?;
        let format = FORMATETC {
            cfFormat: format as u16,
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
            ..Default::default()
        };
        let Ok(mut medium) = data.GetData(&format) else {
            return Ok(HashSet::new());
        };
        let global = medium.u.hGlobal;
        let mut cut = false;
        if GlobalSize(global) >= 4 {
            let pointer = GlobalLock(global).cast::<u32>();
            if !pointer.is_null() {
                cut = pointer.read_unaligned() & 2 != 0;
                if let Err(error) = GlobalUnlock(global) {
                    if error.code() != windows::Win32::Foundation::ERROR_SUCCESS.to_hresult() {
                        eprintln!("native clipboard unlock: {error}");
                    }
                }
            }
        }
        ReleaseStgMedium(&mut medium);
        Ok(if cut {
            crate::shell::ops::dropped_files(&data)
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        })
    }
}

pub fn update(hwnd: HWND) {
    super::chrome::clipboard(hwnd);
    match paths() {
        Ok(paths) => super::WINDOW.with(|slot| {
            if let Some(w) = slot.borrow_mut().as_mut() {
                if w.list.cut != paths {
                    w.list.cut = paths;
                    w.list.interaction.mark_all();
                }
            }
        }),
        Err(error) => eprintln!("native clipboard state: {error}"),
    }
    super::input::flush(hwnd);
}
