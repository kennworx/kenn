//! Allocating short ids for tables minted after the `.sql` pass.
//!
//! A barrier step that mints an external table needs an id in the `Sql`
//! partition that the `.sql` producer has not already used. More than one step
//! mints — code literals and SQL carried in markup both do — and they run in
//! the same pipeline against the same partition.
//!
//! **That is why this is a shared allocator rather than a rule.** Two steps
//! each computing "one past the high-water mark" from the store both compute
//! the *same* number, and hand two different tables the same id. Nothing
//! downstream would report it: one symbol simply overwrites the other, and the
//! edges of the loser silently retarget. Passing one allocator through every
//! minting step makes the collision unrepresentable instead of merely unlikely.

use kenn_model::{compose_short_id, counter_of, partition_of, Language, ShortId};

/// Hands out unused `Sql`-partition short ids, continuing past whatever the
/// `.sql` producer already wrote.
#[derive(Debug)]
pub struct TableMinter {
    next: u32,
}

impl TableMinter {
    /// Start after the highest `Sql`-partition counter among `existing`.
    ///
    /// Ids from other partitions are ignored rather than trusted to be absent:
    /// the caller scans symbols, and a scan that later stops filtering by
    /// language would otherwise push this allocator into another partition's
    /// range.
    #[must_use]
    pub fn after_existing(existing: impl IntoIterator<Item = ShortId>) -> Self {
        let sql = partition_of(compose_short_id(Language::Sql, 1));
        let high = existing
            .into_iter()
            .filter(|id| partition_of(*id) == sql)
            .map(counter_of)
            .max()
            .unwrap_or(0);
        Self {
            next: high.saturating_add(1),
        }
    }

    /// The next unused id. Never returns the same value twice.
    pub fn mint(&mut self) -> ShortId {
        let id = compose_short_id(Language::Sql, self.next);
        self.next = self.next.saturating_add(1);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn minting_starts_after_the_highest_existing_counter() {
        let existing = [
            compose_short_id(Language::Sql, 1),
            compose_short_id(Language::Sql, 7),
            compose_short_id(Language::Sql, 3),
        ];
        let mut m = TableMinter::after_existing(existing);
        assert_eq!(counter_of(m.mint()), 8);
    }

    #[test]
    fn an_empty_workspace_starts_at_one() {
        let mut m = TableMinter::after_existing([]);
        assert_eq!(counter_of(m.mint()), 1);
    }

    #[test]
    fn another_partitions_ids_do_not_move_the_mark() {
        // A scan that stops filtering by language must not push this allocator
        // into another partition's range.
        let existing = [
            compose_short_id(Language::Sql, 2),
            compose_short_id(Language::Rust, 9000),
        ];
        let mut m = TableMinter::after_existing(existing);
        assert_eq!(counter_of(m.mint()), 3);
    }

    #[test]
    fn one_allocator_shared_by_two_steps_never_repeats() {
        // The hazard this type exists for. Two barrier steps mint into the same
        // partition; sharing one allocator is what makes a collision
        // unrepresentable rather than merely unlikely.
        let mut m = TableMinter::after_existing([compose_short_id(Language::Sql, 5)]);
        let step_one: Vec<ShortId> = (0..3).map(|_| m.mint()).collect();
        let step_two: Vec<ShortId> = (0..3).map(|_| m.mint()).collect();

        let all: Vec<ShortId> = step_one.iter().chain(&step_two).copied().collect();
        let unique: HashSet<ShortId> = all.iter().copied().collect();
        assert_eq!(unique.len(), all.len(), "no id handed out twice: {all:?}");
    }

    #[test]
    fn two_independent_allocators_would_collide() {
        // Not a guard on this type — a demonstration of what it prevents, so
        // the reason survives someone deciding a second allocator is simpler.
        let existing = [compose_short_id(Language::Sql, 5)];
        let mut a = TableMinter::after_existing(existing);
        let mut b = TableMinter::after_existing(existing);
        assert_eq!(
            a.mint(),
            b.mint(),
            "independently-built allocators start at the same counter — \
             which is exactly why the pipeline must pass one through"
        );
    }

    #[test]
    fn every_minted_id_is_in_the_sql_partition() {
        let sql = partition_of(compose_short_id(Language::Sql, 1));
        let mut m = TableMinter::after_existing([]);
        for _ in 0..5 {
            assert_eq!(partition_of(m.mint()), sql);
        }
    }
}
