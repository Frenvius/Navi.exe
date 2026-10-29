use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
pub(crate) const MARKER: usize = 0x535745;
pub(crate) const DATA: usize = 0x53574554;
pub(crate) const HOVER: u32 = WM_APP + 0x36;
pub(crate) const CLEAR: u32 = WM_APP + 0x37;
pub fn install(hwnd: HWND) {
    unsafe {
        let _ = SetPropW(hwnd, w!("NaviMain"), Some(HANDLE(MARKER as _)));
    }
}

pub(crate) fn peer(hwnd: HWND) -> bool {
    (unsafe { GetPropW(hwnd, w!("NaviMain")).0 as usize == MARKER })
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
pub(crate) fn target_at(point: POINT) -> Option<HWND> {
    let mut data = (point, None);
    unsafe {
        let _ = EnumWindows(Some(hit_window), LPARAM(&mut data as *mut _ as isize));
    }
    data.1
}
pub(crate) fn send(hwnd: HWND, msg: u32, source: HWND, param: LPARAM) -> usize {
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
pub(crate) fn packed(point: POINT) -> LPARAM {
    LPARAM(((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize)
}

pub fn incoming_slot(
    width: f32,
    count: usize,
    (x, y): (f32, f32),
) -> Option<(usize, Option<usize>)> {
    if !(8.0..40.0).contains(&y) || x < 8.0 || x >= width - 136.0 || count >= 128 {
        return None;
    }
    let pitch = (((width - 232.0) / (count + 1) as f32).floor()).clamp(80.0, 240.0);
    let relative = ((x - 8.0) / pitch).max(0.0);
    let slot = ((relative + 0.5).floor() as usize).min(count);
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
    Some((slot, candidate.map(|slot| slot.min(count))))
}

pub fn payload(units: &[u16]) -> Option<(usize, String)> {
    if units.len() < 2 || units.len() > 32768 {
        return None;
    }
    let text = String::from_utf16(units.strip_suffix(&[0]).unwrap_or(units)).ok()?;
    let (slot, path) = text.split_once('\n')?;
    if path.contains('\0') {
        return None;
    }
    Some((slot.parse().ok()?, path.to_owned()))
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    #[test]
    fn receiving_strip_keeps_middle_drops_distinct_from_delayed_edge_gaps() {
        assert_eq!(incoming_slot(1100.0, 2, (120.0, 24.0)), Some((0, None)));
        assert_eq!(incoming_slot(1100.0, 2, (300.0, 24.0)), Some((1, None)));
        assert_eq!(incoming_slot(1100.0, 2, (480.0, 24.0)), Some((2, Some(2))));
        assert_eq!(incoming_slot(1100.0, 2, (650.0, 24.0)), Some((2, Some(2))));
        assert_eq!(incoming_slot(1100.0, 3, (226.0, 24.0)), Some((1, Some(1))));
        for point in [(120.0, 7.0), (120.0, 40.0), (7.0, 24.0), (964.0, 24.0)] {
            assert!(incoming_slot(1100.0, 2, point).is_none());
        }
        assert!(incoming_slot(1100.0, 128, (120.0, 24.0)).is_none());
    }

    #[test]
    fn bounded_unicode_tab_payloads_preserve_the_empty_desktop_address() {
        let encode = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        assert_eq!(
            payload(&encode("2\nC:\\Temp\\ação")),
            Some((2, "C:\\Temp\\ação".into()))
        );
        assert_eq!(payload(&encode("0\n")), Some((0, String::new())));
        for value in ["x\nC:\\Temp", "-1\nC:\\Temp", "2C:\\Temp", "0\na\0b"] {
            assert!(payload(&encode(value)).is_none());
        }
        assert!(payload(&[b'0' as u16, b'\n' as u16, 0xd800]).is_none());
        assert!(payload(&vec![65; 32769]).is_none());
    }
}
