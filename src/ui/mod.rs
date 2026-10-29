mod address;
mod address_input;
mod address_popup;
mod bars;
mod caret;
mod check;
mod chrome;
mod cloud;
mod context;
mod coverage;
mod cut;
mod details;
mod dib;
mod drag_visual;
mod drop;
mod edit;
mod filter;
mod grid;
mod header;
mod hosted;
mod icons;
mod input;
mod interaction;
mod jumplist;
mod launch;
mod list;
mod list_surface;
mod location;
mod memory;
mod menu;
mod menu_actions;
mod menu_layout;
mod menu_tip;
mod navigation;
mod navigation_input;
mod navigation_surface;
mod options;
mod panes;
mod preferences;
mod rename;
mod render;
mod scroll;
mod search;
mod session;
mod settings;
mod svg;
mod tab_transfer;
mod tabs;
mod tabs_input;
mod text;
mod text_run;
pub mod theme;
mod titlebar;
mod tooltip;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use windows::core::{w, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use context::{DAMAGE, LIST_PENDING, NAV_PENDING, WINDOW};
use render::{Damage, Renderer, Surface};
use theme::Theme;
use titlebar::{Target, TitleBar};

struct Window {
    panes: panes::State,
    settings: settings::State,
    search: search::State,
    drag: drop::State,
    tab_transfer: tab_transfer::State,
    before_new: Option<(String, std::collections::HashSet<String>)>,
    menu_mailbox: menu_actions::Mailbox,
    rename_created: Option<String>,
    undo: Vec<crate::app::undo::Undo>,
    redo: Vec<crate::app::undo::Undo>,
    _preferences: preferences::Watch,
    tabs: tabs::Tabs,
    journal: Option<crate::app::session::Tracker>,
    tab_clock: u64,
    pending_scroll: Option<(f32, f32)>,
    pending_visit: Option<crate::app::selection::Selection>,
    reveal_path: Option<String>,
    input_timers: std::collections::HashSet<usize>,
    graphics: Option<(Rc<Renderer>, Surface)>,
    theme: Theme,
    title: TitleBar,
    text: text::TextEngine,
    list: list::List,
    chrome: chrome::State,
    address: address::State,
    editors: edit::Controls,
    address_worker: address::Worker,
    navigation: navigation::State,
    navigation_worker: navigation::Worker,
    _navigation_notifications: navigation::Notifications,
    worker: list::Worker,
    scroll: scroll::State,
    started: std::time::Instant,
    first_list: bool,
    paint_max_us: usize,
}

thread_local! {
    static FAILURE: RefCell<Option<windows::core::Error>> = const { RefCell::new(None) };
}

pub fn run() -> Result<()> {
    memory::checkpoint("native-entry");
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        OleInitialize(None)?;
    }
    memory::checkpoint("com-initialized");
    let result = run_window();
    unsafe {
        OleUninitialize();
    }
    result
}

fn run_window() -> Result<()> {
    let started = std::time::Instant::now();
    crate::shell::jumplist::identity();
    if crate::app::entry::start().map_err(|error| {
        windows::core::Error::new(windows::Win32::Foundation::E_INVALIDARG, error)
    })? {
        return Ok(());
    }
    let launch = crate::app::entry::options();
    let path = launch
        .path
        .clone()
        .unwrap_or_else(crate::app::ids::start_location);
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(instance.into()), windows::core::PCWSTR(1 as _))?,
            lpszClassName: w!("SwExplorerFrame"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        let result =
            create_window_with_launch(&path, started, &launch).and_then(|_| message_loop());
        for hwnd in context::handles() {
            if IsWindow(Some(hwnd)).as_bool() {
                DestroyWindow(hwnd)?;
            }
        }
        UnregisterClassW(class.lpszClassName, Some(instance.into()))?;
        result
    }
}

fn create_window(path: &str, started: std::time::Instant) -> Result<HWND> {
    let launch = crate::app::entry::Launch {
        mode: crate::app::entry::Mode::NewWindow,
        path: Some(path.into()),
        ..Default::default()
    };
    create_window_with_launch(path, started, &launch)
}

fn create_window_with_launch(
    path: &str,
    started: std::time::Instant,
    launch: &crate::app::entry::Launch,
) -> Result<HWND> {
    let renderer = context::renderer()?;
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("SwExplorerFrame"),
            w!("slint-win-explorer"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1116,
            727,
            None,
            None,
            Some(GetModuleHandleW(None)?.into()),
            None,
        )?
    };
    if let Err(error) = initialize(hwnd, renderer, path, started) {
        if let Err(cleanup) = unsafe { DestroyWindow(hwnd) } {
            eprintln!("native window cleanup: {cleanup}");
        }
        return Err(error);
    }
    if let Err(error) = jumplist::start(hwnd) {
        eprintln!("native taskbar identity: {error}");
    }
    let others = match session::start(hwnd, launch) {
        Ok(others) => others,
        Err(error) => {
            eprintln!("native session start: {error}");
            Vec::new()
        }
    };
    for (id, snapshot) in others {
        let launch = crate::app::entry::Launch {
            restore: Some(id.clone()),
            ..Default::default()
        };
        if let Err(error) = create_window_with_launch(
            &snapshot.paths[snapshot.active],
            std::time::Instant::now(),
            &launch,
        ) {
            eprintln!("native restored window: {error}");
            if let Err(error) = crate::app::session::release_claim(&id) {
                eprintln!("release session claim: {error}");
            }
        }
    }
    {
        let _context = context::enter(hwnd);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow().as_ref() {
                launch::routes(hwnd, &window.tabs);
            }
        });
        if let Some(path) = &launch.select {
            launch::select(hwnd, path.clone());
        }
        crate::app::entry::ready_window(hwnd);
        if launch.path.is_some() || launch.mode == crate::app::entry::Mode::NewWindow {
            launch::activate(hwnd);
        }
    }
    Ok(hwnd)
}

fn initialize(
    hwnd: HWND,
    renderer: Rc<Renderer>,
    path: &str,
    started: std::time::Instant,
) -> Result<()> {
    let _context = context::enter(hwnd);
    crate::frame::native_window(hwnd);
    crate::frame::prepare(hwnd)?;
    memory::checkpoint("window-frame");
    let title = TitleBar::new(path)?;
    memory::checkpoint("shell-title-icon");
    let surface = renderer.surface(hwnd, &title.icon)?;
    let caption = crate::shell::wide(&crate::i18n::system_format(
        "explorerframe.dll",
        35104,
        &[&title.title],
    ));
    unsafe {
        SetWindowTextW(hwnd, windows::core::PCWSTR(caption.as_ptr()))?;
    }
    let text = text::TextEngine::new(unsafe { GetDpiForWindow(hwnd) })?;
    memory::checkpoint("dwrite-factory");
    let worker = list::Worker::start().map_err(|error| {
        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
    })?;
    let list = list::List::start(&worker, hwnd, path.to_string(), None).map_err(|error| {
        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
    })?;
    let preferences = preferences::Watch::start(hwnd)?;
    unsafe {
        windows::Win32::System::DataExchange::AddClipboardFormatListener(hwnd)?;
    }
    let editors = edit::Controls::new(hwnd)?;
    let address_worker = address::Worker::start(hwnd).map_err(|error| {
        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
    })?;
    memory::checkpoint("address-workers-created");
    let navigation_worker = navigation::Worker::start(hwnd, &address_worker);
    let navigation_notifications = navigation::Notifications::new(hwnd)?;
    let mut navigation = navigation::State::default();
    navigation.refresh(&navigation_worker);
    memory::checkpoint("tab-model-before");
    let tabs = crate::app::tabs::Collection::new(tabs::Saved::new(path));
    memory::checkpoint("tab-model-after");
    WINDOW.with(|window| {
        *window.borrow_mut() = Some(Window {
            panes: Default::default(),
            settings: Default::default(),
            search: Default::default(),
            drag: Default::default(),
            tab_transfer: Default::default(),
            before_new: None,
            menu_mailbox: menu_actions::Mailbox::new(),
            rename_created: None,
            undo: Vec::new(),
            redo: Vec::new(),
            _preferences: preferences,
            tabs,
            journal: None,
            tab_clock: 0,
            pending_scroll: None,
            pending_visit: None,
            reveal_path: None,
            input_timers: Default::default(),
            graphics: Some((renderer, surface)),
            theme: Theme::system(),
            title,
            text,
            list,
            chrome: Default::default(),
            address: Default::default(),
            editors,
            address_worker,
            navigation,
            navigation_worker,
            _navigation_notifications: navigation_notifications,
            worker,
            scroll: Default::default(),
            started,
            first_list: false,
            paint_max_us: 0,
        })
    });
    crate::dnd::register(hwnd, Box::new(drop::Adapter(hwnd)))?;
    crate::app::tab_transfer::install(hwnd);
    cut::update(hwnd);
    crate::frame::restore_placement(hwnd)?;
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        invalidate_all(hwnd);
    }
    Ok(())
}

