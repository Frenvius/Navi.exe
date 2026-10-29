use std::cmp::Ordering;

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use super::sort::{compare_names, SortKey};
use crate::shell::columns::SortValue;
use crate::shell::listing::ShellEntry;

pub fn arrange(
    order: &mut [usize],
    buckets: &std::collections::HashMap<usize, Bucket>,
    group: Group,
) -> Vec<String> {
    order.sort_by(|a, b| {
        let result = buckets[a].cmp(&buckets[b]);
        if group.descending {
            result.reverse()
        } else {
            result
        }
    });
    let mut last = None;
    order
        .iter()
        .map(|index| {
            let bucket = &buckets[index];
            let label = if last != Some(bucket) {
                bucket.label.clone()
            } else {
                String::new()
            };
            last = Some(bucket);
            label
        })
        .collect()
}

#[derive(Clone, Copy)]
pub enum Expansion {
    Expanded,
    Collapsed,
}

pub fn header_height(label: &str, expansion: Expansion) -> f32 {
    (match expansion {
        Expansion::Collapsed => super::GROUP_FOLDED,
        Expansion::Expanded => super::GROUP_HEADER,
    }) + if label.contains(super::GROUP_SUBTITLE) {
        18.0
    } else {
        0.0
    }
}

#[derive(Clone)]
pub struct Header {
    pub label: String,
    pub rows: std::ops::Range<usize>,
    pub top: f32,
    pub height: f32,
    pub folded: bool,
}

#[derive(Clone, Default)]
pub struct DetailsLayout {
    pub tops: Vec<f32>,
    pub folded: Vec<bool>,
    pub headers: Vec<Header>,
    pub extent: f32,
}

impl DetailsLayout {
    pub fn new(
        count: usize,
        labels: &[String],
        collapsed: &std::collections::HashSet<String>,
        pitch: f32,
        plate: f32,
    ) -> Self {
        let mut layout = Self::default();
        let mut y = if labels.is_empty() {
            super::ROW_TOP
        } else {
            super::GROUPED_TOP
        };
        let library = labels
            .iter()
            .any(|label| label.contains(super::GROUP_SUBTITLE));
        let mut folded = false;
        for row in 0..count {
            if let Some(label) = labels.get(row).filter(|label| !label.is_empty()) {
                if let Some(previous) = layout.headers.last_mut() {
                    previous.rows.end = row;
                }
                if row > 0 && !folded {
                    y -= (pitch - plate + 1.0).max(0.0);
                }
                if library && row > 0 {
                    y += 13.0;
                }
                folded = collapsed.contains(label);
                let height = header_height(
                    label,
                    if folded {
                        Expansion::Collapsed
                    } else {
                        Expansion::Expanded
                    },
                );
                layout.headers.push(Header {
                    label: label.clone(),
                    rows: row..count,
                    top: y,
                    height,
                    folded,
                });
                y += height;
            }
            layout.tops.push(y);
            layout.folded.push(folded);
            if !folded {
                y += pitch;
            }
        }
        layout.extent = if count == 0 {
            0.0
        } else if !folded {
            y - pitch + plate
        } else {
            y
        };
        layout
    }

    pub fn row_at(&self, y: f32, pitch: f32) -> Option<usize> {
        let row = self.tops.partition_point(|&top| top <= y).checked_sub(1)?;
        (!self.folded[row] && y < self.tops[row] + pitch).then_some(row)
    }

    pub fn visible(&self, y: f32, height: f32, plate: f32) -> std::ops::Range<usize> {
        self.tops.partition_point(|&top| top + plate <= y)
            ..self.tops.partition_point(|&top| top < y + height)
    }

    pub fn steps(&self, plate: f32) -> Vec<(f32, f32)> {
        let mut steps = self
            .headers
            .iter()
            .map(|group| {
                (
                    group.top,
                    group.top + header_height(&group.label, Expansion::Collapsed),
                )
            })
            .collect::<Vec<_>>();
        steps.extend(
            self.tops
                .iter()
                .enumerate()
                .filter(|(row, _)| !self.folded[*row])
                .map(|(_, &top)| (top, top + plate)),
        );
        steps.sort_by(|left, right| left.0.total_cmp(&right.0));
        steps
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Group {
    pub key: SortKey,
    pub descending: bool,
}

impl Group {
    /// Explorer's first direction for a key: largest and newest first.
    pub fn new(key: SortKey) -> Group {
        Group {
            key,
            descending: key.descending_first(),
        }
    }
}

/// A group: its place among the others (then by label) and its header.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bucket {
    pub rank: i32,
    pub label: String,
}

impl Bucket {
    fn new(rank: i32, label: &str) -> Bucket {
        Bucket {
            rank,
            label: label.to_string(),
        }
    }

    /// A group named by a propsys.dll string.
    fn named(rank: i32, id: u32) -> Bucket {
        Bucket {
            rank,
            label: crate::i18n::system_string("propsys.dll", id),
        }
    }

