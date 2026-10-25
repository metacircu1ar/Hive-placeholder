//! Deliberately small syntax checks for the supported elicitation formats.
// A URI escape is a percent sign followed by two hexadecimal digits.
const PERCENT_ESCAPE_BYTES: usize = 3;
const DECIMAL_RADIX: u32 = 10;
// Fixed RFC 3339 date/time field positions; separators occupy the intervening bytes.
const YEAR_END: usize = 4;
const MONTH_START: usize = YEAR_END + 1;
const MONTH_END: usize = MONTH_START + 2;
const DAY_START: usize = MONTH_END + 1;
const DATE_BYTES: usize = DAY_START + 2;
const HOUR_START: usize = DATE_BYTES + 1;
const HOUR_END: usize = HOUR_START + 2;
const MINUTE_START: usize = HOUR_END + 1;
const MINUTE_END: usize = MINUTE_START + 2;
const SECOND_START: usize = MINUTE_END + 1;
const SECOND_END: usize = SECOND_START + 2;
// The shortest accepted timestamp has a one-byte UTC suffix after its seconds.
const MIN_TIMESTAMP_BYTES: usize = SECOND_END + 1;
const MAX_HOUR: u32 = 23;
const MAX_MINUTE_SECOND: u32 = 59;
// Numeric offsets use sign, two hour digits, colon, and two minute digits.
const OFFSET_BYTES: usize = 6;
const OFFSET_COLON: usize = 3;

pub(super) fn valid(kind: &str, value: &str) -> bool {
    match kind {
        "email" => value.contains('@'),
        "uri" => uri(value),
        "date" => date(value),
        "date-time" => date_time(value),
        _ => false,
    }
}

fn uri(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    scheme
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|^`\\".contains(c))
        && value.as_bytes().iter().enumerate().all(|(i, b)| {
            *b != b'%'
                || value
                    .as_bytes()
                    .get(i + 1..i + PERCENT_ESCAPE_BYTES)
                    .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        })
}

fn digits(value: &[u8]) -> Option<u32> {
    value.iter().try_fold(0, |total, c| {
        c.is_ascii_digit()
            .then(|| total * DECIMAL_RADIX + u32::from(c - b'0'))
    })
}

fn date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != DATE_BYTES || b[YEAR_END] != b'-' || b[MONTH_END] != b'-' {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (
        digits(&b[..YEAR_END]),
        digits(&b[MONTH_START..MONTH_END]),
        digits(&b[DAY_START..]),
    ) else {
        return false;
    };
    // Gregorian month numbers, month lengths, and the 4/100/400-year leap rule.
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn date_time(value: &str) -> bool {
    if !value.is_ascii() || value.len() < MIN_TIMESTAMP_BYTES || !date(&value[..DATE_BYTES]) {
        return false;
    }
    let b = value.as_bytes();
    if !matches!(b[DATE_BYTES], b'T' | b't') || b[HOUR_END] != b':' || b[MINUTE_END] != b':' {
        return false;
    }
    if digits(&b[HOUR_START..HOUR_END]).is_none_or(|n| n > MAX_HOUR)
        || digits(&b[MINUTE_START..MINUTE_END]).is_none_or(|n| n > MAX_MINUTE_SECOND)
        || digits(&b[SECOND_START..SECOND_END]).is_none_or(|n| n > MAX_MINUTE_SECOND)
    {
        return false;
    }
    let mut tail = &b[SECOND_END..];
    if tail.first() == Some(&b'.') {
        let count = tail[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if count == 0 {
            return false;
        }
        tail = &tail[count + 1..];
    }
    matches!(tail, [b'Z' | b'z'])
        || (tail.len() == OFFSET_BYTES
            && matches!(tail[0], b'+' | b'-')
            && tail[OFFSET_COLON] == b':'
            && digits(&tail[1..OFFSET_COLON]).is_some_and(|n| n <= MAX_HOUR)
            && digits(&tail[OFFSET_COLON + 1..]).is_some_and(|n| n <= MAX_MINUTE_SECOND))
}
