//! Moving tabs between this app's windows. Like Explorer, a cross-window transfer
//! recreates the current location; its old selection and navigation journal do not move.
use super::{with_app, App};
use crate::{Shell, TabDragPreview, Theme};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::*;

const MARKER: usize = 0x535745;
const DATA: usize = 0x53574554;
const HOVER: u32 = WM_APP + 0x36;
const CLEAR: u32 = WM_APP + 0x37;
struct Drag {
    index: usize,
    x: f32,
    y: f32,
    pitch: f32,
    target: Option<HWND>,
    preview: Option<TabDragPreview>,
}
struct Incoming {
    source: HWND,
    slot: usize,
    since: Instant,
}
thread_local! {
    static DRAG:RefCell<Option<Drag>>=const{RefCell::new(None)};
    static INCOMING:RefCell<Option<Incoming>>=const{RefCell::new(None)};
}

pub fn install(hwnd: HWND) {
    unsafe {
        let _ = SetPropW(hwnd, w!("SlintExplorerMain"), Some(HANDLE(MARKER as _)));
    }
}

fn peer(hwnd: HWND) -> bool {
    (unsafe { GetPropW(hwnd, w!("SlintExplorerMain")).0 as usize == MARKER })
        && same_executable(hwnd)
}
pub fn same_executable(hwnd: HWND) -> bool {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut name = vec![0u16; 32768];
        let mut n = name.len() as u32;
        let valid = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(name.as_mut_ptr()),
            &mut n,
        )
        .is_ok()
            && std::env::current_exe().is_ok_and(|exe| {
                exe.to_string_lossy()
                    .eq_ignore_ascii_case(&String::from_utf16_lossy(&name[..n as usize]))
            });
        let _ = windows::Win32::Foundation::CloseHandle(process);
        valid
    }
}

