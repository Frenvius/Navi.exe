use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::core::{w, Result, PWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const SHOW: usize = 50;
const HIDE: usize = 51;

#[derive(Clone, Copy)]
enum Style {
    Xaml(super::menu_tip::Placement),
    Classic,
}

#[derive(Clone)]
struct Tip {
    key: String,
    text: String,
    item: Option<String>,
    prefix: Option<String>,
    point: POINT,
    style: Style,
    lifetime: Option<u32>,
}

struct Pending {
    tip: Tip,
    since: Instant,
    delay: u32,
    token: u64,
}

struct Classic {
    hwnd: HWND,
    _text: Vec<u16>,
}

enum Popup {
    Xaml(Rc<super::render::Renderer>, super::menu_tip::Overlay),
    Classic(Classic),
}

impl Popup {
    fn close(self) -> Result<()> {
        match self {
            Self::Xaml(renderer, popup) => popup.close(&renderer),
            Self::Classic(popup) => unsafe { DestroyWindow(popup.hwnd) },
        }
    }
}

struct State {
    owner: HWND,
    pending: Option<Pending>,
    popup: Option<Popup>,
    prepared: Option<Popup>,
    token: u64,
    recent: Option<Instant>,
    last_style: Option<Style>,
    suppressed: Option<(i32, i32)>,
    scroll_pending: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            owner: HWND::default(),
            pending: None,
            popup: None,
            prepared: None,
            token: 0,
            recent: None,
            last_style: None,
            suppressed: None,
            scroll_pending: false,
        }
    }
}

thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

fn publish(owner: HWND) {
    let count = usize::from(showing(owner));
    let backing = STATE.with(|slot| {
        let state = slot.borrow();
        usize::from(state.owner == owner)
            * (usize::from(state.popup.is_some()) + usize::from(state.prepared.is_some()))
    });
    for (name, value) in [
        (w!("NativeTooltipCount"), count),
        (w!("NativeTooltipBackingCount"), backing),
    ] {
        if let Err(error) = unsafe { SetPropW(owner, name, Some(HANDLE(value as _))) } {
            super::fail(error);
        }
    }
}

pub fn showing(owner: HWND) -> bool {
    STATE.with(|slot| {
        let state = slot.borrow();
        state.owner == owner && state.popup.is_some()
    })
}

pub fn dismiss(owner: HWND) {
    let _context = super::context::enter(owner);
    let popups = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.owner != owner {
            return (None, None);
        }
        state.token = state.token.wrapping_add(1);
        state.pending = None;
        let popup = state.popup.take();
        if popup.is_some() {
            state.recent = Some(Instant::now());
        }
        (popup, state.prepared.take())
    });
    super::input::stop(owner, SHOW);
    super::input::stop(owner, HIDE);
    for popup in [popups.0, popups.1].into_iter().flatten() {
        if let Err(error) = popup.close() {
            eprintln!("native tooltip close: {error}");
        }
    }
    publish(owner);
}

pub fn suppress(owner: HWND) {
    dismiss(owner);
    let mut point = POINT::default();
    unsafe {
        if let Err(error) = GetCursorPos(&mut point) {
            eprintln!("tooltip pointer: {error}");
        }
    }
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        state.suppressed = Some((point.x, point.y));
        state.recent = None;
    });
}

pub fn menu(
    renderer: Rc<super::render::Renderer>,
    owner: HWND,
    content: super::menu_tip::Content,
) -> Result<()> {
    let previous = STATE.with(|slot| slot.borrow().owner);
    if !previous.is_invalid() && previous != owner {
        dismiss(previous);
    }
    dismiss(owner);
    let popup = super::menu_tip::Overlay::show(&renderer, owner, content)?;
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        state.owner = owner;
        state.popup = Some(Popup::Xaml(renderer, popup));
        state.last_style = Some(Style::Xaml(super::menu_tip::Placement::Above));
    });
    publish(owner);
    Ok(())
}