fn message_loop() -> Result<()> {
    unsafe {
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0).0;
            if result == -1 {
                return Err(windows::core::Error::from_win32());
            }
            if result == 0 {
                break;
            }
            if hosted::message(&message) {
                continue;
            }
            if matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN)
                && panes::contains(message.hwnd)
            {
                let key = message.wParam.0 as u32;
                let mods = input::modifiers();
                let alt = windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0;
                if key == 0x75
                    || alt && matches!(key, 0x50 | 0x25 | 0x27 | 0x26)
                    || mods.ctrl && matches!(key, 0x4c | 0x45 | 0x4e | 0x54 | 0x57)
                {
                    let owner = GetAncestor(message.hwnd, GA_ROOT);
                    let _ = SendMessageW(
                        owner,
                        message.message,
                        Some(message.wParam),
                        Some(message.lParam),
                    );
                    continue;
                }
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    match FAILURE.with(|failure| failure.borrow_mut().take()) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        context::register(hwnd);
    }
    let _context = context::enter(hwnd);
    if let Some(result) = launch::message(hwnd, message, wparam, lparam) {
        return result;
    }
    if let Some(result) = tab_transfer::message(hwnd, message, wparam, lparam) {
        return result;
    }
    if message == drop::FINISHED {
        drop::finished(
            hwnd,
            windows::Win32::System::Ole::DROPEFFECT(wparam.0 as u32),
        );
        return LRESULT(0);
    }
    if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN)
        && wparam.0 == 0x1b
        && crate::dnd::cancel_drag()
    {
        return LRESULT(0);
    }
    if message == WM_KEYDOWN && wparam.0 == 0x1b && !menu::is_open(hwnd) && search::cancel(hwnd) {
        return LRESULT(0);
    }
    if crate::dnd::active_workers() != 0
        && matches!(
            message,
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR
        )
    {
        return LRESULT(0);
    }
    if message == WM_TIMER && tooltip::timer(hwnd, wparam.0) {
        return LRESULT(0);
    }
    if matches!(
        message,
        WM_LBUTTONDOWN
            | WM_NCLBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_MOUSEWHEEL
            | WM_KEYDOWN
            | WM_SYSKEYDOWN
            | WM_CHAR
            | WM_DESTROY
            | WM_CANCELMODE
    ) {
        tooltip::suppress(hwnd);
    }
    if message == WM_ACTIVATE {
        if wparam.0 as u32 & 0xffff == WA_INACTIVE {
            tooltip::dismiss(hwnd);
        } else {
            settings::refresh(hwnd);
        }
    }
    if message == WM_DESTROY {
        filter::close(hwnd);
        launch::close(hwnd);
        jumplist::close(hwnd);
        location::close(hwnd);
        hosted::close(hwnd);
        panes::close(hwnd);
        tooltip::closed(hwnd);
    }
    if let Some(result) = filter::owner_message(hwnd, message, wparam, lparam) {
        return result;
    }
    if let Some(result) = menu::owner_message(hwnd, message, wparam) {
        return result;
    }
    if let Some(result) = rename::owner_message(hwnd, message, wparam, lparam) {
        return result;
    }
    if let Some(result) = crate::frame::non_client(hwnd, message, wparam, lparam, DefWindowProcW) {
        return result;
    }
    match message {
        #[cfg(debug_assertions)]
        navigation::DIAGNOSTICS => {
            navigation_input::diagnostics();
            return LRESULT(0);
        }
        launch::READY => {
            launch::ready(hwnd);
            return LRESULT(0);
        }
        hosted::READY => {
            hosted::ready(hwnd);
            return LRESULT(0);
        }
        location::CHANGED => {
            location::changed(hwnd, wparam.0, lparam.0);
            return LRESULT(0);
        }
        navigation::CHANGED => {
            navigation_input::notified(wparam.0, lparam.0);
            return LRESULT(0);
        }
        navigation::READY => {
            if WINDOW.with(|slot| slot.borrow().is_none()) {
                NAV_PENDING.with(|pending| pending.set(true));
            } else {
                navigation_input::ready(hwnd);
            }
            return LRESULT(0);
        }
        address::READY => {
            if WINDOW.with(|slot| slot.borrow().is_none()) {
                context::ADDRESS_PENDING.with(|pending| pending.set(true));
            } else {
                address_input::ready(hwnd);
            }
            return LRESULT(0);
        }
        menu_actions::ASYNC_DONE => {
            menu_actions::completed(hwnd);
            return LRESULT(0);
        }
        list::READY => {
            let started = std::time::Instant::now();
            let update = WINDOW.with(|slot| {
                slot.borrow_mut()
                    .as_mut()
                    .map(|window| window.list.receive(&mut window.scroll))
            });
            match update {
                Some(Ok(Some(rows))) if rows.end == usize::MAX => {
                    if let Err(error) = loaded(hwnd) {
                        fail(error);
                    }
                    invalidate_all(hwnd);
                }
                Some(Ok(Some(rows))) if rows.is_empty() => invalidate_view(hwnd),
                Some(Ok(Some(rows))) => invalidate_rows(hwnd, rows),
                Some(Err(error)) => fail(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    error,
                )),
                None => LIST_PENDING.with(|pending| pending.set(true)),
                _ => {}
            }
            header::flush(hwnd);
            search::sync(hwnd);
            chrome::sync(hwnd);
            menu_actions::loaded(hwnd);
            filter::ready(hwnd);
            memory::work("update", started);
            return LRESULT(0);
        }
        WM_NCMOUSEMOVE => {
            panes::leave(hwnd);
            navigation_input::leave(hwnd);
            address_input::leave(hwnd);
            chrome::leave(hwnd);
            header::leave(hwnd);
            input::leave(hwnd);
            leave_scroll(hwnd);
            crate::frame::track_nc_leave(hwnd);
            let mut tip_point = windows::Win32::Foundation::POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            if windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut tip_point).as_bool() {
                let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
                tooltip::moved(
                    hwnd,
                    (tip_point.x as f32 / scale, tip_point.y as f32 / scale),
                );
            }
            let button = crate::frame::hit_button(wparam.0 as u32);
            set_hover(
                hwnd,
                if button == crate::frame::Button::None {
                    Target::None
                } else {
                    Target::Caption(button)
                },
            );
        }
        WM_MOUSEMOVE => {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
            };
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            if let Err(error) = TrackMouseEvent(&mut tracking) {
                fail(error);
            }
            if drop::moved(hwnd, client_point(hwnd, lparam)) || drop::active() {
                return LRESULT(0);
            }
            if tabs_input::moved(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if settings::moved(hwnd, client_point(hwnd, lparam)) {
                address_input::moved(hwnd, client_point(hwnd, lparam));
                return LRESULT(0);
            }
            if panes::moved(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            navigation_input::moved(hwnd, client_point(hwnd, lparam));
            address_input::moved(hwnd, client_point(hwnd, lparam));
            chrome::moved(hwnd, client_point(hwnd, lparam));
            move_scroll(hwnd, client_point(hwnd, lparam));
            header::moved(hwnd, client_point(hwnd, lparam));
            input::moved(hwnd, client_point(hwnd, lparam));
            tooltip::moved(hwnd, client_point(hwnd, lparam));
            match target_at(hwnd, lparam) {
                Ok(target) => set_hover(hwnd, target),
                Err(error) => fail(error),
            }
        }
        WM_NCMOUSELEAVE | WM_MOUSELEAVE => {
            tooltip::dismiss(hwnd);
            navigation_input::leave(hwnd);
            address_input::leave(hwnd);
            chrome::leave(hwnd);
            header::leave(hwnd);
            input::leave(hwnd);
            set_hover(hwnd, Target::None);
            leave_scroll(hwnd);
        }
        WM_MOUSEWHEEL => {
            if !drop::active() && grid::zoom(hwnd, (wparam.0 >> 16) as i16 as i32) {
                return LRESULT(0);
            }
            let mut point = windows::Win32::Foundation::POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            if !windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point).as_bool() {
                fail(windows::core::Error::from_win32());
                return LRESULT(0);
            }
            let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
            if settings::wheel(
                hwnd,
                (point.x as f32 / scale, point.y as f32 / scale),
                (wparam.0 >> 16) as i16 as i32,
            ) {
                return LRESULT(0);
            }
            if navigation_input::wheel(
                hwnd,
                (point.x as f32 / scale, point.y as f32 / scale),
                (wparam.0 >> 16) as i16 as i32,
            ) {
                return LRESULT(0);
            }
            let changed = WINDOW.with(|slot| {
                slot.borrow_mut().as_mut().is_some_and(|w| {
                    w.scroll.wheel(
                        (point.x as f32 / scale, point.y as f32 / scale),
                        (wparam.0 >> 16) as i16 as i32,
                    )
                })
            });
            if changed {
                grid::start_animation(hwnd);
                tooltip::scrolled();
                invalidate_view(hwnd);
            }
            return LRESULT(0);
        }
        WM_MOUSEHWHEEL => return LRESULT(0),
        WM_SETCURSOR
            if lparam.0 as u32 & 0xffff == HTCLIENT
                && (panes::cursor()
                    || navigation_input::cursor()
                    || address_input::cursor()
                    || header::cursor(hwnd)
                    || input::cursor()) =>
        {
            return LRESULT(1)
        }
        WM_TIMER
            if location::timer(hwnd, wparam.0)
                || launch::timer(hwnd, wparam.0)
                || search::timer(hwnd, wparam.0)
                || drop::timer(hwnd, wparam.0)
                || tab_transfer::timer(hwnd, wparam.0)
                || panes::timer(hwnd, wparam.0)
                || grid::timer(hwnd, wparam.0)
                || tabs_input::timer(hwnd, wparam.0)
                || navigation_input::timer(hwnd, wparam.0)
                || address_input::timer(hwnd, wparam.0)
                || header::timer(hwnd, wparam.0)
                || input::timer(hwnd, wparam.0) =>
        {
            return LRESULT(0)
        }
        WM_TIMER if wparam.0 == SCROLL_TIMER => {
            let (first, changed) = WINDOW.with(|slot| {
                slot.borrow_mut().as_mut().map_or((false, false), |w| {
                    if w.scroll.pressed.is_none() {
                        return (false, false);
                    }
                    let first = !w.scroll.repeating;
                    (first, w.scroll.tick())
                })
            });
            if first && SetTimer(Some(hwnd), SCROLL_TIMER, 62, None) == 0 {
                fail(windows::core::Error::from_win32());
            }
            if changed {
                invalidate_view(hwnd);
            }
            return LRESULT(0);
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            let button = crate::frame::hit_button(wparam.0 as u32);
            if button != crate::frame::Button::None {
                press_title(hwnd, Target::Caption(button));
                return LRESULT(0);
            }
        }
        WM_LBUTTONDOWN => {
            if input::network_notice(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if settings::press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if panes::press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if tabs_input::press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if navigation_input::press(hwnd, client_point(hwnd, lparam)) {
                drop::pressed(hwnd, client_point(hwnd, lparam));
                return LRESULT(0);
            }
            if address_input::press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if chrome::press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if header::press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if grid::timeline_press(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            match press_scroll(hwnd, client_point(hwnd, lparam)) {
                Ok(true) => return LRESULT(0),
                Err(error) => fail(error),
                _ => {}
            }
            if input::press(hwnd, client_point(hwnd, lparam)) {
                drop::pressed(hwnd, client_point(hwnd, lparam));
                return LRESULT(0);
            }
            match target_at(hwnd, lparam) {
                Ok(target) if target != Target::None => {
                    press_title(hwnd, target);
                    return LRESULT(0);
                }
                Err(error) => fail(error),
                _ => {}
            }
        }
        WM_MBUTTONUP if tabs_input::middle(hwnd, client_point(hwnd, lparam)) => return LRESULT(0),
        WM_RBUTTONDOWN if drop::right_pressed(hwnd, client_point(hwnd, lparam)) => {
            return LRESULT(0)
        }
        WM_RBUTTONUP if settings::active() && client_point(hwnd, lparam).1 >= 88.0 => {
            return LRESULT(0)
        }
        WM_RBUTTONUP if drop::active() || drop::release_right(hwnd) => return LRESULT(0),
        WM_RBUTTONUP
            if tabs_input::context(hwnd, client_point(hwnd, lparam))
                || navigation_input::context(hwnd, client_point(hwnd, lparam))
                || address_input::context(hwnd, client_point(hwnd, lparam))
                || header::context(hwnd, client_point(hwnd, lparam))
                || tabs_input::list_context(hwnd, client_point(hwnd, lparam))
                || options::context(hwnd, client_point(hwnd, lparam)) =>
        {
            return LRESULT(0)
        }
        WM_LBUTTONUP | WM_NCLBUTTONUP => {
            drop::released();
            if message == WM_LBUTTONUP && settings::release(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP && panes::release(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP && tabs_input::release(hwnd) {
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP
                && navigation_input::release(hwnd, client_point(hwnd, lparam))
            {
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP && address_input::release(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP && chrome::release(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP && header::release(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if release_scroll(hwnd) {
                if let Err(error) = release_capture(hwnd) {
                    fail(error);
                }
                return LRESULT(0);
            }
            if message == WM_LBUTTONUP && input::release(hwnd) {
                return LRESULT(0);
            }
            let pressed = WINDOW.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map_or(Target::None, |window| window.title.pressed)
            });
            if pressed != Target::None {
                let target = if message == WM_LBUTTONUP {
                    target_at(hwnd, lparam)
                } else {
                    let button = crate::frame::hit_button(wparam.0 as u32);
                    Ok(if button == crate::frame::Button::None {
                        Target::None
                    } else {
                        Target::Caption(button)
                    })
                };
                match target {
                    Ok(target) => set_hover(hwnd, target),
                    Err(error) => fail(error),
                }
                if let Err(error) = release_title(hwnd, pressed) {
                    fail(error);
                }
                return LRESULT(0);
            }
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            if message == WM_CAPTURECHANGED {
                tabs_input::capture_changed(hwnd);
            } else {
                tabs_input::cancel(hwnd);
            }
            settings::cancel();
            panes::cancel(hwnd);
            navigation_input::cancel(hwnd);
            address_input::cancel(hwnd);
            chrome::cancel(hwnd);
            header::cancel(hwnd);
            input::cancel(hwnd);
            release_scroll(hwnd);
            let changed = WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    let changed = window.title.pressed != Target::None;
                    window.title.pressed = Target::None;
                    changed
                } else {
                    false
                }
            });
            if changed {
                invalidate_title(hwnd);
            }
        }
        WM_SETFOCUS if !menu::is_open(hwnd) => address_input::restore_focus(),
        WM_ACTIVATE => {
            let active = wparam.0 as u32 & 0xffff != WA_INACTIVE;
            let changed = WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    window.list.interaction.active = active;
                    window.list.interaction.mark_all();
                    let changed = window.title.active != active;
                    window.title.active = active;
                    changed
                } else {
                    false
                }
            });
            if changed {
                invalidate_title(hwnd);
            }
            navigation_input::focus_changed(hwnd);
            if active && changed {
                navigation_input::refresh();
            }
            input::flush(hwnd);
            if !active {
                address_input::cancel(hwnd);
                chrome::cancel(hwnd);
                header::cancel(hwnd);
                input::cancel(hwnd);
            }
        }
        WM_LBUTTONDBLCLK => {
            if settings::active() && client_point(hwnd, lparam).1 >= 88.0 {
                return LRESULT(0);
            }
            if navigation_input::double_click(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if address_input::double_click(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            if header::double_click(hwnd, client_point(hwnd, lparam)) {
                return LRESULT(0);
            }
            input::double_click(hwnd, client_point(hwnd, lparam));
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN
            if settings::key(hwnd, wparam.0 as u32)
                || hosted::key(hwnd, wparam.0 as u32)
                || panes::key(hwnd, wparam.0 as u32)
                || grid::key(hwnd, wparam.0 as u32)
                || tabs_input::key(hwnd, wparam.0 as u32)
                || navigation_input::key(hwnd, wparam.0 as u32)
                || address_input::key(hwnd, wparam.0 as u32)
                || menu_actions::key(hwnd, wparam.0 as u32)
                || chrome::key(hwnd, wparam.0 as u32)
                || header::key(hwnd, wparam.0 as u32)
                || input::key(hwnd, wparam.0 as u32) =>
        {
            return LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP if chrome::key_up(hwnd, wparam.0 as u32) => return LRESULT(0),
        WM_SYSKEYUP if wparam.0 == 0x12 => return LRESULT(0),
        WM_CHAR => {
            if navigation_input::character(hwnd, wparam.0 as u32) {
                return LRESULT(0);
            }
            input::character(hwnd, wparam.0 as u32);
            return LRESULT(0);
        }
        preferences::CHANGED => {
            if WINDOW.with(|slot| slot.borrow().is_none()) {
                context::PREFERENCES_PENDING.with(|pending| pending.set(true));
                return LRESULT(0);
            }
            jumplist::preferences();
            settings::refresh(hwnd);
            session::checkpoint(hwnd);
            if input::update_settings() {
                invalidate_all(hwnd);
            }
            input::flush(hwnd);
            return LRESULT(0);
        }
        WM_CLIPBOARDUPDATE => {
            cut::update(hwnd);
            return LRESULT(0);
        }
        WM_SETTINGCHANGE if input::settings_changed(lparam) => {
            navigation_input::refresh();
            input::cancel(hwnd);
            if input::update_settings() {
                invalidate_all(hwnd);
            }
            input::flush(hwnd);
            return LRESULT(0);
        }
        WM_NCRBUTTONUP if wparam.0 as u32 == HTCAPTION => {
            crate::frame::show_system_menu(hwnd, lparam);
            return LRESULT(0);
        }
        WM_SYSKEYDOWN if wparam.0 == 0x20 => {
            let (x, y) = crate::frame::client_to_screen(hwnd, 0, 32);
            crate::frame::system_menu_at(hwnd, x, y);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let result = paint_window(hwnd);
            let _ = EndPaint(hwnd, &paint);
            if let Err(error) = result {
                fail(error);
            }
            return LRESULT(0);
        }
        WM_SIZE if wparam.0 as u32 != SIZE_MINIMIZED => {
            let dpi = GetDpiForWindow(hwnd) as f32 / 96.0;
            let width = (lparam.0 as u32 & 0xffff) as f32 / dpi;
            crate::frame::set_native_tabs_end(
                hwnd,
                titlebar::tabs_end(
                    width,
                    WINDOW.with(|slot| {
                        slot.borrow()
                            .as_ref()
                            .map_or(1, |window| window.tabs.entries.len())
                    }),
                ),
            );
            WINDOW.with(|slot| {
                if let Some(window) = slot.borrow_mut().as_mut() {
                    window.title.maximized = wparam.0 as u32 == SIZE_MAXIMIZED;
                }
            });
            invalidate_all(hwnd);
        }
        WM_DPICHANGED => {
            if let Err(error) = refresh_text(hwnd) {
                fail(error);
            }
            let bounds = &*(lparam.0 as *const windows::Win32::Foundation::RECT);
            if let Err(error) = SetWindowPos(
                hwnd,
                None,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            ) {
                fail(error);
            }
            invalidate_all(hwnd);
        }
        WM_SETTINGCHANGE
            if crate::frame::theme_changed(lparam) || text::settings_changed(wparam.0) =>
        {
            if let Err(error) = refresh_text(hwnd) {
                fail(error);
            }
            if crate::frame::theme_changed(lparam) {
                if let Err(error) = crate::frame::apply_dark(hwnd, crate::shell::theme::dark()) {
                    fail(error);
                }
            }
            invalidate_all(hwnd);
            if let Err(error) = address_popup::refresh(hwnd) {
                fail(error);
            }
        }
        WM_DISPLAYCHANGE => {
            invalidate_all(hwnd);
        }
        WM_QUERYENDSESSION => {
            session::checkpoint(hwnd);
            return LRESULT(1);
        }
        WM_ENDSESSION if wparam.0 != 0 => session::ending(hwnd),
        WM_EXITSIZEMOVE => {
            session::checkpoint(hwnd);
            if let Err(error) = crate::frame::save_placement(hwnd) {
                eprintln!("native placement checkpoint: {error}");
            }
        }
        WM_CLOSE => {
            if drop::request_close() {
                return LRESULT(0);
            }
            session::finish(hwnd);
            if let Err(error) = crate::frame::save_placement(hwnd) {
                eprintln!("save placement: {error}");
            }
        }
        WM_DESTROY => {
            tab_transfer::cancel(hwnd);
            drop::close(hwnd);
            address_popup::close_owner(hwnd);
            crate::shell::share::close(hwnd);
            if let Err(error) =
                windows::Win32::System::DataExchange::RemoveClipboardFormatListener(hwnd)
            {
                eprintln!("native clipboard listener cleanup: {error}");
            }
            let window = WINDOW.with(|slot| slot.borrow_mut().take());
            if let Some(mut window) = window {
                if let Err(error) = window.list.save(window.tabs.active().location.tab.path()) {
                    fail(windows::core::Error::new(
                        windows::Win32::Foundation::E_FAIL,
                        error,
                    ));
                }
                drop(window);
            }
            if context::handles().len() <= 1 {
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            crate::frame::remove_native_window(hwnd);
            context::remove(hwnd);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn paint_window(hwnd: HWND) -> Result<()> {
    let paint_started = std::time::Instant::now();
    let window = WINDOW.with(|window| window.borrow_mut().take());
    let Some(mut window) = window else {
        return Ok(());
    };
    let mut graphics_lost = false;
    let result = (|| {
        let mut rect = windows::Win32::Foundation::RECT::default();
        unsafe {
            GetClientRect(hwnd, &mut rect)?;
        }
        if rect.right == 0 || rect.bottom == 0 {
            return Ok(());
        }
        let size = (rect.right as u32, rect.bottom as u32);
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        window
            .settings
            .sync(window.tabs.active().location.tab.path());
        let mut damage = DAMAGE.with(|damage| damage.replace(Damage::NONE));
        window.navigation.height = (size.1 as f32 * 96.0 / dpi as f32
            - details::top()
            - if window.chrome.status_visible {
                24.0
            } else {
                0.0
            })
        .max(0.0);
        window.list.navigation_width = window.navigation.left();
        window.panes.focus = if window.chrome.cues && window.title.active {
            match window.chrome.focus {
                chrome::Focus::Pane(button) => Some(button),
                _ => None,
            }
        } else {
            None
        };
        if window.panes.sync(
            &mut window.list,
            (
                size.0 as f32 * 96.0 / dpi as f32,
                size.1 as f32 * 96.0 / dpi as f32,
            ),
            window.navigation.left(),
        ) {
            damage = damage.merge(Damage {
                pane: true,
                ..Damage::NONE
            });
        }
        if let Some(folder) = &window.list.folder {
            let view = details::Viewport::new(
                (
                    size.0 as f32 * 96.0 / dpi as f32,
                    size.1 as f32 * 96.0 / dpi as f32,
                ),
                folder,
                window.navigation.left(),
            );
            window
                .list
                .prepare_grid((view.width, view.height))
                .map_err(|error| {
                    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                })?;
        }
        if let Some(folder) = window
            .list
            .folder
            .as_ref()
            .filter(|folder| !folder.awaiting_grid())
        {
            let view = details::Viewport::new(
                (
                    size.0 as f32 * 96.0 / dpi as f32,
                    size.1 as f32 * 96.0 / dpi as f32,
                ),
                folder,
                window.navigation.left(),
            );
            window.scroll.layout_folder(view, folder);
            if let Some(path) = window.reveal_path.take() {
                if let Some(row) =
                    crate::app::selection::row_of(&folder.order, &folder.entries, &path)
                {
                    window.scroll.y = crate::app::selection::reveal_offset(
                        window.scroll.y,
                        view.rows_height,
                        view.row_top(folder, row),
                        view.row_top(folder, row) + view.plate,
                    )
                    .clamp(0.0, view.maximum_y());
                }
            }
            window
                .list
                .interaction
                .rehover(view, &window.scroll, folder);
            if let Some(rows) = window.list.interaction.take_dirty() {
                damage = damage.merge(Damage {
                    rows: Some((rows.start, rows.end)),
                    ..Damage::NONE
                });
            }
            let needed = folder.grid.as_ref().map_or_else(
                || view.needed(folder, window.scroll.y),
                |model| {
                    model.range(
                        (window.scroll.x, window.scroll.y),
                        (view.right, view.rows_height),
                    )
                },
            );
            window.list.request(needed).map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
            })?;
        }
        if let Some(folder) = window.list.folder.as_deref() {
            if window.navigation.compact != folder.settings.compact {
                window.navigation.compact = folder.settings.compact;
                window.navigation.changed();
            }
            window.navigation.sync(
                window.tabs.active().location.tab.path(),
                &folder.crumbs,
                &window.navigation_worker,
            );
        }
        window.address.sync(
            &window.tabs.active().location.tab,
            window.list.folder.as_deref(),
        );
        window
            .address_worker
            .path(window.tabs.active().location.tab.path());
        let (commands, status) = window
            .chrome
            .sync(&window.list, window.tabs.active().location.tab.path());
        damage = damage.merge(Damage {
            commands,
            status,
            ..Damage::NONE
        });
        if let Err(error) = session::checkpoint_window(hwnd, &mut window) {
            eprintln!("native session paint checkpoint: {error}");
        }
        let result = draw(&mut window, hwnd, size, dpi, damage);
        if let Err(error) = result {
            if !render::device_lost(&error) {
                return Err(error);
            }
            graphics_lost = true;
            eprintln!("recreating graphics after device loss: {error}");
            address_popup::close();
            window.address.open = None;
            drop(window.graphics.take());
            context::reset_renderer();
            if let Err(error) = draw(&mut window, hwnd, size, dpi, Damage::ALL) {
                if !render::device_lost(&error) {
                    return Err(error);
                }
                eprintln!("device still unavailable; waiting for the next invalidation: {error}");
                drop(window.graphics.take());
                return Ok(());
            }
        }
        window.navigation.dirty_rows = None;
        let (title_presents, content_presents) = window.graphics.as_ref().unwrap().1.presents();
        let list_presents = window.graphics.as_ref().unwrap().1.list_presents();
        let chrome_presents = window.graphics.as_ref().unwrap().1.chrome_presents();
        unsafe {
            if let Some(folder) = &window.list.folder {
                SetPropW(
                    hwnd,
                    w!("NativeViewMode"),
                    Some(HANDLE(folder.view.mode as _)),
                )?;
                SetPropW(
                    hwnd,
                    w!("NativeIconSize"),
                    Some(HANDLE(folder.view.icon_size as _)),
                )?;
                SetPropW(
                    hwnd,
                    w!("NativeGridMeasured"),
                    Some(HANDLE(
                        folder.grid.as_ref().map_or(0, |grid| grid.measured) as _
                    )),
                )?;
            }
            SetPropW(
                hwnd,
                w!("NativeListPresents"),
                Some(HANDLE(list_presents as _)),
            )?;
            SetPropW(
                hwnd,
                w!("NativeListRows"),
                Some(HANDLE(window.list.rows.len() as _)),
            )?;
            SetPropW(
                hwnd,
                w!("NativeItemCount"),
                Some(HANDLE(
                    window.list.folder.as_ref().map_or(0, |f| f.order.len()) as _,
                )),
            )?;
            if !window.first_list && list_presents > 0 && !window.list.rows.is_empty() {
                if memory::enabled() {
                    let mut seen = std::collections::HashSet::new();
                    let list_images = window
                        .list
                        .rows
                        .values()
                        .filter_map(|row| row.image.as_ref())
                        .filter(|image| seen.insert(std::sync::Arc::as_ptr(image) as usize))
                        .map(|image| image.pixels.capacity())
                        .sum::<usize>();
                    let navigation_images = window
                        .navigation
                        .images
                        .values()
                        .filter(|image| seen.insert(std::sync::Arc::as_ptr(image) as usize))
                        .map(|image| image.pixels.capacity())
                        .sum::<usize>();
                    eprintln!("native-retained|folder={}|grid={}|list-images={list_images}|additional-navigation-images={navigation_images}|rows={}|tabs={}|background-tabs={}|text={:?}", window.list.folder.as_ref().map_or(0, |folder| tabs::weight(folder)), window.list.folder.as_ref().and_then(|folder| folder.grid.as_ref()).map_or(0, |grid| grid.retained_bytes()), window.list.rows.len(), window.tabs.entries.len(), tabs::cached_bytes(&window.tabs), window.text.cache_counts());
                }
                let milliseconds = window.started.elapsed().as_millis().max(1) as usize;
                SetPropW(
                    hwnd,
                    w!("NativeFirstListMs"),
                    Some(HANDLE(milliseconds as _)),
                )?;
                eprintln!("native first list: {milliseconds}ms");
                window.first_list = true;
            }
            SetPropW(
                hwnd,
                w!("NativeTitlePresents"),
                Some(HANDLE(title_presents as _)),
            )?;
            SetPropW(
                hwnd,
                w!("NativeContentPresents"),
                Some(HANDLE(content_presents as _)),
            )?;
            SetPropW(hwnd, w!("NativeBrushCreations"), Some(HANDLE(1 as _)))?;
            SetPropW(
                hwnd,
                w!("NativeTextLayoutMisses"),
                Some(HANDLE(window.text.layout_misses as _)),
            )?;
            SetPropW(
                hwnd,
                w!("NativeTextFormatMisses"),
                Some(HANDLE(window.text.format_misses as _)),
            )?;
            SetPropW(
                hwnd,
                w!("NativePresentCount"),
                Some(HANDLE(
                    (title_presents
                        + content_presents
                        + list_presents
                        + chrome_presents.0
                        + chrome_presents.1) as _,
                )),
            )
        }
    })();
    if result.is_ok() {
        let elapsed = paint_started.elapsed().as_micros().min(usize::MAX as u128) as usize;
        window.paint_max_us = window.paint_max_us.max(elapsed);
        let work = window
            .graphics
            .as_ref()
            .map_or((0, 0), |graphics| graphics.1.list_work());
        let chrome_presents = window
            .graphics
            .as_ref()
            .map_or((0, 0), |graphics| graphics.1.chrome_presents());
        let navigation_work = window
            .graphics
            .as_ref()
            .map_or((0, 0), |graphics| graphics.1.navigation_work());
        chrome::publish_focus(hwnd, window.chrome.focus);
        for (name, value) in [
            (
                w!("NativeRenderer"),
                window
                    .graphics
                    .as_ref()
                    .map_or(0, |graphics| Rc::as_ptr(&graphics.0) as usize),
            ),
            (w!("NativeNavPresents"), navigation_work.0),
            (w!("NativeNavRowsDrawn"), navigation_work.1),
            (w!("NativeCommandPresents"), chrome_presents.0),
            (w!("NativeStatusPresents"), chrome_presents.1),
            (
                w!("NativeCanPaste"),
                window.chrome.enabled[chrome::Command::Paste as usize] as usize + 1,
            ),
            (w!("NativePaintLastUs"), elapsed),
            (w!("NativePaintMaxUs"), window.paint_max_us),
            (w!("NativeRowsDrawn"), work.0),
            (w!("NativePixelsDrawn"), work.1),
            (w!("NativeScrollY"), window.scroll.y as usize),
            (
                w!("NativeScrollMaxY"),
                window.scroll.view.map_or(0.0, |view| view.maximum_y()) as usize,
            ),
            (w!("NativeScrollX"), window.scroll.x as usize),
            (
                w!("NativeSelectionCount"),
                window.list.interaction.selection.selected.len(),
            ),
            (
                w!("NativeFocusedRow"),
                window
                    .list
                    .folder
                    .as_ref()
                    .and_then(|f| window.list.interaction.focus_row(f))
                    .map_or(0, |r| r + 1),
            ),
            (
                w!("NativeHoverRow"),
                window.list.interaction.hover.map_or(0, |r| r + 1),
            ),
            (w!("NativeInputTimers"), window.input_timers.len()),
            (w!("NativeEnumerations"), window.list.enumerations),
            (w!("NativeEntryReads"), window.list.entry_reads),
            (w!("NativeNavRows"), window.navigation.tree.rows.len()),
            (w!("NativeNavTop"), window.navigation.top),
            (
                w!("NativeNavSelected"),
                window.navigation.selected.map_or(0, |row| row + 1),
            ),
            (w!("NativeNavWidth"), window.navigation.width as usize),
            (w!("NativeTabCount"), window.tabs.entries.len()),
            (w!("NativeCurrentTab"), window.tabs.current),
            (w!("NativeTabCacheBytes"), tabs::cached_bytes(&window.tabs)),
            (
                w!("NativeNavOpacity"),
                (window.navigation.opacity * 1000.0).round() as usize,
            ),
            (
                w!("NativeNavFade"),
                usize::from(window.navigation.fade.is_some()),
            ),
        ] {
            if let Err(error) = unsafe { SetPropW(hwnd, name, Some(HANDLE(value as _))) } {
                fail(error);
            }
        }
        static PAINT_PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *PAINT_PROBE.get_or_init(|| std::env::var("SW_SCROLL_PROBE").as_deref() == Ok("1")) {
            eprintln!(
                "native-paint|{elapsed}|{}|{}|{}|{}",
                work.0, work.1, window.scroll.y, window.scroll.x
            );
        }
    }
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if graphics_lost {
        filter::close(hwnd);
        menu::close_owner(hwnd);
        tooltip::dismiss(hwnd);
    }
    if NAV_PENDING.with(|pending| pending.replace(false)) {
        unsafe {
            PostMessageW(Some(hwnd), navigation::READY, WPARAM(0), LPARAM(0))?;
        }
    }
    for (pending, message) in [
        (
            context::ADDRESS_PENDING.with(|pending| pending.replace(false)),
            address::READY,
        ),
        (
            context::PREFERENCES_PENDING.with(|pending| pending.replace(false)),
            preferences::CHANGED,
        ),
    ] {
        if pending {
            unsafe {
                PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0))?;
            }
        }
    }
    if result.is_ok() {
        launch::after_paint(hwnd);
        location::sync(hwnd);
        hosted::sync(hwnd);
        panes::after_paint(hwnd);
        settings::after_paint(hwnd);
        edit::place(hwnd).map_err(|error| {
            windows::core::Error::new(error.code(), format!("edit placement: {error}"))
        })?;
        input::refresh_hover(hwnd);
        tooltip::refresh(hwnd);
    }
    if result.is_ok() && LIST_PENDING.with(|pending| pending.replace(false)) {
        unsafe {
            PostMessageW(Some(hwnd), list::READY, WPARAM(0), LPARAM(0))?;
        }
    }
    result
}

fn draw(
    window: &mut Window,
    hwnd: HWND,
    size: (u32, u32),
    dpi: u32,
    mut damage: Damage,
) -> Result<()> {
    let renderer = context::renderer()?;
    if window
        .graphics
        .as_ref()
        .is_none_or(|graphics| !Rc::ptr_eq(&graphics.0, &renderer))
    {
        let surface = renderer.surface(hwnd, &window.title.icon)?;
        damage = Damage::ALL;
        window.graphics = Some((renderer, surface));
    }
    let (renderer, surface) = window.graphics.as_mut().unwrap();
    renderer.paint(
        surface,
        size,
        &window.theme,
        dpi,
        &mut window.text,
        &window.list,
        &mut window.title,
        &mut window.chrome,
        &mut window.address,
        &window.navigation,
        &window.panes,
        &mut window.settings,
        damage,
        &window.scroll,
    )
}

fn refresh_text(hwnd: HWND) -> Result<()> {
    let window = WINDOW.with(|slot| slot.borrow_mut().take());
    let Some(mut window) = window else {
        return Ok(());
    };
    let theme = Theme::system();
    if window.theme.content.r != theme.content.r {
        window.list.cut.clear();
    }
    window.theme = theme;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let result = window
        .text
        .refresh(dpi)
        .and_then(|()| window.editors.refresh(dpi));
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    result
}

fn release_capture(hwnd: HWND) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture};
    if unsafe { GetCapture() } != hwnd {
        return Ok(());
    }
    let released = unsafe { ReleaseCapture() };
    if unsafe { GetCapture() } != hwnd {
        return Ok(());
    }
    released.and(Err(windows::core::Error::new(
        windows::Win32::Foundation::E_FAIL,
        "Native pointer capture remained active",
    )))
}

const SCROLL_TIMER: usize = 6;

fn client_point(hwnd: HWND, value: LPARAM) -> (f32, f32) {
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    (
        value.0 as i16 as f32 / scale,
        (value.0 >> 16) as i16 as f32 / scale,
    )
}

fn move_scroll(hwnd: HWND, point: (f32, f32)) {
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|w| w.scroll.mouse_move(point))
    });
    if changed {
        invalidate_view(hwnd);
    }
}

