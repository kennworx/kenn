//! `Timestamp` — a UTC instant that (de)serializes as an RFC 3339 (ISO 8601)
//! string, so committed `.md` record and `.anchor.jsonl` times are
//! human- and agent-readable. Being `Ord`, it folds directly in liveness math
//! without any manual string parsing at the call sites.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// A UTC timestamp, serialized as an RFC 3339 string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// The current UTC instant.
    #[must_use]
    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    /// This instant as Unix seconds — for interval math (e.g. liveness decay).
    #[must_use]
    pub fn unix(self) -> i64 {
        self.0.unix_timestamp()
    }
}

impl From<OffsetDateTime> for Timestamp {
    fn from(dt: OffsetDateTime) -> Self {
        Self(dt)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.format(&Rfc3339) {
            Ok(s) => f.write_str(&s),
            // Unreachable for a valid `OffsetDateTime`; never panic in Display.
            Err(_) => write!(f, "{:?}", self.0),
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        OffsetDateTime::parse(&s, &Rfc3339)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
