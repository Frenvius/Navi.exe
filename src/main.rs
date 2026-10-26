#![windows_subsystem = "windows"]

mod app;
mod dnd;
mod frame;
mod i18n;
mod shell;
mod watch;

use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use windows::Win32::Foundation::HWND;

slint::include_modules!();

fn hwnd_of(window: &slint::Window) -> HWND {
    match window.window_handle().window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get() as _),
        _ => HWND::default(),
    }
}

struct Host(std::rc::Weak<app::App>);

impl Host {
    fn shell(&self, f: impl FnOnce(&Shell)) {
        if let Some(app) = self.0.upgrade() {
            let ui = app.ui();
            f(&ui.global::<Shell>());
        }
    }
}

impl frame::Host for Host {
    fn theme_changed(&self) {
        let _ = slint::invoke_from_event_loop(|| app::with_app(|a| a.apply_theme()));
    }
    fn settings_changed(&self) {
        // The listing reads them again.
        let _ = slint::invoke_from_event_loop(|| app::with_app(|a| {a.reload();a.refresh_nav();}));
    }
    fn hover(&self, button: frame::Button, pressed: bool) {
        self.shell(|s| {
            s.set_caption_hover(button.index());
            s.set_caption_pressed(pressed);
        });
    }
    fn maximized_changed(&self, maximized: bool) {
        self.shell(|s| s.set_maximized(maximized));
    }
    fn active_changed(&self, active: bool) {
        self.shell(|s| s.set_active(active));
        if active {
            // Clipboard and pins may have changed while we were in the background.
            let _ = slint::invoke_from_event_loop(|| {
                app::with_app(|a| {
                    a.state.borrow_mut().can_paste = shell::ops::can_paste();
                    a.sync_view_state();
                    a.refresh_nav();
                });
            });
        }
    }
    fn x_button(&self, forward: bool) {
        let _ = slint::invoke_from_event_loop(move || {
            app::with_app(|a| if forward { a.forward() } else { a.back() });
        });
    }
    fn wheel(&self, x: f32, y: f32, delta: i32, ctrl: bool) -> bool {
        let Some(app) = self.0.upgrade() else { return false };
        let ui = app.ui();
        if ui.global::<Shell>().get_settings_view() || ui.global::<Shell>().get_native_shell_view(){return false;}
        let (left, top) = (ui.get_view_left(), ui.get_view_top());
        // Over the navigation pane the tree scrolls whole rows.
        let pane = ui.global::<Shell>().get_sidebar_width();
        if !ctrl && pane > 0.0 && x >= 2.0 && x < 2.0 + pane && y >= top && y < top + ui.get_view_height() {
            let steps = wheel_steps(delta);
            if steps != 0 {
                let _ = slint::invoke_from_event_loop(move || app::with_app(|a| a.nav_wheel(steps)));
            }
            return true;
        }
        if x < left || y < top || x >= left + ui.get_view_width() || y >= top + ui.get_view_height() {
            return false;
        }
        if app.current_path()==app::ids::GALLERY {
            let steps=wheel_steps(delta);
            let _=slint::invoke_from_event_loop(move||app::with_app(|a|if ctrl {a.gallery_zoom(steps)}else{a.gallery_scroll(-(steps as f32)*48.0)}));
            return true;
        }
        if !ctrl && !app.wheel_across() {
            // The details list scrolls by rows, as Explorer's does.
            if !app.wheel_rows() {
                return false;
            }
            let steps = wheel_steps(delta);
            if steps != 0 {
                let _ = slint::invoke_from_event_loop(move || app::with_app(|a| a.wheel_list(steps)));
            }
            return true;
        }
        let steps = wheel_steps(delta);
        if steps != 0 {
            let _ = slint::invoke_from_event_loop(move || app::with_app(|a| if ctrl { a.zoom(steps) } else { a.scroll_columns(steps) }));
        }
        true
    }
}

/// Whole wheel notches; a touchpad's small deltas add up first.
fn wheel_steps(delta: i32) -> i32 {
    thread_local!(static WHEEL: std::cell::Cell<i32> = const { std::cell::Cell::new(0) });
    let total = WHEEL.with(|w| w.get()) + delta;
    let steps = total / 120;
    WHEEL.with(|w| w.set(total - steps * 120));
    steps
}

fn main() -> Result<(), slint::PlatformError> {
    shell::jumplist::identity();
    shell::com_init();
    dnd::init_ole();
    match app::entry::start(){Ok(true)=>return Ok(()),Ok(false)=>{},Err(error)=>return Err(slint::PlatformError::Other(format!("{error}\n{}",app::entry::USAGE)))}
    // Only the software surface hands DWM per-pixel alpha, which Mica needs. Drops are
    // handled by our own shell drop target, so winit must not register its file-drop one.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia-software".into())
        .with_winit_window_attributes_hook(|attrs| {
            use slint::winit_030::winit::platform::windows::{WindowAttributesExtWindows,IconExtWindows};
            let attrs = attrs.with_drag_and_drop(false);
            // A menu is born owned and off the taskbar, and shows without activating (so
            // that its warm-up showing leaves the main window active; an open menu is then
            // activated by hand): restyling it once shown left a moment where it was a
            // second app window (taskbar group, open animation).
            match frame::popup_owner() {
                Some(owner) => attrs.with_owner_window(owner.0 as isize).with_skip_taskbar(true).with_active(false),
                None => {
                    use slint::winit_030::winit::{window::Icon,dpi::PhysicalSize};
                    // Keep these in winit's own state, from the first HWND frame;
                    // later style/visibility updates can reapply its cached icons.
                    let attrs=attrs.with_window_icon(Icon::from_resource(1,Some(PhysicalSize::new(16,16))).ok())
                        .with_taskbar_icon(Icon::from_resource(1,Some(PhysicalSize::new(32,32))).ok());
                    let position=std::env::var("SLINT_TAB_WINDOW").ok().and_then(|s|{
                        let mut fields=s.split(',');Some((fields.next()?.parse::<i32>().ok()?,fields.next()?.parse::<i32>().ok()?))
                    });
                    if let Some((x,y))=position{attrs.with_position(slint::winit_030::winit::dpi::PhysicalPosition::new(x,y))}else{attrs}
                },
            }
        })
        .select()?;

    let ui = AppWindow::new()?;
    i18n::select_ui_language();
    let _app = app::App::new(&ui);
    ui.set_font_test(std::env::var("FONT_TEST").is_ok());
    ui.set_menu_chrome_probe(std::env::var_os("MENU_CHROME_PROBE").is_some());
    ui.show()?;

    // The native window only exists once the event loop has started.
    let _ = slint::invoke_from_event_loop(|| app::with_app(|app| {
        let hwnd = hwnd_of(app.ui().window());
        app.hwnd.set(hwnd);
        frame::install(hwnd, Box::new(Host(Rc::downgrade(app))));
        if let Err(e)=shell::jumplist::window(hwnd){eprintln!("taskbar identity: {}",e.message());}
        app::tab_transfer::install(hwnd);
        app::tab_transfer::initial_position(hwnd);
        frame::set_popup_dismiss(Box::new(|| app::with_app(|a| app::dismiss_menu(a))));
        app.warm_up_popups();
        dnd::register(hwnd, Box::new(app::drop::DropResolver));
        app::session::start(app);
        app::entry::ready(app);
        app.open_current();
        app.refresh_nav();
        app.load_new_items();
    }));
    slint::run_event_loop()?;
    // Closing leaves the folder too.
    app::with_app(|app| {shell::jumplist::close(app.hwnd.get());app.close_native_view();app.save_view();app::session::finish(app);});
    Ok(())
}
