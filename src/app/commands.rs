use crate::shell::ops;
#[derive(Clone, Copy)]
pub enum FileAction {
    Cut,
    Copy,
    Paste,
    Delete,
    DeletePermanently,
    Share,
}

pub fn perform_file_action(
    hwnd: windows::Win32::Foundation::HWND,
    folder: &str,
    paths: &[String],
    action: FileAction,
) -> Result<Option<ops::Transfer>, String> {
    let (name, shift) = match action {
        FileAction::Cut => ("cut", false),
        FileAction::Copy => ("copy", false),
        FileAction::Paste => ("paste", false),
        FileAction::Delete => ("delete", false),
        FileAction::DeletePermanently => ("delete", true),
        FileAction::Share => {
            if paths.is_empty() {
                return Ok(None);
            }
            let title = if paths.len() == 1 {
                std::path::Path::new(&paths[0])
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| crate::i18n::tr("cmd.share").to_owned())
            } else {
                super::format::count(paths.len(), "count.item", "count.items")
            };
            crate::shell::share::show(hwnd, paths, &title).map_err(|error| error.message())?;
            return Ok(None);
        }
    };
    if !matches!(action, FileAction::Paste) && paths.is_empty() {
        return Ok(None);
    }
    let sources = if matches!(action, FileAction::Paste) {
        ops::clipboard_files()
    } else {
        Vec::new()
    };
    let before = (!sources.is_empty())
        .then(|| ops::snapshot(folder))
        .flatten();
    ops::invoke_verb(hwnd, folder, paths, name, shift)?;
    Ok(before.map(|before| ops::transfer(&sources, folder, &before)))
}

pub(crate) fn archive(
    paths: Vec<String>,
    ext: &'static str,
    done: impl FnOnce(Result<Option<String>, String>) + Send + 'static,
) {
    if paths.is_empty() {
        return;
    }
    let clsid = match ext {
        ".7z" => "{21DEF7E5-832F-44D1-9109-E1161753B062}",
        ".tar" => "{3DA1A190-B430-49F5-86BF-765D29F7B9F0}",
        _ => "{812EE37B-5065-4C14-B0D1-A2266454B0C1}",
    };
    match ops::explorer_command(clsid, &paths) {
        Ok(()) => done(Ok(None)),
        Err(error) if ext.is_empty() => done(Err(error)),
        Err(_) => {
            std::thread::spawn(move || done(ops::compress_with_tar(&paths, ext).map(Some)));
        }
    }
}

#[derive(Clone, Copy)]
pub enum Pane {
    Details,
    Preview,
    Navigation,
}

pub fn toggle_pane(pane: Pane) -> (bool, bool) {
    let current = crate::shell::listing::panes();
    let next = match pane {
        Pane::Navigation => {
            if let Err(error) = crate::shell::nav_options::set_pane_visible(
                !crate::shell::nav_options::pane_visible(),
            ) {
                eprintln!("navigation pane visibility: {error}");
            }
            return current;
        }
        Pane::Details => (!current.0, false),
        Pane::Preview => (false, !current.1),
    };
    crate::shell::listing::set_panes(next.0, next.1);
    next
}