fn leave_scroll(hwnd: HWND) {
    let changed = WINDOW.with(|slot| slot.borrow_mut().as_mut().is_some_and(|w| w.scroll.leave()));
    if changed {
        invalidate_view(hwnd);
    }
}

fn press_scroll(hwnd: HWND, point: (f32, f32)) -> Result<bool> {
    let (pressed, repeat) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((false, false), |w| {
            let pressed = w.scroll.press(point);
            (
                pressed,
                pressed
                    && w.scroll
                        .pressed
                        .is_some_and(|hit| hit.part != scroll::Part::Thumb),
            )
        })
    });
    if !pressed {
        return Ok(false);
    }
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
        if repeat && SetTimer(Some(hwnd), SCROLL_TIMER, 400, None) == 0 {
            return Err(windows::core::Error::from_win32());
        }
    }
    invalidate_view(hwnd);
    Ok(true)
}

fn release_scroll(hwnd: HWND) -> bool {
    let (pressed, repeat) = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().map_or((false, false), |w| {
            let repeat = w
                .scroll
                .pressed
                .is_some_and(|hit| hit.part != scroll::Part::Thumb);
            (w.scroll.release(), repeat)
        })
    });
    if repeat {
        if let Err(error) = unsafe { KillTimer(Some(hwnd), SCROLL_TIMER) } {
            fail(error);
        }
    }
    if pressed {
        invalidate_view(hwnd);
    }
    pressed
}

