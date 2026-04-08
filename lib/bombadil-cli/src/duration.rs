use std::time::Duration;

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (digits, suffix) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| s.split_at(i))
        .ok_or_else(|| {
            format!("missing unit suffix in {:?} (expected s, m, h, or d)", s)
        })?;

    if digits.is_empty() {
        return Err(format!(
            "missing number before unit in {:?} (expected e.g. 30s, 5m, 2h, 1d)",
            s
        ));
    }

    let value: u64 = digits
        .parse()
        .map_err(|_| format!("invalid number in {:?}", s))?;

    let seconds = match suffix {
        "s" => Some(value),
        "m" => value.checked_mul(60),
        "h" => value.checked_mul(3600),
        "d" => value.checked_mul(86400),
        _ => {
            return Err(format!(
                "unknown unit {:?} (expected s, m, h, or d)",
                suffix
            ));
        }
    }
    .ok_or_else(|| "duration value too large".to_string())?;

    if seconds == 0 {
        return Err("duration must be greater than zero".to_string());
    }
    Ok(Duration::from_secs(seconds))
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs == 0 {
        return "0s".to_string();
    }
    if secs.is_multiple_of(86400) {
        format!("{}d", secs / 86400)
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn roundtrip(secs in 1u64..=100_000_000u64) {
            let d = Duration::from_secs(secs);
            let formatted = format_duration(d);
            let parsed = parse_duration(&formatted).unwrap();
            prop_assert_eq!(parsed, d);
        }
    }

    #[test]
    fn rejects_compound_duration() {
        assert!(parse_duration("1h30m").is_err());
    }

    #[test]
    fn rejects_zero() {
        let err = parse_duration("0s").unwrap_err();
        assert_eq!(err, "duration must be greater than zero");
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(parse_duration("abc").is_err());
    }
}