fn classic(owner: HWND, tip: &Tip) -> Result<Classic> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        if !InitCommonControlsEx(&controls).as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            TOOLTIPS_CLASSW,
            w!(""),
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            0,
            0,
            0,
            0,
            Some(owner),
            None,
            None,
            None,
        )?;
        let result = (|| {
            {
                SetWindowTheme(
                    hwnd,
                    if crate::shell::theme::dark() {
                        w!("DarkMode_Explorer")
                    } else {
                        w!("Explorer")
                    },
                    None,
                )?;
            }
            let mut text = crate::shell::wide(&tip.text);
            let info = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_TRACK | TTF_ABSOLUTE | TTF_TRANSPARENT,
                hwnd: owner,
                uId: 1,
                lpszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            if SendMessageW(
                hwnd,
                TTM_ADDTOOLW,
                None,
                Some(LPARAM(&info as *const _ as isize)),
            )
            .0 == 0
            {
                return Err(windows::core::Error::from_win32());
            }
            for (kind, delay) in [
                (TTDT_INITIAL, 1000),
                (TTDT_RESHOW, 100),
                (TTDT_AUTOPOP, 32767),
            ] {
                SendMessageW(
                    hwnd,
                    TTM_SETDELAYTIME,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(delay)),
                );
            }
            SendMessageW(hwnd, TTM_SETMAXTIPWIDTH, None, Some(LPARAM(400)));
            SendMessageW(
                hwnd,
                TTM_TRACKPOSITION,
                None,
                Some(LPARAM(
                    ((tip.point.y as u32) << 16 | tip.point.x as u32 & 0xffff) as isize,
                )),
            );
            SendMessageW(
                hwnd,
                TTM_TRACKACTIVATE,
                Some(WPARAM(1)),
                Some(LPARAM(&info as *const _ as isize)),
            );
            Ok(Classic { hwnd, _text: text })
        })();
        if result.is_err() {
            DestroyWindow(hwnd)?;
        }
        result
    }
}

fn show(owner: HWND) {
    if STATE.with(|slot| {
        let state = slot.borrow();
        state.owner != owner || state.popup.is_some()
    }) {
        return;
    }
    let tip = STATE.with(|slot| {
        slot.borrow()
            .pending
            .as_ref()
            .filter(|pending| {
                !pending.tip.text.is_empty()
                    && pending.since.elapsed() >= Duration::from_millis(pending.delay as u64)
            })
            .map(|pending| pending.tip.clone())
    });
    let Some(tip) = tip else {
        return;
    };
    let result = match tip.style {
        Style::Xaml(placement) => {
            let prepared = STATE.with(|slot| slot.borrow_mut().prepared.take());
            if let Some(Popup::Xaml(renderer, popup)) = prepared {
                if let Err(error) = popup.reposition(tip.point) {
                    if let Err(cleanup) = popup.close(&renderer) {
                        eprintln!("tooltip prepare cleanup: {cleanup}");
                    }
                    Err(error)
                } else {
                    match popup.reveal(&renderer) {
                        Ok(()) => Ok(Popup::Xaml(renderer, popup)),
                        Err(error) => {
                            if let Err(cleanup) = popup.close(&renderer) {
                                eprintln!("tooltip reveal cleanup: {cleanup}");
                            }
                            Err(error)
                        }
                    }
                }
            } else {
                xaml(owner, &tip, placement).and_then(|(renderer, popup)| {
                    match popup.reveal(&renderer) {
                        Ok(()) => Ok(Popup::Xaml(renderer, popup)),
                        Err(error) => {
                            if let Err(cleanup) = popup.close(&renderer) {
                                eprintln!("tooltip reveal cleanup: {cleanup}");
                            }
                            Err(error)
                        }
                    }
                })
            }
        }
        Style::Classic => classic(owner, &tip).map(Popup::Classic),
    };
    match result {
        Ok(popup) => {
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                state.popup = Some(popup);
                state.last_style = Some(tip.style);
            });
            if let Some(delay) = tip.lifetime {
                super::input::arm(owner, HIDE, delay);
            }
            publish(owner);
        }
        Err(error) => {
            eprintln!("native tooltip: {error}");
            dismiss(owner);
        }
    }
}

fn xaml(
    owner: HWND,
    tip: &Tip,
    placement: super::menu_tip::Placement,
) -> Result<(Rc<super::render::Renderer>, super::menu_tip::Overlay)> {
    let renderer = super::WINDOW
        .with(|slot| {
            slot.borrow()
                .as_ref()?
                .graphics
                .as_ref()
                .map(|(renderer, _)| renderer.clone())
        })
        .ok_or_else(windows::core::Error::from_win32)?;
    let popup = super::menu_tip::Overlay::prepare(
        &renderer,
        owner,
        super::menu_tip::Content::Tooltip {
            text: tip.text.clone(),
            pointer: tip.point,
            placement,
        },
    )?;
    Ok((renderer, popup))
}