unsafe extern "system" fn hit_window(hwnd: HWND, param: LPARAM) -> BOOL {
    let (point, found) = &mut *(param.0 as *mut (POINT, Option<HWND>));
    if !IsWindowVisible(hwnd).as_bool()
        || GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize != 0
    {
        return true.into();
    }
    let mut rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rect);
    if point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    {
        if peer(hwnd) {
            *found = Some(hwnd)
        }
        return false.into();
    }
    true.into()
}
fn target_at(point: POINT) -> Option<HWND> {
    let mut data = (point, None);
    unsafe {
        let _ = EnumWindows(Some(hit_window), LPARAM(&mut data as *mut _ as isize));
    }
    data.1
}
fn send(hwnd: HWND, msg: u32, source: HWND, param: LPARAM) -> usize {
    let mut answer = 0;
    unsafe {
        let _ = SendMessageTimeoutW(
            hwnd,
            msg,
            WPARAM(source.0 as usize),
            param,
            SMTO_ABORTIFHUNG,
            250,
            Some(&mut answer),
        );
    }
    answer
}
fn packed(point: POINT) -> LPARAM {
    LPARAM(((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize)
}

fn clear_target(target: Option<HWND>, source: HWND) {
    if let Some(target) = target {
        send(target, CLEAR, source, LPARAM(0));
    }
}
fn hide(app: &Rc<App>, drag: Drag) {
    clear_target(drag.target, app.hwnd.get());
    if let Some(preview) = drag.preview {
        let _ = preview.hide();
    }
    app.ui().global::<Shell>().set_tab_native_ghost(false);
}

pub fn pointer(app: &Rc<App>, index: i32, kind: i32, x: f32, y: f32, local: bool) -> bool {
    if kind == 0 {
        if let Some(old) = DRAG.with(|d| d.borrow_mut().take()) {
            hide(app, old)
        }
        let pitch = app.tab_drop_layout.get().pitch;
        DRAG.with(|d| {
            *d.borrow_mut() = Some(Drag {
                index: index.max(0) as usize,
                x,
                y,
                pitch,
                target: None,
                preview: None,
            })
        });
        return false;
    }
    let Some(mut drag) = DRAG.with(|d| d.borrow_mut().take()) else {
        return false;
    };
    if kind == 3 {
        hide(app, drag);
        return false;
    }
    let ui = app.ui();
    let scale = ui.window().scale_factor();
    let mut point = POINT {
        x: (x * scale).round() as i32,
        y: (y * scale).round() as i32,
    };
    unsafe {
        let _ = ClientToScreen(app.hwnd.get(), &mut point);
    }
    if kind == 1 {
        if drag.preview.is_none() {
            let preview = crate::frame::creating_popup(app.hwnd.get(), || {
                TabDragPreview::new().expect("tab drag preview")
            });
            preview
                .global::<Theme>()
                .set_dark(ui.global::<Theme>().get_dark());
            preview
                .global::<Theme>()
                .set_accent(ui.global::<Theme>().get_accent());
            drag.preview = Some(preview);
        }
        let preview = drag.preview.as_ref().unwrap();
        let target = target_at(point).filter(|h| *h != app.hwnd.get());
        if target != drag.target {
            clear_target(drag.target, app.hwnd.get());
            drag.target = None;
        }
        if let Some(target) = target {
            if send(target, HOVER, app.hwnd.get(), packed(point)) != 0 {
                drag.target = Some(target);
            }
        }
        let width = ui.window().size().to_logical(scale).width;
        preview.set_detached_look(
            drag.target.is_none() && (!(8.0..40.0).contains(&y) || x < 8.0 || x >= width - 136.0),
        );
        if let Some(item) = ui.global::<Shell>().get_tabs().row_data(drag.index) {
            preview.set_item(item);
        }
        preview.set_tab_width(drag.pitch + 8.0);
        let mut position = POINT {
            x: ((4.0 + drag.index as f32 * drag.pitch + x - drag.x) * scale).round() as i32,
            y: ((8.0 + y - drag.y) * scale).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(app.hwnd.get(), &mut position);
        }
        preview
            .window()
            .set_position(slint::PhysicalPosition::new(position.x, position.y));
        ui.global::<Shell>().set_tab_native_ghost(true);
        let _ = preview.show();
        style(preview.as_weak(), 0);
        DRAG.with(|d| *d.borrow_mut() = Some(drag));
        return false;
    }
    let hit = target_at(point);
    let destination = hit.filter(|h| *h != app.hwnd.get()).and_then(|target| {
        let slot = send(target, HOVER, app.hwnd.get(), packed(point));
        (slot != 0).then_some((target, slot - 1))
    });
    if hit == Some(app.hwnd.get()) && local {
        hide(app, drag);
        return false;
    }
    let path = {
        let st = app.state.borrow();
        st.tabs.get(drag.index).map(|t| t.path().to_string())
    };
    let Some(path) = path else {
        hide(app, drag);
        return true;
    };
    app.save_view();
    let moved = if let Some((target, slot)) = destination {
        let payload = format!("{}\n{}", slot, path);
        let data: Vec<u16> = payload.encode_utf16().chain(Some(0)).collect();
        let packet = COPYDATASTRUCT {
            dwData: DATA,
            cbData: (data.len() * 2) as u32,
            lpData: data.as_ptr() as *mut _,
        };
        let accepted = send(
            target,
            WM_COPYDATA,
            app.hwnd.get(),
            LPARAM(&packet as *const _ as isize),
        ) == 1;
        if accepted {
            unsafe {
                let _ = SetForegroundWindow(target);
            }
        }
        accepted
    } else {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(app.hwnd.get(), &mut rect);
        }
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                std::process::Command::new(exe)
                    .arg("--new-window")
                    .arg(&path)
                    .env(
                        "SLINT_TAB_WINDOW",
                        format!(
                            "{},{},{},{}",
                            point.x,
                            point.y,
                            rect.right - rect.left,
                            rect.bottom - rect.top
                        ),
                    )
                    .spawn()
                    .ok()
                    .inspect(|child| unsafe {
                        let _ = AllowSetForegroundWindow(child.id());
                    })
            })
            .is_some()
    };
    let index = drag.index;
    hide(app, drag);
    if moved {
        app.close_tab(index)
    }
    true
}

fn style(preview: slint::Weak<TabDragPreview>, attempt: u32) {
    let Some(preview) = preview.upgrade() else {
        return;
    };
    let hwnd = crate::hwnd_of(preview.window());
    if hwnd.is_invalid() {
        if attempt < 30 {
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                style(preview.as_weak(), attempt + 1)
            });
        }
    } else {
        crate::frame::style_drop_preview(hwnd);
    }
}

