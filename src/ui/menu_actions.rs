use super::{
    chrome::Command,
    menu::{Anchor, Origin},
    WINDOW,
};
use crate::app::menu_model::{self, Action, Context, Menu};
use windows::core::{Error, Result};
use windows::Win32::Foundation::{E_FAIL, HWND};

fn error(error: String) -> Error {
    Error::new(E_FAIL, error)
}

pub fn open_location(hwnd: HWND, path: String) -> Result<()> {
    let Some(parent) = std::path::Path::new(&path).parent() else {
        return Ok(());
    };
    super::navigate(
        hwnd,
        super::Navigation::Open(parent.to_string_lossy().into_owned()),
    )?;
    WINDOW
        .with(|slot| {
            slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                window.list.select_created(vec![path], &mut window.scroll)
            })
        })
        .map_err(error)
}

pub fn context(hwnd: HWND) -> Option<Context> {
    WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let folder = window.list.folder.as_ref()?;
        let preview_kind = matches!(
            window.chrome.pane_kind,
            Some(crate::app::commands::Pane::Preview)
        ) || window.chrome.pane_kind.is_none() && folder.panes.1;
        Some(Context {
            hwnd,
            path: window.tabs.active().location.tab.path().to_owned(),
            selected: folder
                .order
                .iter()
                .map(|&index| &folder.entries[index])
                .filter(|entry| {
                    window
                        .list
                        .interaction
                        .selection
                        .selected
                        .contains(&entry.path)
                })
                .cloned()
                .collect(),
            cols: folder.columns.clone(),
            menu_columns: folder.view.menu_columns.clone(),
            sort: if folder.view.search_ranked {
                crate::app::sort::Sort {
                    key: crate::app::sort::SortKey::from_property(
                        &windows::Win32::Storage::EnhancedStorage::PKEY_Search_Rank,
                    ),
                    descending: true,
                }
            } else {
                folder.view.sort
            },
            group: folder.view.group,
            view_mode: folder.view.mode,
            icon_size: folder.view.icon_size,
            compact: folder.settings.compact,
            nav_pane: window.navigation.visible,
            details_pane: !preview_kind,
            preview_pane: preview_kind,
            can_paste: window.chrome.can_paste,
            can_undo: !window.undo.is_empty(),
            undo_label: window.undo.last().map(crate::app::undo::operation_label),
            close_search_in_bar: window.chrome.has_command(Command::CloseSearch),
            new_items: Vec::new(),
            editor: if window.address.search_focus {
                Some(window.editors.search)
            } else if window.address.editing {
                Some(window.editors.address)
            } else {
                None
            },
        })
    })
}

fn new_items(context: &mut Context) {
    match crate::shell::newmenu::entries(context.hwnd, &context.path) {
        Ok(entries) => context.new_items = entries,
        Err(error) => eprintln!("native new menu entries: {error}"),
    }
}

pub fn command(hwnd: HWND, command: Command, origin: Origin) {
    let Some(mut context) = context(hwnd) else {
        return;
    };
    let anchor = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| {
            window
                .chrome
                .buttons
                .iter()
                .find(|button| button.command == command)
                .map(|button| Anchor::Dropdown {
                    left: button.left,
                    right: button.left + button.width,
                    bottom: 136.0,
                })
        })
    });
    let Some(anchor) = anchor else {
        return;
    };
    let (owner, items) = match command {
        Command::New => {
            new_items(&mut context);
            ("new", menu_model::new_command_items(&context))
        }
        Command::Filter => {
            let kind = WINDOW.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .and_then(|window| window.list.folder.as_ref())
                    .map_or(0, |folder| folder.home.filter)
            });
            ("filter", crate::app::home::filter_items(kind))
        }
        Command::Sort => ("sort", menu_model::sort_menu_items(&context)),
        Command::View => ("view", menu_model::view_items(&context, true)),
        Command::More => {
            let mut items = menu_model::more_items(&context);
            let search_overflow = WINDOW.with(|slot| {
                slot.borrow().as_ref().is_some_and(|window| {
                    crate::app::ids::is_search(&context.path)
                        && !window.chrome.has_command(Command::SearchOptions)
                })
            });
            if search_overflow {
                if let Some(options) = super::search::options() {
                    let query =
                        crate::app::ids::search_parts(&context.path).map_or("", |(_, query)| query);
                    items.insert(
                        0,
                        menu_model::item(
                            crate::i18n::tr("cmd.search-options"),
                            menu_model::Icon::Command("SearchAdvancedOptions"),
                            Action::None,
                        )
                        .submenu(menu_model::search_options_items(options, query)),
                    );
                }
            }
            ("more", items)
        }
        Command::SearchOptions => {
            let Some(options) = super::search::options() else {
                return;
            };
            let data = WINDOW.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|window| window.address.search_edit.text.clone())
            });
            let Some(query) = data else { return };
            (
                "search-options",
                menu_model::search_options_items(options, &query),
            )
        }
        _ => return,
    };
    let menu = Menu {
        owner,
        items,
        buttons: Vec::new(),
    };
    if let Err(error) = super::menu::show(hwnd, context, menu, anchor, origin) {
        super::fail(error);
    }
}

