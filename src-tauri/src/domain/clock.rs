use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

/// Milliseconds since the Unix epoch.
pub fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default()
}

/// UTC timestamp in `YYYY-MM-DDTHH:MM:SSZ` form.
///
/// Hand-rolled instead of pulling `chrono`: the only requirement is a sortable
/// ISO-8601 string for SQLite `TEXT` columns and log fields.
pub fn now_iso8601() -> String {
    format_iso8601(unix_seconds())
}

pub fn format_iso8601(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

/// Howard Hinnant's `civil_from_days` algorithm, shifted to the 0000-03-01 era.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::format_iso8601;

    #[test]
    fn formats_known_epochs() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn handles_leap_day() {
        // 2024-02-29T12:00:00Z
        assert_eq!(format_iso8601(1_709_208_000), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn timestamps_sort_lexicographically() {
        let earlier = format_iso8601(1_700_000_000);
        let later = format_iso8601(1_700_000_001);
        assert!(earlier < later);
    }
}
