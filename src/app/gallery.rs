use super::group;
use crate::shell::gallery::Photo;
use windows::Win32::Globalization::{GetDateFormatEx, DATE_YEARMONTH};
pub fn zoom_size(size: u32, steps: i32) -> u32 {
    let mut size = size.clamp(16, 256) as i32;
    for _ in 0..steps.unsigned_abs().min(64) {
        let delta = match size {
            16..=48 => 8,
            49..=96 => 12,
            _ => 40,
        };
        size = (size + steps.signum() * delta).clamp(16, 256);
    }
    size as u32
}

pub fn selection_modifiers(mods: super::selection::Modifiers) -> super::selection::Modifiers {
    if mods.shift {
        mods
    } else {
        super::selection::Modifiers {
            ctrl: true,
            shift: false,
        }
    }
}

pub fn animated_offset(from: f32, to: f32, elapsed: std::time::Duration) -> (f32, bool) {
    let t = ((elapsed.as_secs_f32() - 0.032) / 0.250).clamp(0.0, 1.0);
    (from + (to - from) * (1.0 - (1.0 - t) * (1.0 - t)), t >= 1.0)
}
pub fn retry_interval(attempt: u8) -> std::time::Duration {
    std::time::Duration::from_millis((300u64 << attempt.min(4)).min(5000))
}
pub fn order(
    entries: &[crate::shell::listing::ShellEntry],
    order: &mut Vec<usize>,
    filter: Option<&str>,
) {
    order.sort_by(|&a, &b| {
        entries[b]
            .photo
            .unwrap_or_default()
            .date
            .cmp(&entries[a].photo.unwrap_or_default().date)
            .then_with(|| {
                entries[a]
                    .path
                    .to_lowercase()
                    .cmp(&entries[b].path.to_lowercase())
            })
    });
    if let Some(root) = filter {
        let root = root.to_lowercase();
        order.retain(|&i| {
            let path = entries[i].path.to_lowercase();
            path == root
                || path
                    .strip_prefix(&root)
                    .is_some_and(|tail| tail.starts_with('\\'))
        });
    }
}
fn aspect(photo: Photo) -> f32 {
    if photo.width == 0 || photo.height == 0 {
        1.0
    } else {
        (photo.width as f32 / photo.height as f32).clamp(0.01, 100.0)
    }
}
pub fn mosaic(photos: &[Photo], width: f32, height: f32) -> (Vec<[f32; 4]>, f32) {
    let mut cells = Vec::with_capacity(photos.len());
    let mut start = 0;
    let mut y = 0.0;
    while start < photos.len() {
        let mut end = start + 1;
        let mut sum = aspect(photos[start]);
        let mut error = (width / sum - height).abs();
        while end < photos.len() && end - start < 32 {
            let next = sum + aspect(photos[end]);
            let avail = width - 12.0 * (end - start) as f32;
            if avail <= 1.0 {
                break;
            }
            let next_error = (avail / next - height).abs();
            if next_error > error {
                break;
            }
            end += 1;
            sum = next;
            error = next_error;
        }
        let available = (width - 12.0 * (end - start - 1) as f32).max(1.0);
        let mut x: f32 = 32.0;
        for photo in &photos[start..end] {
            let w = available * aspect(*photo) / sum;
            cells.push([x.floor(), y, w.round().max(1.0), height]);
            x += w + 12.0;
        }
        y += height + 12.0;
        start = end;
    }
    (cells, if photos.is_empty() { 0.0 } else { y - 12.0 })
}
pub fn month(date: u64) -> String {
    let Some(date) = group::local_time(date) else {
        return String::new();
    };
    let mut text = [0u16; 128];
    let count = unsafe {
        GetDateFormatEx(
            None,
            DATE_YEARMONTH,
            Some(&date),
            None,
            Some(&mut text),
            None,
        )
    };
    String::from_utf16_lossy(&text[..count.saturating_sub(1) as usize])
        .chars()
        .filter(|c| !matches!(*c, '\u{200e}' | '\u{200f}'))
        .collect()
}

pub struct Timeline {
    pub dates: Vec<(String, f32)>,
    pub heading: String,
    pub indicator: f32,
}

