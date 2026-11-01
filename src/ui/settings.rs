use super::{
    address::{rect, rounded},
    chrome::Focus,
    render::Canvas,
    text::{Family, TextEngine, TextStyle},
    theme::Theme,
    WINDOW,
};
use crate::app::launch_preferences::{Destination, Preferences, Source, Startup};
use crate::app::zoxide::{Preferences as ZoxidePreferences, Setting as ZoxideSetting};
use crate::app::zoxide_install::{self, Failure as InstallFailure, State as InstallState};
use windows::{
    core::Result,
    Win32::{Foundation::HWND, Graphics::Direct2D::Common::*},
};

#[path = "settings_view.rs"]
mod view;
pub use view::draw;

const ANIMATION: usize = 91;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Section,
    Integrations,
    Startup,
    StartupChoice(Startup),
    Open(Source),
    OpenIn(Source, Destination),
    Toggle,
    Restore,
    ExpandZoxide,
    Zoxide(ZoxideSetting),
    ZoxideInstall,
}

#[derive(Clone, Copy)]
enum Control {
    Dropdown,
    Toggle(usize),
    Expand,
    Button,
}

struct Row {
    target: Target,
    bounds: D2D_RECT_F,
    icon: &'static str,
    title: &'static str,
    description: String,
    title_height: f32,
    description_height: f32,
    control: Control,
    button_width: f32,
}

