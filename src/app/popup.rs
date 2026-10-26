//! Menus as popup windows (acrylic, may extend past the main window), one per level.

use std::rc::Rc;

use slint::{ComponentHandle, Model, PhysicalPosition};
use windows::Win32::Foundation::HWND;

use super::{menus, App};
use crate::{frame, hwnd_of, MenuPopup, PopupData};

/// Text-free native chrome recovered over black/white surfaces. Nine-grid stretching
/// preserves the 8px corner coverage, 1px border and compact asymmetric shadow.
fn menu_chrome(width: i32, height: i32, dark: bool, context: bool) -> slint::Image {
    slint::Image::from_rgba8(menu_pixels(width, height, dark, context))
}

fn menu_pixels(width: i32, height: i32, dark: bool, context: bool) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let atlas: &[u8] = match (dark, context) {
        (true, false) => include_bytes!("../../ui/assets/menu-chrome-dark-view.rgba"),
        (true, true) => include_bytes!("../../ui/assets/menu-chrome-dark-context.rgba"),
        (false, false) => include_bytes!("../../ui/assets/menu-chrome-light-view.rgba"),
        (false, true) => include_bytes!("../../ui/assets/menu-chrome-light-context.rgba"),
    };
    let (w, h) = (width.max(45) as usize, height.max(45) as usize);
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w as u32, h as u32);
    let dest = pixels.make_mut_bytes();
    for y in 0..h {
        let sy = if y < 14 { y } else if y >= h - 30 { 45 - (h - y) } else { 14 };
        for x in 0..w {
            let sx = if x < 22 { x } else if x >= w - 22 { 45 - (w - x) } else { 22 };
            let source = (sy * 45 + sx) * 4;
            let target = (y * w + x) * 4;
            dest[target..target + 4].copy_from_slice(&atlas[source..source + 4]);
        }
    }
    pixels
}

fn row_y(data: &PopupData, index: usize, height: f32) -> f32 {
    let rows_height: f32 = (0..data.items.row_count()).map(|i| if data.items.row_data(i).unwrap().separator { data.separator_height } else { data.row_height }).sum();
    let bar = if data.buttons.row_count() > 0 && !data.buttons_bottom { height - rows_height - 2.0 * data.padding - data.extra_top } else { 0.0 };
    data.padding + data.extra_top + bar + (0..index).map(|i| if data.items.row_data(i).unwrap().separator { data.separator_height } else { data.row_height }).sum::<f32>()
}

fn style_key_tips(popup: slint::Weak<crate::MenuKeyTips>, attempt: u32) {
    let Some(p) = popup.upgrade() else { return };
    let hwnd = hwnd_of(p.window());
    if hwnd.is_invalid() {
        if attempt < 50 { slint::Timer::single_shot(std::time::Duration::from_millis(1), move || style_key_tips(popup, attempt + 1)); }
    } else { frame::style_drop_preview(hwnd); }
}

#[derive(Default)]
pub struct Popups {
    key_tips: Option<crate::MenuKeyTips>,
    key_prefix: String,
    main: Option<MenuPopup>,
    sub: Option<MenuPopup>,
    styled: Vec<HWND>,
    /// Screen position and size of the main menu (physical pixels).
    main_rect: (i32, i32, i32, i32),
    /// The hovered item's top within the main menu (logical), for its submenu.
    sub_anchor: f32,
    /// Frame color (0x00BBGGRR): dark for dropdowns, light for context menus.
    border: u32,
    /// Levels kept cloaked until they paint their new contents.
    cloaked: [bool; 2],
    /// Levels shown off screen at startup only to create their windows, hidden once styled.
    warming: [bool; 2],
}

