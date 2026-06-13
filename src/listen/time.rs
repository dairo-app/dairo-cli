//! Self-contained time helpers for `dairo listen`.
//!
//! Split out of the former monolithic `listen.rs`. These are pure functions
//! with no dependency on the listen runtime state, kept here so the orchestrator
//! module stays focused on the stream loop. `chrono` is deliberately avoided to
//! keep it out of the CLI's dependency set.

use std::time::Duration;

/// Parses a compact duration like `30s`, `15m`, `2h`, `7d` into a [`Duration`].
/// Returns `None` for malformed input or an unsupported unit.
pub(super) fn parse_duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.len() < 2 {
        return None;
    }
    let (num, unit) = value.split_at(value.len() - 1);
    let amount: u64 = num.parse().ok()?;
    let seconds = match unit {
        "s" | "S" => amount,
        "m" | "M" => amount.checked_mul(60)?,
        "h" | "H" => amount.checked_mul(3_600)?,
        "d" | "D" => amount.checked_mul(86_400)?,
        _ => return None,
    };
    Some(Duration::from_secs(seconds))
}

/// Current unix time in whole seconds (UTC), or 0 if the clock is before epoch.
pub(super) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Formats a unix timestamp as a minimal RFC3339 UTC string without pulling in
/// chrono (kept out of the CLI's dependency set). Sufficient for the `updatedAt`
/// audit field and the duration-replay lower bound.
pub(super) fn format_unix_rfc3339(unix_seconds: i64) -> String {
    // Days since epoch + seconds-of-day, civil-date conversion (Howard Hinnant's
    // algorithm). Always UTC ("Z").
    let days = unix_seconds.div_euclid(86_400);
    let secs_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Civil date from days since 1970-01-01 (Hinnant). Returns (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}