pub fn tab(hwnd: HWND, index: usize, point: (f32, f32), origin: Origin) {
    let Some(context) = context(hwnd) else {
        return;
    };
    let count = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(0, |window| window.tabs.entries.len())
    });
    let menu = Menu {
        owner: "tab",
        items: menu_model::tab_items(index, count),
        buttons: Vec::new(),
    };
    if let Err(error) =
        super::menu::show(hwnd, context, menu, Anchor::Point(point.0, point.1), origin)
    {
        super::fail(error);
    }
}

pub fn list(hwnd: HWND, point: (f32, f32), origin: Origin) {
    let Some(mut context) = context(hwnd) else {
        return;
    };
    if context
        .selected
        .iter()
        .any(|entry| crate::shell::network::is_computer(&entry.path))
    {
        let paths = context
            .selected
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let (x, y) = crate::frame::client_to_screen(hwnd, point.0 as i32, point.1 as i32);
        let shift = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) } < 0;
        match crate::shell::menu::show_network(hwnd, &context.path, &paths, x, y, shift) {
            Ok(Some(verb)) => match verb.as_str() {
                "open" => run(hwnd, context, Action::OpenSelection),
                "opennewtab" => run(hwnd, context, Action::NewTab(paths)),
                "opennewwindow" => run(hwnd, context, Action::NewWindow(paths)),
                _ => {
                    if let Err(error) = refresh(hwnd) {
                        eprintln!("native network menu refresh: {error}");
                    }
                }
            },
            Ok(None) => {}
            Err(error) => eprintln!("native network menu: {error}"),
        }
        return;
    }
    let menu = if context.selected.is_empty() {
        new_items(&mut context);
        menu_model::background_menu(&context, point.0, point.1)
    } else {
        menu_model::item_menu(&context, &context.selected, Vec::new(), point.0, point.1)
    };
    if let Err(error) = super::menu::show(
        hwnd,
        context,
        menu,
        Anchor::Context(point.0, point.1),
        origin,
    ) {
        super::fail(error);
    }
}

pub fn background(hwnd: HWND, point: (f32, f32), origin: Origin) {
    let Some(mut context) = context(hwnd) else {
        return;
    };
    context.selected.clear();
    new_items(&mut context);
    let menu = menu_model::background_menu(&context, point.0, point.1);
    if let Err(error) = super::menu::show(
        hwnd,
        context,
        menu,
        Anchor::Context(point.0, point.1),
        origin,
    ) {
        super::fail(error);
    }
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    let modifiers = super::input::modifiers();
    if modifiers.ctrl && modifiers.shift && key == 0x4e {
        if let Some(context) = context(hwnd) {
            run(hwnd, context, Action::NewFolder);
        }
        return true;
    }
    if modifiers.ctrl && matches!(key, 0x5a | 0x59) {
        if let Err(error) = history(
            hwnd,
            if key == 0x59 {
                History::Redo
            } else {
                History::Undo
            },
        ) {
            eprintln!("native history: {error}");
        }
        return true;
    }
    if key != 0x5d && !(key == 0x79 && modifiers.shift) {
        return false;
    }
    let point = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        if window.chrome.focus != super::chrome::Focus::List {
            return None;
        }
        let folder = window.list.folder.as_ref()?;
        let view = window.scroll.view?;
        if window.list.interaction.selection.selected.is_empty() {
            return Some((view.x, view.y));
        }
        let index = folder.order.iter().position(|&index| {
            Some(&folder.entries[index].path) == window.list.interaction.selection.focused.as_ref()
        })?;
        if let Some(grid) = &folder.grid {
            let cell = grid.layout.cells[index];
            Some((
                view.x + cell[0] - window.scroll.x + cell[2] / 2.0,
                view.y + view.header + cell[1] - window.scroll.y + 16.0,
            ))
        } else {
            Some((
                view.x + 29.0 - window.scroll.x,
                view.y + view.header + view.row_top(folder, index) - window.scroll.y
                    + view.plate / 2.0,
            ))
        }
    });
    let Some(point) = point else {
        return false;
    };
    list(hwnd, point, Origin::Keyboard);
    true
}

