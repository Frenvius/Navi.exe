use super::{
    drag_visual::{Frame, Popup, TabLook},
    WINDOW,
};
use crate::app::tab_transfer::{self as shared, incoming_slot};
use std::time::Instant;
use windows::core::Result;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::*;

const INCOMING: usize = 63;
struct Incoming {
    source: HWND,
    slot: usize,
    shown: usize,
    since: Instant,
}
#[derive(Default)]
pub struct State {
    target: Option<HWND>,
    preview: Option<(TabLook, Popup)>,
    incoming: Option<Incoming>,
}

fn screen(hwnd: HWND, point: (f32, f32)) -> Result<POINT> {
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    let mut point = POINT {
        x: (point.0 * scale).round() as i32,
        y: (point.1 * scale).round() as i32,
    };
    if !unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut point) }.as_bool() {
        return Err(windows::core::Error::from_win32());
    }
    Ok(point)
}

pub fn moved(hwnd: HWND, point: (f32, f32), pitch: f32) -> Result<()> {
    let data = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let drag = window
            .title
            .drag
            .filter(|drag| drag.active && !drag.dropping)?;
        Some((
            drag,
            window.title.items.get(drag.from)?.clone(),
            window.address.width,
            window.tab_transfer.target,
        ))
    });
    let Some((drag, item, width, old)) = data else {
        return Ok(());
    };
    let at = screen(hwnd, point)?;
    let mut target = shared::target_at(at).filter(|target| *target != hwnd);
    if let Some(peer) = target {
        if shared::send(peer, shared::HOVER, hwnd, shared::packed(at)) == 0 {
            target = None;
        }
    }
    if old != target {
        clear_target(hwnd, old);
    }
    let look = if target.is_none()
        && (!(8.0..40.0).contains(&point.1) || point.0 < 8.0 || point.0 >= width - 136.0)
    {
        TabLook::Detached
    } else {
        TabLook::Strip
    };
    let Some(mut window) = WINDOW.with(|slot| slot.borrow_mut().take()) else {
        return Ok(());
    };
    let result = (|| {
        window.tab_transfer.target = target;
        if window
            .tab_transfer
            .preview
            .as_ref()
            .is_none_or(|(old, _)| *old != look)
        {
            window.tab_transfer.preview.take();
            let renderer = window
                .graphics
                .as_ref()
                .ok_or_else(windows::core::Error::from_win32)?
                .0
                .clone();
            window.tab_transfer.preview = Some((
                look,
                Popup::new(
                    hwnd,
                    renderer,
                    &mut window.text,
                    Frame::Tab {
                        item,
                        width: pitch + 8.0,
                        look,
                    },
                )?,
            ));
        }
        window.title.external_ghost = true;
        let origin = screen(
            hwnd,
            (
                4.0 + drag.from as f32 * pitch + point.0 - drag.origin.0,
                8.0 + point.1 - drag.origin.1,
            ),
        )?;
        if let Some((_, popup)) = &window.tab_transfer.preview {
            popup.move_to(origin)?;
        }
        Ok(())
    })();
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    publish(hwnd);
    result
}

fn clear_target(source: HWND, target: Option<HWND>) {
    if let Some(target) = target {
        shared::send(target, shared::CLEAR, source, LPARAM(0));
    }
}

pub fn cancel(hwnd: HWND) {
    let data = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        window.title.external_ghost = false;
        Some((
            window.tab_transfer.target.take(),
            window.tab_transfer.preview.take(),
        ))
    });
    if let Some((target, preview)) = data {
        clear_target(hwnd, target);
        drop(preview);
    }
    publish(hwnd);
}

pub fn release(hwnd: HWND) -> bool {
    let data = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let drag = window
            .title
            .drag
            .filter(|drag| drag.active && !drag.dropping)?;
        Some((
            drag,
            window
                .tabs
                .entries
                .get(drag.from)?
                .location
                .tab
                .path()
                .to_owned(),
        ))
    });
    let Some((drag, path)) = data else {
        return false;
    };
    let point = match screen(hwnd, drag.pointer) {
        Ok(point) => point,
        Err(error) => {
            super::fail(error);
            cancel(hwnd);
            return false;
        }
    };
    let hit = shared::target_at(point);
    let local = hit == Some(hwnd)
        && (8.0..40.0).contains(&drag.pointer.1)
        && drag.slot == drag.shown
        && drag.slot != drag.from;
    if local {
        cancel(hwnd);
        return false;
    }
    let destination = hit.filter(|peer| *peer != hwnd).and_then(|peer| {
        let slot = shared::send(peer, shared::HOVER, hwnd, shared::packed(point));
        slot.checked_sub(1).map(|slot| (peer, slot))
    });
    let moved = if let Some((peer, slot)) = destination {
        let units: Vec<_> = format!("{slot}\n{path}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let packet = COPYDATASTRUCT {
            dwData: shared::DATA,
            cbData: (units.len() * 2) as u32,
            lpData: units.as_ptr() as _,
        };
        let accepted = shared::send(
            peer,
            WM_COPYDATA,
            hwnd,
            LPARAM(&packet as *const _ as isize),
        ) == 1;
        if accepted {
            unsafe {
                let _ = SetForegroundWindow(peer);
            }
        }
        accepted
    } else {
        match super::create_window(&path, Instant::now()) {
            Ok(created) => {
                let mut bounds = RECT::default();
                let result = unsafe {
                    GetWindowRect(hwnd, &mut bounds).and_then(|_| {
                        SetWindowPos(
                            created,
                            None,
                            point.x,
                            point.y,
                            (bounds.right - bounds.left).max(514),
                            (bounds.bottom - bounds.top).max(307),
                            SWP_NOZORDER,
                        )
                    })
                };
                if let Err(error) = result {
                    eprintln!("detached window position: {error}");
                }
                unsafe {
                    let _ = SetForegroundWindow(created);
                }
                true
            }
            Err(error) => {
                eprintln!("detach native tab: {error}");
                false
            }
        }
    };
    cancel(hwnd);
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.title.drag = None;
            window.title.pressed = super::titlebar::Target::None;
        }
    });
    super::input::stop(hwnd, 21);
    super::input::stop(hwnd, 22);
    if let Err(error) = super::release_capture(hwnd) {
        super::fail(error);
    }
    if moved {
        if let Err(error) = super::tabs::close(hwnd, drag.from) {
            super::fail(error);
        }
    }
    super::invalidate_title(hwnd);
    true
}

