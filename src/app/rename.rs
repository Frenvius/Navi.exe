use std::collections::HashMap;
use windows::Win32::Foundation::HWND;

pub struct Batch {
    pub done: Vec<(String, String)>,
    pub errors: Vec<String>,
}

pub fn many(owner: HWND, edited: &str, paths: &[String], text: &str) -> Batch {
    let extension = |value: &str| {
        let path = std::path::Path::new(value);
        if path.is_dir() {
            String::new()
        } else {
            path.extension()
                .map(|value| format!(".{}", value.to_string_lossy()))
                .unwrap_or_default()
        }
    };
    let edited_ext = extension(edited);
    let stem =
        if !edited_ext.is_empty() && text.to_lowercase().ends_with(&edited_ext.to_lowercase()) {
            &text[..text.len() - edited_ext.len()]
        } else {
            text
        };
    let mut counters: HashMap<String, u32> = HashMap::new();
    let mut result = Batch {
        done: Vec::new(),
        errors: Vec::new(),
    };
    let start = paths
        .iter()
        .position(|path| super::ids::same(path, edited))
        .unwrap_or(0);
    for path in paths[start..].iter().chain(paths[..start].iter()) {
        let ext = extension(path);
        let parent = std::path::Path::new(path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""));
        let counter = counters.entry(ext.to_lowercase()).or_insert(0);
        let name = loop {
            *counter += 1;
            let name = format!("{stem} ({counter}){ext}");
            let target = parent.join(&name);
            if !target.exists()
                || paths
                    .iter()
                    .any(|path| std::path::Path::new(path) == target)
            {
                break name;
            }
        };
        match crate::shell::ops::rename(owner, path, &name) {
            Ok(next) => result.done.push((path.clone(), next)),
            Err(error) => result.errors.push(error.to_string()),
        }
    }
    result
}