pub fn run(hwnd: HWND, context: Context, action: Action) {
    let paths: Vec<_> = context
        .selected
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let result = (|| -> Result<()> {
        match action {
            Action::None => {}
            Action::View(mode) => {
                super::grid::set_mode(hwnd, mode, crate::app::grid_layout::mode_icon_size(mode))
            }
            Action::CloseTab(index) => super::tabs::close(hwnd, index)?,
            Action::CloseOtherTabs(index) => {
                super::tabs::close_set(hwnd, index, super::tabs::CloseSet::Others)?
            }
            Action::CloseTabsRight(index) => {
                super::tabs::close_set(hwnd, index, super::tabs::CloseSet::Right)?
            }
            Action::DuplicateTab(index) => super::tabs::duplicate(hwnd, index)?,
            Action::History(index) => super::navigate(hwnd, super::Navigation::History(index))?,
            Action::NewTab(paths) => {
                for path in paths {
                    super::tabs::open_background(hwnd, &path)?;
                }
            }
            Action::NewWindow(paths) => {
                for path in paths {
                    super::create_window(&path, std::time::Instant::now())?;
                }
            }
            Action::OpenSelection => {
                for path in paths {
                    if let Some(path) = crate::shell::ops::open(hwnd, &path).map_err(error)? {
                        super::navigate(hwnd, super::Navigation::Open(path))?;
                        break;
                    }
                }
            }
            Action::Cut | Action::Copy | Action::Paste | Action::Delete | Action::Share => {
                use crate::app::commands::FileAction;
                let verb = match action {
                    Action::Cut => FileAction::Cut,
                    Action::Copy => FileAction::Copy,
                    Action::Paste => FileAction::Paste,
                    Action::Delete => FileAction::Delete,
                    _ => FileAction::Share,
                };
                file(hwnd, &context.path, &paths, verb)?;
            }
            Action::Undo => history(hwnd, History::Undo)?,
            Action::Rename => {
                let focused = WINDOW.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .and_then(|window| window.list.interaction.selection.focused.clone())
                });
                if let Some(path) = focused
                    .as_ref()
                    .filter(|path| paths.contains(path))
                    .or_else(|| paths.first())
                {
                    super::rename::begin(hwnd, path)?;
                }
            }
            Action::ToggleDetailsPane | Action::TogglePreviewPane => {
                let pane = if matches!(action, Action::ToggleDetailsPane) {
                    crate::app::commands::Pane::Details
                } else {
                    crate::app::commands::Pane::Preview
                };
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        window.chrome.pane_kind = Some(pane);
                    }
                });
                crate::app::commands::toggle_pane(pane);
                super::input::update_settings();
                super::invalidate_all(hwnd);
            }
            Action::ToggleNavPane => {
                crate::app::commands::toggle_pane(crate::app::commands::Pane::Navigation);
                super::input::update_settings();
                super::navigation_input::refresh();
                super::invalidate_all(hwnd);
            }
            Action::ToggleCompact => toggle(crate::shell::listing::FolderOption::Compact)?,
            Action::FolderOption(option) => toggle(option)?,
            Action::NavOption(option, on) => {
                crate::shell::nav_options::set(option, on).map_err(error)?;
                super::navigation_input::refresh();
            }
            Action::SortKey(key) => update_sort(
                hwnd,
                crate::app::sort::Sort {
                    key,
                    ..context.sort
                },
            )?,
            Action::SortDescending(descending) => update_sort(
                hwnd,
                crate::app::sort::Sort {
                    descending,
                    ..context.sort
                },
            )?,
            Action::GroupBy(key) => update_group(hwnd, key.map(crate::app::group::Group::new))?,
            Action::GroupDescending(descending) => {
                if let Some(group) = context.group {
                    update_group(
                        hwnd,
                        Some(crate::app::group::Group {
                            descending,
                            ..group
                        }),
                    )?;
                }
            }
            Action::ChooseDetails => super::header::choose(hwnd),
            Action::ShellMenu(paths, x, y) => {
                super::tabs_input::item_menu(hwnd, &context.path, &paths, (x, y))
            }
            Action::OpenWith(path) => crate::shell::ops::open_with(hwnd, &path).map_err(error)?,
            Action::OpenWithApp(app, paths) => {
                crate::shell::assoc::open_with(&app, hwnd, &paths).map_err(error)?
            }
            Action::CommandStore(name, paths) => {
                crate::shell::ops::command_store(hwnd, name, &paths).map_err(error)?
            }
            Action::AppCommand(id, index, paths) => {
                crate::shell::extmenu::invoke(&id, index, &paths).map_err(error)?
            }
            Action::Launch(target) => {
                crate::shell::ops::shell_execute(hwnd, &target, None).map_err(error)?
            }
            Action::ShellVerb(name, paths) => {
                crate::shell::ops::invoke_verb(hwnd, &context.path, &paths, &name, false)
                    .map_err(error)?
            }
            Action::SelectAll | Action::SelectNone | Action::InvertSelection => {
                WINDOW.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(window) = slot.as_mut() else {
                        return;
                    };
                    let Some(folder) = &window.list.folder else {
                        return;
                    };
                    let selection = &mut window.list.interaction.selection;
                    selection.selected = folder
                        .order
                        .iter()
                        .map(|&index| folder.entries[index].path.clone())
                        .filter(|path| match action {
                            Action::SelectAll => true,
                            Action::InvertSelection => !selection.selected.contains(path),
                            _ => false,
                        })
                        .collect();
                    if !matches!(action, Action::SelectAll) {
                        selection.anchor = None;
                        selection.focused = None;
                    }
                    window.list.interaction.mark_all();
                });
                super::chrome::sync(hwnd);
                super::input::flush(hwnd);
            }
            Action::CopyItems(paths) => file(
                hwnd,
                &context.path,
                &paths,
                crate::app::commands::FileAction::Copy,
            )?,
            Action::CopyText(text) => crate::app::clipboard::set_text(hwnd, &text),
            Action::CopyPath(paths) => {
                if !paths.is_empty() {
                    crate::app::clipboard::set_text(
                        hwnd,
                        &paths
                            .iter()
                            .map(|path| format!("\"{path}\""))
                            .collect::<Vec<_>>()
                            .join("\r\n"),
                    );
                }
            }
            Action::AddressCommand(command) => address_command(hwnd, context.editor, command),
            Action::ClearAddressHistory => crate::shell::address::clear_typed_paths(),
            Action::Pin(paths, on) => {
                for path in paths {
                    crate::shell::nav::set_pinned(&path, on).map_err(error)?;
                }
                super::navigation_input::refresh();
            }
            Action::PinSearch(id) => {
                if let Some((scope, query)) = crate::app::ids::search_parts(&id) {
                    let pidl = crate::shell::search::folder(scope, query, Default::default())
                        .map_err(error)?;
                    crate::shell::nav::set_pinned_pidl(&pidl, true).map_err(error)?;
                    super::navigation_input::refresh();
                }
            }
            Action::ToggleNav(row) => super::navigation_input::toggle(hwnd, row),
            Action::PinToStart(paths) => crate::shell::ops::invoke_verb(
                hwnd,
                &context.path,
                &paths,
                "pintostartscreen",
                false,
            )
            .map_err(error)?,
            Action::RunAsAdmin(paths) => {
                crate::shell::ops::invoke_verb(hwnd, &context.path, &paths, "runas", false)
                    .map_err(error)?
            }
            Action::Properties(paths) => {
                let (folder, paths) = if paths.is_empty() {
                    (
                        crate::app::ids::parent_of(&context.path),
                        vec![context.path.clone()],
                    )
                } else {
                    (context.path.clone(), paths)
                };
                crate::shell::ops::invoke_verb(hwnd, &folder, &paths, "properties", false)
                    .map_err(error)?;
            }
            Action::OpenLocation(path) => open_location(hwnd, path)?,
            Action::NewFolder => {
                let folder = crate::shell::library::save_folder(&context.path)
                    .unwrap_or(context.path.clone());
                if folder.is_empty() || crate::app::ids::is_virtual(&folder) {
                    return Ok(());
                }
                created(hwnd, crate::shell::ops::new_folder(&folder))?;
            }
            Action::NewItem(index) => {
                if let Some(entry) = context.new_items.get(index) {
                    new_item(hwnd, &context, &context.path, &entry.verb)?;
                }
            }
            Action::NewItemIn(folder, index) => {
                if let Some(entry) = context.new_items.get(index) {
                    new_item(hwnd, &context, &folder, &entry.verb)?;
                }
            }
            Action::Zip(paths) => {
                if !paths.is_empty() {
                    created(hwnd, crate::shell::ops::compress_to_zip(hwnd, &paths))?;
                }
            }
            Action::Archive(paths, extension) => {
                let owner = hwnd.0 as isize;
                let Some(mailbox) = mailbox() else {
                    return Ok(());
                };
                crate::app::commands::archive(paths, extension, move |created| {
                    post_created(owner, mailbox, created)
                });
            }
            Action::Rotate(paths, clockwise) => {
                crate::shell::ops::invoke_verb(
                    hwnd,
                    &context.path,
                    &paths,
                    if clockwise { "rotate90" } else { "rotate270" },
                    false,
                )
                .map_err(error)?;
                refresh(hwnd)?;
            }
            Action::Wallpaper(paths) => crate::shell::ops::invoke_verb(
                hwnd,
                &context.path,
                &paths,
                "setdesktopwallpaper",
                false,
            )
            .map_err(error)?,
            Action::FolderOptions => {
                let state = WINDOW.with(|slot| {
                    slot.borrow().as_ref().and_then(|window| {
                        window
                            .list
                            .folder
                            .as_ref()
                            .map(|folder| folder.view.snapshot())
                    })
                });
                if let Some(state) = state {
                    let owner = hwnd.0 as isize;
                    let Some(mailbox) = mailbox() else {
                        return Ok(());
                    };
                    crate::shell::folder_options::show(
                        hwnd,
                        context.path.clone(),
                        state,
                        WINDOW
                            .with(|slot| {
                                slot.borrow().as_ref().and_then(|window| {
                                    window.scroll.view.map(|view| {
                                        (view.right.round() as i32, view.bottom.round() as i32)
                                    })
                                })
                            })
                            .unwrap_or((0, 0)),
                        move |changed| {
                            if changed {
                                post_created(owner, mailbox, Ok(None));
                            }
                        },
                    )
                    .map_err(error)?;
                }
            }
            Action::GalleryFilter(index) => {
                let root = if index == 0 {
                    None
                } else {
                    Some(
                        crate::shell::gallery::collection_roots()
                            .get(index - 1)
                            .ok_or_else(|| {
                                Error::new(E_FAIL, "Gallery collection source unavailable")
                            })?
                            .clone(),
                    )
                };
                WINDOW
                    .with(|slot| {
                        slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                            window.scroll.x = 0.0;
                            window.scroll.y = 0.0;
                            window.list.gallery_filter(root)
                        })
                    })
                    .map_err(error)?;
                super::invalidate_all(hwnd);
            }
            Action::GalleryManage => {
                std::thread::Builder::new()
                    .name("gallery-locations".into())
                    .spawn(|| {
                        if let Err(error) = crate::shell::extmenu::invoke_store_dialog(
                            "Windows.GalleryManageLocationsCommand",
                        ) {
                            eprintln!("native Gallery locations: {error}");
                        }
                    })
                    .map_err(|failure| error(failure.to_string()))?;
            }
            Action::SetGroupOpen(row, open) => {
                let label = WINDOW.with(|slot| {
                    slot.borrow().as_ref().and_then(|window| {
                        window.list.folder.as_ref().and_then(|folder| {
                            folder
                                .layout
                                .headers
                                .get(row)
                                .map(|header| header.label.clone())
                        })
                    })
                });
                if let Some(label) = label {
                    expand(
                        hwnd,
                        vec![label],
                        if open {
                            crate::app::group::Expansion::Expanded
                        } else {
                            crate::app::group::Expansion::Collapsed
                        },
                    )?;
                }
            }
            Action::SetGroupsOpen(open) => {
                let labels = WINDOW
                    .with(|slot| {
                        slot.borrow().as_ref().and_then(|window| {
                            window.list.folder.as_ref().map(|folder| {
                                folder
                                    .layout
                                    .headers
                                    .iter()
                                    .map(|header| header.label.clone())
                                    .collect()
                            })
                        })
                    })
                    .unwrap_or_default();
                expand(
                    hwnd,
                    labels,
                    if open {
                        crate::app::group::Expansion::Expanded
                    } else {
                        crate::app::group::Expansion::Collapsed
                    },
                )?;
            }
            Action::RestoreBin(all) => {
                if all
                    && !crate::shell::ops::confirm(
                        hwnd,
                        &crate::shell::listing::display_name_of(crate::app::ids::RECYCLE_BIN),
                        &crate::i18n::system_string("shell32.dll", 16917),
                    )
                {
                    return Ok(());
                }
                let children = if all {
                    WINDOW
                        .with(|slot| {
                            slot.borrow().as_ref().and_then(|window| {
                                window.list.folder.as_ref().map(|folder| {
                                    folder
                                        .entries
                                        .iter()
                                        .map(|entry| entry.child.clone())
                                        .collect::<Vec<_>>()
                                })
                            })
                        })
                        .unwrap_or_default()
                } else {
                    context
                        .selected
                        .iter()
                        .map(|entry| entry.child.clone())
                        .collect()
                };
                crate::shell::ops::restore_from_recycle_bin(hwnd, &children).map_err(error)?;
                refresh(hwnd)?;
            }
            Action::CloseSearch => super::search::close(hwnd)?,
            Action::SearchRefine(keyword, value) => super::search::refine(hwnd, &keyword, &value)?,
            Action::SearchCurrentFolderOnly(on) => super::search::scope(hwnd, on)?,
            Action::SearchToggle(which) => super::search::toggle(hwnd, which)?,
            Action::CloudCommand(revision, slot) => super::cloud::invoke(revision, slot)?,
            Action::HomeFilter(kind) => {
                WINDOW
                    .with(|slot| {
                        slot.borrow_mut()
                            .as_mut()
                            .map_or(Ok(()), |window| window.list.home_filter(kind))
                    })
                    .map_err(error)?;
                super::invalidate_all(hwnd);
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("native menu action: {error}");
    }
}