fn make(app: &Rc<App>, level: i32) -> MenuPopup {
    let popup = frame::creating_popup(app.hwnd.get(), || MenuPopup::new().expect("menu window"));
    popup.set_level(level);
    let weak=Rc::downgrade(app);
    popup.on_cloud_command(move|i|{if let Some(a)=weak.upgrade(){a.cloud_command(i);}});
    popup.on_chrome(menu_chrome);
    copy_theme(&app.ui(), &popup);
    let weak = Rc::downgrade(app);
    popup.on_activated(move |i| {
        if let Some(a) = weak.upgrade() {
            menus::activate(&a, level, i as usize);
        }
    });
    let weak = Rc::downgrade(app);
    popup.on_button(move |i| {
        if let Some(a) = weak.upgrade() {
            menus::activate_button(&a, i as usize);
        }
    });
    let weak = Rc::downgrade(app);
    popup.on_hover(move |i, y| {
        if let Some(a) = weak.upgrade() {
            menus::hover_tip(&a, level, i);
            if level == 0 {
                let anchor = if y == 0.0 {
                    let p = a.popups.borrow().main.as_ref().unwrap().clone_strong();
                    row_y(&p.get_data(), i.max(0) as usize, p.get_menu_height())
                } else { y - 2.0 };
                a.popups.borrow_mut().sub_anchor = anchor;
                menus::hover(&a, 0, i);
            }
        }
    });
    let weak = Rc::downgrade(app);
    popup.on_dismissed(move || {
        if let Some(a) = weak.upgrade() {
            if level == 1 {
                a.close_sub_popup();
            } else {
                menus::dismiss(&a);
            }
        }
    });
    let weak = Rc::downgrade(app);
    popup.on_key(move |e| weak.upgrade().is_some_and(|a| a.menu_access_key(level, &e.text)));
    let weak = Rc::downgrade(app);
    popup.on_key_tips(move |show| { if let Some(a) = weak.upgrade() { a.menu_key_tips(level, show); } });
    let weak = Rc::downgrade(app);
    popup.on_focus_button(move |index| {
        let Some(a) = weak.upgrade() else { return };
        let weak = Rc::downgrade(&a);
        a.tip_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(1000), move || {
            let Some(a) = weak.upgrade() else { return };
            let Some(p) = a.popups.borrow().main.as_ref().map(|p| p.clone_strong()) else { return };
            if !p.window().is_visible() || !p.get_keyboard() { return; }
            let Some(button) = p.get_data().buttons.row_data(index.max(0) as usize) else { return };
            let shortcut = match button.key_tip.as_str() {
                "O" | "T" => "Ctrl+X", "C" => "Ctrl+C", "R1" | "M" => "F2", "X" => "Delete", _ => "",
            };
            let text = if shortcut.is_empty() { button.label.to_string() } else { format!("{} ({shortcut})", button.label) };
            a.show_tip(&text);
            if let Some(tip) = a.tip_popup.borrow().as_ref() {
                let scale = p.window().scale_factor();
                let (x, y, _, _) = a.popups.borrow().main_rect;
                let center = x + ((36.0 + index as f32 * 65.0) * scale).round() as i32;
                tip.window().set_position(PhysicalPosition::new(center - (tip.get_tip_width() * scale).round() as i32 / 2,
                    y - ((12.0 + tip.get_tip_height()) * scale).round() as i32));
            };
        });
    });
    // Rendered a frame: it's presented by the time the event loop runs timers.
    let _ = popup.window().set_rendering_notifier(move |state, _| {
        if matches!(state, slint::RenderingState::AfterRendering) {
            slint::Timer::single_shot(std::time::Duration::ZERO, move || super::with_app(|a| a.uncloak_popup(level)));
        }
    });
    popup
}

/// A window's globals are its own: menus take the main window's theme.
fn copy_theme(ui: &crate::AppWindow, popup: &MenuPopup) {
    let (from, to) = (ui.global::<crate::Theme>(), popup.global::<crate::Theme>());
    to.set_dark(from.get_dark());
    to.set_accent(from.get_accent());
}

