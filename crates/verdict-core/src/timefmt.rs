//! Minimal UTC timestamp formatting (RFC 3339, millisecond precision) without extra crates.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time as `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC).
pub fn now_rfc3339() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_unix_millis(d.as_millis() as i64)
}

/// Format milliseconds since the Unix epoch as RFC 3339 UTC.
pub fn format_unix_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_unix_millis(0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29T12:34:56.789Z
        assert_eq!(
            format_unix_millis(1_709_210_096_789),
            "2024-02-29T12:34:56.789Z"
        );
    }
}