fn update_sort(hwnd: HWND, sort: crate::app::sort::Sort) -> Result<()> {
    WINDOW
        .with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map_or(Ok(()), |window| window.list.sort(sort, &window.scroll))
        })
        .map_err(error)?;
    super::invalidate_all(hwnd);
    Ok(())
}

fn update_group(hwnd: HWND, group: Option<crate::app::group::Group>) -> Result<()> {
    WINDOW
        .with(|slot| {
            slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                window.list.set_group(group, &window.scroll)
            })
        })
        .map_err(error)?;
    super::invalidate_all(hwnd);
    Ok(())
}

fn toggle(option: crate::shell::listing::FolderOption) -> Result<()> {
    WINDOW
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(Ok(()), |window| window.list.toggle(option))
        })
        .map_err(error)
}

pub fn push(hwnd: HWND, operation: crate::app::undo::Undo) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.undo.push(operation);
            window.redo.clear();
        }
    });
    publish_history(hwnd);
}

pub fn file(
    hwnd: HWND,
    folder: &str,
    paths: &[String],
    action: crate::app::commands::FileAction,
) -> Result<()> {
    use crate::app::{
        commands::{perform_file_action, FileAction},
        undo::{self, Undo},
    };
    let transfer = perform_file_action(hwnd, folder, paths, action).map_err(error)?;
    if matches!(action, FileAction::Delete) {
        let deleted: Vec<_> = paths
            .iter()
            .filter(|path| !std::path::Path::new(path).exists())
            .cloned()
            .collect();
        if !deleted.is_empty() {
            push(hwnd, Undo::Delete(deleted));
        }
    }
    if let Some(transfer) = transfer {
        let created = transfer
            .copied
            .iter()
            .chain(transfer.moved.iter())
            .map(|(_, destination)| destination.clone())
            .collect();
        for operation in undo::transfers(transfer) {
            push(hwnd, operation);
        }
        WINDOW
            .with(|slot| {
                slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                    window.list.select_created(created, &mut window.scroll)
                })
            })
            .map_err(error)?;
        super::chrome::set_focus(hwnd, super::chrome::Focus::List);
    }
    super::cut::update(hwnd);
    Ok(())
}

