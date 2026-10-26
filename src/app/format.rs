//! Sizes, counts and dates the way Explorer prints them, in the user's locale (pt-BR:
//! "1.234 KB", "1,16 KB", "22/10/2026 20:33"; en-US: "1,234 KB", "1.16 KB", "10/22/2026 8:33 PM").

use std::sync::OnceLock;

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, DATE_SHORTDATE, LOCALE_STHOUSAND, TIME_NOSECONDS};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::{StrFormatByteSizeEx, StrFormatKBSizeW, SFBS_FLAGS_TRUNCATE_UNDISPLAYED_DECIMAL_DIGITS};

use crate::i18n::tr;

/// The locale's digit grouping separator.
fn thousands() -> &'static str {
    static SEP: OnceLock<String> = OnceLock::new();
    SEP.get_or_init(|| {
        let mut buf = [0u16; 8];
        let n = unsafe { GetLocaleInfoEx(None, LOCALE_STHOUSAND, Some(&mut buf)) };
        if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { ".".into() }
    })
}

/// Groups digits by thousands: 1234567 -> "1.234.567" (pt-BR).
pub fn integer(n: u64) -> String {
    group_digits(n, thousands())
}

fn group_digits(n: u64, sep: &str) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * sep.len());
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Size column of the details view: whole kilobytes, rounded up ("1 KB", "1.234 KB").
pub fn size_kb(bytes: u64) -> String {
    let mut buf = [0u16; 64];
    unsafe { StrFormatKBSizeW(bytes.min(i64::MAX as u64) as i64, &mut buf) };
    from_wide(&buf)
}

/// Shell "short size" with three significant digits ("1,16 KB", "33,0 GB", "237 GB").
pub fn short_size(bytes: u64) -> String {
    let mut buf = [0u16; 64];
    match unsafe { StrFormatByteSizeEx(bytes, SFBS_FLAGS_TRUNCATE_UNDISPLAYED_DECIMAL_DIGITS, &mut buf) } {
        Ok(()) => from_wide(&buf),
        Err(_) => format!("{} bytes", integer(bytes)),
    }
}

/// "1 item", "1.500 itens" (`one` and `many` are keys: "count.item", "count.items").
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", integer(n as u64), tr(if n == 1 { one } else { many }))
}

/// The locale's short date and short time in local time (with the historical DST rules,
/// like Explorer).
pub fn modified(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let ticks = ms * 10_000 + 116_444_736_000_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return String::new();
        }
    }
    let mut date = [0u16; 64];
    let mut time = [0u16; 64];
    unsafe {
        GetDateFormatEx(None, DATE_SHORTDATE, Some(&local), None, Some(&mut date), None);
        GetTimeFormatEx(None, TIME_NOSECONDS, Some(&local), None, Some(&mut time));
    }
    format!("{} {}", from_wide(&date), from_wide(&time))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping() {
        assert_eq!(group_digits(0, "."), "0");
        assert_eq!(group_digits(1500, "."), "1.500");
        assert_eq!(group_digits(1_234_567, ","), "1,234,567");
    }
}