fn invalidate_view(hwnd: HWND) {
    DAMAGE.with(|damage| damage.set(damage.get().merge(Damage::VIEW)));
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn invalidate_header(hwnd: HWND) {
    DAMAGE.with(|damage| {
        damage.set(damage.get().merge(Damage {
            header: true,
            ..Damage::NONE
        }))
    });
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

enum Navigation {
    Open(String),
    History(usize),
    Back,
    Forward,
    Up,
}

fn navigate(hwnd: HWND, navigation: Navigation) -> Result<()> {
    filter::close(hwnd);
    search::leaving(hwnd);
    tooltip::suppress(hwnd);
    rename::blur();
    address_input::blur(hwnd);
    header::cancel(hwnd);
    input::cancel(hwnd);
    let window = WINDOW.with(|slot| slot.borrow_mut().take());
    let Some(mut window) = window else {
        return Ok(());
    };
    let result = (|| {
        let location = &mut window.tabs.active_mut().location;
        let previous_path = location.tab.path().to_owned();
        let current = location.tab.index;
        let cues = window.list.interaction.cues;
        window.reveal_path = None;
        location.visits[current] = window.list.interaction.selection.clone();
        let target = match navigation {
            Navigation::Open(path) => Some(path),
            Navigation::Up => {
                let parent = crate::app::ids::parent_of(location.tab.path());
                let path = location
                    .tab
                    .library()
                    .filter(|context| !context.contains(&parent))
                    .map_or(parent, |context| context.library);
                (!crate::app::ids::same(&path, location.tab.path())).then_some(path)
            }
            Navigation::Back if current > 0 => {
                location.tab.index -= 1;
                None
            }
            Navigation::History(index)
                if index < location.tab.history.len() && index != current =>
            {
                location.tab.index = index;
                None
            }
            Navigation::Forward if current + 1 < location.tab.history.len() => {
                location.tab.index += 1;
                None
            }
            _ => return Ok(()),
        };
        if let Some(path) = target {
            if crate::app::ids::same(&path, location.tab.path()) {
                return Ok(());
            }
            let old_path = location.tab.path().to_string();
            let parent_target =
                crate::app::ids::same(&crate::app::ids::parent_of(&old_path), &path);
            location.tab.push(path, None);
            location.visits.truncate(location.tab.index);
            let selection = if parent_target {
                crate::app::selection::Selection {
                    selected: std::collections::HashSet::from([old_path.clone()]),
                    anchor: Some(old_path.clone()),
                    focused: Some(old_path),
                }
            } else {
                Default::default()
            };
            location.visits.push(selection);
        }
        let visit = location.visits[location.tab.index].clone();
        window.reveal_path = visit.focused.clone();
        if window.list.dirty {
            if let Some(folder) = &window.list.folder {
                location
                    .views
                    .insert(previous_path.clone(), folder.view.clone());
            }
        }
        window.list.save(&previous_path).map_err(|error| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
        })?;
        let list = list::List::search_start(
            &window.worker,
            hwnd,
            location.tab.path().to_string(),
            location.views.get(location.tab.path()).cloned(),
            window.search.options,
        )
        .map_err(|error| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
        })?;
        window.list = list;
        window
            .list
            .set_filter(location.tab.filter())
            .map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
            })?;
        let tree_focus = window.chrome.focus == chrome::Focus::Tree;
        window.chrome.focus = if tree_focus {
            chrome::Focus::Tree
        } else {
            chrome::Focus::List
        };
        window.title.focus = titlebar::Target::None;
        window.list.interaction.has_focus = !tree_focus;
        window.list.interaction.cues = cues;
        window.list.interaction.active = window.title.active;
        window.scroll = Default::default();
        window.pending_scroll = None;
        window.pending_visit = Some(visit);
        window.started = std::time::Instant::now();
        window.first_list = false;
        Ok(())
    })();
    launch::routes(hwnd, &window.tabs);
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    if result.is_ok() {
        invalidate_all(hwnd);
    }
    result
}