impl Popups {
    pub fn is_open(&self) -> bool {
        self.main.as_ref().map(|p| p.window().is_visible()).unwrap_or(false)
    }
}

impl App {
    pub(super) fn update_cloud_popup(&self,data:crate::CloudData){if let Some(p)=self.popups.borrow().main.as_ref(){if p.get_data().cloud{p.set_cloud_data(data);}}}
}

impl App {
    fn menu_key_tips(&self, level: i32, show: bool) {
        let mut popups = self.popups.borrow_mut();
        if !show {
            if let Some(tips) = &popups.key_tips { let _ = tips.hide(); }
            return;
        }
        let Some(popup) = (if level == 0 { &popups.main } else { &popups.sub }).as_ref().map(|p| p.clone_strong()) else { return };
        let (w, h) = (popup.get_menu_width(), popup.get_menu_height());
        let data = popup.get_data();
        if !popup.window().is_visible() || !data.key_tips { return; }
        let scale = popup.window().scale_factor();
        let pos = popup.window().position();
        let owner = hwnd_of(popup.window());
        let mut tips = Vec::new();
        for i in 0..data.items.row_count() {
            let entry = data.items.row_data(i).unwrap();
            if !entry.key_tip.is_empty() && !entry.disabled {
                tips.push(crate::MenuKeyTip { x: w + 11.0, y: 28.0 + row_y(&data, i, h) + 4.0, label: entry.key_tip });
            }
        }
        for i in 0..data.buttons.row_count() {
            let button = data.buttons.row_data(i).unwrap();
            if !button.key_tip.is_empty() && !button.disabled {
                tips.push(crate::MenuKeyTip { x: 16.0 + 4.0 + i as f32 * 65.0 + 25.0,
                    y: if data.buttons_bottom { 28.0 + h - 70.0 } else { 8.0 }, label: button.key_tip });
            }
        }
        let overlay = popups.key_tips.get_or_insert_with(|| frame::creating_popup(owner, || crate::MenuKeyTips::new().expect("menu key tips")));
        overlay.global::<crate::Theme>().set_dark(popup.global::<crate::Theme>().get_dark());
        overlay.set_tips(slint::ModelRc::from(Rc::new(slint::VecModel::from(tips))));
        overlay.set_body_width(w);
        overlay.set_body_height(h);
        overlay.window().set_position(PhysicalPosition::new(pos.x - (6.0 * scale).round() as i32, pos.y - (26.0 * scale).round() as i32));
        let _ = overlay.show();
        style_key_tips(overlay.as_weak(), 0);
    }

    fn menu_access_key(self: &Rc<Self>, level: i32, text: &str) -> bool {
        if text.chars().count() != 1 || !text.chars().all(|c| c.is_ascii_alphanumeric()) { return false; }
        let (data, prefix) = {
            let mut popups = self.popups.borrow_mut();
            let Some(p) = (if level == 0 { &popups.main } else { &popups.sub }).as_ref() else { return false };
            let data = p.get_data();
            if !data.key_tips { return false; }
            popups.key_prefix.push_str(&text.to_ascii_uppercase());
            (data, popups.key_prefix.clone())
        };
        for i in 0..data.items.row_count() {
            let e = data.items.row_data(i).unwrap();
            if !e.disabled && e.key_tip == prefix {
                if e.submenu {
                    if let Some(p) = self.popups.borrow().main.as_ref() { p.invoke_set_hot(i as i32); }
                    let anchor = {
                        let p = self.popups.borrow().main.as_ref().unwrap().clone_strong();
                        row_y(&data, i, p.get_menu_height())
                    };
                    self.popups.borrow_mut().sub_anchor = anchor;
                    menus::hover(self, level, i as i32);
                } else { menus::activate(self, level, i); }
                self.popups.borrow_mut().key_prefix.clear();
                return true;
            }
        }
        for i in 0..data.buttons.row_count() {
            let b = data.buttons.row_data(i).unwrap();
            if !b.disabled && b.key_tip == prefix {
                self.popups.borrow_mut().key_prefix.clear();
                menus::activate_button(self, i);
                return true;
            }
        }
        if !(0..data.items.row_count()).any(|i| data.items.row_data(i).unwrap().key_tip.starts_with(&prefix)) &&
            !(0..data.buttons.row_count()).any(|i| data.buttons.row_data(i).unwrap().key_tip.starts_with(&prefix)) {
            self.popups.borrow_mut().key_prefix.clear();
        }
        true
    }
    /// Creates both menu levels hidden, so that even the first menu opens in an existing
    /// (styled, cloakable) window instead of appearing as a plain window being set up.
    pub fn warm_up_popups(self: &Rc<Self>) {
        for level in 0..2 {
            let popup = make(self, level);
            popup.window().set_position(PhysicalPosition::new(-32000, -32000));
            let _ = popup.show();
            self.popups.borrow_mut().warming[level as usize] = true;
            Self::style_when_created(popup.as_weak(), self.hwnd.get(), frame::flyout_border(true), level, 0);
            let mut popups = self.popups.borrow_mut();
            if level == 0 { popups.main = Some(popup) } else { popups.sub = Some(popup) }
        }
    }

