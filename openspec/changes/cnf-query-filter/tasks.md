## 0. Gate (do first)

- [ ] 0.1 Collect real agent search queries + outcomes; confirm a material
      fraction are multi-concept queries that a CNF filter would improve. If not,
      do not build — close this change.

## 1. CNF filter (only after the gate passes)

- [ ] 1.1 Add an optional `groups: [[..], ..]` parameter to the symbol-search
      tool; plain-string query + RRF ranking stays the default.
- [ ] 1.2 Build the filter via `fts5_match`: OR within group, AND between groups,
      parenthesized (design C4).
- [ ] 1.3 Implement the relaxation fallback — relax/drop a group rather than
      return empty (design C3).
- [ ] 1.4 Vector arm: operate on a natural-language flattening of the groups, or
      omit it from the filtered pass (design C2).

## 2. Verification

- [ ] 2.1 Structured query narrows to results touching every concept; relaxation
      avoids empty results.
- [ ] 2.2 Default (plain-string) path is unchanged.