enum History {
    Undo,
    Redo,
}

fn history(hwnd: HWND, direction: History) -> Result<()> {
    let operation = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| match direction {
            History::Undo => window.undo.last().cloned(),
            History::Redo => window.redo.last().cloned(),
        })
    });
    let Some(operation) = operation else {
        return Ok(());
    };
    let applied = match direction {
        History::Undo => crate::app::undo::undo_operation(hwnd, operation),
        History::Redo => crate::app::undo::redo_operation(hwnd, operation),
    }
    .map_err(error)?;
    WINDOW
        .with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(window) = slot.as_mut() else {
                return Ok(());
            };
            match direction {
                History::Undo => {
                    window.undo.pop();
                    window.redo.push(applied.operation);
                }
                History::Redo => {
                    window.redo.pop();
                    window.undo.push(applied.operation);
                }
            }
            window.list.refresh(&window.scroll)?;
            if let Some(path) = applied.select {
                window.list.select_created(vec![path], &mut window.scroll)?;
            }
            Ok::<(), String>(())
        })
        .map_err(error)?;
    publish_history(hwnd);
    super::invalidate_all(hwnd);
    Ok(())
}

fn publish_history(hwnd: HWND) {
    let counts = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or((0, 0), |window| (window.undo.len(), window.redo.len()))
    });
    for (name, count) in [
        (windows::core::w!("NativeUndoCount"), counts.0),
        (windows::core::w!("NativeRedoCount"), counts.1),
    ] {
        if let Err(error) = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetPropW(
                hwnd,
                name,
                Some(windows::Win32::Foundation::HANDLE(count as _)),
            )
        } {
            super::fail(error);
        }
    }
}

