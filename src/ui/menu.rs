use std::cell::RefCell;
use std::rc::Rc;
use windows::core::{w, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionTarget};
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_PREMULTIPLIED;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetFocus, ReleaseCapture, SetCapture, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::address::rect;
use super::menu_layout::{self, Hit, Layout, Level, CAPTION, LABEL, SHORTCUT};
use super::render::{Canvas, Layer, Renderer};
use super::text::{Family, TextEngine, TextStyle};
use super::theme::Theme;
use crate::app::menu_model::{Action, Context, Icon, Menu};

const SUBMENU_TIMER: usize = 40;
const TIP_TIMER: usize = 42;
const KEYS_TIMER: usize = 43;

enum Pending {
    Open(usize, usize),
    Close(usize),
    Hover(usize, Option<Hit>),
}

pub enum Origin {
    Pointer,
    Keyboard,
}

pub enum Anchor {
    Dropdown { left: f32, right: f32, bottom: f32 },
    Point(f32, f32),
    Context(f32, f32),
}

struct Popup {
    hwnd: HWND,
    menu: Menu,
    layout: Layout,
    at: (i32, i32),
    hover: Option<Hit>,
    pressed: Option<Hit>,
    keyboard: bool,
    offset: f32,
    opened: Option<usize>,
    layer: Option<Layer>,
    target: IDCompositionTarget,
    chrome: ID2D1Bitmap1,
    svg: super::svg::Assets,
    icons: std::collections::HashMap<usize, Rc<ID2D1Bitmap1>>,
    animation: Option<f32>,
    cloud: Option<super::cloud::State>,
}

struct Session {
    owner: HWND,
    context: Context,
    renderer: Rc<Renderer>,
    levels: Vec<Popup>,
    pending: Option<Pending>,
    pointer: POINT,
    key_prefix: String,
    focus: HWND,
    key_overlay: Option<super::menu_tip::Overlay>,
    key_tips: bool,
}

thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }

pub fn close() {
    let session = SESSION.with(|slot| slot.borrow_mut().take());
    if let Some(mut session) = session {
        let _context = super::context::enter(session.owner);
        super::tooltip::dismiss(session.owner);
        for timer in [SUBMENU_TIMER, TIP_TIMER, KEYS_TIMER] {
            super::input::stop(session.owner, timer);
        }
        if let Some(overlay) = session.key_overlay.take() {
            if let Err(error) = overlay.close(&session.renderer) {
                eprintln!("native menu adornment release: {error}");
            }
        }
        if unsafe { GetCapture() } == session.owner {
            if let Err(error) = unsafe { ReleaseCapture() } {
                super::fail(error);
            }
        }
        while let Some(popup) = session.levels.pop() {
            if let Err(error) = release(&session.renderer, popup) {
                if super::render::device_lost(&error) {
                    eprintln!("native menu backing device loss: {error}");
                } else {
                    super::fail(error);
                }
            }
        }
        restore_editor(session.owner, session.context.editor);
        if !session.focus.is_invalid()
            && unsafe {
                IsWindow(Some(session.focus)).as_bool() && GetForegroundWindow() == session.owner
            }
        {
            unsafe {
                let _ = SetFocus(Some(session.focus));
            }
        }
        publish(session.owner, 0);
        command_state(session.owner, None);
        restore_list_focus(session.owner);
        super::chrome::sync(session.owner);
        super::memory::checkpoint("menu-closed");
    }
}

pub fn close_owner(owner: HWND) {
    if SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|session| session.owner == owner)
    }) {
        close();
    }
}

fn publish(owner: HWND, count: usize) {
    if count == 0 {
        for name in [w!("NativeMenuRow"), w!("NativeMenuButton")] {
            if let Err(error) = unsafe { SetPropW(owner, name, Some(HANDLE::default())) } {
                super::fail(error);
            }
        }
    }
    for name in [w!("NativeMenuCount"), w!("NativeMenuBackingCount")] {
        if let Err(error) = unsafe { SetPropW(owner, name, Some(HANDLE(count as _))) } {
            super::fail(error);
        }
    }
}

fn release(renderer: &Renderer, popup: Popup) -> Result<()> {
    unsafe {
        let results = [
            popup.target.SetRoot(None),
            popup
                .layer
                .as_ref()
                .map_or(Ok(()), |layer| layer.surface.Trim(Some(&[]))),
            DestroyWindow(popup.hwnd),
            renderer.composition.Commit(),
        ];
        let mut failure = None;
        for result in results {
            if let Err(error) = result {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

pub fn show(
    owner: HWND,
    context: Context,
    menu: Menu,
    anchor: Anchor,
    origin: Origin,
) -> Result<()> {
    close();
    super::tooltip::suppress(owner);
    let editor = if matches!(menu.owner, "address-text" | "search-text") {
        context.editor
    } else {
        None
    };
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.interaction.has_focus = false;
            window.list.interaction.mark_all();
            if editor == Some(window.editors.address) {
                window.address.editing = false;
            }
            if editor == Some(window.editors.search) {
                window.address.search_focus = false;
            }
        }
    });
    super::input::flush(owner);
    if editor.is_some() {
        super::chrome::invalidate(
            owner,
            super::render::Damage {
                address: true,
                ..super::render::Damage::NONE
            },
        );
    }
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(owner);
    }
    let result = open(owner, context, menu, anchor, origin);
    if result.is_err() {
        restore_editor(owner, editor);
        restore_list_focus(owner);
    }
    result
}

fn restore_editor(owner: HWND, editor: Option<HWND>) {
    let Some(editor) = editor else { return };
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            if editor == window.editors.address {
                window.address.editing = true;
            }
            if editor == window.editors.search {
                window.address.search_focus = true;
            }
        }
    });
    super::chrome::invalidate(
        owner,
        super::render::Damage {
            address: true,
            ..super::render::Damage::NONE
        },
    );
}

fn restore_list_focus(owner: HWND) {
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.interaction.has_focus = window.chrome.focus == super::chrome::Focus::List;
            window.list.interaction.mark_all();
        }
    });
    super::input::flush(owner);
}