#[derive(Default)]
pub struct State {
    pub active: bool,
    loaded: bool,
    enabled: bool,
    recovery: bool,
    error: String,
    opening: Preferences,
    opening_error: String,
    startup: Startup,
    startup_error: String,
    zoxide: ZoxidePreferences,
    zoxide_error: String,
    zoxide_install: InstallState,
    zoxide_winget: bool,
    zoxide_recheck: bool,
    size: (f32, f32),
    hover: Option<Target>,
    pressed: Option<Target>,
    captured: bool,
    offset: f32,
    maximum: f32,
    integrations: bool,
    expanded: bool,
    dropdown: Option<Target>,
    choice: usize,
    rows: Vec<Row>,
    groups: Vec<(f32, &'static str)>,
    notices: Vec<(D2D_RECT_F, String)>,
    toggles: [super::toggle::Motion; 3],
}

impl State {
    pub fn sync(&mut self, path: &str) {
        let active = crate::app::ids::is_settings(path);
        if self.active != active {
            self.active = active;
            self.loaded = false;
            self.hover = None;
            self.pressed = None;
            self.captured = false;
            self.dropdown = None;
        }
    }
    fn zoxide_status(&self) -> String {
        if !self.zoxide_error.is_empty() {
            return self.zoxide_error.clone();
        }
        match &self.zoxide_install {
            InstallState::Installing => crate::i18n::tr("settings.zoxide.installing").into(),
            InstallState::Installed => crate::i18n::tr("settings.zoxide.installed").into(),
            InstallState::Failed(error) => {
                let detail = match error {
                    InstallFailure::Launch(message) => {
                        crate::i18n::trf("settings.zoxide.install.launch-error", &[message])
                    }
                    InstallFailure::Exit(code, message) => crate::i18n::trf(
                        "settings.zoxide.install.exit-error",
                        &[&format!("0x{code:08X}"), message],
                    ),
                    InstallFailure::Timeout => {
                        crate::i18n::tr("settings.zoxide.install.timeout").into()
                    }
                    InstallFailure::NotFound => {
                        crate::i18n::tr("settings.zoxide.install.not-found").into()
                    }
                    InstallFailure::Settings(message) => {
                        crate::i18n::trf("settings.zoxide.error", &[message])
                    }
                };
                crate::i18n::trf("settings.zoxide.install.failed", &[&detail])
            }
            InstallState::Idle => crate::i18n::tr(if self.zoxide.available {
                "settings.zoxide.found"
            } else {
                "settings.zoxide.missing"
            })
            .into(),
        }
    }
    fn install_visible(&self) -> bool {
        !self.zoxide.available || self.zoxide_install == InstallState::Installing
    }
    fn install_enabled(&self) -> bool {
        self.install_visible() && self.zoxide_install != InstallState::Installing
    }
}

fn zoxide_labels(setting: ZoxideSetting) -> (&'static str, &'static str) {
    match setting {
        ZoxideSetting::Integration => (
            "settings.zoxide.integration.title",
            "settings.zoxide.integration.description",
        ),
        ZoxideSetting::Record => (
            "settings.zoxide.record.title",
            "settings.zoxide.record.description",
        ),
    }
}

fn opening_labels(source: Source) -> (&'static str, &'static str) {
    match source {
        Source::External => (
            "settings.open.external.title",
            "settings.open.external.description",
        ),
        Source::WinE => (
            "settings.open.win-e.title",
            "settings.open.win-e.description",
        ),
    }
}

const fn label(size: f32, weight: u16) -> TextStyle {
    TextStyle::new(Family::Variable, size, weight)
}

fn paragraph(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    style: TextStyle,
    bounds: D2D_RECT_F,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    text.draw_wrapped(canvas, value, style, bounds, color)
}

pub fn active() -> bool {
    WINDOW.with(|slot| {
        slot.borrow().as_ref().is_some_and(|window| {
            crate::app::ids::is_settings(window.tabs.active().location.tab.path())
        })
    })
}
pub fn open(hwnd: HWND) -> Result<()> {
    let existing = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()?
            .tabs
            .entries
            .iter()
            .enumerate()
            .find_map(|(tab, saved)| {
                saved
                    .location
                    .tab
                    .history
                    .iter()
                    .position(|path| crate::app::ids::is_settings(path))
                    .map(|history| (tab, history))
            })
    });
    if let Some((tab, history)) = existing {
        super::tabs::select(hwnd, tab)?;
        super::navigate(hwnd, super::Navigation::History(history))?;
    } else {
        super::tabs::open(hwnd, crate::app::ids::SETTINGS)?;
    }
    refresh(hwnd);
    Ok(())
}
pub fn refresh(hwnd: HWND) {
    if !active() {
        return;
    }
    let status = crate::app::default_manager::status();
    let opening = Preferences::read();
    let startup = Startup::read();
    let zoxide = ZoxidePreferences::read();
    let install = zoxide_install::state();
    let winget = zoxide_install::winget_available();
    let mut detect = None;
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let s = &mut window.settings;
        let before = (
            s.enabled,
            s.recovery,
            s.error.clone(),
            s.opening,
            s.opening_error.clone(),
            s.zoxide,
            s.zoxide_install.clone(),
            s.zoxide_winget,
            s.startup,
            s.startup_error.clone(),
        );
        if !s.loaded || s.zoxide_recheck {
            detect = Some(s.zoxide_recheck);
            s.zoxide_recheck = false;
        }
        let initial = !s.loaded;
        s.loaded = true;
        s.zoxide = zoxide;
        s.zoxide_install = install;
        s.zoxide_winget = winget;
        match status {
            Ok(status) => {
                s.enabled = status.enabled;
                s.recovery = status.recovery;
                if (status.enabled || !status.recovery)
                    && s.error == crate::i18n::tr("settings.default.changed")
                {
                    s.error.clear();
                }
                if status.recovery && !status.enabled && s.error.is_empty() {
                    s.error = crate::i18n::tr("settings.default.changed").into();
                }
            }
            Err(error) => {
                s.error = crate::i18n::trf("settings.default.error", &[&error.to_string()])
            }
        }
        match opening {
            Ok(preferences) => {
                s.opening = preferences;
                s.opening_error.clear();
            }
            Err(error) => {
                s.opening_error = crate::i18n::trf("settings.open.error", &[&error.to_string()])
            }
        }
        match startup {
            Ok(value) => {
                s.startup = value;
                s.startup_error.clear();
            }
            Err(error) => {
                s.startup_error = crate::i18n::trf("settings.startup.error", &[&error.to_string()]);
            }
        }
        if initial {
            s.motion(false);
        }
        before
            != (
                s.enabled,
                s.recovery,
                s.error.clone(),
                s.opening,
                s.opening_error.clone(),
                s.zoxide,
                s.zoxide_install.clone(),
                s.zoxide_winget,
                s.startup,
                s.startup_error.clone(),
            )
    });
    if let Some(enable) = detect.filter(|_| !zoxide.available) {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow().as_ref() {
                window
                    .address_worker
                    .send(super::address::Request::ZoxideDetect(enable));
            }
        });
    }
    if changed {
        animate(hwnd);
        let normalize = WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                window.chrome.focus == Focus::Settings(Target::ZoxideInstall)
                    && window.settings.zoxide.available
                    && !window.settings.install_visible()
            })
        });
        if normalize {
            super::chrome::set_focus(
                hwnd,
                Focus::Settings(Target::Zoxide(ZoxideSetting::Integration)),
            );
        }
        super::invalidate_all(hwnd);
    }
}
pub fn after_paint(hwnd: HWND) {
    if WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.settings.active && !window.settings.loaded)
    }) {
        refresh(hwnd);
    }
}
fn activate(hwnd: HWND, target: Target) {
    let cues = WINDOW.with(|slot| slot.borrow().as_ref().is_some_and(|w| w.chrome.cues));
    if matches!(
        target,
        Target::Section
            | Target::Integrations
            | Target::ExpandZoxide
            | Target::Startup
            | Target::Open(_)
    ) {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                let s = &mut window.settings;
                match target {
                    Target::Section | Target::Integrations => {
                        s.integrations = target == Target::Integrations;
                        s.offset = 0.0;
                        s.dropdown = None;
                    }
                    Target::ExpandZoxide => {
                        s.expanded = !s.expanded;
                        s.dropdown = None;
                    }
                    Target::Startup | Target::Open(_) => {
                        s.dropdown = if s.dropdown == Some(target) {
                            None
                        } else {
                            Some(target)
                        };
                        s.choice = match target {
                            Target::Open(source) => s.opening.destination(source) as usize,
                            _ => s.startup as usize,
                        };
                    }
                    _ => {}
                }
            }
        });
        invalidate(hwnd);
        return;
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.settings.dropdown = None;
        }
    });
    if let Target::StartupChoice(value) = target {
        let result = value.save();
        for handle in super::context::handles() {
            let _context = super::context::enter(handle);
            super::session::checkpoint(handle);
            refresh(handle);
        }
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.settings.startup_error = match result {
                    Ok(()) => String::new(),
                    Err(error) => crate::i18n::trf("settings.startup.error", &[&error.to_string()]),
                };
            }
        });
        control_focus(hwnd, Target::Startup, cues);
        invalidate(hwnd);
        return;
    }
    if target == Target::ZoxideInstall {
        let action = WINDOW.with(|slot| {
            slot.borrow().as_ref().and_then(|window| {
                window
                    .settings
                    .install_enabled()
                    .then_some(window.settings.zoxide_winget)
            })
        });
        match action {
            Some(true) => {
                let mut senders = Vec::new();
                for handle in super::context::handles() {
                    let _context = super::context::enter(handle);
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow().as_ref() {
                            senders.push(window.address_worker.dispatcher());
                        }
                    });
                }
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow_mut().as_mut() {
                        window.settings.zoxide_error.clear();
                    }
                });
                zoxide_install::start(move || {
                    for sender in &senders {
                        let _ = sender.send(super::address::Request::ZoxideInstallationChanged);
                    }
                });
                refresh(hwnd);
            }
            Some(false) => {
                if let Err(error) =
                    crate::shell::ops::shell_execute(hwnd, zoxide_install::INSTALL_URL, None)
                {
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow_mut().as_mut() {
                            window.settings.zoxide_error =
                                crate::i18n::trf("settings.zoxide.install.launch-error", &[&error]);
                        }
                    });
                } else {
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow_mut().as_mut() {
                            window.settings.zoxide_recheck = true;
                        }
                    });
                }
            }
            None => {}
        }
        super::invalidate_all(hwnd);
        return;
    }
    if let Target::Zoxide(setting) = target {
        let preferences = ZoxidePreferences::read();
        if !preferences.available {
            return;
        }
        let enabled = if setting == ZoxideSetting::Integration {
            preferences.enabled
        } else {
            preferences.record
        };
        let result = setting.save(!enabled);
        refresh(hwnd);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.settings.zoxide_error = match result {
                    Ok(()) => String::new(),
                    Err(error) => crate::i18n::trf("settings.zoxide.error", &[&error.to_string()]),
                };
            }
        });
        super::invalidate_all(hwnd);
        return;
    }
    if let Target::OpenIn(source, destination) = target {
        let result = source.save(destination);
        refresh(hwnd);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.settings.opening_error = match result {
                    Ok(()) => String::new(),
                    Err(error) => crate::i18n::trf("settings.open.error", &[&error.to_string()]),
                };
            }
        });
        control_focus(hwnd, Target::Open(source), cues);
        super::invalidate_all(hwnd);
        return;
    }
    let enabled = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|window| window.settings.enabled)
    });
    let result = crate::app::default_manager::set_enabled(target == Target::Toggle && !enabled);
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.settings.error = match result {
                Ok(()) => String::new(),
                Err(error) => crate::i18n::trf("settings.default.error", &[&error.to_string()]),
            };
        }
    });
    refresh(hwnd);
    super::invalidate_all(hwnd);
}
fn invalidate(hwnd: HWND) {
    super::invalidate(
        hwnd,
        super::render::Damage {
            pane: true,
            ..super::render::Damage::NONE
        },
    );
}

