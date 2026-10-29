use std::sync::Arc;

use crate::i18n::tr;
use crate::shell::{icons, listing, listing::ShellEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Details,
    Preview,
}

#[derive(Clone)]
pub enum Selection {
    Folder {
        path: String,
        name: String,
        icon: u32,
        count: usize,
    },
    Item {
        parent: String,
        entry: ShellEntry,
    },
    Multiple(usize),
}

#[derive(Clone)]
pub struct Row {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Default)]
pub struct Model {
    pub name: String,
    pub icon: Option<Arc<icons::Rgba>>,
    pub picture: Option<Arc<icons::Rgba>>,
    pub large_picture: bool,
    pub rows: Vec<Row>,
    pub info: String,
    pub can_share: bool,
    pub can_properties: bool,
    pub preview_path: Option<String>,
    pub preview_message: String,
}

fn icon(index: u32, size: u32) -> Option<Arc<icons::Rgba>> {
    match icons::icon(index & 0x00ff_ffff, size) {
        Ok(image) => Some(Arc::new(image)),
        Err(error) => {
            eprintln!("pane icon {index}/{size}: {error}");
            None
        }
    }
}

fn picture(path: &str, index: u32) -> Option<Arc<icons::Rgba>> {
    match icons::pane_picture(path, 96) {
        Ok(image) => Some(Arc::new(image)),
        Err(error) => {
            eprintln!("pane picture {path}: {error}; using its system icon");
            icon(index, 96)
        }
    }
}

fn stack() -> Option<Arc<icons::Rgba>> {
    match icons::stack_picture() {
        Ok(image) => Some(Arc::new(image)),
        Err(error) => {
            eprintln!("pane stack: {error}");
            icon(icons::stack_icon(), 96)
        }
    }
}

pub fn load(kind: Kind, selection: Selection, width: u32) -> Model {
    let mut model = Model::default();
    if kind == Kind::Preview {
        match selection {
            Selection::Item { entry, .. } if !entry.is_dir && entry.is_fs => {
                if crate::shell::preview::is_image(&entry.path) {
                    match icons::preview_picture(&entry.path) {
                        Ok(image) => model.picture = Some(Arc::new(image)),
                        Err(error) => {
                            eprintln!("preview image {}: {error}", entry.path);
                            model.preview_message = tr("preview.failed").into();
                        }
                    }
                } else if listing::settings().preview_handlers {
                    model.preview_path = Some(entry.path);
                } else {
                    model.preview_message = tr("preview.none").into();
                }
            }
            Selection::Folder { .. } => model.preview_message = tr("preview.select").into(),
            _ => model.preview_message = tr("preview.none").into(),
        }
        return model;
    }
    let row = |label: &str, value: String| Row {
        label: label.into(),
        value,
    };
    match selection {
        Selection::Item { parent, entry } => {
            let e = entry;
            model.name = if super::ids::same(&parent, super::ids::RECYCLE_BIN) {
                listing::property(&e.path, "System.Recycle.DeletedFrom")
                    .filter(|(_, value)| !value.is_empty())
                    .map(|(_, value)| format!("{}\\{}", value.trim_end_matches('\\'), e.name))
                    .unwrap_or_else(|| e.name.clone())
            } else {
                e.name.clone()
            };
            if !e.is_dir {
                model.icon = match icons::pane_icon(&e.path) {
                    Ok(image) => Some(Arc::new(image)),
                    Err(error) => {
                        eprintln!("pane title icon {}: {error}", e.path);
                        icon(e.icon, 32)
                    }
                };
                if e.is_fs {
                    if let Ok(image) = icons::pane_thumbnail(&e.path, width.max(1), 220) {
                        model.picture = Some(Arc::new(image));
                        model.large_picture = true;
                    }
                }
            }
            if model.picture.is_none() {
                model.picture = picture(&e.path, e.icon);
            }
            if super::ids::same(&parent, super::ids::NETWORK) && !e.is_fs {
                model.rows.extend(
                    listing::prop_list_with_id(&e.path, listing::PropList::Pane, &e.absolute)
                        .into_iter()
                        .flatten()
                        .map(|(label, value)| Row { label, value }),
                );
            } else {
                model.rows.push(row(tr("column.type"), e.type_name.clone()));
            }
            let archived = crate::shell::archive::root(&e.path).is_some();
            if !e.is_dir && (e.is_fs || archived) {
                model
                    .rows
                    .push(row(tr("column.size"), super::format::short_size(e.size)));
            }
            if e.is_fs || archived {
                model.rows.push(row(
                    tr("column.location"),
                    listing::folder_path_display(&e.path)
                        .unwrap_or_else(|| super::ids::parent_of(&e.path)),
                ));
            }
            if e.modified > 0 {
                model.rows.push(row(
                    tr("column.modified"),
                    super::format::modified(e.modified),
                ));
            }
            if archived && !e.is_dir {
                for key in ["System.ZipFolder.CompressedSize", "System.ZipFolder.Ratio"] {
                    if let Some((label, value)) = listing::property(&e.path, key) {
                        model.rows.push(Row { label, value });
                    }
                }
            }
            if e.is_fs && !e.is_dir && crate::shell::preview::is_image(&e.path) {
                if let Some((label, value)) = listing::property(&e.path, "System.Image.Dimensions")
                {
                    model.rows.push(Row { label, value });
                }
            }
            model.can_share = (e.is_fs || archived) && !e.is_dir;
            model.can_properties =
                if super::ids::same(&parent, super::ids::NETWORK) && !e.absolute.is_empty() {
                    unsafe {
                        use windows::Win32::System::SystemServices::SFGAO_HASPROPSHEET;
                        use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromIDList};
                        SHCreateItemFromIDList::<IShellItem>(e.absolute.as_ptr().cast())
                            .and_then(|item| item.GetAttributes(SFGAO_HASPROPSHEET))
                            .map(|flags| flags.0 & SFGAO_HASPROPSHEET.0 != 0)
                            .unwrap_or(false)
                    }
                } else {
                    true
                };
        }
        Selection::Folder {
            path,
            name,
            icon: index,
            count,
        } => {
            let name =
                super::ids::search_parts(&path).map_or(name.as_str(), |(_, query)| query.trim());
            let name = if crate::shell::archive::is_root(&path) {
                std::path::Path::new(name)
                    .file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.into())
            } else {
                name.into()
            };
            model.name = format!(
                "{name} ({})",
                super::format::count(count, "count.item", "count.items")
            );
            model.picture = if super::ids::is_search(&path) {
                stack()
            } else {
                picture(&path, index)
            };
            model.info = tr("pane.select-one").into();
        }
        Selection::Multiple(count) => {
            model.name = super::format::count(count, "count.selected", "count.selected-many");
            model.picture = stack();
            model.info = tr("pane.select-one").into();
        }
    }
    model
}