pub fn message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if ![shared::HOVER, shared::CLEAR, WM_COPYDATA].contains(&message) {
        return None;
    }
    let source = HWND(wparam.0 as _);
    if source == hwnd || !shared::peer(source) {
        return Some(LRESULT(0));
    }
    if message == shared::CLEAR {
        let clear = WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                window
                    .tab_transfer
                    .incoming
                    .as_ref()
                    .is_some_and(|incoming| incoming.source == source)
            })
        });
        if clear {
            clear_incoming(hwnd);
        }
        return Some(LRESULT(1));
    }
    if message == WM_COPYDATA {
        if lparam.0 == 0 {
            return Some(LRESULT(0));
        }
        let packet = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        if packet.dwData != shared::DATA
            || packet.cbData < 4
            || packet.cbData > 65536
            || packet.cbData % 2 != 0
            || packet.lpData.is_null()
        {
            return Some(LRESULT(0));
        }
        let units = unsafe {
            std::slice::from_raw_parts(packet.lpData as *const u16, packet.cbData as usize / 2)
        };
        let Some((slot, path)) = shared::payload(units) else {
            return Some(LRESULT(0));
        };
        let capacity = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|window| window.tabs.entries.len() < 128)
        });
        if !capacity {
            return Some(LRESULT(0));
        }
        clear_incoming(hwnd);
        let result = super::tabs::import(hwnd, &path, slot);
        if let Err(error) = &result {
            eprintln!("receive native tab: {error}");
        }
        return Some(LRESULT(isize::from(result.is_ok())));
    }
    let mut point = POINT {
        x: lparam.0 as i16 as i32,
        y: (lparam.0 >> 16) as i16 as i32,
    };
    if !unsafe { windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point) }.as_bool() {
        return Some(LRESULT(0));
    }
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    let mut pending = false;
    let result = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let count = window.tabs.entries.len();
        let (slot, candidate) = incoming_slot(
            window.address.width,
            count,
            (point.x as f32 / scale, point.y as f32 / scale),
        )?;
        if window
            .tab_transfer
            .incoming
            .as_ref()
            .is_none_or(|incoming| incoming.source != source)
        {
            window.tab_transfer.incoming = Some(Incoming {
                source,
                slot: count,
                shown: count,
                since: Instant::now(),
            });
        }
        let incoming = window.tab_transfer.incoming.as_mut().unwrap();
        if let Some(candidate) = candidate {
            if incoming.slot != candidate {
                incoming.slot = candidate;
                incoming.since = Instant::now();
                pending = true;
            }
        }
        window.title.incoming = Some(incoming.shown);
        Some(slot + 1)
    });
    if pending {
        super::input::arm(hwnd, INCOMING, 300);
    }
    if result.is_some() {
        super::invalidate_title(hwnd);
        publish(hwnd);
    }
    Some(LRESULT(result.unwrap_or(0) as isize))
}

fn clear_incoming(hwnd: HWND) {
    super::input::stop(hwnd, INCOMING);
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.tab_transfer.incoming = None;
            window.title.incoming = None;
        }
    });
    super::invalidate_title(hwnd);
    publish(hwnd);
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id != INCOMING {
        return false;
    }
    super::input::stop(hwnd, INCOMING);
    let remaining = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let incoming = window.tab_transfer.incoming.as_mut()?;
        let remaining = 300u128.saturating_sub(incoming.since.elapsed().as_millis());
        if remaining == 0 {
            incoming.shown = incoming.slot;
            window.title.incoming = Some(incoming.shown);
        }
        Some(remaining)
    });
    if let Some(remaining) = remaining.filter(|remaining| *remaining > 0) {
        super::input::arm(hwnd, INCOMING, remaining as u32 + 1);
    } else {
        super::invalidate_title(hwnd);
    }
    publish(hwnd);
    true
}

fn publish(hwnd: HWND) {
    let values = WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|window| {
            (
                usize::from(window.tab_transfer.preview.is_some()),
                window.title.incoming.map_or(0, |slot| slot + 1),
            )
        })
    });
    if let Some((preview, incoming)) = values {
        for (name, value) in [
            (windows::core::w!("NativeTabPreviewCount"), preview),
            (windows::core::w!("NativeIncomingTabSlot"), incoming),
        ] {
            if let Err(error) = unsafe { SetPropW(hwnd, name, Some(HANDLE(value as _))) } {
                super::fail(error);
            }
        }
    }
}
