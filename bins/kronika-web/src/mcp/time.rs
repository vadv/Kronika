//! Strict public MCP timestamp expressions.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::time::TimeRange;

const VALID_FORMS: &str = "a JSON integer Unix timestamp in microseconds, RFC 3339 with Z or a numeric UTC offset, now, or now-N{us,ms,s,m,h,d,w}";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum TimeSpecInput {
    /// Exact Unix timestamp in microseconds.
    UnixMicros(i64),
    /// RFC 3339, `now`, or a fixed-duration expression such as `now-1h`.
    Expression(String),
}

impl TimeSpecInput {
    pub(crate) fn resolve(&self, now: i64) -> Result<i64, TimeSpecError> {
        match self {
            Self::UnixMicros(timestamp) => Ok(*timestamp),
            Self::Expression(expression) => resolve_expression(expression, now),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimeSpecError(String);

impl std::fmt::Display for TimeSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; expected {VALID_FORMS}", self.0)
    }
}

impl std::error::Error for TimeSpecError {}

pub(crate) fn resolve_range(
    from: &TimeSpecInput,
    to: &TimeSpecInput,
) -> Result<TimeRange, TimeSpecError> {
    resolve_range_with(from, to, system_now_micros)
}

fn resolve_range_with(
    from: &TimeSpecInput,
    to: &TimeSpecInput,
    clock: impl FnOnce() -> Result<i64, TimeSpecError>,
) -> Result<TimeRange, TimeSpecError> {
    let now = clock()?;
    let from = from.resolve(now)?;
    let to = to.resolve(now)?;
    TimeRange::new(from, to).map_err(|error| TimeSpecError(error.to_string()))
}

fn resolve_expression(expression: &str, now: i64) -> Result<i64, TimeSpecError> {
    if expression == "now" {
        return Ok(now);
    }
    if let Some(relative) = expression.strip_prefix("now-") {
        return resolve_relative(relative, now);
    }
    resolve_rfc3339(expression)
}

fn resolve_relative(relative: &str, now: i64) -> Result<i64, TimeSpecError> {
    let (digits, multiplier) = [
        ("us", 1_i64),
        ("ms", 1_000),
        ("s", 1_000_000),
        ("m", 60_000_000),
        ("h", 3_600_000_000),
        ("d", 86_400_000_000),
        ("w", 604_800_000_000),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| {
        relative
            .strip_suffix(suffix)
            .map(|digits| (digits, multiplier))
    })
    .ok_or_else(|| TimeSpecError(format!("invalid timestamp expression {relative:?}")))?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TimeSpecError(format!(
            "invalid relative timestamp {relative:?}"
        )));
    }
    let count = digits
        .parse::<i64>()
        .map_err(|_error| TimeSpecError(format!("relative timestamp {relative:?} overflows")))?;
    let duration = count
        .checked_mul(multiplier)
        .ok_or_else(|| TimeSpecError(format!("relative timestamp {relative:?} overflows")))?;
    now.checked_sub(duration)
        .ok_or_else(|| TimeSpecError(format!("relative timestamp {relative:?} overflows")))
}

fn resolve_rfc3339(expression: &str) -> Result<i64, TimeSpecError> {
    let has_numeric_offset = expression.len() >= 6
        && matches!(expression.as_bytes()[expression.len() - 6], b'+' | b'-')
        && expression.as_bytes()[expression.len() - 3] == b':';
    if !expression.ends_with('Z') && !has_numeric_offset {
        return Err(TimeSpecError(format!(
            "invalid timestamp expression {expression:?}"
        )));
    }
    let parsed = DateTime::parse_from_rfc3339(expression)
        .map_err(|_error| TimeSpecError(format!("invalid timestamp expression {expression:?}")))?;
    if parsed.timestamp_subsec_nanos() % 1_000 != 0 {
        return Err(TimeSpecError(format!(
            "timestamp {expression:?} is not exactly representable in microseconds"
        )));
    }
    Ok(parsed.timestamp_micros())
}

fn system_now_micros() -> Result<i64, TimeSpecError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_error| TimeSpecError("system clock precedes the Unix epoch".to_owned()))?;
    let micros = elapsed.as_micros();
    i64::try_from(micros).map_err(|_error| TimeSpecError("system clock overflows i64".to_owned()))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{TimeSpecInput, resolve_range_with};

    fn expression(value: &str) -> TimeSpecInput {
        TimeSpecInput::Expression(value.to_owned())
    }

    #[test]
    fn resolves_integer_rfc3339_and_relative_forms() {
        let now = 1_800_000_000_000_000_i64;
        assert_eq!(
            TimeSpecInput::UnixMicros(i64::MIN).resolve(now),
            Ok(i64::MIN)
        );
        assert_eq!(
            TimeSpecInput::UnixMicros(i64::MAX).resolve(now),
            Ok(i64::MAX)
        );
        assert_eq!(expression("now").resolve(now), Ok(now));
        assert_eq!(expression("now-2us").resolve(now), Ok(now - 2));
        assert_eq!(expression("now-2ms").resolve(now), Ok(now - 2_000));
        assert_eq!(expression("now-2s").resolve(now), Ok(now - 2_000_000));
        assert_eq!(expression("now-2m").resolve(now), Ok(now - 120_000_000));
        assert_eq!(expression("now-2h").resolve(now), Ok(now - 7_200_000_000));
        assert_eq!(expression("now-2d").resolve(now), Ok(now - 172_800_000_000));
        assert_eq!(
            expression("now-2w").resolve(now),
            Ok(now - 1_209_600_000_000)
        );
        assert_eq!(
            expression("1970-01-01T00:00:01.123456Z").resolve(now),
            Ok(1_123_456)
        );
        assert_eq!(
            expression("1970-01-01T01:00:01+01:00").resolve(now),
            Ok(1_000_000)
        );
        assert_eq!(
            expression("1969-12-31T23:00:01-01:00").resolve(now),
            Ok(1_000_000)
        );
        assert_eq!(
            expression("1970-01-01T00:00:01.123456000Z").resolve(now),
            Ok(1_123_456)
        );
    }

    #[test]
    fn rejects_every_unlisted_form_and_overflow() {
        for invalid in [
            "1",
            "  now",
            "now ",
            "now-1month",
            "now-1y",
            "now--1s",
            "now-+1s",
            "now-1.5s",
            "2026-08-29",
            "2026-08-29T12:00:00",
            "tomorrow",
            "1970-01-01T00:00:01.123456001Z",
            "1970-01-01T00:00:01z",
        ] {
            assert!(
                expression(invalid).resolve(0).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(expression("now-9223372036854775807w").resolve(0).is_err());
        assert!(expression("now-1us").resolve(i64::MIN).is_err());
        assert!(serde_json::from_str::<TimeSpecInput>("1.0").is_err());
        assert!(serde_json::from_str::<TimeSpecInput>("1e3").is_err());
    }

    #[test]
    fn one_clock_anchor_resolves_both_bounds() {
        let calls = Cell::new(0_u8);
        let range = resolve_range_with(&expression("now-1h"), &expression("now"), || {
            calls.set(calls.get() + 1);
            Ok(9_000_000_000)
        })
        .expect("range");
        assert_eq!(calls.get(), 1);
        assert_eq!(range.from, 5_400_000_000);
        assert_eq!(range.to_exclusive, 9_000_000_000);
        assert!(resolve_range_with(&expression("now"), &expression("now-1us"), || Ok(7)).is_err());
        assert!(resolve_range_with(&expression("now"), &expression("now"), || Ok(7)).is_ok());
    }
}