fn loaded(hwnd: HWND) -> Result<()> {
    tooltip::dismiss(hwnd);
    let window = WINDOW.with(|slot| slot.borrow_mut().take());
    let Some(mut window) = window else {
        return Ok(());
    };
    let result = (|| {
        if let Some(context) = window.tabs.active().location.tab.library() {
            if let Some(crumbs) = context.crumbs(window.tabs.active().location.tab.path()) {
                if let Some(folder) = window.list.folder.as_mut() {
                    Arc::make_mut(folder).crumbs = Arc::new(crumbs);
                }
            }
        }
        let Some(folder) = &window.list.folder else {
            return Ok(());
        };
        window
            .title
            .relabel(folder.title.clone(), folder.icon.clone());
        window.tabs.active_mut().name = folder.crumbs.last().map_or_else(
            || window.tabs.active().name.clone(),
            |crumb| crumb.name.clone(),
        );
        window.tabs.active_mut().icon = Some(folder.icon.clone());
        window.tabs.active_mut().location.tab.title = folder.title.clone();
        window.tabs.active_mut().location.tab.icon =
            folder.crumbs.last().map_or(0, |crumb| crumb.icon);
        {
            let mut selection = window
                .pending_visit
                .take()
                .unwrap_or_else(|| window.list.interaction.selection.clone());
            let paths: std::collections::HashSet<_> =
                folder.entries.iter().map(|e| e.path.as_str()).collect();
            if !window.list.search.busy() {
                selection
                    .selected
                    .retain(|path| paths.contains(path.as_str()));
            }
            let unfocused = selection.focused.is_none();
            if !window.list.search.busy()
                && selection
                    .focused
                    .as_ref()
                    .is_none_or(|path| !paths.contains(path.as_str()))
            {
                selection.focused = folder
                    .order
                    .first()
                    .map(|&i| folder.entries[i].path.clone());
            }
            window.list.interaction.selection = selection;
            if unfocused && window.list.interaction.selection.selected.is_empty() {
                window.list.interaction.group_focus = folder
                    .layout
                    .headers
                    .first()
                    .map(|header| header.label.clone());
            }
        }
        if let Some((x, y)) = window.pending_scroll.take() {
            window.scroll.x = x;
            window.scroll.y = y;
            window.reveal_path = None;
        }
        tabs::sync(&mut window, hwnd)?;
        Ok(())
    })();
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    result
}

