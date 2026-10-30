pub enum Submission {
    Navigate(String),
    Open(String),
    Execute(String),
}

pub fn resolve(text: &str, path: &str, location: &str) -> Option<Submission> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() {
        return None;
    }
    if text == path || text == location {
        crate::shell::address::push_typed_path(text);
        return None;
    }
    let mut expanded = expand_environment(text);
    if crate::shell::Pidl::parse(&expanded).is_err() {
        let named = [String::new(), super::ids::THIS_PC.to_string()]
            .iter()
            .find_map(|parent| {
                crate::shell::listing::subfolders(parent)
                    .ok()?
                    .into_iter()
                    .find(|(name, _)| name.to_lowercase() == expanded.to_lowercase())
                    .map(|(_, path)| path)
            });
        if let Some(path) = named {
            expanded = path;
        }
    }
    match crate::shell::listing::crumbs(&expanded) {
        Ok(chain) => {
            crate::shell::address::push_typed_path(text);
            chain.last().map(|last| {
                let target = last.id.clone();
                if crate::shell::ops::is_folder(&target)
                    || target.is_empty()
                    || super::ids::is_virtual(&target)
                {
                    Submission::Navigate(target)
                } else {
                    Submission::Open(target)
                }
            })
        }
        Err(_) => Some(Submission::Execute(expanded)),
    }
}

pub fn execute(
    hwnd: windows::Win32::Foundation::HWND,
    submission: Submission,
    path: &str,
    text: &str,
) -> Option<String> {
    use crate::shell::ops;
    match submission {
        Submission::Navigate(target) => Some(target),
        Submission::Open(target) => {
            if let Err(error) = ops::open(hwnd, &target) {
                eprintln!("address open: {error}");
            }
            None
        }
        Submission::Execute(target) => {
            if ops::shell_execute(hwnd, &target, Some(path)).is_err() {
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
                let message = crate::shell::wide(&crate::i18n::system_format(
                    "shell32.dll",
                    12353,
                    &[text.trim().trim_matches('"')],
                ));
                let title = crate::shell::wide(crate::i18n::tr("app.title"));
                unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        windows::core::PCWSTR(message.as_ptr()),
                        windows::core::PCWSTR(title.as_ptr()),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            None
        }
    }
}

fn expand_environment(text: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    let wide = crate::shell::wide(text);
    let mut buffer = vec![0u16; 4096];
    let length = unsafe {
        ExpandEnvironmentStringsW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut buffer))
    } as usize;
    if length == 0 || length > buffer.len() {
        return text.to_string();
    }
    String::from_utf16_lossy(&buffer[..length - 1])
}

pub fn hidden_crumbs(widths: &[f32], available: f32) -> usize {
    let mut hidden = 0;
    if available > 0.0 {
        while hidden + 1 < widths.len()
            && widths[hidden..].iter().sum::<f32>() + if hidden > 0 { 56.0 } else { 0.0 }
                > available + 0.5
        {
            hidden += 1;
        }
    }
    hidden
}
