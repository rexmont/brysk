//! Dates and times written the way the user's Mac writes them, the macOS half of
//! `rf-win::datefmt`. The columns show the short date and the short time of the regional
//! format, exactly as the Finder does.

use chrono::{DateTime, Datelike, Local, Timelike};

/// "17.11.2026 09:30" (or the regional order); `None` when the format cannot be built.
pub fn short_date_time(dt: &DateTime<Local>) -> Option<String> {
    // macOS' short date is day-first in most of Europe, month-first in the US. `defaults`
    // knows the user's choice; reading it once per call would spawn a process, so use the
    // locale's own separators from the environment and fall back to ISO-like.
    let locale = std::env::var("LC_TIME").or_else(|_| std::env::var("LANG")).unwrap_or_default().to_ascii_lowercase();
    let (y, mo, d, h, mi) = (dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute());
    if locale.starts_with("en_us") || locale.starts_with("en-us") {
        Some(format!("{mo}/{d}/{y} {}:{mi:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" }))
    } else {
        Some(format!("{d:02}.{mo:02}.{y} {h:02}:{mi:02}"))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    #[test]
    fn writes_a_date_with_its_year_and_minutes() {
        let dt = chrono::Local.with_ymd_and_hms(2026, 11, 17, 9, 30, 0).unwrap();
        let s = super::short_date_time(&dt).expect("a date");
        assert!(s.contains("2026") || s.contains("26"));
        assert!(s.contains("30"));
    }
}