fn open(
    owner: HWND,
    context: Context,
    mut menu: Menu,
    anchor: Anchor,
    origin: Origin,
) -> Result<()> {
    crate::app::menu_model::finalize(&mut menu.items, menu.owner);
    super::address_popup::close();
    super::input::cancel(owner);
    let renderer = super::WINDOW
        .with(|slot| {
            slot.borrow().as_ref().and_then(|window| {
                window
                    .graphics
                    .as_ref()
                    .map(|(renderer, _)| renderer.clone())
            })
        })
        .ok_or_else(windows::core::Error::from_win32)?;
    let cloud = if menu.owner == "cloud" {
        super::WINDOW
            .with(|slot| {
                slot.borrow()
                    .as_ref()
                    .and_then(|window| window.address.cloud.clone())
            })
            .map(super::cloud::State::new)
    } else {
        None
    };
    let mut layout = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        Layout::new(
            &menu,
            Level::Root,
            &mut slot
                .as_mut()
                .ok_or_else(windows::core::Error::from_win32)?
                .text,
        )
    })?;
    if let Some(cloud) = &cloud {
        layout = cloud.layout();
    }
    let scale = unsafe { GetDpiForWindow(owner) } as f32 / 96.0;
    let (point, alignment) = match anchor {
        Anchor::Dropdown {
            left,
            right,
            bottom,
        } => match menu.owner {
            "new" => ((left, bottom + 5.0), 0.0),
            "more" => ((right, bottom + 1.0), 1.0),
            _ => (((left + right) / 2.0, bottom + 5.0), 0.5),
        },
        Anchor::Point(x, y) => ((x, y), 0.0),
        Anchor::Context(x, y) => (
            (x, y + if menu.buttons.is_empty() { 2.0 } else { 0.0 }),
            0.0,
        ),
    };
    let at = crate::frame::client_to_screen(
        owner,
        (point.0 * scale).round() as i32,
        (point.1 * scale).round() as i32,
    );
    let anchor_y = at.1;
    let work = crate::frame::work_area(at.0, at.1);
    layout.height = layout
        .height
        .min((work.bottom - work.top - 20) as f32 / scale);
    let size = (
        (layout.width * scale).round() as i32,
        (layout.height * scale).round() as i32,
    );
    let at = if matches!(anchor, Anchor::Context(..)) {
        let placement = menu_layout::context_position(
            at,
            size,
            work,
            if menu.buttons.is_empty() {
                menu_layout::ContextCommands::None
            } else {
                menu_layout::ContextCommands::Primary((layout.buttons_height * scale).round() as i32)
            },
            scale,
        );
        if placement.buttons_bottom {
            layout.flip_buttons();
        }
        placement.point
    } else {
        (
            (at.0 - (size.0 as f32 * alignment).round() as i32)
                .max(work.left)
                .min((work.right - size.0).max(work.left)),
            at.1.min((work.bottom - size.1).max(work.top)),
        )
    };
    let mut popup = create(&renderer, owner, menu, layout, at)?;
    popup.cloud = cloud;
    animate(
        &mut popup,
        if at.1 < anchor_y {
            Direction::Up
        } else {
            Direction::Down
        },
    );
    if matches!(origin, Origin::Keyboard) {
        popup.keyboard = true;
        popup.hover = if popup.menu.buttons.is_empty() {
            menu_layout::next(&popup.menu, None, 1)
        } else {
            Some(Hit::Button(0))
        };
    }
    let mut pointer = POINT::default();
    unsafe {
        GetCursorPos(&mut pointer)?;
    }
    let focus = unsafe { GetFocus() };
    let keyboard = popup.keyboard;
    use super::chrome::Command;
    command_state(
        owner,
        match popup.menu.owner {
            "new" => Some(Command::New),
            "sort" => Some(Command::Sort),
            "view" => Some(Command::View),
            "more" => Some(Command::More),
            _ => None,
        },
    );
    SESSION.with(|slot| {
        *slot.borrow_mut() = Some(Session {
            owner,
            context,
            renderer,
            levels: vec![popup],
            pending: None,
            pointer,
            key_prefix: String::new(),
            focus,
            key_overlay: None,
            key_tips: false,
        })
    });
    publish(owner, 1);
    unsafe {
        let _ = SetFocus(Some(owner));
        SetCapture(owner);
    }
    if keyboard {
        arm_keys(owner);
        super::input::arm(owner, TIP_TIMER, 1000);
    }
    if let Err(error) = repaint() {
        close();
        return Err(error);
    }
    Ok(())
}

fn create(
    renderer: &Renderer,
    owner: HWND,
    menu: Menu,
    layout: Layout,
    at: (i32, i32),
) -> Result<Popup> {
    unsafe {
        static REGISTER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        if !*REGISTER.get_or_init(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hCursor: cursor,
                lpszClassName: w!("SwMenuPopup"),
                ..Default::default()
            }) != 0
        }) {
            return Err(windows::core::Error::from_win32());
        }
        let scale = GetDpiForWindow(owner) as f32 / 96.0;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            w!("SwMenuPopup"),
            w!(""),
            WS_POPUP,
            at.0 - (10.0 * scale).round() as i32,
            at.1 - (2.0 * scale).round() as i32,
            ((layout.width + 20.0) * scale).round() as i32,
            ((layout.height + 20.0) * scale).round() as i32,
            Some(owner),
            None,
            None,
            None,
        )?;
        let made = (|| {
            use windows::Win32::Graphics::Dwm::*;
            for (attribute, value) in [
                (DWMWA_WINDOW_CORNER_PREFERENCE, 1u32),
                (DWMWA_BORDER_COLOR, 0xfffffffe),
                (DWMWA_SYSTEMBACKDROP_TYPE, 1),
            ] {
                DwmSetWindowAttribute(hwnd, attribute, &value as *const _ as _, 4)?;
            }
            let mut pixels = crate::app::menu_chrome::pixels(
                (layout.width + 20.0).round() as i32,
                (layout.height + 20.0).round() as i32,
                crate::shell::theme::dark(),
                layout.context,
            );
            crate::app::menu_chrome::backdrop(
                &mut pixels,
                (at.0 - (10.0 * scale) as i32, at.1 - (2.0 * scale) as i32),
                scale,
            )
            .map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
            })?;
            crate::frame::menu_region(
                hwnd,
                &pixels.pixels,
                pixels.width as usize,
                pixels.height as usize,
                scale,
            );
            let chrome = renderer.canvas.system_icon(&pixels)?;
            let mut layer = renderer.layer(DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            layer.resize((
                ((layout.width + 20.0) * scale).round() as u32,
                ((layout.height + 20.0) * scale).round() as u32,
            ))?;
            let target = renderer.composition.CreateTargetForHwnd(hwnd, true)?;
            target.SetRoot(&layer.visual)?;
            Ok(Popup {
                hwnd,
                menu,
                layout,
                at,
                hover: None,
                pressed: None,
                keyboard: false,
                offset: 0.0,
                opened: None,
                layer: Some(layer),
                target,
                chrome,
                svg: Default::default(),
                icons: Default::default(),
                animation: None,
                cloud: None,
            })
        })();
        if made.is_err() {
            DestroyWindow(hwnd)?;
        }
        made
    }
}