fn fail(error: windows::core::Error) {
    eprintln!("native UI: {error}");
    eprintln!("{}", std::backtrace::Backtrace::capture());
    FAILURE.with(|failure| *failure.borrow_mut() = Some(error));
    unsafe {
        PostQuitMessage(1);
    }
}

fn invalidate_title(hwnd: HWND) {
    DAMAGE.with(|damage| damage.set(damage.get().merge(Damage::TITLE)));
    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe {
        if let Err(error) = GetClientRect(hwnd, &mut rect) {
            fail(error);
            return;
        }
        rect.bottom = (titlebar::HEIGHT * GetDpiForWindow(hwnd) as f32 / 96.0).round() as i32;
        let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
    }
}

fn invalidate_rows(hwnd: HWND, rows: std::ops::Range<usize>) {
    let view = WINDOW.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref().and_then(|window| window.list.folder.clone())
    });
    let Some(folder) = view else {
        return;
    };
    let mut bounds = windows::Win32::Foundation::RECT::default();
    unsafe {
        if let Err(error) = GetClientRect(hwnd, &mut bounds) {
            fail(error);
            return;
        }
        let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        let view = details::Viewport::new(
            (bounds.right as f32 / scale, bounds.bottom as f32 / scale),
            &folder,
            WINDOW.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map_or(202.0, |window| window.navigation.left())
            }),
        );
        let y = WINDOW.with(|slot| slot.borrow().as_ref().map_or(0.0, |w| w.scroll.y));
        if let Some(model) = &folder.grid {
            let x =
                WINDOW.with(|slot| slot.borrow().as_ref().map_or(0.0, |window| window.scroll.x));
            let Some([left, top, right, bottom]) =
                model.damage(rows.clone(), (x, y), (view.right, view.rows_height))
            else {
                return;
            };
            DAMAGE.with(|damage| {
                damage.set(damage.get().merge(Damage {
                    rows: Some((rows.start, rows.end)),
                    ..Damage::VIEW
                }))
            });
            let rect = windows::Win32::Foundation::RECT {
                left: ((view.x + left - x) * scale).floor() as i32,
                top: ((view.y + view.header + top - y) * scale).floor() as i32,
                right: ((view.x + right - x) * scale).ceil() as i32,
                bottom: ((view.y + view.header + bottom - y) * scale).ceil() as i32,
            };
            let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
            return;
        }
        if !folder.layout.headers.is_empty() && rows.start == 0 && rows.end >= folder.order.len() {
            DAMAGE.with(|damage| {
                damage.set(damage.get().merge(Damage {
                    rows: Some((0, usize::MAX)),
                    ..Damage::VIEW
                }))
            });
            let rect = windows::Win32::Foundation::RECT {
                left: (view.x * scale) as i32,
                top: ((view.y + 25.0) * scale) as i32,
                right: ((view.x + view.width) * scale) as i32,
                bottom: ((view.y + view.bottom) * scale) as i32,
            };
            let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
            return;
        }
        let visible = view.visible(&folder, y);
        if rows.start >= visible.end || rows.end <= visible.start {
            return;
        }
        let rows = rows.start.max(visible.start)..rows.end.min(visible.end);
        let rect = windows::Win32::Foundation::RECT {
            left: (view.x * scale).round() as i32,
            top: ((view.y + 25.0 + view.row_top(&folder, rows.start) - y) * scale).floor() as i32,
            right: ((view.x + view.width) * scale).ceil() as i32,
            bottom: ((view.y + 25.0 + view.row_top(&folder, rows.end) + 1.0 - y)
                .min(view.y + view.bottom)
                * scale)
                .ceil() as i32,
        };
        DAMAGE.with(|damage| {
            damage.set(damage.get().merge(Damage {
                rows: Some((rows.start, rows.end)),
                ..Damage::VIEW
            }))
        });
        let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
    }
}