fn arm(owner: HWND, tip: Tip, delay: u32) {
    let previous = STATE.with(|slot| slot.borrow().owner);
    if !previous.is_invalid() && previous != owner {
        dismiss(previous);
    }
    dismiss(owner);
    let request = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        state.owner = owner;
        state.token = state.token.wrapping_add(1);
        let token = state.token;
        let request = tip.item.clone().map(|path| (token, path));
        state.pending = Some(Pending {
            tip,
            since: Instant::now(),
            delay,
            token,
        });
        request
    });
    super::input::arm(owner, SHOW, delay);
    let tip = STATE.with(|slot| {
        slot.borrow()
            .pending
            .as_ref()
            .map(|pending| pending.tip.clone())
    });
    if let Some(tip) = tip {
        if let Style::Xaml(placement) = tip.style {
            match xaml(owner, &tip, placement) {
                Ok((renderer, popup)) => STATE
                    .with(|slot| slot.borrow_mut().prepared = Some(Popup::Xaml(renderer, popup))),
                Err(error) => {
                    eprintln!("prepare tooltip: {error}");
                    dismiss(owner);
                }
            }
        }
    }
    if let Some((token, path)) = request {
        super::WINDOW.with(|slot| {
            if let Some(window) = slot.borrow().as_ref() {
                window
                    .address_worker
                    .send(super::address::Request::Infotip(token, path));
            }
        });
    }
    publish(owner);
}

pub fn ready(owner: HWND, token: u64, text: String) {
    let accepted = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.owner != owner {
            return false;
        }
        let Some(pending) = state
            .pending
            .as_mut()
            .filter(|pending| pending.token == token)
        else {
            return false;
        };
        pending.tip.text = if let Some(prefix) = &pending.tip.prefix {
            if text.lines().next().is_some_and(|line| line == prefix) {
                text
            } else if text.is_empty() {
                prefix.clone()
            } else {
                format!("{prefix}\r\n{text}")
            }
        } else {
            text
        };
        true
    });
    if accepted {
        show(owner);
    }
}

pub fn timer(owner: HWND, timer: usize) -> bool {
    if !matches!(timer, SHOW | HIDE) {
        return false;
    }
    super::input::stop(owner, timer);
    if timer == SHOW {
        let remaining = STATE.with(|slot| {
            slot.borrow().pending.as_ref().map_or(0, |pending| {
                (pending.delay as u128).saturating_sub(pending.since.elapsed().as_millis())
            })
        });
        if remaining > 0 {
            super::input::arm(owner, SHOW, remaining as u32 + 1);
        } else {
            show(owner);
        }
    } else {
        dismiss(owner);
    }
    true
}

pub fn moved(owner: HWND, point: (f32, f32)) {
    if super::menu::is_open(owner) || super::rename::selected().is_some() {
        return;
    }
    let mut cursor = POINT::default();
    if let Err(error) = unsafe { GetCursorPos(&mut cursor) } {
        eprintln!("tooltip pointer: {error}");
        return;
    }
    let suppressed = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.suppressed == Some((cursor.x, cursor.y)) {
            return true;
        }
        state.suppressed = None;
        false
    });
    if suppressed {
        return;
    }
    let tip = target(point, cursor);
    let same = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let same = state.owner == owner
            && state
                .pending
                .as_ref()
                .is_some_and(|pending| tip.as_ref().is_some_and(|tip| tip.key == pending.tip.key));
        if same && state.popup.is_none() {
            if let (Some(pending), Some(tip)) = (state.pending.as_mut(), tip.as_ref()) {
                pending.tip.point = tip.point;
            }
        }
        same
    });
    if same {
        return;
    }
    let Some(tip) = tip else {
        dismiss(owner);
        return;
    };
    let delay = STATE.with(|slot| {
        let state = slot.borrow();
        let recent = state.popup.is_some()
            || state
                .recent
                .is_some_and(|recent| recent.elapsed() < Duration::from_millis(100));
        match (tip.style, state.last_style, recent) {
            (Style::Classic, Some(Style::Classic), true) => 100,
            (Style::Xaml(_), Some(Style::Xaml(_)), true) => 0,
            (Style::Classic, _, _) => 1000,
            _ => 900,
        }
    });
    arm(owner, tip, delay.max(1));
}