fn repaint() -> Result<()> {
    let session = SESSION.with(|slot| slot.borrow_mut().take());
    let Some(mut session) = session else {
        return Ok(());
    };
    let _context = super::context::enter(session.owner);
    let result = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return Ok(());
        };
        let theme = Theme::system();
        for popup in &mut session.levels {
            let dpi = unsafe { GetDpiForWindow(popup.hwnd) };
            prepare(popup, &session.renderer.canvas, &theme, dpi)?;
            let mut layer = popup
                .layer
                .take()
                .ok_or_else(windows::core::Error::from_win32)?;
            let painted = session.renderer.update(
                &mut layer,
                None,
                dpi,
                &D2D1_COLOR_F::default(),
                |canvas| {
                    unsafe {
                        canvas.context.DrawBitmap(
                            &popup.chrome,
                            None,
                            1.0,
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            None,
                            None,
                        );
                    }
                    let mut original = Default::default();
                    unsafe {
                        canvas.context.GetTransform(&mut original);
                        let mut moved = original;
                        moved.M31 += 10.0;
                        moved.M32 += 2.0;
                        canvas.context.SetTransform(&moved);
                    }
                    let result = draw(popup, canvas, &mut window.text, &theme);
                    unsafe {
                        canvas.context.SetTransform(&original);
                    }
                    result
                },
            );
            popup.layer = Some(layer);
            painted?;
            advance(popup, &session.renderer.composition)?;
            unsafe {
                let _ = ShowWindow(popup.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        if let Some(popup) = session.levels.last() {
            let (row, button) = match popup.hover {
                Some(Hit::Row(index)) => (index + 1, 0),
                Some(Hit::Button(index)) => (0, index + 1),
                None => (0, 0),
            };
            for (name, count) in [(w!("NativeMenuRow"), row), (w!("NativeMenuButton"), button)] {
                unsafe {
                    SetPropW(session.owner, name, Some(HANDLE(count as _)))?;
                }
            }
        }
        unsafe {
            session.renderer.composition.Commit()?;
        }
        Ok(())
    });
    SESSION.with(|slot| *slot.borrow_mut() = Some(session));
    result
}

fn prepare(popup: &mut Popup, canvas: &Canvas, theme: &Theme, dpi: u32) -> Result<()> {
    if let Some(cloud) = &mut popup.cloud {
        return cloud.prepare(canvas, dpi);
    }
    let mut names = Vec::new();
    for icon in popup
        .menu
        .items
        .iter()
        .map(|item| &item.entry.icon)
        .chain(popup.menu.buttons.iter().map(|(button, _)| &button.icon))
    {
        if let Icon::Command(name) = icon {
            if !names.contains(name) {
                names.push(*name);
            }
        }
    }
    if !names.is_empty() {
        popup.svg.prepare(canvas, theme, dpi, &names)?;
    }
    for (index, icon) in popup
        .menu
        .items
        .iter()
        .map(|item| &item.entry.icon)
        .chain(popup.menu.buttons.iter().map(|(button, _)| &button.icon))
        .enumerate()
    {
        if popup.icons.contains_key(&index) {
            continue;
        }
        let image = match icon {
            Icon::System(index) => Some(super::icons::small(*index).map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
            })?),
            Icon::Pixels(image) => Some(image.clone()),
            _ => None,
        };
        if let Some(image) = image {
            popup.icons.insert(index, canvas.shared_icon(&image)?);
        }
    }
    Ok(())
}