fn control_focus(hwnd: HWND, target: Target, cues: bool) {
    super::chrome::set_focus(hwnd, Focus::Settings(target));
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.chrome.cues = cues;
        }
    });
    invalidate(hwnd);
}

impl State {
    fn motion(&mut self, animate: bool) {
        for (index, target, on, available) in [
            (0, Target::Toggle, self.enabled, true),
            (
                1,
                Target::Zoxide(ZoxideSetting::Integration),
                self.zoxide.enabled,
                self.zoxide.available,
            ),
            (
                2,
                Target::Zoxide(ZoxideSetting::Record),
                self.zoxide.record,
                self.zoxide.available,
            ),
        ] {
            self.toggles[index].target(
                on,
                available && self.hover == Some(target),
                available && self.pressed == Some(target),
                animate,
            );
        }
    }
}

fn animate(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let mut enabled = windows::core::BOOL(0);
    let allowed = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut _ as _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
        .is_ok()
    } && enabled.as_bool();
    let (active, running) = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return (false, false);
        };
        let s = &mut window.settings;
        s.motion(allowed && s.active);
        (
            s.active,
            s.toggles.iter().any(super::toggle::Motion::running),
        )
    });
    if running {
        super::input::arm(hwnd, ANIMATION, 16);
    } else {
        super::input::stop(hwnd, ANIMATION);
    }
    if active {
        invalidate(hwnd);
    }
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id != ANIMATION {
        return false;
    }
    let running = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let s = &mut window.settings;
        if !s.active {
            s.motion(false);
            return false;
        }
        let now = std::time::Instant::now();
        let mut running = false;
        for motion in &mut s.toggles {
            running |= motion.advance(now);
        }
        running
    });
    if !running {
        super::input::stop(hwnd, ANIMATION);
    }
    invalidate(hwnd);
    true
}