pub fn show(hwnd: HWND, menu: Menu, anchor: Anchor, origin: Origin) -> Result<()> {
    if let Some(context) = context(hwnd) {
        super::menu::show(hwnd, context, menu, anchor, origin)?;
    }
    Ok(())
}

pub fn navigation(hwnd: HWND, row: Option<usize>, point: (f32, f32), origin: Origin) {
    let result = (|| -> Result<()> {
        let Some(mut context) = context(hwnd) else {
            return Ok(());
        };
        context.selected.clear();
        let menu = if let Some(row) = row {
            let target = WINDOW.with(|slot| {
                let slot = slot.borrow();
                let window = slot.as_ref()?;
                let reference = &window.navigation.tree.rows.get(row)?.reference;
                let node = window.navigation.tree.node(reference)?;
                Some(menu_model::NavTarget {
                    node: node.node.clone(),
                    row,
                    in_pins: matches!(
                        reference,
                        crate::app::navtree::RowRef::Node {
                            section: crate::app::navtree::PINS,
                            ..
                        }
                    ),
                    expanded: node.expanded,
                })
            });
            let Some(target) = target else {
                return Ok(());
            };
            context.new_items =
                crate::shell::newmenu::entries(hwnd, &target.node.id).map_err(error)?;
            menu_model::nav_menu(&context, target, point.0, point.1)
        } else {
            menu_model::nav_options_menu()
        };
        super::menu::show(hwnd, context, menu, Anchor::Point(point.0, point.1), origin)
    })();
    if let Err(error) = result {
        eprintln!("native navigation menu: {error}");
    }
}