fn icon(
    popup: &Popup,
    index: usize,
    source: &Icon,
    canvas: &Canvas,
    point: (f32, f32),
    opacity: f32,
) -> Result<()> {
    if let Icon::Command(name) = source {
        return popup.svg.icon(canvas, name, point, opacity);
    }
    if let Some(image) = popup.icons.get(&index) {
        unsafe {
            canvas.context.DrawBitmap(
                image.as_ref(),
                Some(&rect(point.0, point.1, 16.0, 16.0)),
                opacity,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
        }
    }
    Ok(())
}

fn label(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    style: TextStyle,
    bounds: D2D_RECT_F,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    let height = text.layout(value, style, 4096.0)?.height;
    text.draw(
        canvas,
        value,
        style,
        (
            bounds.left,
            bounds.top + (bounds.bottom - bounds.top - height) / 2.0,
        ),
        color,
    )
}

fn draw(popup: &Popup, canvas: &Canvas, text: &mut TextEngine, theme: &Theme) -> Result<()> {
    if let Some(cloud) = &popup.cloud {
        return cloud.draw(canvas, text, theme, popup.hover, popup.pressed, popup.keyboard);
    }
    let layout = &popup.layout;
    unsafe {
        canvas.context.PushAxisAlignedClip(
            &rect(
                1.0,
                layout.padding,
                layout.width - 2.0,
                layout.height - 2.0 * layout.padding,
            ),
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
    }
    let drawn = (|| {
        let (top, bottom) = layout.row_area();
        unsafe {
            canvas.context.PushAxisAlignedClip(
                &rect(1.0, top, layout.width - 2.0, (bottom - top).max(0.0)),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        let rows = (|| {
            for (index, item) in popup.menu.items.iter().enumerate() {
                let (top, height) = layout.rows[index];
                let top = top - popup.offset;
                if top + height < 0.0 || top > layout.height {
                    continue;
                }
                if item.entry.separator {
                    super::details::fill(
                        canvas,
                        rect(
                            1.0,
                            top + ((height - 1.0) / 2.0).floor(),
                            layout.width - 2.0,
                            1.0,
                        ),
                        &theme.flyout_separator,
                    );
                    continue;
                }
                let hot = popup.hover == Some(Hit::Row(index)) || popup.opened == Some(index);
                if hot {
                    let plate = rect(
                        4.0,
                        top + if layout.overflow { 0.0 } else { 2.0 },
                        layout.width - 8.0,
                        height - if layout.overflow { 0.0 } else { 4.0 },
                    );
                    if popup.keyboard {
                        let bounds = if layout.context {
                            plate
                        } else {
                            rect(
                                plate.left - 3.0,
                                plate.top - 2.0,
                                plate.right - plate.left + 6.0,
                                plate.bottom - plate.top + 4.0,
                            )
                        };
                        focus(canvas, bounds, theme);
                    } else {
                        super::address::rounded(
                            canvas,
                            plate,
                            4.0,
                            if popup.pressed == Some(Hit::Row(index)) {
                                &theme.subtle_pressed
                            } else {
                                &theme.subtle_hover
                            },
                        );
                    }
                }
                let shift = if item.entry.toggle_submenu { -1.0 } else { 0.0 };
                let color = if item.entry.disabled {
                    &theme.text_disabled
                } else {
                    &theme.text
                };
                label(
                    canvas,
                    text,
                    &item.entry.label,
                    LABEL,
                    rect(
                        layout.label + shift,
                        top + if layout.overflow {
                            -2.0
                        } else if layout.row == 33.0 || popup.menu.owner == "nav-options" {
                            0.0
                        } else {
                            -1.0
                        },
                        layout.width - layout.label - 16.0,
                        height,
                    ),
                    color,
                )?;
                if !item.entry.shortcut.is_empty() {
                    let width = text.layout(&item.entry.shortcut, SHORTCUT, 4096.0)?.advance;
                    label(
                        canvas,
                        text,
                        &item.entry.shortcut,
                        SHORTCUT,
                        rect(
                            layout.width - 16.0 - layout.shortcut_width,
                            top + if layout.row == 33.0 { 2.0 } else { 0.0 },
                            width,
                            height,
                        ),
                        if item.entry.disabled {
                            &theme.text_disabled
                        } else {
                            &theme.text_secondary
                        },
                    )?;
                }
                let opacity = if item.entry.disabled { 0.36 } else { 1.0 };
                if item.entry.checked {
                    label(
                        canvas,
                        text,
                        "\u{E73E}",
                        TextStyle::new(Family::Icons, 12.0, 400),
                        rect(
                            if popup.menu.owner == "nav-options" {
                                19.0
                            } else {
                                16.0
                            } + shift,
                            top + if popup.menu.owner == "nav-options" {
                                1.0
                            } else {
                                0.0
                            },
                            16.0,
                            height,
                        ),
                        if popup.menu.owner == "nav-options" {
                            &theme.text
                        } else {
                            &theme.text_secondary
                        },
                    )?;
                } else if item.entry.bullet {
                    super::address::rounded(
                        canvas,
                        rect(19.5, top + 13.5, 5.0, 5.0),
                        2.5,
                        &theme.text_secondary,
                    );
                }
                if layout.toggle_column || !item.entry.bullet && !item.entry.checked {
                    icon(
                        popup,
                        index,
                        &item.entry.icon,
                        canvas,
                        (layout.icon + shift, top + 8.0),
                        opacity,
                    )?;
                }
                if !item.entry.glyph.is_empty() {
                    label(
                        canvas,
                        text,
                        &item.entry.glyph,
                        TextStyle::new(Family::Icons, 16.0, 400),
                        rect(
                            layout.icon,
                            top + if layout.row == 33.0 {
                                -1.0
                            } else if matches!(popup.menu.owner, "address-text" | "search-text") {
                                0.0
                            } else {
                                2.0
                            },
                            16.0,
                            height,
                        ),
                        color,
                    )?;
                }
                if item.entry.submenu {
                    let size = if item.entry.toggle_submenu {
                        14.0
                    } else {
                        12.0
                    };
                    let style = TextStyle::new(Family::Icons, size, 400);
                    let glyph = if layout.dropdown || item.entry.toggle_submenu {
                        "\u{E974}"
                    } else {
                        "\u{E76C}"
                    };
                    let advance = text.layout(glyph, style, 4096.0)?.advance;
                    let left = layout.width - 30.0
                        + (16.0 - advance) / 2.0
                        + if item.sub.iter().any(|child| {
                            matches!(
                                child.action,
                                Action::SearchRefine(..) | Action::SearchCurrentFolderOnly(_)
                            )
                        }) {
                            -1.0
                        } else {
                            0.0
                        };
                    label(
                        canvas,
                        text,
                        glyph,
                        style,
                        rect(left, top, 16.0, height),
                        &theme.text_secondary,
                    )?;
                }
            }
            Ok::<(), windows::core::Error>(())
        })();
        unsafe {
            canvas.context.PopAxisAlignedClip();
        }
        rows?;
        for (index, (button, action)) in popup.menu.buttons.iter().enumerate() {
            let left = layout.button_left(index);
            let width = layout.button_width;
            let top = layout.buttons_top();
            let height = layout.buttons_height - 11.0;
            if popup.hover == Some(Hit::Button(index)) {
                if popup.keyboard {
                    focus(
                        canvas,
                        rect(left + 2.0, top + 1.0, width, height - 2.0),
                        theme,
                    );
                } else {
                    super::address::rounded(
                        canvas,
                        rect(left, top, width, height),
                        4.0,
                        &theme.subtle_hover,
                    );
                }
            }
            icon(
                popup,
                popup.menu.items.len() + index,
                &button.icon,
                canvas,
                (left + (width - 16.0) / 2.0, top + 7.0),
                if button.disabled { 0.36 } else { 1.0 },
            )?;
            let label_width = text.layout(&button.label, CAPTION, 4096.0)?.advance;
            let caption_shift = if crate::i18n::language() != "pt-BR" {
                match action {
                    Action::Cut => 2.0 / 3.0,
                    Action::Share => 0.5,
                    _ => 0.0,
                }
            } else if index + 1 == popup.menu.buttons.len() {
                0.5
            } else {
                0.0
            };
            label(
                canvas,
                text,
                &button.label,
                CAPTION,
                rect(
                    left + (width - label_width) / 2.0 + caption_shift,
                    top + 29.0,
                    label_width,
                    16.0,
                ),
                if button.disabled {
                    &theme.text_disabled
                } else {
                    &theme.text
                },
            )?;
            if index > 0 {
                super::details::fill(
                    canvas,
                    rect(left - 5.0, top + 11.0, 1.0, height - 11.0),
                    &theme.flyout_divider,
                );
            }
        }
        if !popup.menu.buttons.is_empty() {
            super::details::fill(
                canvas,
                rect(
                    5.0,
                    if layout.buttons_bottom {
                        layout.buttons_top() - 5.0
                    } else {
                        layout.padding + layout.buttons_height - 5.0
                    },
                    layout.width - 10.0,
                    1.0,
                ),
                &theme.flyout_separator,
            );
        }
        Ok(())
    })();
    unsafe {
        canvas.context.PopAxisAlignedClip();
    }
    drawn
}

fn focus(canvas: &Canvas, outer: D2D_RECT_F, theme: &Theme) {
    unsafe {
        canvas.brush.SetColor(&theme.text);
        canvas.context.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect(
                    outer.left + 1.0,
                    outer.top + 1.0,
                    outer.right - outer.left - 2.0,
                    outer.bottom - outer.top - 2.0,
                ),
                radiusX: 4.0,
                radiusY: 4.0,
            },
            &canvas.brush,
            2.0,
            None,
        );
        canvas.brush.SetColor(&theme.focus_inner);
        canvas.context.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect(
                    outer.left + 2.5,
                    outer.top + 2.5,
                    outer.right - outer.left - 5.0,
                    outer.bottom - outer.top - 5.0,
                ),
                radiusX: 2.5,
                radiusY: 2.5,
            },
            &canvas.brush,
            1.0,
            None,
        );
    }
}

