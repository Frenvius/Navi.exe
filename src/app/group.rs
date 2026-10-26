//! "Agrupar por": which group each item falls in, named as Explorer names them (the
//! relative dates and size ranges are propsys.dll's strings, in the display language).

use std::cmp::Ordering;

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use super::sort::{compare_names, SortKey};
use crate::shell::columns::SortValue;
use crate::shell::listing::ShellEntry;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Group {
    pub key: SortKey,
    pub descending: bool,
}

impl Group {
    /// Explorer's first direction for a key: largest and newest first.
    pub fn new(key: SortKey) -> Group {
        Group { key, descending: key.descending_first() }
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
        Bucket { rank, label: label.to_string() }
    }

    /// A group named by a propsys.dll string.
    fn named(rank: i32, id: u32) -> Bucket {
        Bucket { rank, label: crate::i18n::system_string("propsys.dll", id) }
    }

    pub fn cmp(&self, other: &Bucket) -> Ordering {
        self.rank.cmp(&other.rank).then_with(|| compare_names(&self.label, &other.label))
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
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let local = local_time(now).unwrap_or_default();
        let day = day_number(local.wYear as i32, local.wMonth as i32, local.wDay as i32);
        let back = (local.wDayOfWeek as i64 - first_day_of_week() + 7) % 7;
        Today { day, year: local.wYear as i32, month: local.wMonth as i32, week_start: day - back }
    }
}

/// The locale's first day of the week, 0 = Sunday (Sunday in pt-BR).
fn first_day_of_week() -> i64 {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    let mut buf = [0u16; 4];
    let n = unsafe { GetLocaleInfoEx(None, LOCALE_IFIRSTDAYOFWEEK, Some(&mut buf)) };
    // LOCALE_IFIRSTDAYOFWEEK counts from Monday (0) to Sunday (6).
    let monday_based = if n > 1 { (buf[0] as i64 - '0' as i64).clamp(0, 6) } else { 6 };
    (monday_based + 1) % 7
}

pub(super) fn local_time(ms: u64) -> Option<SYSTEMTIME> {
    let ticks = ms * 10_000 + 116_444_736_000_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
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
    let (y, m) = if m <= 2 { (y as i64 - 1, m as i64 + 12) } else { (y as i64, m as i64) };
    365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + d as i64
}

fn date_bucket(ms: u64, today: &Today) -> Bucket {
    let Some(t) = (ms > 0).then(|| local_time(ms)).flatten() else { return Bucket::named(-1, 120) };
    let day = day_number(t.wYear as i32, t.wMonth as i32, t.wDay as i32);
    let (year, month) = (t.wYear as i32, t.wMonth as i32);
    let same_month = year == today.year && month == today.month;
    let last_month = if today.month == 1 { year == today.year - 1 && month == 12 } else { year == today.year && month == today.month - 1 };
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

/// The group of an entry. `value` is its value of an on-demand key (authors, tags, any
/// other column); dates group as the listing's dates do.
pub fn bucket(key: SortKey, e: &ShellEntry, today: &Today, value: Option<&SortValue>) -> Bucket {
    match key {
        SortKey::Name => name_bucket(&e.name),
        SortKey::Modified => date_bucket(e.modified, today),
        SortKey::Created => date_bucket(e.created, today),
        SortKey::Size => size_bucket(e),
        SortKey::Type => Bucket::new(0, &e.type_name),
        SortKey::Authors | SortKey::Tags | SortKey::Title | SortKey::Prop(_) => match value.filter(|v| !v.is_empty()) {
            Some(SortValue { date: true, number, .. }) => {
                // FILETIME ticks to milliseconds since 1970.
                let ms = number.map_or(0.0, |t| (t - 116_444_736_000_000_000.0) / 10_000.0);
                date_bucket(ms.max(0.0) as u64, today)
            }
            Some(v) => Bucket::new(0, &v.text),
            None => Bucket::named(-1, UNSPECIFIED),
        },
    }
}
