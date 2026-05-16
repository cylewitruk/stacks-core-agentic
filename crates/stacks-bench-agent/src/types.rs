//! Shared primitive types used across the crate.

use std::ops::Deref;
use std::str::FromStr;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors produced when constructing a [`SessionId`] from a raw string.
#[derive(Debug, Error)]
#[error("invalid session id: {0}")]
pub struct SessionIdFormatError(String);

impl From<&str> for SessionIdFormatError {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Stable identifier for one optimization session. Used as a path segment
/// (`<data>/sessions/<id>/results`), so the constructor enforces basic
/// filesystem safety: non-empty, no `/`, no NUL, no leading dots.
///
/// The bash framework uses `YYYYMMDD-HHMMSS` by convention but does not
/// enforce it; we do the same — opaque-but-safe.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// Borrow the inner string (e.g. for path joins).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Mint a fresh session id from the current local time, formatted as
    /// `YYYYMMDD-HHMMSS`. Used by `sbagent session run` when the operator
    /// did not supply `--session-id`.
    pub fn mint_now() -> Self {
        // SystemTime → ymd-hms conversion via the bash strftime style. We
        // avoid pulling chrono just for this — std + a tiny civil-time
        // calc is enough for a YYYYMMDD-HHMMSS stamp.
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self(format_ymd_hms_utc(secs))
    }
}

/// Format `unix_secs` as `YYYYMMDD-HHMMSS` in UTC. Matches the convention
/// the bash coordinator used (`date -u +%Y%m%d-%H%M%S`).
fn format_ymd_hms_utc(unix_secs: u64) -> String {
    let secs_per_day: u64 = 86_400;
    let days = unix_secs / secs_per_day;
    let s_of_day = unix_secs % secs_per_day;
    let hour = s_of_day / 3600;
    let minute = (s_of_day % 3600) / 60;
    let second = s_of_day % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// Days-since-1970-01-01 → (year, month, day) using Howard Hinnant's
/// `civil_from_days` algorithm (public-domain).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

impl TryFrom<String> for SessionId {
    type Error = SessionIdFormatError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err("session id cannot be empty".into());
        }
        if value.starts_with('.') {
            return Err("session id may not start with '.'".into());
        }
        if value.contains('/') || value.contains('\\') || value.contains('\0') {
            return Err("session id may not contain '/', '\\', or NUL".into());
        }
        Ok(SessionId(value))
    }
}

impl From<SessionId> for String {
    fn from(value: SessionId) -> String {
        value.0
    }
}

impl FromStr for SessionId {
    type Err = SessionIdFormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_owned().try_into()
    }
}

impl Deref for SessionId {
    type Target = String;

    fn deref(&self) -> &String {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_now_emits_canonical_yyyymmdd_hhmmss() {
        let id = SessionId::mint_now();
        let s = id.as_str();
        // 8 + '-' + 6 = 15.
        assert_eq!(s.len(), 15, "expected YYYYMMDD-HHMMSS shape, got {s:?}");
        assert_eq!(&s[8..9], "-");
        assert!(
            s[..8]
                .chars()
                .all(|c| c.is_ascii_digit()),
            "{s:?}"
        );
        assert!(
            s[9..]
                .chars()
                .all(|c| c.is_ascii_digit()),
            "{s:?}"
        );
    }

    #[test]
    fn format_ymd_hms_utc_known_epochs() {
        // 1970-01-01 00:00:00 UTC.
        assert_eq!(format_ymd_hms_utc(0), "19700101-000000");
        // 2026-05-09 00:00:00 UTC.
        assert_eq!(format_ymd_hms_utc(1_778_284_800), "20260509-000000");
        // 2000-02-29 23:59:59 UTC (leap day, century divisible-by-400 case).
        assert_eq!(format_ymd_hms_utc(951_868_799), "20000229-235959");
        // 2024-03-01 00:00:00 UTC (post-leap-day).
        assert_eq!(format_ymd_hms_utc(1_709_251_200), "20240301-000000");
    }
}