    /// Follows a theme change in the menus already created.
    pub fn sync_popup_theme(&self) {
        let ui = self.ui();
        let popups = self.popups.borrow();
        for popup in popups.main.iter().chain(popups.sub.iter()) {
            copy_theme(&ui, popup);
        }
    }

    /// Shows a menu, cloaked until it has rendered its new contents (a hidden window keeps
    /// its last frame, so it would flash the previous menu, or a blank when new), and returns
    /// its size (physical pixels), which it has only once shown.
    fn show_cloaked(&self, popup: &MenuPopup, level: i32, scale: f32) -> (i32, i32) {
        let hwnd = hwnd_of(popup.window());
        let ready = !hwnd.is_invalid() && self.popups.borrow().styled.contains(&hwnd);
        {
            let mut popups = self.popups.borrow_mut();
            popups.warming[level as usize] = false;
            popups.cloaked[level as usize] = ready;
        }
        if ready {
            frame::cloak(hwnd, true);
        } else {
            popup.window().set_position(PhysicalPosition::new(-32000, -32000));
        }
        let _ = popup.show();
        let border = self.popups.borrow().border;
        Self::style_when_created(popup.as_weak(), self.hwnd.get(), border, level, 0);
        ((popup.get_menu_width() * scale).round() as i32, (popup.get_menu_height() * scale).round() as i32)
    }

    /// Moves a menu shown by `show_cloaked` into place; it uncloaks once it has rendered.
    fn open_at(&self, popup: &MenuPopup, level: i32, x: i32, y: i32) {
        let scale = popup.window().scale_factor();
        let (w, h) = ((popup.get_menu_width() + 20.0).round() as i32, (popup.get_menu_height() + 20.0).round() as i32);
        let dark = popup.global::<crate::Theme>().get_dark();
        let mut pixels = menu_pixels(w, h, dark, popup.get_data().padding == 4.0);
        // The menu is still cloaked: capture only the content behind it, including
        // another menu level, then bake its blurred contribution into fully covered
        // body pixels. Shadow and partial edge coverage keep their recovered alpha.
        let _ = unsafe { windows::Win32::Graphics::Dwm::DwmFlush() };
        let (pw, ph) = ((w as f32 * scale).round() as usize, (h as f32 * scale).round() as usize);
        if let Some(backdrop) = crate::shell::backdrop::blurred(x - (10.0 * scale).round() as i32,
            y - (2.0 * scale).round() as i32, pw, ph, scale) {
            for (i, p) in pixels.make_mut_bytes().chunks_exact_mut(4).enumerate() {
                let a = p[3] as u32;
                if a >= if dark { 245 } else { 217 } {
                    let bx = (((i % w as usize) as f32 + 0.5) * scale - 0.5).round().max(0.0) as usize;
                    let by = (((i / w as usize) as f32 + 0.5) * scale - 0.5).round().max(0.0) as usize;
                    let source = (by.min(ph - 1) * pw + bx.min(pw - 1)) * 3;
                    for c in 0..3 {
                        p[c] = ((p[c] as u32 * a + backdrop[source+c] as u32 * (255-a) + 127) / 255 + u32::from(dark)).min(255) as u8;
                    }
                    p[3] = 255;
                }
            }
        }
        popup.set_chrome_image(slint::Image::from_rgba8(pixels));
        popup.window().set_position(PhysicalPosition::new(x - (10.0 * scale).round() as i32, y - (2.0 * scale).round() as i32));
        popup.window().request_redraw();
        // In case no frame comes (nothing changed since it was last drawn).
        slint::Timer::single_shot(std::time::Duration::from_millis(100), move || super::with_app(|a| a.uncloak_popup(level)));
    }

