use std::collections::HashMap;
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    LoadLibraryExW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
};
use windows::Win32::UI::WindowsAndMessaging::LoadStringW;

const EN: &str = include_str!("../translations/en/LC_MESSAGES/navi.po");
const PT_BR: &str = include_str!("../translations/pt-BR/LC_MESSAGES/navi.po");

/// The catalog for the display language: "pt-BR" or "en".
pub fn language() -> &'static str {
    static LANGUAGE: OnceLock<&str> = OnceLock::new();
    LANGUAGE.get_or_init(|| {
        let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        // PRIMARYLANGID: LANG_PORTUGUESE is 0x16.
        if lang & 0x3ff == 0x16 {
            "pt-BR"
        } else {
            "en"
        }
    })
}

fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| parse_po(if language() == "pt-BR" { PT_BR } else { EN }))
}

/// The text of a key in the display language.
pub fn tr(key: &str) -> &str {
    table()
        .get(key)
        .map(String::as_str)
        .filter(|t| !t.is_empty())
        .unwrap_or(key)
}

/// `tr` with its `{}` placeholders filled in order.
pub fn trf(key: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = tr(key);
    for arg in args {
        let Some(at) = rest.find("{}") else { break };
        out.push_str(&rest[..at]);
        out.push_str(arg);
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// `msgid`/`msgstr` pairs of a gettext catalog (single and continued lines, C escapes).
/// Entries with a `msgctxt` are kept under "context\u{4}id", as gettext does.
fn parse_po(po: &str) -> HashMap<String, String> {
    #[derive(PartialEq)]
    enum Part {
        None,
        Context,
        Id,
        Text,
    }
    let mut table = HashMap::new();
    let (mut ctxt, mut id, mut text) = (String::new(), String::new(), String::new());
    let mut part = Part::None;
    let mut flush = |ctxt: &mut String, id: &mut String, text: &mut String| {
        if !id.is_empty() {
            let key = if ctxt.is_empty() {
                id.clone()
            } else {
                format!("{ctxt}\u{4}{id}")
            };
            table.insert(key, std::mem::take(text));
        }
        ctxt.clear();
        id.clear();
        text.clear();
    };
    for line in po.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("msgctxt ") {
            flush(&mut ctxt, &mut id, &mut text);
            ctxt = unquote(rest);
            part = Part::Context;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if part != Part::Context {
                flush(&mut ctxt, &mut id, &mut text);
            }
            id = unquote(rest);
            part = Part::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            text = unquote(rest);
            part = Part::Text;
        } else if line.starts_with('"') {
            match part {
                Part::Context => ctxt.push_str(&unquote(line)),
                Part::Id => id.push_str(&unquote(line)),
                Part::Text => text.push_str(&unquote(line)),
                Part::None => {}
            }
        }
    }
    flush(&mut ctxt, &mut id, &mut text);
    table
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// A string resource of a system DLL, in the display language (e.g. propsys.dll's group
/// names), without the direction marks some carry.
pub fn system_string(dll: &str, id: u32) -> String {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<(String, u32), String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&(dll.to_string(), id)) {
        return hit.clone();
    }
    let text = load_string(dll, id);
    cache
        .lock()
        .unwrap()
        .insert((dll.to_string(), id), text.clone());
    text
}

fn load_string(dll: &str, id: u32) -> String {
    static MODULES: OnceLock<std::sync::Mutex<HashMap<String, isize>>> = OnceLock::new();
    let modules = MODULES.get_or_init(Default::default);
    let module = {
        let mut map = modules.lock().unwrap();
        *map.entry(dll.to_string()).or_insert_with(|| {
            let name: Vec<u16> = dll.encode_utf16().chain(Some(0)).collect();
            unsafe {
                LoadLibraryExW(
                    PCWSTR(name.as_ptr()),
                    None,
                    LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
                )
            }
            .map(|h| h.0 as isize)
            .unwrap_or(0)
        })
    };
    if module == 0 {
        return String::new();
    }
    let mut buf = [0u16; 512];
    let n = unsafe {
        LoadStringW(
            Some(HMODULE(module as _).into()),
            id,
            windows::core::PWSTR(buf.as_mut_ptr()),
            buf.len() as i32,
        )
    };
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).replace(['\u{200e}', '\u{200f}'], "")
}

/// A FormatMessage-style resource ("%1!ls! free of %2!ls!", "%2!u! mais guias") with its
/// inserts filled, whatever their format.
pub fn system_format(dll: &str, id: u32, args: &[&str]) -> String {
    let mut text = system_string(dll, id);
    for (i, arg) in args.iter().enumerate() {
        let insert = format!("%{}", i + 1);
        while let Some(at) = text.find(&insert) {
            let mut end = at + insert.len();
            // A "!format!" after the number goes with it.
            if text[end..].starts_with('!') {
                if let Some(close) = text[end + 1..].find('!') {
                    end += close + 2;
                }
            }
            text.replace_range(at..end, arg);
        }
    }
    text
}

/// A printf-style shell resource ("Nova pasta", "Resultados da Pesquisa em %s").
pub fn shell_string(id: u32, arg: &str) -> String {
    system_string("shell32.dll", id).replacen("%s", arg, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn used_keys() -> BTreeSet<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut keys = BTreeSet::new();
        let mut files = Vec::new();
        for dir in ["src", "src/app", "src/shell", "src/ui"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        let is_key = |s: &str| {
            s.contains('.')
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'))
        };
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            for quoted in text.split('"') {
                let prefix = quoted.split('.').next().unwrap_or("");
                if is_key(quoted) && PREFIXES.contains(&prefix) {
                    keys.insert(quoted.to_string());
                }
            }
        }
        keys
    }

    const PREFIXES: [&str; 18] = [
        "address", "app", "cmd", "column", "context", "count", "empty", "file", "key", "menu",
        "nav", "pane", "preview", "status", "tab", "tiles", "view", "settings",
    ];

    /// Both catalogs have text for the same keys, every key the code uses, and only those
    /// (the failure lists what's missing where).
    #[test]
    fn catalogs_complete() {
        let used = used_keys();
        let mut problems = Vec::new();
        for (lang, po) in [("en", EN), ("pt-BR", PT_BR)] {
            let table = parse_po(po);
            for key in &used {
                if table.get(key).is_none_or(|t| t.is_empty()) {
                    problems.push(format!("{lang}: missing {key}"));
                }
            }
            for key in table.keys().filter(|k| !k.is_empty() && !used.contains(*k)) {
                problems.push(format!("{lang}: unused {key}"));
            }
        }
        problems.sort();
        assert!(
            problems.is_empty(),
            "
{}",
            problems.join(
                "
"
            )
        );
    }
}

/// A command's description in Explorer's CommandStore (HKLM, `shell\<command>`), resolved
/// from its resource reference ("@shell32.dll,-37469").
pub fn command_description(command: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    use windows::Win32::UI::Shell::SHLoadIndirectString;
    let key: Vec<u16> =
        format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\CommandStore\shell\{command}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
    let value: Vec<u16> = "Description".encode_utf16().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
        let source = buf.clone();
        let mut out = vec![0u16; 1024];
        if SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut out, None).is_err() {
            return None;
        }
        let n = out.iter().position(|&c| c == 0).unwrap_or(out.len());
        Some(String::from_utf16_lossy(&out[..n])).filter(|s| !s.is_empty())
    }
}
