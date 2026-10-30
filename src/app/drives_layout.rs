pub const PITCH: f32 = 56.0;
pub fn is_device(entry: &crate::shell::listing::ShellEntry) -> bool {
    super::ids::is_drive_root(&entry.path) || !entry.is_fs
}

pub fn bucket(entry: &crate::shell::listing::ShellEntry) -> super::group::Bucket {
    let device = is_device(entry);
    super::group::Bucket {
        rank: i32::from(!device),
        label: crate::i18n::tr(if device {
            "tiles.drives"
        } else {
            "tiles.network"
        })
        .to_owned(),
    }
}

pub struct Capacity {
    pub used: f32,
    pub low: bool,
    pub text: String,
}

pub fn capacity(entry: &crate::shell::listing::ShellEntry) -> Option<Capacity> {
    if entry.total == 0 {
        return None;
    }
    let text = crate::i18n::system_format(
        "propsys.dll",
        195,
        &[
            &super::format::short_size(entry.free),
            &super::format::short_size(entry.total),
        ],
    );
    let percent = ((entry.total - entry.free.min(entry.total)) as u128 * 100
        + entry.total as u128 / 2)
        / entry.total as u128;
    Some(Capacity {
        used: percent as f32 / 100.0,
        low: percent > 90,
        text,
    })
}
#[derive(Clone)]
pub enum Kind {
    Header(usize),
    Cells(Vec<usize>),
}
#[derive(Clone)]
pub struct NavRow {
    pub top: f32,
    pub bottom: f32,
    pub kind: Kind,
}

pub struct Layout {
    pub cells: Vec<[f32; 4]>,
    pub rows: Vec<NavRow>,
    pub starts: Vec<f32>,
    pub heights: [f32; 2],
    pub extent: f32,
}

pub fn layout(counts: [usize; 2], columns: usize, collapsed: [bool; 2]) -> Layout {
    let mut result = Layout {
        cells: vec![[0.0; 4]; counts.iter().sum()],
        rows: Vec::new(),
        starts: Vec::new(),
        heights: [0.0; 2],
        extent: 0.0,
    };
    let mut y = 6.0;
    let mut first = 0;
    for group in 0..2 {
        let count = counts[group];
        if count == 0 {
            continue;
        }
        result
            .starts
            .push(if result.starts.is_empty() { 0.0 } else { y });
        result.rows.push(NavRow {
            top: y,
            bottom: y + 27.0,
            kind: Kind::Header(first),
        });
        let row_count = if collapsed[group] {
            0
        } else {
            count.div_ceil(columns)
        };
        for row in 0..row_count {
            let top = y + 28.0 + row as f32 * PITCH;
            let step_top = top - if row == 0 { 1.0 } else { 0.0 };
            let mut members = Vec::new();
            for col in 0..columns {
                let local = row * columns + col;
                if local >= count {
                    break;
                }
                let index = first + local;
                result.cells[index] = [14.0 + col as f32 * 254.0, top, 250.0, 52.0];
                members.push(index);
            }
            result.starts.push(step_top);
            result.rows.push(NavRow {
                top: step_top,
                bottom: top + PITCH,
                kind: Kind::Cells(members),
            });
        }
        result.heights[group] = if collapsed[group] {
            30.0
        } else {
            27.0 + row_count as f32 * PITCH
        };
        y += result.heights[group];
        first += count;
    }
    result.extent = if result.rows.is_empty() { 0.0 } else { y };
    result.starts.push(result.extent);
    result
}

pub fn units(starts: &[f32], pixels: f32) -> f32 {
    if starts.len() < 2 {
        return 0.0;
    }
    let i = starts
        .partition_point(|&top| top <= pixels)
        .saturating_sub(1)
        .min(starts.len() - 2);
    (i as f32 * 100.0
        + (pixels - starts[i]).max(0.0) / (starts[i + 1] - starts[i]).max(1.0) * 100.0)
        .floor()
}
pub fn pixels(starts: &[f32], units: f32) -> f32 {
    if starts.len() < 2 {
        return 0.0;
    }
    let i = ((units.max(0.0) / 100.0).floor() as usize).min(starts.len() - 2);
    starts[i] + (starts[i + 1] - starts[i]) * (units / 100.0 - i as f32).clamp(0.0, 1.0)
}

pub enum Navigation {
    Line(i32),
    Page(i32),
}

pub fn target(
    rows: &[NavRow],
    cells: &[[f32; 4]],
    header: Option<usize>,
    current: Option<usize>,
    offset: f32,
    view: f32,
    navigation: Navigation,
) -> Option<Kind> {
    if rows.is_empty() {
        return None;
    }
    let at = rows
        .iter()
        .position(|row| match &row.kind {
            Kind::Header(index) => header == Some(*index),
            Kind::Cells(members) => {
                header.is_none() && current.is_some_and(|index| members.contains(&index))
            }
        })
        .unwrap_or_else(|| {
            rows.iter()
                .position(|row| matches!(row.kind, Kind::Cells(_)))
                .unwrap_or(0)
        });
    let to = match navigation {
        Navigation::Page(direction) => {
            let count = (view / PITCH).floor().max(1.0) as usize;
            if direction > 0 {
                match rows
                    .iter()
                    .rposition(|row| row.bottom <= offset + view + 0.01)
                {
                    Some(edge) if edge > at => edge,
                    _ => (at + count).min(rows.len() - 1),
                }
            } else {
                match rows.iter().position(|row| row.top >= offset - 0.01) {
                    Some(edge) if edge < at => edge,
                    _ => at.saturating_sub(count),
                }
            }
        }
        Navigation::Line(_) if current.is_none() && header.is_none() => at,
        Navigation::Line(direction) if direction > 0 => (at + 1).min(rows.len() - 1),
        Navigation::Line(_) => at.saturating_sub(1),
    };
    let x = current
        .and_then(|index| cells.get(index))
        .map_or(14.0, |cell| cell[0]);
    Some(match &rows[to].kind {
        Kind::Header(index) => Kind::Header(*index),
        Kind::Cells(members) => Kind::Cells(vec![*members
            .iter()
            .min_by(|&&a, &&b| (cells[a][0] - x).abs().total_cmp(&(cells[b][0] - x).abs()))?]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouped_drive_scroll_geometry_and_units() {
        let l = layout([1, 32], 2, [false, false]);
        assert_eq!(l.extent, 1012.0);
        assert_eq!(l.cells[1], [14.0, 117.0, 250.0, 52.0]);
        assert_eq!(l.cells[2][0], 268.0);
        assert_eq!(units(&l.starts, 118.0), 303.0);
        assert_eq!(units(&l.starts, 286.0), 601.0);
        assert_eq!(units(&l.starts, 453.0), 900.0);
        assert_eq!(pixels(&l.starts, 362.0).round(), 151.0);
        let folded = layout([1, 32], 2, [false, true]);
        assert_eq!(folded.extent, 119.0);
    }
}