    fn uncloak_popup(&self, level: i32) {
        let mut popups = self.popups.borrow_mut();
        if !std::mem::take(&mut popups.cloaked[level as usize]) {
            return;
        }
        let popup = if level == 0 { popups.main.as_ref() } else { popups.sub.as_ref() };
        if let Some(p) = popup {
            frame::cloak(hwnd_of(p.window()), false);
        }
    }

    /// The native window appears a moment after show(); style it as soon as it exists.
    fn style_when_created(popup: slint::Weak<MenuPopup>, owner: HWND, border: u32, level: i32, attempt: u32) {
        let Some(p) = popup.upgrade() else { return };
        let hwnd = hwnd_of(p.window());
        if hwnd.is_invalid() {
            if attempt < 50 {
                slint::Timer::single_shot(std::time::Duration::from_millis(1), move || Self::style_when_created(popup, owner, border, level, attempt + 1));
            }
            return;
        }
        crate::app::with_app(|app| {
            let mut popups = app.popups.borrow_mut();
            if !popups.styled.contains(&hwnd) {
                frame::style_menu_popup(hwnd, owner);
                popups.styled.push(hwnd);
            }
            if std::mem::take(&mut popups.warming[level as usize]) {
                drop(popups);
                let _ = p.hide();
            } else {
                // Menus are shown without activation (see main.rs); an open one takes the
                // keyboard, and losing activation is what light-dismisses it.
                frame::activate(hwnd);
            }
        });
        let _ = border; // The measured per-pixel menu chrome includes its own border.
        let pixels = menu_pixels((p.get_menu_width() + 20.0).round() as i32,
            (p.get_menu_height() + 20.0).round() as i32, p.global::<crate::Theme>().get_dark(), p.get_data().padding == 4.0);
        frame::menu_region(hwnd, pixels.as_bytes(), pixels.width() as usize, pixels.height() as usize, p.window().scale_factor());
    }