    pub fn cmp(&self, other: &Bucket) -> Ordering {
        self.rank
            .cmp(&other.rank)
            .then_with(|| compare_names(&self.label, &other.label))
    }
}

/// Today's local date, for the relative date groups.
pub struct Today {
    day: i64,
    year: i32,
    month: i32,
    week_start: i64,
}

impl Today {
    pub fn now() -> Today {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let local = local_time(now).unwrap_or_default();
        let day = day_number(local.wYear as i32, local.wMonth as i32, local.wDay as i32);
        let back = (local.wDayOfWeek as i64 - first_day_of_week() + 7) % 7;
        Today {
            day,
            year: local.wYear as i32,
            month: local.wMonth as i32,
            week_start: day - back,
        }
    }
}

/// The locale's first day of the week, 0 = Sunday (Sunday in pt-BR).
fn first_day_of_week() -> i64 {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    let mut buf = [0u16; 4];
    let n = unsafe { GetLocaleInfoEx(None, LOCALE_IFIRSTDAYOFWEEK, Some(&mut buf)) };
    // LOCALE_IFIRSTDAYOFWEEK counts from Monday (0) to Sunday (6).
    let monday_based = if n > 1 {
        (buf[0] as i64 - '0' as i64).clamp(0, 6)
    } else {
        6
    };
    (monday_based + 1) % 7
}

pub(super) fn local_time(ms: u64) -> Option<SYSTEMTIME> {
    let ticks = ms * 10_000 + 116_444_736_000_000_000;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

/// Days since a fixed epoch for a civil date.
fn day_number(y: i32, m: i32, d: i32) -> i64 {
    let (y, m) = if m <= 2 {
        (y as i64 - 1, m as i64 + 12)
    } else {
        (y as i64, m as i64)
    };
    365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + d as i64
}

fn date_bucket(ms: u64, today: &Today) -> Bucket {
    let Some(t) = (ms > 0).then(|| local_time(ms)).flatten() else {
        return Bucket::named(-1, 120);
    };
    let day = day_number(t.wYear as i32, t.wMonth as i32, t.wDay as i32);
    let (year, month) = (t.wYear as i32, t.wMonth as i32);
    let same_month = year == today.year && month == today.month;
    let last_month = if today.month == 1 {
        year == today.year - 1 && month == 12
    } else {
        year == today.year && month == today.month - 1
    };
    // propsys.dll: "Hoje" is 106, "Ontem" 105, ... "Há muito tempo" 100, "Último mês" 502.
    let (rank, id) = match day.cmp(&today.day) {
        Ordering::Equal => (7, 106),
        Ordering::Less if day == today.day - 1 => (6, 105),
        Ordering::Less if day >= today.week_start => (5, 103),
        Ordering::Less if day >= today.week_start - 7 => (4, 104),
        Ordering::Less if same_month => (3, 102),
        Ordering::Less if last_month => (2, 502),
        Ordering::Less if year == today.year => (1, 101),
        Ordering::Less => (0, 100),
        Ordering::Greater if day == today.day + 1 => (8, 107),
        Ordering::Greater if day < today.week_start + 7 => (9, 108),
        Ordering::Greater if day < today.week_start + 14 => (10, 109),
        Ordering::Greater if same_month => (11, 110),
        Ordering::Greater if year == today.year => (12, 111),
        Ordering::Greater => (13, 112),
    };
    Bucket::named(rank, id)
}

/// propsys.dll's "Não Especificado".
const UNSPECIFIED: u32 = 41157;

fn size_bucket(e: &ShellEntry) -> Bucket {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if e.is_dir || !e.is_fs {
        // Above every size, so first when the largest go first.
        return Bucket::named(7, UNSPECIFIED);
    }
    // propsys.dll: "Vazio (0 KB)" is 38569, up to "Gigante (superior a 4 GB)" at 38575.
    let rank = match e.size {
        0 => 0,
        s if s <= 16 * KB => 1,
        s if s <= MB => 2,
        s if s <= 128 * MB => 3,
        s if s <= GB => 4,
        s if s <= 4 * GB => 5,
        _ => 6,
    };
    Bucket::named(rank, 38569 + rank as u32)
}

/// The letter a name files under, without its accent ("Ébano" goes with E).
fn base_letter(c: char) -> char {
    let c = c.to_uppercase().next().unwrap_or(c);
    match c {
        'À'..='Å' | 'Ā' | 'Ă' | 'Ą' => 'A',
        'Ç' | 'Ć' | 'Č' => 'C',
        'È'..='Ë' | 'Ē' | 'Ė' | 'Ę' | 'Ě' => 'E',
        'Ì'..='Ï' | 'Ī' | 'Į' => 'I',
        'Ñ' | 'Ń' | 'Ň' => 'N',
        'Ò'..='Ö' | 'Ø' | 'Ō' | 'Ő' => 'O',
        'Ù'..='Ü' | 'Ū' | 'Ů' | 'Ű' => 'U',
        'Ý' | 'Ÿ' => 'Y',
        'Ś' | 'Š' => 'S',
        'Ź' | 'Ż' | 'Ž' => 'Z',
        _ => c,
    }
}

fn name_bucket(name: &str) -> Bucket {
    let first = name.chars().next().map(base_letter).unwrap_or(' ');
    let (rank, label) = match first {
        '0'..='9' => (0, "0 – 9"),
        'A'..='E' => (1, "A – E"),
        'F'..='L' => (2, "F – L"),
        'M'..='R' => (3, "M – R"),
        'S'..='Z' => (4, "S – Z"),
        _ => return Bucket::named(5, 250),
    };
    Bucket::new(rank, label)
}

pub fn bucket(key: SortKey, e: &ShellEntry, today: &Today, value: Option<&SortValue>) -> Bucket {
    match key {
        SortKey::Name => name_bucket(&e.name),
        SortKey::Modified => date_bucket(e.modified, today),
        SortKey::Created => date_bucket(e.created, today),
        SortKey::Size => size_bucket(e),
        SortKey::Type => Bucket::new(0, &e.type_name),
        SortKey::Authors | SortKey::Tags | SortKey::Title | SortKey::Prop(_) => {
            match value.filter(|v| !v.is_empty()) {
                Some(SortValue {
                    date: true, number, ..
                }) => {
                    let ms = number.map_or(0.0, |t| (t - 116_444_736_000_000_000.0) / 10_000.0);
                    date_bucket(ms.max(0.0) as u64, today)
                }
                Some(v) => Bucket::new(0, &v.text),
                None => Bucket::named(-1, UNSPECIFIED),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roomy_groups_trim_the_last_row_gap_without_trimming_folded_headers() {
        let labels = ["A", "", "B", "", "C"].map(str::to_owned);
        let open = DetailsLayout::new(5, &labels, &Default::default(), 28.0, 24.0);
        assert_eq!(open.tops, vec![34.0, 62.0, 117.0, 145.0, 200.0]);
        assert_eq!(open.extent, 224.0);
        let folded = DetailsLayout::new(5, &labels, &["A".into()].into(), 28.0, 24.0);
        assert_eq!(
            folded
                .headers
                .iter()
                .map(|header| header.top)
                .collect::<Vec<_>>(),
            vec![2.0, 32.0, 115.0]
        );
        assert_eq!(folded.tops, vec![32.0, 32.0, 64.0, 92.0, 147.0]);
        assert_eq!(folded.extent, 171.0);
    }

    #[test]
    fn collapsed_groups_keep_header_steps_and_exclude_items_from_hits() {
        let labels = ["A", "", "B", "", "", "C"].map(str::to_owned);
        let expanded = DetailsLayout::new(6, &labels, &Default::default(), 21.0, 22.0);
        assert_eq!(expanded.tops, vec![34.0, 55.0, 108.0, 129.0, 150.0, 203.0]);
        assert_eq!(expanded.extent, 225.0);
        let collapsed = DetailsLayout::new(
            6,
            &labels,
            &std::collections::HashSet::from(["B".to_owned()]),
            21.0,
            22.0,
        );
        assert_eq!(collapsed.tops, vec![34.0, 55.0, 106.0, 106.0, 106.0, 138.0]);
        assert_eq!(collapsed.extent, 160.0);
        assert_eq!(collapsed.row_at(107.0, 21.0), None);
        assert_eq!(collapsed.row_at(139.0, 21.0), Some(5));
        assert_eq!(
            collapsed.steps(22.0),
            vec![
                (2.0, 32.0),
                (34.0, 56.0),
                (55.0, 77.0),
                (76.0, 106.0),
                (106.0, 136.0),
                (138.0, 160.0)
            ]
        );
        assert!(collapsed
            .visible(105.0, 33.0, 22.0)
            .all(|row| collapsed.folded[row]));
    }

    #[test]
    fn grouping_preserves_existing_sort_order_inside_each_bucket() {
        let mut order = vec![3, 1, 2, 0];
        let buckets = std::collections::HashMap::from([
            (0, Bucket::new(0, "A")),
            (1, Bucket::new(0, "B")),
            (2, Bucket::new(0, "A")),
            (3, Bucket::new(0, "B")),
        ]);
        assert_eq!(
            arrange(
                &mut order,
                &buckets,
                Group {
                    key: SortKey::Type,
                    descending: false
                }
            ),
            vec!["A", "", "B", ""]
        );
        assert_eq!(order, vec![2, 0, 3, 1]);
        assert_eq!(
            arrange(
                &mut order,
                &buckets,
                Group {
                    key: SortKey::Type,
                    descending: true
                }
            ),
            vec!["B", "", "A", ""]
        );
        assert_eq!(order, vec![3, 1, 2, 0]);
    }
}
