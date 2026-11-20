//! Small helpers: time formatting and spawning external processes.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Build a `Command` that never flashes a console window on Windows.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `2026-11-20 14:03 +0900`, in the commit's own time zone.
pub fn format_time(secs: i64, offset_minutes: i32) -> String {
    let local = secs + i64::from(offset_minutes) * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let rem = local.rem_euclid(86_400);
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let off = offset_minutes.unsigned_abs();
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} {sign}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        off / 60,
        off % 60
    )
}

/// Compact relative time such as `5 min ago` or `3 days ago`.
pub fn relative_time(secs: i64, now: i64) -> String {
    let diff = now - secs;
    if diff < 60 {
        return "just now".to_owned();
    }
    let (value, unit) = match diff {
        d if d < 3_600 => (d / 60, "min"),
        d if d < 86_400 => (d / 3_600, "hour"),
        d if d < 86_400 * 30 => (d / 86_400, "day"),
        d if d < 86_400 * 365 => (d / (86_400 * 30), "month"),
        d => (d / (86_400 * 365), "year"),
    };
    let plural = if value == 1 || unit == "min" { "" } else { "s" };
    format!("{value} {unit}{plural} ago")
}

/// Parse GitHub's `2026-11-19T08:12:33Z` timestamps into Unix seconds.
pub fn parse_github_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3_600 + mi * 60 + sec)
}

// Howard Hinnant's civil calendar algorithms (proleptic Gregorian, UTC).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_commit_time_in_its_offset() {
        assert_eq!(format_time(0, 0), "1970-01-01 00:00 +0000");
        // 2023-11-14T22:13:20Z shown in KST.
        assert_eq!(format_time(1_700_000_000, 540), "2023-11-15 07:13 +0900");
        assert_eq!(format_time(1_700_000_000, -330), "2023-11-14 16:43 -0530");
    }

    #[test]
    fn parses_github_timestamps() {
        assert_eq!(parse_github_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_github_time("2023-11-14T22:13:20Z"),
            Some(1_700_000_000)
        );
        assert_eq!(
            parse_github_time("2024-02-29T12:00:00Z"),
            Some(1_709_208_000)
        );
        assert_eq!(parse_github_time("garbage"), None);
    }

    #[test]
    fn relative_times() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now - 10, now), "just now");
        assert_eq!(relative_time(now - 120, now), "2 min ago");
        assert_eq!(relative_time(now - 3_600, now), "1 hour ago");
        assert_eq!(relative_time(now - 86_400 * 3, now), "3 days ago");
        assert_eq!(relative_time(now - 86_400 * 400, now), "1 year ago");
    }
}
