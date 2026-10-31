use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetPropW, PostMessageW, WM_APP, WM_CLOSE,
};

use crate::app::update::State;

pub const CHANGED: u32 = WM_APP + 49;

unsafe extern "system" fn post_changed(hwnd: HWND, _: LPARAM) -> BOOL {
    if unsafe { GetPropW(hwnd, w!("NaviMain")) }.0 as usize == crate::app::tab_transfer::MARKER {
        let _ = unsafe { PostMessageW(Some(hwnd), CHANGED, WPARAM(0), LPARAM(0)) };
    }
    true.into()
}

fn notifier() -> impl Fn() + Send + 'static {
    let thread = unsafe { GetCurrentThreadId() };
    move || {
        let _ = unsafe { EnumThreadWindows(thread, Some(post_changed), LPARAM(0)) };
    }
}

pub fn start() {
    crate::app::update::start_checks(notifier());
}

pub fn activate() {
    crate::app::update::install(notifier());
}

pub fn changed(hwnd: HWND) {
    if crate::app::update::state() != State::Launched {
        super::invalidate(
            hwnd,
            super::render::Damage {
                address: true,
                ..super::render::Damage::NONE
            },
        );
        return;
    }
    for handle in super::context::handles() {
        let _ = unsafe { PostMessageW(Some(handle), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

pub fn tooltip(state: &State) -> Option<String> {
    Some(match state {
        State::Available(release) => {
            crate::i18n::trf("app.update.available", &[&release.version.to_string()])
        }
        State::Installing(release) => {
            crate::i18n::trf("app.update.installing", &[&release.version.to_string()])
        }
        State::Failed(_, error) => crate::i18n::trf("app.update.failed", &[error]),
        State::Idle | State::Launched => return None,
    })
}