pub fn leave(hwnd: HWND) {
    let changed = WINDOW.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|w| w.settings.hover.take().is_some())
    });
    if changed {
        animate(hwnd);
    }
}

pub fn moved(hwnd: HWND, point: (f32, f32)) -> bool {
    let changed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let w = slot.as_mut()?;
        if !w.settings.active {
            return None;
        }
        let hover = w.settings.hit(point);
        let changed = hover != w.settings.hover;
        w.settings.hover = hover;
        if let Some(target) = hover {
            if w.settings.dropdown.is_some() {
                if let Some(index) = w
                    .settings
                    .choices()
                    .iter()
                    .position(|value| *value == target)
                {
                    w.settings.choice = index;
                }
            }
        }
        Some(changed)
    });
    if changed == Some(true) {
        animate(hwnd);
    }
    changed.is_some() && point.1 >= 88.0
}

pub fn press(hwnd: HWND, point: (f32, f32)) -> bool {
    if point.1 < 88.0 || !active() {
        return false;
    }
    let target = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let w = slot.as_mut()?;
        let target = w.settings.hit(point);
        if !matches!(
            target,
            Some(
                Target::StartupChoice(_) | Target::OpenIn(_, _) | Target::Startup | Target::Open(_)
            )
        ) {
            w.settings.dropdown = None;
        }
        w.settings.pressed = target;
        w.settings.captured = true;
        target
    });
    if let Some(target) = target {
        let focus = match target {
            Target::StartupChoice(_) => Target::Startup,
            Target::OpenIn(source, _) => Target::Open(source),
            _ => target,
        };
        control_focus(hwnd, focus, false);
    }
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
    }
    animate(hwnd);
    true
}

pub fn release(hwnd: HWND, point: (f32, f32)) -> bool {
    if !active() {
        return false;
    }
    let captured = WINDOW.with(|slot| slot.borrow().as_ref().is_some_and(|w| w.settings.captured));
    if !captured {
        return false;
    }
    let pressed = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let w = slot.as_mut()?;
        w.settings.captured = false;
        w.settings
            .pressed
            .take()
            .filter(|target| Some(*target) == w.settings.hit(point))
    });
    unsafe {
        if windows::Win32::UI::Input::KeyboardAndMouse::GetCapture() == hwnd {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
    }
    if let Some(target) = pressed {
        activate(hwnd, target);
    }
    animate(hwnd);
    true
}