fn child(session: &mut Session, level: usize, index: usize) -> Result<()> {
    while session.levels.len() > level + 1 {
        release(&session.renderer, session.levels.pop().unwrap())?;
    }
    session.levels[level].opened = None;
    let refresh = session.levels[level]
        .menu
        .items
        .get(index)
        .and_then(|item| match &item.action {
            crate::app::menu_model::Action::AppCommand(id, None, paths) if !item.sub.is_empty() => {
                Some((*id, paths.clone()))
            }
            _ => None,
        });
    if let Some((id, paths)) = refresh {
        if let Some(command) = crate::shell::extmenu::refreshed_sharing(&id, &paths) {
            let mut item = crate::app::menu_model::extension_item(&command, &paths);
            crate::app::menu_model::finalize(
                std::slice::from_mut(&mut item),
                session.levels[level].menu.owner,
            );
            session.levels[level].menu.items[index] = item;
        }
    }
    let parent = &session.levels[level];
    let Some(item) = parent
        .menu
        .items
        .get(index)
        .filter(|item| !item.sub.is_empty() && !item.entry.disabled)
    else {
        return Ok(());
    };
    let menu = Menu {
        owner: parent.menu.owner,
        items: item.sub.clone(),
        buttons: Vec::new(),
    };
    let mut layout = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        Layout::new(
            &menu,
            Level::Child,
            &mut slot
                .as_mut()
                .ok_or_else(windows::core::Error::from_win32)?
                .text,
        )
    })?;
    let scale = unsafe { GetDpiForWindow(session.owner) } as f32 / 96.0;
    let work = crate::frame::work_area(parent.at.0, parent.at.1);
    layout.height = layout
        .height
        .min((work.bottom - work.top - 20) as f32 / scale);
    let mut x = parent.at.0
        + ((parent.layout.width - if parent.layout.context { 8.0 } else { 5.0 }) * scale).round()
            as i32;
    if x + (layout.width * scale) as i32 > work.right {
        x = parent.at.0 - ((layout.width - 5.0) * scale).round() as i32;
    }
    let y = (parent.at.1 + ((parent.layout.rows[index].0 - parent.offset) * scale).round() as i32)
        .min(work.bottom - (layout.height * scale).round() as i32)
        .max(work.top);
    let intended_y =
        parent.at.1 + ((parent.layout.rows[index].0 - parent.offset) * scale).round() as i32;
    let mut popup = create(&session.renderer, session.owner, menu, layout, (x, y))?;
    animate(
        &mut popup,
        if y < intended_y {
            Direction::Up
        } else {
            Direction::Down
        },
    );
    popup.keyboard = parent.keyboard;
    if popup.keyboard {
        popup.hover = menu_layout::next(&popup.menu, None, 1);
    }
    session.levels[level].opened = Some(index);
    session.levels.push(popup);
    publish(session.owner, session.levels.len());
    Ok(())
}

fn parent(session: &mut Session) -> Result<()> {
    if session.levels.len() <= 1 {
        return Ok(());
    }
    let result = release(&session.renderer, session.levels.pop().unwrap());
    let parent = session.levels.last_mut().unwrap();
    parent.opened = None;
    parent.keyboard = true;
    result
}

