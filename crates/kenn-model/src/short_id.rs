//! Language-partitioned `short_id` encoding.
//!
//! A [`ShortId`] is a `u32` split into two fields: the high
//! [`PARTITION_BITS`] hold a stable per-language partition index and the
//! low [`COUNTER_BITS`] hold a per-language 1-based counter. Each ingester
//! owns one language partition and interns into it independently, so no
//! run-global registry is needed and ids never collide across languages.
//!
//! `0` stays the reserved "no reference" sentinel: counters are 1-based,
//! so [`compose`] never yields `0`.

use crate::{language::Language, record::ShortId};

/// Bits reserved for the language partition in the high end of a `short_id`.
/// Four bits give 16 partitions — ample for the closed [`Language`] enum.
pub const PARTITION_BITS: u32 = 4;

/// Bits available to the per-language counter (the low end of a `short_id`).
pub const COUNTER_BITS: u32 = ShortId::BITS - PARTITION_BITS;

/// Largest per-language counter value (`2^COUNTER_BITS - 1`).
pub const MAX_COUNTER: u32 = (1 << COUNTER_BITS) - 1;

impl Language {
    /// Stable partition index occupying the high [`PARTITION_BITS`] of a
    /// `short_id`. Explicit so reordering the enum can never silently
    /// remap existing ids.
    #[must_use]
    pub const fn partition(self) -> u32 {
        match self {
            Self::Csharp => 0,
            Self::TypeScript => 1,
            Self::Rust => 2,
            Self::Go => 3,
            Self::Python => 4,
            Self::Markdown => 5,
            Self::Css => 6,
            Self::Sass => 7,
            Self::Html => 8,
            Self::Swift => 9,
            Self::Text => 10,
            // Appended, never inserted: a partition index is load-bearing for
            // every id already on disk. 13 of 16 partitions are now spoken for.
            Self::Sql => 11,
            Self::Xml => 12,
        }
    }
}

/// Compose a partitioned `short_id` from a language and a 1-based
/// per-language `counter`.
///
/// # Panics
/// Panics when `counter` exceeds [`MAX_COUNTER`] — the partition is full.
#[must_use]
pub fn compose(language: Language, counter: u32) -> ShortId {
    assert!(
        counter <= MAX_COUNTER,
        "short_id counter overflow in partition"
    );
    (language.partition() << COUNTER_BITS) | counter
}

/// The language partition index a `short_id` belongs to.
#[must_use]
pub fn partition_of(id: ShortId) -> u32 {
    id >> COUNTER_BITS
}

/// The per-language counter component of a `short_id`.
#[must_use]
pub fn counter_of(id: ShortId) -> ShortId {
    id & MAX_COUNTER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_are_distinct() {
        let langs = [
            Language::Csharp,
            Language::TypeScript,
            Language::Rust,
            Language::Go,
            Language::Python,
        ];
        for (i, a) in langs.iter().enumerate() {
            for b in &langs[i + 1..] {
                assert_ne!(a.partition(), b.partition());
            }
        }
    }

    #[test]
    fn compose_inspect_round_trip() {
        for lang in [Language::Csharp, Language::Rust, Language::Python] {
            for counter in [1, 2, 1000, MAX_COUNTER] {
                let id = compose(lang, counter);
                assert_eq!(partition_of(id), lang.partition());
                assert_eq!(counter_of(id), counter);
            }
        }
    }

    #[test]
    fn compose_never_yields_the_zero_sentinel() {
        for lang in [Language::Csharp, Language::TypeScript, Language::Rust] {
            assert_ne!(compose(lang, 1), 0);
        }
    }

    #[test]
    fn distinct_languages_never_collide() {
        assert_ne!(compose(Language::Rust, 1), compose(Language::Go, 1));
        assert_ne!(compose(Language::Csharp, 5), compose(Language::Python, 5));
    }

    #[test]
    #[should_panic(expected = "short_id counter overflow")]
    fn counter_overflow_panics() {
        let _id = compose(Language::Rust, MAX_COUNTER + 1);
    }
}
