const EPSILON: f32 = 0.01;
pub(crate) fn wheel_lines() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut lines: u32 = 3;
    if let Err(error) = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut lines as *mut _ as _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    } {
        eprintln!("wheel scroll settings: {error}; using 3 lines");
        return 3;
    }
    lines
}

pub fn lines_from(items: &[(f32, f32)], y: f32, view: f32, lines: i32) -> f32 {
    if lines > 0 {
        let whole = items.partition_point(|&(_, bottom)| bottom <= y + view + EPSILON);
        match items.get(whole + lines as usize - 1) {
            Some(&(_, bottom)) => bottom - view,
            None => f32::MAX,
        }
    } else {
        let above = items.partition_point(|&(top, _)| top < y - EPSILON);
        match above.checked_sub(lines.unsigned_abs() as usize) {
            Some(i) if i > 0 => items[i].0,
            _ => 0.0,
        }
    }
}

pub fn page_from(items: &[(f32, f32)], y: f32, view: f32, direction: i32, clear: f32) -> f32 {
    let whole_to =
        |y: f32| items.partition_point(|&(_, bottom)| bottom + clear <= y + view + EPSILON);
    if direction > 0 {
        match items.get(whole_to(y)) {
            Some(&(top, _)) => top.max(y + 1.0),
            None => f32::MAX,
        }
    } else {
        let above = items.partition_point(|&(top, _)| top < y - EPSILON);
        let first = items[..above].partition_point(|&(top, _)| whole_to(top) < above);
        if first == 0 {
            0.0
        } else {
            items[first].0.min(y - 1.0)
        }
    }
}

pub fn wheel_line_limit(view: f32, pitch: f32) -> u32 {
    ((view / pitch).floor() as u32).saturating_sub(1).max(1)
}

pub fn wheel_offset(
    items: &[(f32, f32)],
    mut y: f32,
    view: f32,
    maximum: f32,
    clear: f32,
    line_limit: u32,
    notches: i32,
) -> f32 {
    let lines = wheel_lines();
    if lines == 0 || notches == 0 {
        return y;
    }
    if lines == u32::MAX {
        for _ in 0..notches.unsigned_abs() {
            y = page_from(items, y, view, -notches.signum(), clear).clamp(0.0, maximum);
        }
        return y;
    }
    let count = lines
        .saturating_mul(notches.unsigned_abs())
        .min(line_limit)
        .min(i32::MAX as u32) as i32;
    lines_from(items, y, view, -notches.signum() * count).clamp(0.0, maximum)
}

pub fn wheel_steps(remainder: &mut i32, delta: i32) -> i32 {
    let total = remainder.saturating_add(delta);
    let steps = total / 120;
    *remainder = total - steps * 120;
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<(f32, f32)> {
        (0..5019)
            .map(|r| {
                let top = 6.0 + r as f32 * 21.0;
                (top, top + 22.0)
            })
            .collect()
    }

    #[test]
    fn details_steps_match_measured_edges_and_track_clearance() {
        let items = rows();
        assert_eq!(lines_from(&items, 0.0, 517.0, 1), 15.0);
        assert_eq!(lines_from(&items, 0.0, 517.0, 3), 57.0);
        assert_eq!(lines_from(&items, 57.0, 517.0, 3), 120.0);
        assert_eq!(lines_from(&items, 120.0, 517.0, -3), 69.0);
        assert_eq!(lines_from(&items, 57.0, 517.0, -3), 0.0);
        assert_eq!(page_from(&items, 0.0, 517.0, 1, 1.0), 510.0);
        assert_eq!(page_from(&items, 510.0, 517.0, -1, 1.0), 0.0);
        assert_eq!(page_from(&items, 0.0, 49.0, 1, 1.0), 27.0);
        assert_eq!(page_from(&items, 0.0, 49.0, 1, 0.0), 48.0);
    }

    #[test]
    fn high_resolution_wheel_retains_incomplete_and_reversed_deltas() {
        let mut remainder = 0;
        assert_eq!(
            [-30, -30, -30, -30].map(|d| wheel_steps(&mut remainder, d)),
            [0, 0, 0, -1]
        );
        assert_eq!(
            [30, 30, 30, 30].map(|d| wheel_steps(&mut remainder, d)),
            [0, 0, 0, 1]
        );
        assert_eq!(
            [60, -60, 240, -300].map(|d| wheel_steps(&mut remainder, d)),
            [0, 0, 2, -2]
        );
        assert_eq!(remainder, -60);
    }
}