pub fn owner_message(owner: HWND, message: u32, wparam: WPARAM) -> Option<LRESULT> {
    if message == WM_DESTROY {
        close_owner(owner);
        return None;
    }
    let ours = SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|session| session.owner == owner)
    });
    if !ours {
        return None;
    }
    if matches!(message, WM_THEMECHANGED | WM_DPICHANGED | WM_CAPTURECHANGED) {
        close();
        return None;
    }
    if message == WM_ACTIVATEAPP && wparam.0 == 0
        || message == WM_ACTIVATE && (wparam.0 & 0xffff) == WA_INACTIVE as usize
    {
        close();
        return None;
    }
    if message == WM_TIMER && matches!(wparam.0, TIP_TIMER | KEYS_TIMER) {
        super::input::stop(owner, wparam.0);
        let session = SESSION.with(|slot| slot.borrow_mut().take());
        if let Some(mut session) = session {
            session.key_tips = wparam.0 == KEYS_TIMER;
            if let Err(error) = adorn(&mut session) {
                eprintln!("native menu adornment: {error}");
            }
            SESSION.with(|slot| *slot.borrow_mut() = Some(session));
        }
        return Some(LRESULT(0));
    }
    if message == WM_TIMER && wparam.0 == SUBMENU_TIMER {
        super::input::stop(owner, SUBMENU_TIMER);
        let session = SESSION.with(|slot| slot.borrow_mut().take());
        if let Some(mut session) = session {
            let mut failed = None;
            if let Some(pending) = session.pending.take() {
                let result = match pending {
                    Pending::Open(level, index) => child(&mut session, level, index),
                    Pending::Close(level) => child(&mut session, level, usize::MAX),
                    Pending::Hover(level, hit) => {
                        session.levels[level].hover = hit;
                        child(
                            &mut session,
                            level,
                            match hit {
                                Some(Hit::Row(index)) => index,
                                _ => usize::MAX,
                            },
                        )
                    }
                };
                failed = result.err();
                publish(owner, session.levels.len());
            }
            SESSION.with(|slot| *slot.borrow_mut() = Some(session));
            if let Some(error) = failed {
                paint_failure(owner, error);
            } else if let Err(error) = repaint() {
                paint_failure(owner, error);
            }
        }
        return Some(LRESULT(0));
    }
    if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR) {
        let session = SESSION.with(|slot| slot.borrow_mut().take());
        let Some(mut session) = session else {
            return Some(LRESULT(0));
        };
        hide_adornment(&mut session);
        session.pending = None;
        super::input::stop(owner, SUBMENU_TIMER);
        let level = session.levels.len() - 1;
        let popup = &mut session.levels[level];
        popup.keyboard = true;
        let mut chosen = None;
        let mut dismiss = false;
        let mut submenu = None;
        let mut failed = None;
        if matches!(message, WM_CHAR | WM_SYSCHAR) {
            if let Some(character) =
                char::from_u32(wparam.0 as u32).filter(|value| value.is_ascii_alphanumeric())
            {
                if popup.layout.context {
                    if let Some(hit) = menu_layout::access_key_input(
                        &popup.menu,
                        &mut session.key_prefix,
                        character,
                    ) {
                        popup.hover = Some(hit);
                        match hit {
                            Hit::Button(index) => {
                                chosen = Some(popup.menu.buttons[index].1.clone())
                            }
                            Hit::Row(index) => {
                                let item = &popup.menu.items[index];
                                if item.sub.is_empty() {
                                    chosen = Some(item.action.clone());
                                } else {
                                    submenu = Some(index);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            let modifiers = super::input::modifiers();
            let alt = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0 };
            let key = wparam.0 as u32;
            let name = match key {
                0x0d if alt => Some("Enter".to_owned()),
                0x2e => Some("Delete".to_owned()),
                0x71 => Some("F2".to_owned()),
                0x30..=0x5a if modifiers.ctrl => char::from_u32(key).map(|value| value.to_string()),
                _ => None,
            };
            if let Some(name) = name.filter(|_| !popup.layout.context) {
                let chord = format!(
                    "{}{}{}{}",
                    if modifiers.ctrl { "Ctrl+" } else { "" },
                    if modifiers.shift { "Shift+" } else { "" },
                    if alt { "Alt+" } else { "" },
                    name
                );
                chosen = menu_layout::accelerator(&popup.menu, &chord);
            }
            if !(0x30..=0x5a).contains(&(wparam.0 as u32)) {
                session.key_prefix.clear();
            }
            if chosen.is_none() {
                match wparam.0 as u32 {
                    0x1b if level > 0 => {
                        failed = parent(&mut session).err();
                        let popup = session.levels.last_mut().unwrap();
                        if popup.layout.context {
                            popup.hover = menu_layout::next(&popup.menu, None, 1);
                        }
                    }
                    0x1b | 0x12 => dismiss = true,
                    0x09 if popup.cloud.is_some() => {
                        popup.hover = menu_layout::next(
                            &popup.menu,
                            popup.hover,
                            if modifiers.shift { -1 } else { 1 },
                        )
                    }
                    0x26 => popup.hover = menu_layout::next(&popup.menu, popup.hover, -1),
                    0x28 => popup.hover = menu_layout::next(&popup.menu, popup.hover, 1),
                    0x24 => popup.hover = menu_layout::next(&popup.menu, None, 1),
                    0x23 => popup.hover = menu_layout::next(&popup.menu, None, -1),
                    0x25 if matches!(popup.hover, Some(Hit::Button(_))) => {
                        popup.hover = menu_layout::button_next(&popup.menu, popup.hover, -1);
                    }
                    0x25 if level > 0 => {
                        failed = parent(&mut session).err();
                    }
                    0x27 | 0x0d | 0x20 => {
                        if let Some(hit) = popup.hover {
                            match hit {
                                Hit::Button(index) if wparam.0 != 0x27 => {
                                    chosen = popup
                                        .menu
                                        .buttons
                                        .get(index)
                                        .filter(|(button, _)| !button.disabled)
                                        .map(|(_, action)| action.clone())
                                }
                                Hit::Button(index) => {
                                    popup.hover = menu_layout::button_next(
                                        &popup.menu,
                                        Some(Hit::Button(index)),
                                        1,
                                    )
                                }
                                Hit::Row(index) => {
                                    if !popup.menu.items[index].sub.is_empty() {
                                        submenu = Some(index);
                                    } else if wparam.0 != 0x27 {
                                        chosen = Some(popup.menu.items[index].action.clone());
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(Hit::Row(index)) = session.levels.last().and_then(|popup| popup.hover) {
            let popup = session.levels.last_mut().unwrap();
            let (top, height) = popup.layout.rows[index];
            let (first, last) = popup.layout.row_area();
            if top - popup.offset < first {
                popup.offset = (top - first).max(0.0);
            }
            if top + height - popup.offset > last {
                popup.offset = (top + height - last).min(popup.layout.maximum_offset());
            }
        }
        let failed =
            failed.or_else(|| submenu.and_then(|index| child(&mut session, level, index).err()));
        publish(owner, session.levels.len());
        SESSION.with(|slot| *slot.borrow_mut() = Some(session));
        if let Some(error) = failed {
            paint_failure(owner, error);
            return Some(LRESULT(0));
        }
        if !dismiss && chosen.is_none() {
            arm_keys(owner);
            if session_has_tip() {
                super::input::arm(owner, TIP_TIMER, 1000);
            }
        }
        if dismiss || chosen.is_some() {
            let context = SESSION.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|session| session.context.clone())
            });
            close();
            if let (Some(action), Some(context)) = (chosen, context) {
                super::menu_actions::run(owner, context, action);
            }
        } else if let Err(error) = repaint() {
            paint_failure(owner, error);
        }
        return Some(LRESULT(0));
    }
    if matches!(
        message,
        WM_MOUSEMOVE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_MOUSEWHEEL
    ) {
        let mut point = POINT::default();
        if let Err(error) = unsafe { GetCursorPos(&mut point) } {
            super::fail(error);
            return Some(LRESULT(0));
        }
        let session = SESSION.with(|slot| slot.borrow_mut().take());
        let Some(mut session) = session else {
            return Some(LRESULT(0));
        };
        if message == WM_MOUSEMOVE && point == session.pointer {
            SESSION.with(|slot| *slot.borrow_mut() = Some(session));
            return Some(LRESULT(0));
        }
        let previous_pointer = session.pointer;
        session.pointer = point;
        let scale = unsafe { GetDpiForWindow(owner) } as f32 / 96.0;
        let level = session.levels.iter().rposition(|popup| {
            point.x >= popup.at.0
                && point.y >= popup.at.1
                && point.x < popup.at.0 + (popup.layout.width * scale) as i32
                && point.y < popup.at.1 + (popup.layout.height * scale) as i32
        });
        let mut chosen = None;
        let mut dismiss = false;
        if message != WM_MOUSEWHEEL {
            hide_adornment(&mut session);
        }
        if let Some(level) = level {
            let grace = message == WM_MOUSEMOVE
                && session.levels.get(level + 1).is_some_and(|child| {
                    menu_layout::grace(
                        previous_pointer,
                        point,
                        child.at,
                        (
                            (child.layout.width * scale) as i32,
                            (child.layout.height * scale) as i32,
                        ),
                    )
                });
            if level > 0
                && matches!(session.pending, Some(Pending::Close(parent) | Pending::Hover(parent, _)) if parent < level)
            {
                session.pending = None;
                super::input::stop(owner, SUBMENU_TIMER);
            }
            let popup = &mut session.levels[level];
            let local = (
                (point.x - popup.at.0) as f32 / scale,
                (point.y - popup.at.1) as f32 / scale,
            );
            let hit = if let Some(cloud) = &popup.cloud {
                cloud.hit(local)
            } else {
                popup.layout.hit(&popup.menu, local, popup.offset)
            };
            match message {
                WM_MOUSEMOVE => {
                    popup.keyboard = false;
                    if grace && popup.hover != hit {
                        session.pending = Some(Pending::Hover(level, hit));
                        super::input::arm(owner, SUBMENU_TIMER, 250);
                    } else if popup.hover != hit {
                        popup.hover = hit;
                        let index = if let Some(Hit::Row(index)) = hit {
                            Some(index)
                        } else {
                            None
                        };
                        session.pending = index
                            .filter(|&index| {
                                !popup.menu.items[index].sub.is_empty()
                                    && popup.opened != Some(index)
                            })
                            .map(|index| Pending::Open(level, index));
                        if session.pending.is_none()
                            && popup.opened.is_some()
                            && index != popup.opened
                        {
                            session.pending = Some(Pending::Close(level));
                        }
                        if session.pending.is_some() {
                            super::input::arm(owner, SUBMENU_TIMER, 250);
                        } else {
                            super::input::stop(owner, SUBMENU_TIMER);
                        }
                    }
                    if hit.is_some_and(|hit| match hit {
                        Hit::Button(_) => true,
                        Hit::Row(index) => !popup.menu.items[index].tip.is_empty(),
                    }) {
                        super::input::arm(owner, TIP_TIMER, 1000);
                    }
                }
                WM_LBUTTONDOWN => popup.pressed = hit,
                WM_LBUTTONUP if popup.pressed.take() == hit => {
                    chosen = match hit {
                        Some(Hit::Button(index)) => Some(popup.menu.buttons[index].1.clone()),
                        Some(Hit::Row(index)) if popup.menu.items[index].sub.is_empty() => {
                            Some(popup.menu.items[index].action.clone())
                        }
                        Some(Hit::Row(index)) => {
                            session.pending = Some(Pending::Open(level, index));
                            super::input::arm(owner, SUBMENU_TIMER, 1);
                            None
                        }
                        _ => None,
                    };
                }
                WM_MOUSEWHEEL => {
                    let maximum = popup.layout.maximum_offset();
                    popup.offset = (popup.offset - (wparam.0 >> 16) as i16 as f32 / 120.0 * 96.0)
                        .clamp(0.0, maximum);
                }
                _ => {}
            }
        } else if matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) {
            dismiss = true;
        }
        let context = chosen.as_ref().map(|_| session.context.clone());
        SESSION.with(|slot| *slot.borrow_mut() = Some(session));
        if dismiss || chosen.is_some() {
            close();
            if let (Some(action), Some(context)) = (chosen, context) {
                super::menu_actions::run(owner, context, action);
            }
        } else if let Err(error) = repaint() {
            paint_failure(owner, error);
        }
        return Some(LRESULT(0));
    }
    None
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_PAINT => {
            let owner = SESSION.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map_or(HWND::default(), |session| session.owner)
            });
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            if let Err(error) = repaint() {
                paint_failure(owner, error);
            }
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn command_state(owner: HWND, command: Option<super::chrome::Command>) {
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.chrome.open_menu = command;
        }
    });
    super::chrome::invalidate(
        owner,
        super::render::Damage {
            commands: true,
            ..super::render::Damage::NONE
        },
    );
}

enum Direction {
    Up,
    Down,
}

fn animate(popup: &mut Popup, direction: Direction) {
    let mut enabled = windows::core::BOOL(0);
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut _ as _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    };
    if let Err(error) = result {
        eprintln!("native menu animation setting: {error}");
        return;
    }
    if enabled.as_bool() {
        popup.animation = Some(
            popup.layout.height / 2.0
                * if matches!(direction, Direction::Down) {
                    -1.0
                } else {
                    1.0
                },
        );
    }
}

fn paint_failure(owner: HWND, error: windows::core::Error) {
    if super::render::device_lost(&error) {
        eprintln!("native menu device loss: {error}");
        close();
        super::invalidate_all(owner);
    } else {
        super::fail(error);
    }
}

fn arm_keys(owner: HWND) {
    let enabled = SESSION.with(|slot| {
        slot.borrow().as_ref().is_some_and(|session| {
            session.levels.last().is_some_and(|popup| {
                popup.layout.context
                    && (popup
                        .menu
                        .items
                        .iter()
                        .any(|item| !item.entry.disabled && !item.entry.key_tip.is_empty())
                        || popup
                            .menu
                            .buttons
                            .iter()
                            .any(|(button, _)| !button.disabled && !button.key_tip.is_empty()))
            })
        })
    });
    if enabled {
        super::input::arm(owner, KEYS_TIMER, 1200);
    }
}

fn session_has_tip() -> bool {
    SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|session| session.levels.last())
            .is_some_and(|popup| match popup.hover {
                Some(Hit::Button(_)) => true,
                Some(Hit::Row(index)) => !popup.menu.items[index].tip.is_empty(),
                None => false,
            })
    })
}

fn hide_adornment(session: &mut Session) {
    super::tooltip::dismiss(session.owner);
    for timer in [TIP_TIMER, KEYS_TIMER] {
        super::input::stop(session.owner, timer);
    }
    session.key_tips = false;
    publish(session.owner, session.levels.len());
    if let Some(overlay) = session.key_overlay.take() {
        if let Err(error) = overlay.close(&session.renderer) {
            eprintln!("native menu adornment close: {error}");
        }
    }
}

fn adorn(session: &mut Session) -> Result<()> {
    let previous = if session.key_tips {
        session.key_overlay.take()
    } else {
        super::tooltip::dismiss(session.owner);
        None
    };
    if let Some(overlay) = previous {
        overlay.close(&session.renderer)?;
    }
    let popup = session
        .levels
        .last()
        .ok_or_else(windows::core::Error::from_win32)?;
    let data = if session.key_tips {
        let mut badges = Vec::new();
        for (index, item) in popup
            .menu
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.entry.disabled && !item.entry.key_tip.is_empty())
        {
            badges.push((
                popup.layout.width - 5.0,
                popup.layout.rows[index].0 - popup.offset + 4.0,
                item.entry.key_tip.clone(),
            ));
        }
        for (index, (button, _)) in popup
            .menu
            .buttons
            .iter()
            .enumerate()
            .filter(|(_, (button, _))| !button.disabled && !button.key_tip.is_empty())
        {
            badges.push((
                popup.layout.button_left(index) + popup.layout.button_width / 2.0 - 5.0,
                popup.layout.buttons_top() - 26.0,
                button.key_tip.clone(),
            ));
        }
        super::menu_tip::Content::Keys {
            badges,
            size: (popup.layout.width + 40.0, popup.layout.height + 32.0),
            at: (popup.at.0, popup.at.1 - 28),
        }
    } else {
        let (text, pointer) = match popup.hover {
            Some(Hit::Row(index)) => (popup.menu.items[index].tip.clone(), session.pointer),
            Some(Hit::Button(index)) => {
                use crate::app::menu_model::Action;
                let (button, action) = &popup.menu.buttons[index];
                let shortcut = match action {
                    Action::Cut => "Ctrl+X",
                    Action::Copy => "Ctrl+C",
                    Action::Paste => "Ctrl+V",
                    Action::Rename => "F2",
                    Action::Delete => "Delete",
                    _ => "",
                };
                let text = if shortcut.is_empty() {
                    button.label.clone()
                } else {
                    format!("{} ({shortcut})", button.label)
                };
                let scale = unsafe { GetDpiForWindow(session.owner) } as f32 / 96.0;
                let pointer = if popup.keyboard {
                    POINT {
                        x: popup.at.0
                            + ((popup.layout.button_left(index)
                                + popup.layout.button_width / 2.0
                                + 12.0)
                                * scale)
                                .round() as i32,
                        y: popup.at.1 + ((popup.layout.buttons_top() + 3.0) * scale).round() as i32,
                    }
                } else {
                    session.pointer
                };
                (text, pointer)
            }
            None => return Ok(()),
        };
        if text.is_empty() {
            return Ok(());
        }
        super::menu_tip::Content::Description { text, pointer }
    };
    if session.key_tips {
        session.key_overlay = Some(super::menu_tip::Overlay::show(
            &session.renderer,
            session.owner,
            data,
        )?);
    } else {
        super::tooltip::menu(session.renderer.clone(), session.owner, data)?;
    }
    unsafe {
        SetPropW(
            session.owner,
            w!("NativeMenuBackingCount"),
            Some(HANDLE(
                (session.levels.len()
                    + usize::from(super::tooltip::showing(session.owner))
                    + usize::from(session.key_overlay.is_some())) as _,
            )),
        )?;
    }
    Ok(())
}