pub fn collection(hwnd: HWND, origin: Origin) {
    let anchor = WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| {
            let button = window
                .chrome
                .buttons
                .iter()
                .find(|button| button.command == Command::Collection)?;
            let filter = window.list.folder.as_ref()?.gallery_filter.clone();
            Some((
                Anchor::Dropdown {
                    left: button.left,
                    right: button.left + button.width,
                    bottom: 136.0,
                },
                filter,
            ))
        })
    });
    let Some((anchor, filter)) = anchor else {
        return;
    };
    let roots = crate::shell::gallery::collection_roots();
    let selected = filter
        .and_then(|filter| {
            roots
                .iter()
                .position(|root| crate::app::ids::same(root, &filter))
        })
        .map_or(0, |index| index + 1);
    let menu = Menu {
        owner: "collection",
        items: menu_model::collection_items(selected),
        buttons: Vec::new(),
    };
    if let Err(error) = show(hwnd, menu, anchor, origin) {
        super::fail(error);
    }
}

fn refresh(hwnd: HWND) -> Result<()> {
    if super::hosted::refresh(hwnd) {
        return Ok(());
    }
    let search = WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            crate::app::ids::is_search(window.tabs.active().location.tab.path())
        })
    });
    if search {
        return super::search::reload(hwnd);
    }
    WINDOW
        .with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map_or(Ok(()), |window| window.list.refresh(&window.scroll))
        })
        .map_err(error)?;
    super::invalidate_all(hwnd);
    Ok(())
}

fn expand(hwnd: HWND, labels: Vec<String>, expansion: crate::app::group::Expansion) -> Result<()> {
    WINDOW
        .with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(window) = slot.as_mut() else {
                return Ok(());
            };
            for label in labels {
                window.list.expand(&label, expansion)?;
            }
            Ok::<(), String>(())
        })
        .map_err(error)?;
    super::invalidate_all(hwnd);
    Ok(())
}

fn address_command(hwnd: HWND, editor: Option<HWND>, command: i32) {
    if command == 5 {
        super::chrome::set_focus(
            hwnd,
            super::chrome::Focus::Address(super::address::Target::Address),
        );
        return;
    }
    let control = editor.or_else(|| {
        WINDOW.with(|slot| slot.borrow().as_ref().map(|window| window.editors.address))
    });
    let Some(control) = control else {
        return;
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        match command {
            1..=3 => {
                SendMessageW(
                    control,
                    match command {
                        1 => WM_CUT,
                        2 => WM_COPY,
                        _ => WM_PASTE,
                    },
                    None,
                    None,
                );
            }
            4 => {
                SendMessageW(
                    control,
                    windows::Win32::UI::Controls::EM_SETSEL,
                    Some(windows::Win32::Foundation::WPARAM(0)),
                    Some(windows::Win32::Foundation::LPARAM(-1)),
                );
            }
            _ => return,
        }
    }
    super::edit::focus(control);
    super::edit::notify(hwnd, control);
}

