use crate::shell::listing::ShellEntry;

pub fn matches(entry: &ShellEntry, kind: u8) -> bool {
    if kind == 0 {
        return true;
    }
    if entry.is_dir {
        return false;
    }
    let ext = std::path::Path::new(&entry.path)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let any = |list: &[&str]| list.contains(&ext.as_str());
    match kind {
        1 => any(&["doc", "docx", "docm", "dot", "dotx", "dotm", "rtf", "odt"]),
        2 => any(&[
            "xls", "xlsx", "xlsm", "xlsb", "xlt", "xltx", "xltm", "csv", "ods",
        ]),
        3 => any(&[
            "ppt", "pptx", "pptm", "pps", "ppsx", "ppsm", "pot", "potx", "potm", "odp",
        ]),
        4 | 5 => crate::shell::assoc::perceived(&ext) == if kind == 4 { "image" } else { "video" },
        6 => ext == "pdf",
        _ => true,
    }
}

pub fn filter_items(current: u8) -> Vec<super::menu_model::Item> {
    [50292u32, 50293, 50294, 50295, 50296, 50313, 50314]
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            super::menu_model::item(
                &crate::i18n::system_string("explorerframe.dll", id),
                super::menu_model::Icon::None,
                super::menu_model::Action::HomeFilter(index as u8),
            )
            .bullet(current == index as u8)
        })
        .collect()
}
