## ADDED Requirements

### Requirement: A dominant package is decomposed into source-directory components

The atlas SHALL subdivide a package into `component` concepts — one per top-level
source subdirectory beneath the package root — when that package is both
**dominant** (owns a large share of the repo's code symbols, or the repo has few
code anchors) and **structured** (its symbols span at least a minimum number of
subdirectories, each holding at least a minimum number of symbols). Each component
concept SHALL be parented to its package, carry central symbols and members scoped
to `<package>/<subarea>/`, and the package concept SHALL list its components. A
package that is not dominant, or whose symbols do not span enough subdirectories,
SHALL remain a single flat concept (no components).

#### Scenario: A monolithic library is mapped by its source directories

- **WHEN** a single-package Swift library whose sources are organized under
  `Source/Core`, `Source/Features`, and `Source/Extensions` is indexed
- **THEN** the package concept gains a `Core`, a `Features`, and an `Extensions`
  component, each parented to the package with its own central symbols
- **AND** the package concept lists those components

#### Scenario: A small or flat package is not subdivided

- **WHEN** a package's symbols all sit in one directory, or the package is small
- **THEN** it is emitted as a single concept with no components

### Requirement: Multi-package repos keep flat packages

The atlas SHALL NOT introduce components for a repo composed of many balanced code
anchors — the existing per-anchor package concepts and cross-anchor domains already
carry that repo's structure. Component decomposition SHALL engage only for a
dominant anchor.

#### Scenario: A many-crate workspace is unchanged

- **WHEN** a workspace of many similarly-sized crates is indexed
- **THEN** each crate is a flat package concept as before, with no components
- **AND** the concept output is byte-identical to the pre-change atlas

### Requirement: Domains form within a single-dominant package

For a repo dominated by one code anchor, the atlas SHALL form domain concepts from
semantic communities **within** that anchor (not only across anchors), so a
monolithic library still surfaces meaningful domains. For a multi-package repo, a
domain SHALL still require spanning more than one anchor, so intra-package
communities never flood a multi-package atlas.

#### Scenario: A monolithic library surfaces intra-package domains

- **WHEN** a single-package library with distinct semantic clusters is indexed
- **THEN** domains are formed from within-package communities that clear the
  minimum-size floor
- **AND** each domain's hub and central symbols are real types, never a
  module/namespace container

### Requirement: Example and sample code cannot fabricate a domain or central symbol

The atlas SHALL exclude symbols defined under a conventional example / sample /
demo / fixture path segment from domain eligibility and from package and component
central-symbol lists, the same way test symbols are excluded from a production
package. Such symbols SHALL still count toward a package's member and symbol
totals.

#### Scenario: A bundled example app does not create a domain

- **WHEN** a library repo ships an example app that references a library type, and
  that reference is the only cross-boundary link
- **THEN** no domain is created from that example-to-library reference
- **AND** the example's types are not listed as any concept's central symbols

### Requirement: A package's files are reported as a total and per-directory counts

A package concept SHALL render a `## Files under <package>` section stating the
package's total member-file count and then listing, for every directory that holds
member files (the file's exact parent directory, relative to the package root), the
number of files in it — sorted by count descending, then path — instead of a
truncated top-N list of individual files. The total SHALL be the true file count
(never a cap presented as the whole), and no member file SHALL be silently omitted
from the per-directory counts. A `component` concept SHALL continue to list its
individual member files, since it maps a single source directory.

#### Scenario: A flat package reports its total and single directory

- **WHEN** a package whose files all sit under one directory (e.g. `src/`) is indexed
- **THEN** its `## Files under <package>` heading states the total file count
- **AND** the body is a single directory line with that same count

#### Scenario: A multi-directory package hides no files

- **WHEN** a package whose files span several directories is indexed
- **THEN** its `## Files under <package>` heading states the true total file count
- **AND** every directory holding files appears with its own count, so the counts
  sum to the stated total

### Requirement: Intra-package decomposition is deterministic

The atlas SHALL produce byte-identical component and domain concepts on a re-index
of an unchanged repo — every grouping, threshold, and ordering is a pure function
of the persisted aggregate and analysis tables and fixed constants, with no
wall-clock or nondeterministic iteration.

#### Scenario: Re-indexing an unchanged repo is a no-op diff

- **WHEN** a single-package repo is indexed twice with no source change
- **THEN** the emitted component and domain concept files are byte-identical