pub fn is_open(owner: HWND) -> bool {
    SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|session| session.owner == owner)
    })
}

fn remaining(progress: f32) -> f32 {
    let parameter = progress.clamp(0.0, 1.0).cbrt();
    1.0 - 3.0 * parameter * parameter + 2.0 * parameter * parameter * parameter
}

const ANIMATION_SECONDS: f64 = 0.25;
const ANIMATION_SEGMENTS: usize = 12;

fn curve() -> Vec<(f64, [f64; 4])> {
    (0..ANIMATION_SEGMENTS)
        .map(|segment| {
            let progress =
                |third: f64| ((segment as f64 + third / 3.0) / ANIMATION_SEGMENTS as f64).powi(3);
            let start = progress(0.0);
            let step = (progress(3.0) - start) / 3.0;
            let y: [f64; 4] =
                std::array::from_fn(|index| remaining((start + step * index as f64) as f32) as f64);
            let first = y[1] - y[0];
            let second = y[2] - 2.0 * y[1] + y[0];
            let third = y[3] - 3.0 * y[2] + 3.0 * y[1] - y[0];
            let seconds = step * ANIMATION_SECONDS;
            (
                start * ANIMATION_SECONDS,
                [
                    y[0],
                    (first - second / 2.0 + third / 3.0) / seconds,
                    (second - third) / 2.0 / seconds.powi(2),
                    third / 6.0 / seconds.powi(3),
                ],
            )
        })
        .collect()
}

