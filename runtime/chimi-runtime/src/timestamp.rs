//! Timestamps: Unix nanoseconds in an `i64`, read and written as RFC 3339 UTC
//! text by the proleptic Gregorian calendar (Howard Hinnant's day
//! algorithms). The native runtime prints `Clock.timestamp_to_text` and the
//! `Debug` form with [`text`] and reads `--clock-start` with [`parse`]; the
//! compiler's test runner checks `--clock-start` with the same [`parse`].

use crate::TextBuffer;

/// `2262-04-11T23:47:16.854775807Z` is the longest text.
pub const TEXT_BYTES: usize = 32;
pub type TimestampText = TextBuffer<TEXT_BYTES>;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const SECONDS_PER_DAY: i64 = 86_400;

/// A calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: i64,
    pub day: i64,
}

/// Days since 1970-01-01 of a date with `month` in 1..=12.
pub fn days_from_civil(date: Civil) -> i64 {
    let year = if date.month <= 2 { date.year - 1 } else { date.year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (date.month + if date.month > 2 { -3 } else { 9 }) + 2) / 5 + date.day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01, for any `days` an `i64` of nanoseconds
/// can name.
pub fn civil_from_days(days: i64) -> Civil {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    Civil { year: year_of_era + era * 400 + i64::from(month <= 2), month, day }
}

/// The UTC date and the seconds into that day of `seconds` since the epoch.
pub fn civil_seconds(seconds: i64) -> (Civil, i64) {
    (civil_from_days(seconds.div_euclid(SECONDS_PER_DAY)), seconds.rem_euclid(SECONDS_PER_DAY))
}

/// Writes `2026-11-10T09:30:00`, the date and time of day without a fraction
/// or zone.
pub fn push_date_time(text: &mut TimestampText, date: Civil, of_day: i64) {
    let digits = |value: i64| u128::try_from(value).unwrap_or(0);
    text.push_digits(digits(date.year), 4);
    text.push(b'-');
    text.push_digits(digits(date.month), 2);
    text.push(b'-');
    text.push_digits(digits(date.day), 2);
    text.push(b'T');
    text.push_digits(digits(of_day / 3600), 2);
    text.push(b':');
    text.push_digits(digits(of_day / 60 % 60), 2);
    text.push(b':');
    text.push_digits(digits(of_day % 60), 2);
}

/// `nanoseconds` as `2026-11-10T09:30:00.25Z`: the fraction loses its
/// trailing zeros and is left out when it is zero.
pub fn text(nanoseconds: i64) -> TimestampText {
    let (date, of_day) = civil_seconds(nanoseconds.div_euclid(NANOS_PER_SECOND));
    let mut fraction = nanoseconds.rem_euclid(NANOS_PER_SECOND);
    let mut out = TimestampText::new();
    push_date_time(&mut out, date, of_day);
    if fraction != 0 {
        let mut width = 9;
        while fraction % 10 == 0 {
            fraction /= 10;
            width -= 1;
        }
        out.push(b'.');
        out.push_digits(u128::try_from(fraction).unwrap_or(0), width);
    }
    out.push(b'Z');
    out
}

/// Parses an RFC 3339 date-time into Unix nanoseconds: `T` and `Z` in either
/// case, a fraction of one to nine digits, and `Z` or a `+HH:MM`/`-HH:MM`
/// offset. Leap second 60, years outside 1678 through 2262 and results that
/// overflow an `i64` are rejected.
pub fn parse(text: &[u8]) -> Option<i64> {
    let mut cursor = Cursor { text, at: 0 };
    let year = cursor.digits(4)?;
    cursor.expect(b"-")?;
    let month = cursor.digits(2)?;
    cursor.expect(b"-")?;
    let day = cursor.digits(2)?;
    cursor.expect(b"Tt")?;
    let hour = cursor.digits(2)?;
    cursor.expect(b":")?;
    let minute = cursor.digits(2)?;
    cursor.expect(b":")?;
    let second = cursor.digits(2)?;
    let mut fraction = 0;
    if cursor.peek() == Some(b'.') {
        cursor.at += 1;
        let mut count = 0;
        while let Some(digit) = cursor.peek().filter(u8::is_ascii_digit) {
            count += 1;
            if count > 9 {
                return None;
            }
            fraction = fraction * 10 + i64::from(digit - b'0');
            cursor.at += 1;
        }
        if count == 0 {
            return None;
        }
        for _ in count..9 {
            fraction *= 10;
        }
    }
    let offset = match cursor.peek()? {
        b'Z' | b'z' => {
            cursor.at += 1;
            0
        }
        sign @ (b'+' | b'-') => {
            cursor.at += 1;
            let hours = cursor.digits(2)?;
            cursor.expect(b":")?;
            let minutes = cursor.digits(2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let magnitude = hours * 3600 + minutes * 60;
            if sign == b'-' { -magnitude } else { magnitude }
        }
        _ => return None,
    };
    if cursor.at != text.len() {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days: i64 = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    if !(1678..=2262).contains(&year) || day < 1 || day > month_days || hour > 23 || minute > 59 || second > 59
    {
        return None;
    }
    let seconds = days_from_civil(Civil { year, month, day }) * SECONDS_PER_DAY + hour * 3600 + minute * 60
        + second
        - offset;
    seconds.checked_mul(NANOS_PER_SECOND)?.checked_add(fraction)
}

struct Cursor<'a> {
    text: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    /// Exactly `count` ASCII digits.
    fn digits(&mut self, count: usize) -> Option<i64> {
        let mut value = 0;
        for _ in 0..count {
            let digit = self.peek().filter(u8::is_ascii_digit)?;
            value = value * 10 + i64::from(digit - b'0');
            self.at += 1;
        }
        Some(value)
    }

    /// One byte from `allowed`.
    fn expect(&mut self, allowed: &[u8]) -> Option<()> {
        self.peek().filter(|byte| allowed.contains(byte))?;
        self.at += 1;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(nanoseconds: i64) -> String {
        text(nanoseconds).as_str().to_owned()
    }

    #[test]
    fn text_follows_the_calendar_and_trims_the_fraction() {
        assert_eq!(text_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(text_of(951_782_400_000_000_007), "2000-02-29T00:00:00.000000007Z");
        assert_eq!(text_of(1_795_327_200_250_000_000), "2026-11-22T06:00:00.25Z");
        assert_eq!(text_of(-1), "1969-12-31T23:59:59.999999999Z");
        assert_eq!(text_of(i64::MAX), "2262-04-11T23:47:16.854775807Z");
        assert_eq!(text_of(i64::MIN), "1677-09-21T00:12:43.145224192Z");
    }

    #[test]
    fn parse_reads_offsets_fractions_and_either_case() {
        assert_eq!(parse(b"1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse(b"2026-11-22t06:00:00.25z"), Some(1_795_327_200_250_000_000));
        assert_eq!(parse(b"2026-11-22T07:30:00+01:30"), Some(1_795_327_200_000_000_000));
        assert_eq!(parse(b"2026-11-22T04:00:00-02:00"), Some(1_795_327_200_000_000_000));
        assert_eq!(parse(b"2000-02-29T00:00:00.000000007Z"), Some(951_782_400_000_000_007));
        assert_eq!(parse(b"2262-04-11T23:47:16.854775807Z"), Some(i64::MAX));
    }

    #[test]
    fn parse_rejects_what_the_runtime_rejects() {
        for bad in [
            "",
            "1970-01-01T00:00:00",
            "1970-01-01 00:00:00Z",
            "1970-01-01T00:00:60Z",
            "1970-01-01T24:00:00Z",
            "1970-13-01T00:00:00Z",
            "1970-00-01T00:00:00Z",
            "1970-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "1970-04-31T00:00:00Z",
            "1970-01-01T00:00:00.Z",
            "1970-01-01T00:00:00.1234567890Z",
            "1970-01-01T00:00:00+24:00",
            "1970-01-01T00:00:00+01:60",
            "1970-01-01T00:00:00Zx",
            "1677-12-31T00:00:00Z",
            "2263-01-01T00:00:00Z",
            "2262-04-11T23:47:16.854775808Z",
            "+970-01-01T00:00:00Z",
        ] {
            assert_eq!(parse(bad.as_bytes()), None, "{bad} parsed");
        }
        assert_eq!(parse(b"2000-02-29T00:00:00Z"), Some(951_782_400_000_000_000));
    }

    #[test]
    fn days_round_trip_across_eras() {
        for days in (-150_000..150_000).step_by(37) {
            assert_eq!(days_from_civil(civil_from_days(days)), days);
        }
        assert_eq!(civil_from_days(0), Civil { year: 1970, month: 1, day: 1 });
        assert_eq!(civil_from_days(-1), Civil { year: 1969, month: 12, day: 31 });
    }
}
