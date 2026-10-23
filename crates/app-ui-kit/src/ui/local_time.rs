//! Unix seconds as the user's wall clock, for lists that say when things
//! happened.

use time::{Date, OffsetDateTime, UtcOffset};

/// `secs` in the local time zone, with the offset in force at that moment so
/// a daylight-saving change between then and now is respected. Where the
/// platform cannot say (Linux once threads run), UTC.
fn local(secs: i64) -> OffsetDateTime {
    let utc = OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let offset = UtcOffset::local_offset_at(utc).unwrap_or(UtcOffset::UTC);
    utc.to_offset(offset)
}

/// The local calendar day of `secs`.
pub fn day(secs: i64) -> Date {
    local(secs).date()
}

/// `YYYY-MM-DD` of `secs`, local.
pub fn date_text(secs: i64) -> String {
    let date = day(secs);
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// `HH:MM` of `secs`, local.
pub fn clock_text(secs: i64) -> String {
    let time = local(secs);
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// `HH:MM:SS` of `secs`, local.
pub fn clock_text_seconds(secs: i64) -> String {
    let time = local(secs);
    format!(
        "{:02}:{:02}:{:02}",
        time.hour(),
        time.minute(),
        time.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_one_of_two_local_days() {
        // No zone is more than a day from UTC.
        let text = date_text(0);
        assert!(text == "1970-01-01" || text == "1969-12-31", "{text}");
        assert_eq!(clock_text(0).len(), 5);
        assert_eq!(clock_text_seconds(0).len(), 8);
    }
}