    /// Opens the main menu at a client point (logical); it opens leftward from `flip_x` when
    /// it would leave the screen.
    /// `align` places the anchor `x` at the menu's left (0), center (0.5) or right (1).
    /// A context menu (`context`: its gap from the point) opens down from the point when it
    /// fits on the screen below it (keeping room for a command bar it lacks), else up when all
    /// of it fits above, else with the point on its first item; it may extend past the window.
    /// Its command bar goes on the side it opens from.
    pub fn show_menu_popup(self: &Rc<Self>, data: PopupData, x: f32, y: f32, flip_x: f32, align: f32, border: u32, context: Option<f32>) {
        self.popups.borrow_mut().border = border;
        self.close_sub_popup();
        let scale = self.ui().window().scale_factor();
        let popup = {
            let mut popups = self.popups.borrow_mut();
            if popups.main.is_none() {
                drop(popups);
                let made = make(self, 0);
                popups = self.popups.borrow_mut();
                popups.main = Some(made);
            }
            popups.main.as_ref().unwrap().clone_strong()
        };
        popup.set_data(data);
        popup.set_cloud_data(self.ui().global::<crate::Shell>().get_cloud());
        popup.set_chrome_image(slint::Image::default());
        popup.set_keyboard(false);
        self.popups.borrow_mut().key_prefix.clear();
        popup.set_open_index(-1);
        popup.invoke_set_hot(-1);
        if popup.get_data().keyboard { popup.invoke_first_focus(); }
        let (w, h) = self.show_cloaked(&popup, 0, scale);
        let main = self.hwnd.get();
        let (ax, ay) = frame::client_to_screen(main, (x * scale) as i32, (y * scale).round() as i32);
        let (fx, _) = frame::client_to_screen(main, (flip_x * scale) as i32, 0);
        let work = frame::work_area(ax, ay);
        // Round once: a centered menu under a button with half-pixel center lands like XAML's.
        let (ox, _) = frame::client_to_screen(main, 0, 0);
        let mut px = ox + (x * scale - w as f32 * align).round() as i32;
        if px + w > work.right {
            px = if align == 0.0 { fx - w } else { work.right - w };
        }
        px = px.max(work.left);
        let py = if let Some(gap) = context {
            let gap = (gap * scale).round() as i32;
            let (_, top) = frame::client_to_screen(main, 0, 0);
            let bar = popup.get_data().buttons.row_count() > 0;
            // It opens down when all of it fits on the screen, with room kept for the command
            // bar when it has none (measured: at 560 a 385px background menu opens down, at
            // 570 up, over a work area ending at 1032).
            let reserve = if bar { 0 } else { (82.0 * scale) as i32 };
            let (py, bar_bottom) = if ay + gap + h + reserve <= work.bottom {
                (ay + gap, false)
            } else if ay - h >= top {
                // Opening up, it grows toward the top (ending at the point): all of it has to
                // fit.
                (ay - h, true)
            } else {
                (ay - ((if bar { 86.0 } else { 14.0 }) * scale) as i32, false)
            };
            if bar_bottom {
                let mut data = popup.get_data();
                data.buttons_bottom = true;
                popup.set_data(data);
            }
            py.min(work.bottom - h).max(work.top)
        } else if ay + h > work.bottom {
            (work.bottom - h).max(work.top)
        } else {
            ay
        };
        self.popups.borrow_mut().main_rect = (px, py, w, h);
        self.open_at(&popup, 0, px, py);
    }

    /// Opens a submenu beside the hovered item of the main menu (`item_y` logical, within it).
    pub fn show_sub_popup(self: &Rc<Self>, mut data: PopupData, parent_index: i32) {
        let scale = self.ui().window().scale_factor();
        let popup = {
            let mut popups = self.popups.borrow_mut();
            if popups.sub.is_none() {
                drop(popups);
                let made = make(self, 1);
                popups = self.popups.borrow_mut();
                popups.sub = Some(made);
            }
            popups.sub.as_ref().unwrap().clone_strong()
        };
        if let Some(main) = self.popups.borrow().main.as_ref() {
            main.set_open_index(parent_index);
            data.keyboard = main.get_keyboard();
        }
        // Submenus line up with their item and overlap the menu: a MenuFlyout's by 5px, a
        // context menu's (whose window is wider by its shadow margin) by 8px.
        let context_parent = self.popups.borrow().main.as_ref().is_some_and(|m| m.get_data().padding == 4.0);
        popup.set_data(data);
        popup.set_chrome_image(slint::Image::default());
        popup.set_keyboard(false);
        popup.invoke_set_hot(-1);
        if popup.get_data().keyboard { popup.invoke_first_focus(); }
        let (w, h) = self.show_cloaked(&popup, 1, scale);
        let (mx, my, mw, _) = self.popups.borrow().main_rect;
        let anchor = self.popups.borrow().sub_anchor;
        let item_y = my + (anchor * scale) as i32;
        let work = frame::work_area(mx, my);
        let overlap = if context_parent { -8.0 } else { -5.0 };
        let mut px = mx + mw + (overlap * scale) as i32;
        if px + w > work.right {
            px = mx - w - scale as i32;
        }
        let py = if item_y + h > work.bottom { (work.bottom - h).max(work.top) } else { item_y };
        self.open_at(&popup, 1, px.max(work.left), py);
    }

