pub fn root(path: &str) -> Option<&str> {
    let lower = path.to_ascii_lowercase();
    let mut start = 0;
    while let Some(found) = lower[start..].find(".zip") {
        let end = start + found + 4;
        if (end == path.len() || matches!(path.as_bytes().get(end), Some(b'\\' | b'/')))
            && std::path::Path::new(&path[..end]).is_file()
        {
            return Some(&path[..end]);
        }
        start = end;
    }
    None
}
pub fn is_root(path: &str) -> bool {
    root(path).is_some_and(|root| root.len() == path.trim_end_matches(['\\', '/']).len())
}