fn advance(popup: &mut Popup, composition: &IDCompositionDevice) -> Result<()> {
    let (Some(distance), Some(layer)) = (popup.animation.take(), &popup.layer) else {
        return Ok(());
    };
    let pixels = (distance * unsafe { GetDpiForWindow(popup.hwnd) } as f32 / 96.0) as f64;
    unsafe {
        let animation = composition.CreateAnimation()?;
        for (begin, coefficients) in curve() {
            let [constant, linear, quadratic, cubic] =
                coefficients.map(|value| (value * pixels) as f32);
            animation.AddCubic(begin, constant, linear, quadratic, cubic)?;
        }
        animation.End(ANIMATION_SECONDS, 0.0)?;
        layer.visual.SetOffsetY(&animation)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn submenu_curve_uses_time_instead_of_the_bezier_parameter() {
        assert_eq!(super::remaining(0.0), 1.0);
        assert_eq!(super::remaining(1.0), 0.0);
        assert!((super::remaining(0.125) - 0.5).abs() < 0.0001);
    }

    #[test]
    fn composition_curve_follows_the_submenu_curve() {
        let segments = super::curve();
        for sample in 0..=1000 {
            let seconds = sample as f64 / 1000.0 * super::ANIMATION_SECONDS;
            let (begin, [constant, linear, quadratic, cubic]) = segments
                .iter()
                .rev()
                .find(|(begin, _)| *begin <= seconds)
                .unwrap();
            let x = seconds - begin;
            let value = constant + linear * x + quadratic * x * x + cubic * x * x * x;
            let expected = super::remaining((seconds / super::ANIMATION_SECONDS) as f32) as f64;
            assert!(
                (value - expected).abs() < 0.005,
                "{seconds}: {value} vs {expected}"
            );
        }
    }
}

pub fn cloud_changed(owner: HWND) {
    let previous = SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|session| session.owner == owner)
            .and_then(|session| session.levels.first())
            .filter(|popup| popup.cloud.is_some())
            .map(|popup| (popup.hover, popup.keyboard))
    });
    let Some((hover, keyboard)) = previous else {
        return;
    };
    close();
    if let Err(error) = super::cloud::show(owner) {
        eprintln!("native cloud refresh: {error}");
        return;
    }
    SESSION.with(|slot| {
        if let Some(popup) = slot
            .borrow_mut()
            .as_mut()
            .and_then(|session| session.levels.first_mut())
        {
            popup.keyboard = keyboard;
            popup.hover = hover.filter(|hit| match hit {
                Hit::Row(index) => popup
                    .menu
                    .items
                    .get(*index)
                    .is_some_and(|item| !item.entry.disabled),
                _ => false,
            });
        }
    });
    if let Err(error) = repaint() {
        paint_failure(owner, error);
    }
}