fn set_hover(hwnd: HWND, target: Target) {
    let changed = WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            let changed = window.title.hover != target;
            window.title.hover = target;
            changed
        } else {
            false
        }
    });
    if changed {
        invalidate_title(hwnd);
    }
}

fn target_at(hwnd: HWND, lparam: LPARAM) -> Result<Target> {
    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect)?;
    }
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let x = (lparam.0 & 0xffff) as i16 as f32 / scale;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32 / scale;
    Ok(WINDOW.with(|slot| {
        slot.borrow().as_ref().map_or(Target::None, |window| {
            window.title.hit(rect.right as f32 / scale, x, y)
        })
    }))
}

fn press_title(hwnd: HWND, target: Target) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.title.hover = target;
            window.title.pressed = target;
        }
    });
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
    }
    invalidate_title(hwnd);
}

fn release_title(hwnd: HWND, pressed: Target) -> Result<()> {
    let hovered = WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.title.pressed = Target::None;
            window.title.hover
        } else {
            Target::None
        }
    });
    release_capture(hwnd)?;
    invalidate_title(hwnd);
    if hovered != pressed {
        return Ok(());
    }
    let command = match pressed {
        Target::TabClose(index) => return tabs::close(hwnd, index),
        Target::Tab(index) => return tabs::select(hwnd, index),
        Target::Plus => return tabs::open(hwnd, &crate::app::ids::start_location()),
        Target::Caption(crate::frame::Button::Close) => SC_CLOSE,
        Target::Caption(crate::frame::Button::Min) => SC_MINIMIZE,
        Target::Caption(crate::frame::Button::Max) => {
            if unsafe { IsZoomed(hwnd).as_bool() } {
                SC_RESTORE
            } else {
                SC_MAXIMIZE
            }
        }
        _ => return Ok(()),
    };
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_SYSCOMMAND,
            WPARAM(command as usize),
            LPARAM(0),
        )
    }
}

fn invalidate_all(hwnd: HWND) {
    DAMAGE.with(|damage| damage.set(Damage::ALL));
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn invalidate(hwnd: HWND, dirty: Damage) {
    DAMAGE.with(|damage| damage.set(damage.get().merge(dirty)));
    if !unsafe { InvalidateRect(Some(hwnd), None, false) }.as_bool() {
        fail(windows::core::Error::from_win32());
    }
}