fn created(hwnd: HWND, result: std::result::Result<String, String>) -> Result<()> {
    let path = result.map_err(error)?;
    push(hwnd, crate::app::undo::Undo::Create(path.clone()));
    WINDOW
        .with(|slot| {
            slot.borrow_mut().as_mut().map_or(Ok(()), |window| {
                window.rename_created = Some(path.clone());
                window.list.select_created(vec![path], &mut window.scroll)
            })
        })
        .map_err(error)?;
    super::chrome::set_focus(hwnd, super::chrome::Focus::List);
    super::invalidate_all(hwnd);
    Ok(())
}

fn new_item(hwnd: HWND, context: &Context, folder: &str, verb: &str) -> Result<()> {
    let current = crate::app::ids::same(folder, &context.path);
    if current {
        let before = crate::shell::listing::list(folder)
            .map_err(error)?
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.before_new = Some((folder.to_owned(), before));
            }
        });
    }
    if let Err(failure) = crate::shell::newmenu::create(hwnd, folder, verb) {
        if current {
            WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    window.before_new = None;
                }
            });
        }
        return Err(error(failure));
    }
    if current {
        refresh(hwnd)?;
    }
    Ok(())
}

pub const ASYNC_DONE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 35;
type Completion = std::result::Result<Option<String>, String>;
pub struct Mailbox {
    sender: std::sync::mpsc::Sender<Completion>,
    receiver: std::sync::mpsc::Receiver<Completion>,
}
impl Mailbox {
    pub fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self { sender, receiver }
    }
}

fn mailbox() -> Option<std::sync::mpsc::Sender<Completion>> {
    WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|window| window.menu_mailbox.sender.clone())
    })
}

fn post_created(owner: isize, mailbox: std::sync::mpsc::Sender<Completion>, result: Completion) {
    if mailbox.send(result).is_err() {
        return;
    }
    if let Err(error) = unsafe {
        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(HWND(owner as _)),
            ASYNC_DONE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        )
    } {
        eprintln!("native operation completion: {error}");
    }
}

pub fn completed(hwnd: HWND) {
    let results = WINDOW
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|window| window.menu_mailbox.receiver.try_iter().collect::<Vec<_>>())
        })
        .unwrap_or_default();
    for result in results {
        let result = match result {
            Ok(Some(path)) => created(hwnd, Ok(path)),
            Ok(None) => refresh(hwnd),
            Err(failure) => Err(error(failure)),
        };
        if let Err(error) = result {
            eprintln!("native operation: {error}");
        }
    }
}

pub fn loaded(hwnd: HWND) {
    let newly_created = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let (folder, before) = window.before_new.as_ref()?;
        if !crate::app::ids::same(folder, window.tabs.active().location.tab.path()) {
            window.before_new = None;
            return None;
        }
        let path = window
            .list
            .folder
            .as_ref()?
            .entries
            .iter()
            .find(|entry| !before.contains(&entry.path))
            .map(|entry| entry.path.clone())?;
        window.before_new = None;
        Some(path)
    });
    if let Some(path) = newly_created {
        if let Err(error) = created(hwnd, Ok(path)) {
            eprintln!("native New item completion: {error}");
        }
    }

    let path = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let path = window.rename_created.as_ref()?;
        if window
            .list
            .folder
            .as_ref()?
            .entries
            .iter()
            .any(|entry| crate::app::ids::same(&entry.path, path))
        {
            window.rename_created.take()
        } else {
            None
        }
    });
    if let Some(path) = path {
        if let Err(error) = super::rename::begin(hwnd, &path) {
            eprintln!("native created rename: {error}");
        }
    }
}

pub fn group(hwnd: HWND, row: usize, point: (f32, f32)) {
    let expansion = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let folder = window.list.folder.as_ref()?;
        let header = folder.layout.headers.get(row)?;
        window.list.interaction.selection.group(
            &folder.order,
            &folder.entries,
            header.rows.clone(),
            Default::default(),
        );
        window.list.interaction.mark_all();
        Some(if folder.collapsed.contains(&header.label) {
            crate::app::group::Expansion::Collapsed
        } else {
            crate::app::group::Expansion::Expanded
        })
    });
    let Some(expansion) = expansion else {
        return;
    };
    super::input::flush(hwnd);
    let Some(context) = context(hwnd) else {
        return;
    };
    let menu = menu_model::item_menu(
        &context,
        &context.selected,
        menu_model::group_context_items(row, expansion),
        point.0,
        point.1,
    );
    if let Err(error) = super::menu::show(
        hwnd,
        context,
        menu,
        Anchor::Context(point.0, point.1),
        Origin::Pointer,
    ) {
        super::fail(error);
    }
}
