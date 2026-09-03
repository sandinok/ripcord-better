//! Discord snowflake parsing.
//!
//! Layout (Discord docs, verified 2026-08-30):
//!   bits  63..42  (22 bits)  timestamp (ms since Discord Epoch 1420070400000)
//!   bits  41..37  ( 5 bits)  internal worker id
//!   bits  36..32  ( 5 bits)  internal process id
//!   bits  31.. 0  (12 bits)  increment (per-process counter)
//!
//! Discord epoch: 2015-01-01T00:00:00+00:00 → 1420070400000 ms.

use std::fmt;
use std::str::FromStr;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;

pub const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// Strong-ish wrapper around a Discord snowflake so we don't confuse it
/// with a regular u64. Serializes as a string (matching the Discord REST
/// API contract: `"id": "123456789012345678"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Snowflake(pub u64);

impl Snowflake {
    pub const fn from_u64(n: u64) -> Self { Self(n) }
    pub const fn empty() -> Self { Self(0) }

    pub fn timestamp_ms(&self) -> u64 {
        (self.0 >> 22) + DISCORD_EPOCH_MS
    }
    pub fn worker_id(&self) -> u8 { ((self.0 >> 17) & 0x1F) as u8 }
    pub fn process_id(&self) -> u8 { ((self.0 >> 12) & 0x1F) as u8 }
    pub fn increment(&self) -> u16 { (self.0 & 0xFFF) as u16 }
    pub fn to_offset_datetime(&self) -> OffsetDateTime {
        let ms = self.timestamp_ms();
        let secs = (ms / 1000) as i64;
        let nanos = ((ms % 1000) * 1_000_000) as u32;
        OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH)
            + time::Duration::nanoseconds(nanos as i64)
    }
    pub fn is_empty(&self) -> bool { self.0 == 0 }
}

impl From<u64> for Snowflake {
    fn from(n: u64) -> Self { Self(n) }
}
impl From<Snowflake> for u64 {
    fn from(s: Snowflake) -> u64 { s.0 }
}

impl fmt::Display for Snowflake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Snowflake {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(u64::from_str(s)?))
    }
}

impl<'de> Deserialize<'de> for Snowflake {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Discord sends IDs as strings to preserve precision across
        // JS/Precision loss. Accept either a string or a raw u64.
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StrOrU64 {
            S(String),
            N(u64),
        }
        match StrOrU64::deserialize(d)? {
            StrOrU64::S(s) => s
                .parse::<u64>()
                .map(Snowflake)
                .map_err(|e| D::Error::custom(format!("snowflake parse: {e}"))),
            StrOrU64::N(n) => Ok(Snowflake(n)),
        }
    }
}

impl Serialize for Snowflake {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Always serialize as a string so that REST and gateway payloads
        // match the Discord wire format exactly.
        s.serialize_str(&self.0.to_string())
    }
}

/// Marker struct so we can write `Snowflake` instead of the raw epoch.
pub struct DiscordEpoch;

impl DiscordEpoch {
    pub const fn as_ms() -> u64 { DISCORD_EPOCH_MS }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snowflake_parses_string() {
        let s: Snowflake = serde_json::from_str("\"123456789012345678\"").unwrap();
        assert_eq!(s.0, 123456789012345678);
    }
    #[test]
    fn snowflake_serializes_string() {
        let s = Snowflake::from_u64(123456789012345678);
        let out = serde_json::to_string(&s).unwrap();
        assert_eq!(out, "\"123456789012345678\"");
    }
    #[test]
    fn snowflake_decompresses_timestamp() {
        // Snowflake 100000000000000000 ≈ 2015-01-04 06:13:20 UTC
        let s = Snowflake::from_u64(100_000_000_000_000_000);
        let dt = s.to_offset_datetime();
        assert_eq!(dt.year(), 2015);
    }
}