fn target(point: (f32, f32), cursor: POINT) -> Option<Tip> {
    super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let mut tip = Tip {
            key: String::new(),
            text: String::new(),
            item: None,
            prefix: None,
            point: cursor,
            style: Style::Xaml(super::menu_tip::Placement::Above),
            lifetime: None,
        };
        if point.1 < 40.0 {
            use super::titlebar::Target;
            let target = window.title.hit(window.chrome.size.0, point.0, point.1);
            tip.key = format!("title:{target:?}");
            tip.style = Style::Xaml(super::menu_tip::Placement::Below);
            tip.text = match target {
                Target::Tab(index) => window.title.items.get(index)?.title.clone(),
                Target::TabClose(_) => crate::i18n::tr("tab.close.tip").into(),
                Target::Plus => crate::i18n::tr("tab.new.tip").into(),
                Target::Caption(_) => return None,
                Target::None => return None,
            };
        } else if (40.0..88.0).contains(&point.1) {
            use super::address::Target;
            let target = window.address.hit(point)?;
            tip.key = format!("address:{target:?}");
            let tab = &window.tabs.active().location.tab;
            tip.text = match target {
                Target::Back | Target::Forward => {
                    let index = if target == Target::Back {
                        tab.index.checked_sub(1)?
                    } else {
                        tab.index + 1
                    };
                    let path = tab.history.get(index)?;
                    crate::i18n::trf(
                        if target == Target::Back {
                            "nav.back.tip"
                        } else {
                            "nav.forward.tip"
                        },
                        &[&crate::shell::listing::display_name_of(path)],
                    )
                }
                Target::Up => {
                    let crumbs = &window.address.crumbs;
                    if crumbs.len() > 1 {
                        crate::i18n::trf("nav.up-to.tip", &[&crumbs[crumbs.len() - 2].name])
                    } else {
                        crate::i18n::tr("nav.up.tip").into()
                    }
                }
                Target::Settings => crate::i18n::tr("settings.title").into(),
                Target::Refresh => crate::i18n::trf("nav.refresh.tip", &[&window.address.location]),
                Target::Crumb(_) => return None,
                _ => return None,
            };
        } else if (89.0..135.0).contains(&point.1) {
            use super::chrome::Command;
            let button = window
                .chrome
                .buttons
                .iter()
                .find(|button| (button.left..button.left + button.width).contains(&point.0))?;
            tip.key = format!("command:{:?}", button.command);
            tip.text = match button.command {
                Command::New | Command::Filter | Command::EmptyBin | Command::ExtractAll => {
                    return None
                }
                Command::Wallpaper => crate::i18n::tr("cmd.background.tip").into(),
                Command::Sort => crate::i18n::tr("cmd.sort.tip").into(),
                Command::View => crate::i18n::tr("cmd.view.tip").into(),
                Command::Collection => {
                    crate::app::menu_model::command_tip("windows.galleryfiltercommand")
                }
                command => crate::i18n::tr(match command {
                    Command::Cut => "cmd.cut.tip",
                    Command::Copy => "cmd.copy.tip",
                    Command::Paste => "cmd.paste.tip",
                    Command::Rename => "cmd.rename.tip",
                    Command::Share => "cmd.share",
                    Command::Delete => "cmd.delete.tip",
                    Command::More => "cmd.see-more.tip",
                    Command::Pane if window.chrome.preview => "cmd.preview-pane.tip",
                    _ => "cmd.details-pane.tip",
                })
                .into(),
            };
        } else if let Some(super::chrome::Target::Status(index)) = window.chrome.hit(point) {
            tip.key = format!("status:{index}");
            tip.text = crate::i18n::tr(if index == 0 {
                "status.details.tip"
            } else {
                "status.large-icons.tip"
            })
            .into();
        } else {
            let folder = window.list.folder.as_ref()?;
            if !folder.settings.info_tips || window.list.interaction.has_press() {
                return None;
            }
            let row = window.list.interaction.hover?;
            let entry = folder.entries.get(*folder.order.get(row)?)?;
            tip.key = format!("item:{}", entry.path);
            tip.item = Some(entry.path.clone());
            tip.prefix = clipped_name(window, row);
            tip.lifetime = Some(32767);
            tip.style = Style::Classic;
            tip.point.y += 18;
        }
        (!tip.text.is_empty() || tip.item.is_some()).then_some(tip)
    })
}