pub fn message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if ![HOVER, CLEAR, WM_COPYDATA].contains(&msg) {
        return None;
    }
    let source = HWND(wparam.0 as _);
    if !peer(source) || source == hwnd {
        return Some(LRESULT(0));
    }
    if msg == CLEAR {
        let clear = INCOMING.with(|i| {
            let mut i = i.borrow_mut();
            if i.as_ref().is_some_and(|s| s.source == source) {
                *i = None;
                true
            } else {
                false
            }
        });
        if clear {
            with_app(|a| a.ui().global::<Shell>().set_tab_incoming(false));
        }
        return Some(LRESULT(1));
    }
    if msg == WM_COPYDATA {
        if lparam.0 == 0 {
            return Some(LRESULT(0));
        }
        let packet = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        if packet.dwData != DATA
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
        let Ok(text) = String::from_utf16(units.strip_suffix(&[0]).unwrap_or(units)) else {
            return Some(LRESULT(0));
        };
        let Some((slot, path)) = text.split_once('\n') else {
            return Some(LRESULT(0));
        };
        let Ok(slot) = slot.parse::<usize>() else {
            return Some(LRESULT(0));
        };
        let path = path.to_string();
        let queued = slint::invoke_from_event_loop(move || {
            with_app(|app| {
                app.ui().global::<Shell>().set_tab_incoming(false);
                INCOMING.with(|i| *i.borrow_mut() = None);
                let last = app.state.borrow().tabs.len();
                app.new_tab(&path);
                app.reorder_tab(last, slot.min(last));
            })
        })
        .is_ok();
        return Some(LRESULT(queued as isize));
    }
    let mut point = POINT {
        x: lparam.0 as u16 as i16 as i32,
        y: (lparam.0 >> 16) as u16 as i16 as i32,
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
    }
    let mut result = 0;
    with_app(|app| {
        let ui = app.ui();
        let shell = ui.global::<Shell>();
        let scale = ui.window().scale_factor();
        let (x, y) = (point.x as f32 / scale, point.y as f32 / scale);
        let width = ui.window().size().to_logical(scale).width;
        if !(8.0..40.0).contains(&y) || x < 8.0 || x >= width - 136.0 {
            return;
        }
        let count = app.state.borrow().tabs.len();
        let pitch = (((width - 232.0) / (count + 1) as f32).floor()).clamp(80.0, 240.0);
        let relative = ((x - 8.0) / pitch).max(0.0);
        let slot = (relative + 0.5).floor() as usize;
        result = slot.min(count) + 1;
        let fraction = relative.fract();
        let candidate = if relative >= count as f32 {
            Some(count)
        } else if fraction <= 0.2 {
            Some(relative.floor() as usize)
        } else if fraction >= 0.8 {
            Some(relative.floor() as usize + 1)
        } else {
            None
        };
        let mut visible = None;
        let mut pending = None;
        INCOMING.with(|i| {
            let mut state = i.borrow_mut();
            if state.as_ref().is_none_or(|s| s.source != source) {
                *state = Some(Incoming {
                    source,
                    slot: count,
                    since: Instant::now(),
                });
                visible = Some(count);
            }
            let state = state.as_mut().unwrap();
            if let Some(next) = candidate {
                let next = next.min(count);
                if state.slot != next {
                    state.slot = next;
                    state.since = Instant::now();
                    pending = Some((next, state.since));
                }
            }
            if state.since.elapsed() >= Duration::from_millis(300) {
                visible = Some(state.slot);
            }
        });
        shell.set_tab_incoming(true);
        if let Some(slot) = visible {
            shell.set_tab_incoming_slot(slot as i32);
        }
        if let Some((slot, since)) = pending {
            slint::Timer::single_shot(Duration::from_millis(300), move || {
                let valid = INCOMING.with(|i| {
                    i.borrow()
                        .as_ref()
                        .is_some_and(|s| s.source == source && s.slot == slot && s.since == since)
                });
                if valid {
                    with_app(|a| a.ui().global::<Shell>().set_tab_incoming_slot(slot as i32));
                }
            });
        }
    });
    Some(LRESULT(result as isize))
}

pub fn initial_position(hwnd: HWND) {
    if let Ok(value) = std::env::var("SLINT_TAB_WINDOW") {
        let numbers: Vec<i32> = value.split(',').filter_map(|s| s.parse().ok()).collect();
        if numbers.len() == 4 {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    numbers[0],
                    numbers[1],
                    numbers[2].max(514),
                    numbers[3].max(307),
                    SWP_NOZORDER,
                );
                let _ = SetForegroundWindow(hwnd);
            }
        }
        std::env::remove_var("SLINT_TAB_WINDOW");
    }
}
