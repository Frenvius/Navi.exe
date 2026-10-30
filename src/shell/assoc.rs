use windows::core::{PCWSTR, PWSTR};
use windows::Win32::UI::Shell::{IAssocHandler, SHAssocEnumHandlers, ASSOC_FILTER_RECOMMENDED};

use super::icons::Rgba;
use super::{com_init, ops, take_pwstr, wide};

#[derive(Clone)]
pub struct App {
    pub name: String,
    pub icon: Option<Rgba>,
    handler: IAssocHandler,
}

fn icon_of(handler: &IAssocHandler) -> Option<Rgba> {
    unsafe {
        let mut path = PWSTR::null();
        let mut index = 0i32;
        handler.GetIconLocation(&mut path, &mut index).ok()?;
        let path = take_pwstr(path);
        if path.starts_with('@') {
            return super::icons::small_icon_at(&path);
        }
        super::icons::small_icon_at(&format!("{path},{index}"))
    }
}

/// The Microsoft Store's logo, for "Procurar na Microsoft Store".
pub fn store_icon() -> Option<Rgba> {
    super::icons::thumbnail(
        r"shell:AppsFolder\Microsoft.WindowsStore_8wekyb3d8bbwe!App",
        16,
    )
    .ok()
}

/// The name of the app that opens `ext` by default, as the shell shows it.
fn default_app_name(ext: &str) -> Option<String> {
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_FRIENDLYAPPNAME,
    };
    let w = wide(ext);
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_FRIENDLYAPPNAME,
            PCWSTR(w.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
    }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..n]))
}

pub fn has_verb(path: &str, verb: &str) -> bool {
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_PROGID,
    };
    use winreg::{enums::HKEY_CLASSES_ROOT, RegKey};
    let Some(ext) = std::path::Path::new(path).extension() else {
        return false;
    };
    let ext = wide(&format!(".{}", ext.to_string_lossy()));
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let found = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_PROGID,
            PCWSTR(ext.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .is_ok()
    };
    if !found {
        return false;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(0);
    let progid = String::from_utf16_lossy(&buf[..end]);
    RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(format!(r"{progid}\shell\{verb}"))
        .is_ok()
}

/// The recommended apps for the extension of `path`, the default one first.
pub fn apps_for(path: &str) -> Vec<App> {
    com_init();
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    if ext.is_empty() {
        return Vec::new();
    }
    let w = wide(&ext);
    let mut out = Vec::new();
    unsafe {
        let Ok(list) = SHAssocEnumHandlers(PCWSTR(w.as_ptr()), ASSOC_FILTER_RECOMMENDED) else {
            return out;
        };
        loop {
            let mut handlers = [None];
            let mut fetched = 0u32;
            if list.Next(&mut handlers, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(handler) = handlers[0].take() else {
                break;
            };
            let name = handler
                .GetUIName()
                .map(|p| take_pwstr(p))
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            out.push(App {
                name,
                icon: icon_of(&handler),
                handler,
            });
        }
    }
    if let Some(default) = default_app_name(&ext) {
        if let Some(i) = out
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(&default))
        {
            let app = out.remove(i);
            out.insert(0, app);
        }
    }
    out
}

/// Opens `paths` (sharing one folder) with the app.
pub fn open_with(
    app: &App,
    hwnd: windows::Win32::Foundation::HWND,
    paths: &[String],
) -> Result<(), String> {
    let data = ops::data_object(hwnd, paths)?;
    unsafe { app.handler.Invoke(&data).map_err(|e| e.message()) }
}

/// What the system perceives an extension's files as ("image", "video", "audio", "text"...),
/// or "" when it doesn't say.
pub fn perceived(ext: &str) -> &'static str {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::AssocGetPerceivedType;
    use windows::Win32::UI::Shell::Common::{
        PERCEIVED, PERCEIVED_TYPE_AUDIO, PERCEIVED_TYPE_DOCUMENT, PERCEIVED_TYPE_IMAGE,
        PERCEIVED_TYPE_TEXT, PERCEIVED_TYPE_VIDEO,
    };
    if ext.is_empty() {
        return "";
    }
    let dotted: Vec<u16> = format!(".{ext}").encode_utf16().chain(Some(0)).collect();
    let mut kind = PERCEIVED::default();
    let mut flags = 0u32;
    if unsafe { AssocGetPerceivedType(PCWSTR(dotted.as_ptr()), &mut kind, &mut flags, None) }
        .is_err()
    {
        return "";
    }
    match kind {
        PERCEIVED_TYPE_IMAGE => "image",
        PERCEIVED_TYPE_VIDEO => "video",
        PERCEIVED_TYPE_AUDIO => "audio",
        PERCEIVED_TYPE_TEXT => "text",
        PERCEIVED_TYPE_DOCUMENT => "document",
        _ => "",
    }
}
