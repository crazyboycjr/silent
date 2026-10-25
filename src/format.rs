//! How sizes, counts, dates and durations read in the cards and the strip.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gio::glib;

/// Bytes as GNOME shows them: SI units with one decimal.
pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return count(bytes, "byte", "bytes");
    }
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `1 item`, `1,203 items`.
pub fn count(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// `1,234,567`.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `24 Oct 2026, 19:03` in local time.
pub fn date(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    glib::DateTime::from_unix_local(seconds)
        .and_then(|t| t.format("%e %b %Y, %H:%M"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// `0:42`, `4:05`, `1:02:03`.
pub fn duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_gnome() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1000), "1.0 kB");
        assert_eq!(size(5_886_665), "5.9 MB");
        assert_eq!(size(999_949), "999.9 kB");
        assert_eq!(size(999_950), "1.0 MB");
        assert_eq!(size(3_400_000_000), "3.4 GB");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(1, "item", "items"), "1 item");
        assert_eq!(count(0, "item", "items"), "0 items");
        assert_eq!(count(1203, "item", "items"), "1,203 items");
        assert_eq!(count(1_234_567, "entry", "entries"), "1,234,567 entries");
    }

    #[test]
    fn durations_drop_empty_hours() {
        assert_eq!(duration(Duration::from_secs(42)), "0:42");
        assert_eq!(duration(Duration::from_secs(245)), "4:05");
        assert_eq!(duration(Duration::from_secs(3723)), "1:02:03");
    }
}