pub fn timeline(
    photos: &[Photo],
    cells: &[[f32; 4]],
    extent: f32,
    h: f32,
    offset: f32,
    page: f32,
    rail_position: Option<f32>,
) -> Timeline {
    let mut dates = Vec::new();
    let mut last_year = 0;
    let mut years = Vec::new();
    for (row, photo) in photos.iter().enumerate() {
        let year = group::local_time(photo.date).map_or(0, |t| t.wYear);
        if year == 0 || year == last_year {
            continue;
        }
        last_year = year;
        let y = 75.0 + cells[row][1] / extent.max(1.0) * (h - 116.0).max(1.0);
        years.push((year, y));
    }
    for (index, &(year, y)) in years.iter().enumerate() {
        let end = years.get(index + 1).map_or(h - 41.0, |v| v.1);
        if end - y >= 12.0 {
            dates.push((year.to_string(), y.round()));
        }
    }
    let first = cells.iter().position(|c| c[1] + c[3] > offset).unwrap_or(0);
    let last = cells
        .iter()
        .rposition(|c| c[1] < offset + page)
        .unwrap_or(first);
    let a = photos.get(first).map_or(String::new(), |p| month(p.date));
    let b = photos.get(last).map_or(String::new(), |p| month(p.date));
    let heading = if a == b || b.is_empty() {
        a
    } else {
        format!("{a} - {b}")
    };
    let indicator = rail_position.unwrap_or(80.0 + offset / extent.max(1.0) * (h - 116.0).max(1.0));
    Timeline {
        dates,
        heading,
        indicator,
    }
}

pub fn scrub_offset(
    photos: &[Photo],
    cells: &[[f32; 4]],
    extent: f32,
    height: f32,
    offset: f32,
    dates: &[(String, f32)],
    y: f32,
) -> f32 {
    let indicator = 80.0 + offset / extent.max(1.0) * (height - 116.0).max(1.0);
    let year = if (y - indicator).abs() > 12.0 {
        dates
            .iter()
            .find(|(_, top)| y >= *top && y < *top + 19.0)
            .and_then(|(label, _)| label.parse::<u16>().ok())
    } else {
        None
    };
    let row = year.and_then(|year| {
        photos
            .iter()
            .position(|photo| group::local_time(photo.date).is_some_and(|date| date.wYear == year))
    });
    row.and_then(|row| cells.get(row)).map_or_else(
        || ((y - 80.0) / (height - 116.0).max(1.0)).clamp(0.0, 1.0) * extent,
        |cell| cell[1],
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_gallery_zoom_steps() {
        for (size, direction, expected) in [
            (16, 1, 24),
            (16, -1, 16),
            (48, 1, 56),
            (48, -1, 40),
            (96, 1, 108),
            (96, -1, 84),
            (49, 1, 61),
            (49, -1, 37),
            (56, -1, 44),
            (97, 1, 137),
            (97, -1, 57),
            (108, -1, 68),
            (256, 1, 256),
            (256, -1, 216),
        ] {
            assert_eq!(zoom_size(size, direction), expected);
        }
    }
    #[test]
    fn native_mosaic_rows_normalize_aspects_and_keep_160px_crop() {
        let dims = [
            (1693, 982),
            (478, 72),
            (269, 238),
            (759, 484),
            (1004, 631),
            (354, 226),
            (589, 240),
        ];
        let photos: Vec<_> = dims
            .into_iter()
            .map(|(width, height)| Photo {
                width,
                height,
                date: 0,
            })
            .collect();
        let (cells, extent) = mosaic(&photos, 478.0, 160.0);
        assert_eq!(cells[0], [32.0, 0.0, 96.0, 160.0]);
        assert_eq!(cells[1], [140.0, 0.0, 370.0, 160.0]);
        assert_eq!(cells[2], [32.0, 172.0, 195.0, 160.0]);
        assert_eq!(cells[3], [239.0, 172.0, 271.0, 160.0]);
        assert_eq!(cells[4], [32.0, 344.0, 235.0, 160.0]);
        assert_eq!(cells[5], [278.0, 344.0, 231.0, 160.0]);
        assert_eq!(cells[6], [32.0, 516.0, 478.0, 160.0]);
        assert_eq!(extent, 676.0);
    }
}
