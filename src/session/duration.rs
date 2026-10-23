//! Durations written the way koto's session settings and flags take them.

use std::time::Duration;

/// Parse a whole number followed by `s`, `m`, `h` or `d`, such as `90s`,
/// `12h` or `7d`. Anything else, an overflow included, is `None`.
pub fn parse_age(arg: &str) -> Option<Duration> {
    let unit = arg.chars().last()?;
    let digits = &arg[..arg.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    let secs = match unit {
        's' => n,
        'm' => n.checked_mul(60)?,
        'h' => n.checked_mul(3600)?,
        'd' => n.checked_mul(86_400)?,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}