fn clipped_name(window: &super::Window, row: usize) -> Option<String> {
    let folder = window.list.folder.as_ref()?;
    let entry = folder.entries.get(*folder.order.get(row)?)?;
    let width = if let Some(grid) = &folder.grid {
        let cell = *grid.layout.cells.get(row)?;
        match grid.spec.kind {
            0 => {
                let label = grid.labels.get(row)?.as_ref()?;
                return (label.full != label.collapsed).then(|| entry.name.clone());
            }
            1 => cell[2] - grid.spec.icon - 40.0 - grid.spec.label_indent,
            2 => {
                let label = crate::shell::text::tile_label(
                    &entry.name,
                    (189.0 - grid.spec.label_indent) as i32,
                );
                return crate::shell::columns::widths(label.lines())
                    .iter()
                    .any(|&width| width > cell[2] - 61.0 - grid.spec.label_indent)
                    .then(|| entry.name.clone());
            }
            3 => {
                let fields = window
                    .list
                    .rows
                    .get(&row)?
                    .picture
                    .as_ref()?
                    .fields
                    .as_slice();
                let layout =
                    crate::app::grid_layout::content(cell[2], fields, grid.spec.label_indent);
                return (crate::shell::columns::widths_at([entry.name.as_str()], 15)[0]
                    > layout.name_width)
                    .then(|| entry.name.clone());
            }
            _ => return None,
        }
    } else {
        let index = folder.columns.iter().position(|column| {
            column.key == windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay
        })?;
        folder.columns[index].width
            - if index == 0 {
                38.0 + if folder.settings.check_boxes {
                    15.0
                } else {
                    0.0
                }
            } else {
                6.0
            }
            - 6.0
    };
    (crate::shell::columns::widths([entry.name.as_str()])[0] > width).then(|| entry.name.clone())
}

pub fn key_moved(owner: HWND, before: Option<String>) {
    dismiss(owner);
    let tip = super::WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let folder = window.list.folder.as_ref()?;
        if !folder.settings.info_tips || !window.list.interaction.has_focus {
            return None;
        }
        let path = window.list.interaction.selection.focused.as_ref()?;
        if Some(path) == before.as_ref() {
            return None;
        }
        let row = folder
            .order
            .iter()
            .position(|&index| crate::app::ids::same(path, &folder.entries[index].path))?;
        let view = window.scroll.view?;
        let (x, y) = if let Some(grid) = &folder.grid {
            let cell = grid.layout.cells[row];
            (
                cell[0] + 4.0,
                cell[1] + cell[3] - if grid.spec.kind == 0 { 2.0 } else { 0.0 },
            )
        } else {
            (18.0, view.row_top(folder, row) + view.plate)
        };
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(owner) } as f32 / 96.0;
        let point = crate::frame::client_to_screen(
            owner,
            ((view.x + x - window.scroll.x) * scale).round() as i32,
            ((view.y + view.header + y - window.scroll.y) * scale).round() as i32,
        );
        Some(Tip {
            key: format!("keyboard:{path}"),
            text: String::new(),
            item: Some(path.clone()),
            prefix: clipped_name(window, row),
            point: POINT {
                x: point.0,
                y: point.1,
            },
            style: Style::Classic,
            lifetime: None,
        })
    });
    if let Some(tip) = tip {
        arm(owner, tip, 2000);
    }
}

pub fn scrolled() {
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        state.scroll_pending = true;
        state.suppressed = None;
    });
}

pub fn refresh(owner: HWND) {
    let pending = STATE.with(|slot| std::mem::take(&mut slot.borrow_mut().scroll_pending));
    if !pending {
        return;
    }
    let mut point = POINT::default();
    unsafe {
        if GetCursorPos(&mut point).is_err()
            || !windows::Win32::Graphics::Gdi::ScreenToClient(owner, &mut point).as_bool()
        {
            return;
        }
        let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(owner) as f32 / 96.0;
        moved(owner, (point.x as f32 / scale, point.y as f32 / scale));
    }
}

pub fn closed(owner: HWND) {
    dismiss(owner);
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.owner == owner {
            *state = State::default();
        }
    });
}
