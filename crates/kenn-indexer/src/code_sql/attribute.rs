//! Placing a literal against the symbols whose bodies contain it.
//!
//! **Pure.** Extents in, one owner out.
//!
//! Body extents **nest**: a module's span contains its functions', a class's
//! contains its methods'. So "which symbols contain this line" is the wrong
//! question — it answers *all of them*, and attributing to each gives every
//! enclosing scope the tables its children touch. Measured on a self-index,
//! that gave the `gc` module the full table set of the `Store::gc` function
//! inside it, and at scale it degrades "this function reads `sessions`" into
//! "this crate reads everything", which is no answer at all.
//!
//! The right question is which containing extent is *smallest*.

/// One symbol's extent, as far as attribution cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub sym_id: kenn_model::ShortId,
    /// 1-based, inclusive.
    pub start: u32,
    /// 1-based, inclusive.
    pub end: u32,
}

impl Extent {
    const fn contains(self, line: u32) -> bool {
        self.start <= line && line <= self.end
    }

    /// Line count. Ties are impossible to break meaningfully, so a stable tie
    /// break keeps the output deterministic across runs.
    const fn span(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// The symbol that owns `line`: the one whose containing extent is smallest.
///
/// `None` when no extent contains it. A literal outside every recorded body has
/// no symbol to attribute it to, and falling back to its file would reintroduce
/// the same collapse one level up — a file-level owner accumulating every table
/// its symbols touch.
#[must_use]
pub fn owner(extents: &[Extent], line: u32) -> Option<kenn_model::ShortId> {
    extents
        .iter()
        .filter(|e| e.contains(line))
        .min_by_key(|e| (e.span(), e.sym_id))
        .map(|e| e.sym_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(id: u32, start: u32, end: u32) -> Extent {
        Extent {
            sym_id: id,
            start,
            end,
        }
    }

    #[test]
    fn an_enclosing_scope_does_not_win_over_the_function_inside_it() {
        // The measured failure: the module's extent contains the function's, so
        // "every containing symbol" hands the module its child's tables.
        let module = ext(1, 1, 100);
        let function = ext(2, 10, 20);
        assert_eq!(owner(&[module, function], 15), Some(2));
    }

    #[test]
    fn the_nearest_of_three_nested_scopes_wins() {
        let module = ext(1, 1, 100);
        let class = ext(2, 5, 60);
        let method = ext(3, 20, 30);
        assert_eq!(owner(&[module, class, method], 25), Some(3));
    }

    #[test]
    fn a_line_outside_every_extent_has_no_owner() {
        assert_eq!(owner(&[ext(1, 10, 20)], 5), None);
        assert_eq!(owner(&[ext(1, 10, 20)], 21), None);
    }

    #[test]
    fn extent_bounds_are_inclusive() {
        let e = [ext(1, 10, 20)];
        assert_eq!(owner(&e, 10), Some(1), "first line is inside");
        assert_eq!(owner(&e, 20), Some(1), "last line is inside");
    }

    #[test]
    fn a_sibling_scope_does_not_capture_the_other() {
        let a = ext(1, 1, 10);
        let b = ext(2, 11, 20);
        assert_eq!(owner(&[a, b], 5), Some(1));
        assert_eq!(owner(&[a, b], 15), Some(2));
    }

    #[test]
    fn identical_extents_resolve_deterministically() {
        // Two symbols can share a span (a one-line item and its wrapper). No
        // tie break is more correct than another, so it must at least be
        // stable, or the same input yields different edges run to run.
        let a = ext(7, 5, 5);
        let b = ext(3, 5, 5);
        assert_eq!(owner(&[a, b], 5), owner(&[b, a], 5));
    }

    #[test]
    fn no_extents_means_no_owner() {
        assert_eq!(owner(&[], 1), None);
    }
}