    pub fn close_sub_popup(&self) {
        self.menu_key_tips(1, false);
        let popups = self.popups.borrow();
        if let Some(sub) = popups.sub.as_ref() {
            let to = popups.main.as_ref().map(|m| hwnd_of(m.window())).unwrap_or(self.hwnd.get());
            frame::hand_focus_back(hwnd_of(sub.window()), to);
            let _ = sub.hide();
        }
        if let Some(main) = popups.main.as_ref() {
            main.set_open_index(-1);
        }
    }

    /// Shows a menu item's tooltip: centred 10px left of the pointer, its bottom 21px above it
    /// (measured), in a window over the menu that takes neither focus nor clicks.
    pub fn show_tip(self: &Rc<Self>, text: &str) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        if self.tip_popup.borrow().is_none() {
            let made = frame::creating_popup(self.hwnd.get(), || crate::TipPopup::new().expect("tip window"));
            *self.tip_popup.borrow_mut() = Some(made);
        }
        let Some(tip) = self.tip_popup.borrow().as_ref().map(|t| t.clone_strong()) else { return };
        let ui = self.ui();
        tip.global::<crate::Theme>().set_dark(ui.global::<crate::Theme>().get_dark());
        tip.set_text(text.into());
        tip.set_text_width(crate::shell::text::width(text, "Segoe UI Variable Small", 12.0, 400));
        let scale = self.ui().window().scale_factor();
        let (w, h) = ((tip.get_tip_width() * scale).round() as i32, (tip.get_tip_height() * scale).round() as i32);
        let mut cursor = POINT::default();
        let _ = unsafe { GetCursorPos(&mut cursor) };
        let x = cursor.x - (10.0 * scale).round() as i32 - w / 2;
        let y = cursor.y - (21.0 * scale).round() as i32 - h;
        tip.window().set_position(PhysicalPosition::new(x, y));
        let _ = tip.show();
        Self::style_tip_when_created(tip.as_weak(), self.hwnd.get(), 0);
    }

    pub fn hide_tip(&self) {
        self.tip_timer.stop();
        if let Some(tip) = self.tip_popup.borrow().as_ref() {
            let _ = tip.hide();
        }
    }

    /// The tip's native window appears a moment after show(): give it the menus' frame (the
    /// system's backdrop, corners and border), without activating it.
    fn style_tip_when_created(tip: slint::Weak<crate::TipPopup>, owner: HWND, attempt: u32) {
        let Some(t) = tip.upgrade() else { return };
        let hwnd = hwnd_of(t.window());
        if hwnd.is_invalid() {
            if attempt < 50 {
                slint::Timer::single_shot(std::time::Duration::from_millis(1), move || Self::style_tip_when_created(tip, owner, attempt + 1));
            }
            return;
        }
        crate::app::with_app(|app| {
            let mut popups = app.popups.borrow_mut();
            if !popups.styled.contains(&hwnd) {
                frame::style_popup(hwnd, owner);
                frame::set_border_color(hwnd, frame::flyout_border(true));
                popups.styled.push(hwnd);
            }
        });
    }

    pub fn close_popups(&self) {
        self.menu_key_tips(0, false);
        self.hide_tip();
        self.close_sub_popup();
        if let Some(main) = self.popups.borrow().main.as_ref() {
            frame::hand_focus_back(hwnd_of(main.window()), self.hwnd.get());
            let _ = main.hide();
        }
    }
}