pub fn key(hwnd: HWND, key: u32) -> bool {
    if !active() {
        return false;
    }
    let mods = super::input::modifiers();
    let alt = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
    let state = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|w| (w.chrome.focus, w.address.enabled, w.settings.dropdown))
    });
    let Some((focus, enabled, dropdown)) = state else {
        return false;
    };
    if matches!(focus, Focus::Settings(_)) && !mods.ctrl {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.chrome.cues = true;
            }
        });
    }
    if dropdown.is_some() && !mods.ctrl {
        if key == 0x1b || key == 0x09 || alt && key == 0x26 {
            WINDOW.with(|slot| {
                if let Some(w) = slot.borrow_mut().as_mut() {
                    w.settings.dropdown = None;
                }
            });
            invalidate(hwnd);
            if key != 0x09 {
                return true;
            }
        } else if matches!(key, 0x26 | 0x28 | 0x24 | 0x23) {
            WINDOW.with(|slot| {
                if let Some(w) = slot.borrow_mut().as_mut() {
                    w.settings.choice = match key {
                        0x24 => 0,
                        0x23 => 1,
                        _ => 1 - w.settings.choice,
                    };
                }
            });
            invalidate(hwnd);
            return true;
        } else if matches!(key, 0x0d | 0x20) {
            let target = WINDOW.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|w| w.settings.choices()[w.settings.choice])
            });
            if let Some(target) = target {
                activate(hwnd, target);
            }
            return true;
        }
    }
    if key == 0x09 && !mods.ctrl || key == 0x75 {
        let mut ring = vec![Focus::Tab, Focus::NewTab];
        if enabled[0] {
            ring.push(Focus::Address(super::address::Target::Back));
        }
        if enabled[1] {
            ring.push(Focus::Address(super::address::Target::Forward));
        }
        ring.extend([
            Focus::Address(super::address::Target::Refresh),
            Focus::Address(super::address::Target::Address),
            Focus::Address(super::address::Target::Root),
            Focus::Address(super::address::Target::Settings),
        ]);
        let controls = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|w| w.settings.controls())
                .unwrap_or_default()
        });
        ring.extend(controls.into_iter().map(Focus::Settings));
        let current = ring
            .iter()
            .position(|v| *v == focus)
            .unwrap_or(ring.len() - 1);
        let next = (current + if mods.shift { ring.len() - 1 } else { 1 }) % ring.len();
        if let Focus::Settings(target) = ring[next] {
            if !matches!(target, Target::Section | Target::Integrations) {
                WINDOW.with(|slot| {
                    if let Some(w) = slot.borrow_mut().as_mut() {
                        let r = w.settings.bounds(target);
                        let shift = if r.top < 72.0 {
                            r.top - 72.0
                        } else if r.bottom > w.settings.size.1 - 12.0 {
                            r.bottom - w.settings.size.1 + 12.0
                        } else {
                            0.0
                        };
                        w.settings.offset =
                            (w.settings.offset + shift).clamp(0.0, w.settings.maximum);
                    }
                });
            }
        }
        super::chrome::set_focus(hwnd, ring[next]);
        invalidate(hwnd);
        return true;
    }
    if let Focus::Settings(target) = focus {
        if matches!(key, 0x20 | 0x0d) && !mods.ctrl && !alt
            || key == 0x28 && matches!(target, Target::Startup | Target::Open(_)) && !mods.ctrl
        {
            activate(hwnd, target);
            return true;
        }
        if matches!(key, 0x25 | 0x27 | 0x26 | 0x28)
            && matches!(target, Target::Section | Target::Integrations)
            && !mods.ctrl
            && !alt
        {
            let target = if target == Target::Section {
                Target::Integrations
            } else {
                Target::Section
            };
            super::chrome::set_focus(hwnd, Focus::Settings(target));
            activate(hwnd, target);
            return true;
        }
        return !mods.ctrl && !alt;
    }
    false
}

pub fn icon() -> crate::shell::icons::Rgba {
    crate::shell::icons::Rgba {
        width: 16,
        height: 16,
        pixels: vec![0; 16 * 16 * 4],
    }
}

pub fn cancel(hwnd: HWND) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.settings.pressed = None;
            window.settings.captured = false;
        }
    });
    animate(hwnd);
}

pub fn wheel(hwnd: HWND, point: (f32, f32), delta: i32) -> bool {
    let handled = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return false;
        };
        let state = &mut window.settings;
        if !state.active || point.0 < state.left() || point.1 < 88.0 {
            return false;
        }
        state.dropdown = None;
        state.offset = (state.offset - delta as f32 * 48.0 / 120.0).clamp(0.0, state.maximum);
        true
    });
    if handled {
        super::invalidate_all(hwnd);
    }
    handled
}
