use windows::core::{w, Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, LOGFONTW};
use windows::Win32::System::Com::{
    CLSIDFromString, CoCreateInstance, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, STGM_READ,
    STGM_SHARE_DENY_NONE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    AssocQueryStringW, IInitializeWithItem, IPreviewHandler, IPreviewHandlerVisuals, IShellItem,
    SHCreateItemFromParsingName, SHCreateStreamOnFileEx, ASSOCF_INIT_DEFAULTTOSTAR,
    ASSOCF_NOTRUNCATE, ASSOCSTR_SHELLEXTENSION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, MoveWindow, RegisterClassW, WINDOW_EX_STYLE,
    WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};

use super::{com_init, wide};

/// The pane's background, which the handlers are asked to use too (0x00BBGGRR).
const BACKGROUND: u32 = 0x0020_2020;

pub struct Preview {
    handler: IPreviewHandler,
    host: HWND,
}

fn err(e: windows::core::Error) -> String {
    e.message()
}

/// The preview handler registered for the file's type, if any.
fn handler_for(path: &str) -> Option<GUID> {
    let ext = std::path::Path::new(path).extension()?;
    let ext = wide(&format!(".{}", ext.to_string_lossy()));
    let iid = wide(&format!("{{{:?}}}", IPreviewHandler::IID));
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_DEFAULTTOSTAR | ASSOCF_NOTRUNCATE,
            ASSOCSTR_SHELLEXTENSION,
            PCWSTR(ext.as_ptr()),
            PCWSTR(iid.as_ptr()),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
        CLSIDFromString(PCWSTR(buf.as_ptr())).ok()
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn host_class() -> PCWSTR {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    let name = w!("ExplorerPreviewHost");
    REGISTER.call_once(|| unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(host_proc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            hbrBackground: CreateSolidBrush(COLORREF(BACKGROUND)),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassW(&class);
    });
    name
}

/// Pictures, which Explorer previews itself (its 256px thumbnail) rather than through a handler.
pub fn is_image(path: &str) -> bool {
    use windows::Win32::UI::Shell::AssocGetPerceivedType;
    use windows::Win32::UI::Shell::Common::{PERCEIVED, PERCEIVED_TYPE_IMAGE};
    let Some(ext) = std::path::Path::new(path).extension() else {
        return false;
    };
    let ext = wide(&format!(".{}", ext.to_string_lossy()));
    let mut kind = PERCEIVED(0);
    let mut flags = 0u32;
    unsafe {
        AssocGetPerceivedType(PCWSTR(ext.as_ptr()), &mut kind, &mut flags, None).is_ok()
            && kind == PERCEIVED_TYPE_IMAGE
    }
}

/// Hands the file to its handler, whichever way it accepts it.
unsafe fn initialize(handler: &IPreviewHandler, path: &str) -> Result<(), String> {
    let w = wide(path);
    unsafe {
        if let Ok(init) = handler.cast::<IInitializeWithFile>() {
            return init
                .Initialize(PCWSTR(w.as_ptr()), STGM_READ.0)
                .map_err(err);
        }
        if let Ok(init) = handler.cast::<IInitializeWithItem>() {
            let item: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).map_err(err)?;
            return init.Initialize(&item, STGM_READ.0).map_err(err);
        }
        let init = handler.cast::<IInitializeWithStream>().map_err(err)?;
        let stream = SHCreateStreamOnFileEx(
            PCWSTR(w.as_ptr()),
            (STGM_READ | STGM_SHARE_DENY_NONE).0,
            0,
            false,
            None,
        )
        .map_err(err)?;
        init.Initialize(&stream, STGM_READ.0).map_err(err)
    }
}

/// Previews the file in `rect` (physical pixels, in `parent`'s client area).
pub fn open(parent: HWND, rect: RECT, path: &str) -> Result<Preview, String> {
    com_init();
    let clsid = handler_for(path).ok_or_else(|| "sem visualizador".to_string())?;
    unsafe {
        // Most handlers run in the preview surrogate (prevhost.exe), out of our process.
        let handler: IPreviewHandler = CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)
            .or_else(|_| CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER))
            .map_err(err)?;
        initialize(&handler, path)?;
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let host = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            host_class(),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            rect.left,
            rect.top,
            w,
            h,
            Some(parent),
            None,
            None,
            None,
        )
        .map_err(err)?;
        if let Ok(visuals) = handler.cast::<IPreviewHandlerVisuals>() {
            let _ = visuals.SetBackgroundColor(COLORREF(BACKGROUND));
            let _ = visuals.SetTextColor(COLORREF(0x00ff_ffff));
            let mut font = LOGFONTW {
                lfHeight: -12,
                lfWeight: 400,
                ..Default::default()
            };
            for (i, c) in "Segoe UI".encode_utf16().enumerate() {
                font.lfFaceName[i] = c;
            }
            let _ = visuals.SetFont(&font);
        }
        let preview = Preview { handler, host };
        preview
            .handler
            .SetWindow(
                host,
                &RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                },
            )
            .map_err(err)?;
        preview.handler.DoPreview().map_err(err)?;
        Ok(preview)
    }
}

impl Preview {
    pub fn set_rect(&self, rect: RECT) {
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        unsafe {
            if let Err(error) = MoveWindow(self.host, rect.left, rect.top, w, h, true) {
                eprintln!("preview host resize: {error}");
            }
            if let Err(error) = self.handler.SetRect(&RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            }) {
                eprintln!("preview handler resize: {error}");
            }
        }
    }

    pub fn contains(&self, hwnd: HWND) -> bool {
        self.host == hwnd
            || unsafe { windows::Win32::UI::WindowsAndMessaging::IsChild(self.host, hwnd) }
                .as_bool()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        unsafe {
            let _ = self.handler.Unload();
            let _ = DestroyWindow(self.host);
        }
    }
}
