use std::path::Path;
use windows::{
    core::{Result, PCWSTR},
    Win32::{
        Foundation::*,
        System::SystemServices::{SS_LEFT, SS_NOPREFIX},
        UI::WindowsAndMessaging::*,
    },
};

const COMMAND: u16 = 100;
const COPY: u16 = 101;
const DATA: WINDOW_LONG_PTR_INDEX =
    WINDOW_LONG_PTR_INDEX((2 * std::mem::size_of::<isize>()) as i32);

struct Warning {
    commands: String,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn dword(words: &mut Vec<u16>, value: u32) {
    words.extend([value as u16, (value >> 16) as u16]);
}

fn control(words: &mut Vec<u16>, class: u16, id: u16, bounds: [u16; 4], style: u32, caption: &str) {
    if words.len() % 2 != 0 {
        words.push(0);
    }
    dword(words, WS_CHILD.0 | WS_VISIBLE.0 | style);
    dword(words, 0);
    words.extend(bounds);
    words.extend([id, 0xffff, class]);
    words.extend(wide(caption));
    words.push(0);
}

fn template() -> Vec<u32> {
    let mut words = Vec::new();
    dword(
        &mut words,
        WS_POPUP.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | DS_MODALFRAME as u32
            | DS_SETFONT as u32
            | DS_CENTER as u32,
    );
    dword(&mut words, 0);
    words.extend([5, 0, 0, 432, 240, 0, 0]);
    words.extend(wide(crate::i18n::tr("settings.host.warning.title")));
    words.push(9);
    words.extend(wide("Segoe UI"));
    control(
        &mut words,
        0x82,
        99,
        [10, 10, 412, 128],
        SS_LEFT.0 | SS_NOPREFIX.0,
        crate::i18n::tr("settings.host.warning.body"),
    );
    control(
        &mut words,
        0x81,
        COMMAND,
        [10, 142, 412, 46],
        WS_TABSTOP.0
            | WS_BORDER.0
            | WS_HSCROLL.0
            | ES_MULTILINE as u32
            | ES_READONLY as u32
            | ES_AUTOHSCROLL as u32,
        "",
    );
    control(
        &mut words,
        0x80,
        COPY,
        [10, 207, 110, 20],
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        crate::i18n::tr("settings.host.warning.copy"),
    );
    control(
        &mut words,
        0x80,
        IDOK.0 as u16,
        [180, 207, 140, 20],
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        crate::i18n::tr("settings.host.warning.enable"),
    );
    control(
        &mut words,
        0x80,
        IDCANCEL.0 as u16,
        [330, 207, 92, 20],
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        crate::i18n::tr("settings.host.warning.cancel"),
    );
    if words.len() % 2 != 0 {
        words.push(0);
    }
    words
        .chunks_exact(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(pair[1]) << 16))
        .collect()
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, DATA, lparam.0);
            let warning = &*(lparam.0 as *const Warning);
            let value = wide(&warning.commands);
            let _ = SetDlgItemTextW(hwnd, i32::from(COMMAND), PCWSTR(value.as_ptr()));
            1
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as u16;
            if id == COPY {
                let warning = GetWindowLongPtrW(hwnd, DATA) as *const Warning;
                if let Some(warning) = warning.as_ref() {
                    crate::app::clipboard::set_text(hwnd, &warning.commands);
                }
                return 1;
            }
            if id == IDOK.0 as u16 || id == IDCANCEL.0 as u16 {
                let _ = EndDialog(hwnd, id as isize);
                return 1;
            }
            0
        }
        WM_CLOSE => {
            let _ = EndDialog(hwnd, IDCANCEL.0 as isize);
            1
        }
        _ => 0,
    }
}

pub(super) fn confirm(hwnd: HWND, broker: &Path) -> Result<bool> {
    show(hwnd, broker, Some(procedure))
}

fn show(hwnd: HWND, broker: &Path, callback: DLGPROC) -> Result<bool> {
    let warning = Warning {
        commands: crate::explorer_host::exclusion_commands(broker),
    };
    let template = template();
    let result = unsafe {
        DialogBoxIndirectParamW(
            None,
            template.as_ptr().cast(),
            Some(hwnd),
            callback,
            LPARAM(&warning as *const Warning as isize),
        )
    };
    if result == -1 {
        Err(windows::core::Error::from_win32())
    } else {
        Ok(result == IDOK.0 as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static INSPECTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    unsafe extern "system" fn cancel(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> isize {
        let result = procedure(hwnd, message, wparam, lparam);
        if message == WM_INITDIALOG {
            let inspected = (|| -> Result<bool> {
                let edit = GetDlgItem(Some(hwnd), i32::from(COMMAND))?;
                let cancel = GetDlgItem(Some(hwnd), IDCANCEL.0)?;
                let mut content = [0u16; 2048];
                let length = GetWindowTextW(edit, &mut content);
                let content = String::from_utf16_lossy(&content[..length as usize]);
                Ok(GetWindowLongW(edit, GWL_STYLE) & ES_READONLY != 0
                    && GetWindowLongW(cancel, GWL_STYLE) & BS_DEFPUSHBUTTON != 0
                    && content.contains("Add-MpPreference -ExclusionPath '")
                    && content
                        .contains("Add-MpPreference -ExclusionProcess 'navi-explorer-host.exe'"))
            })()
            .unwrap_or(false);
            INSPECTED.with(|value| value.set(inspected));
            let _ = PostMessageW(
                Some(hwnd),
                WM_COMMAND,
                WPARAM(IDCANCEL.0 as usize),
                LPARAM(0),
            );
        }
        result
    }

    #[test]
    fn native_warning_shows_readonly_commands_and_cancel_never_enables_the_host() -> Result<()> {
        let path = Path::new(r"C:\Program Files\NaviExplorerHost\navi-explorer-host.exe");
        assert!(!show(HWND::default(), path, Some(cancel))?);
        assert!(INSPECTED.with(std::cell::Cell::get));
        Ok(())
    }
}
