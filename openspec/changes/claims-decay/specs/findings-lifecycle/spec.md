## ADDED Requirements

### Requirement: A finding declares whether it is a rule or a claim

The system SHALL distinguish findings that assert a **rule** — how this codebase works,
what to do or not do — from findings that assert a **claim** about the current state of
the code, and SHALL treat the absence of a claim marker as "rule".

The distinction is not stylistic. A rule survives edits to the file it is anchored to:
"flooring is per-ingester" stays true when the ingester is refactored. A claim does not:
"the producer emits spurious zero-range defs" is a statement about code that someone may
have since changed, and the person who changed it had no reason to look for a finding
describing it.

Defaulting to "rule" SHALL be preserved when a finding carries no marker. The store is
overwhelmingly rules, and treating an unmarked finding as a decaying claim would flood
the re-verification surface with entries that do not need it.

#### Scenario: An unmarked finding is a rule

- **WHEN** a finding carries no claim marker
- **THEN** it is treated as a rule
- **AND** drift on its anchors does not place it in the re-verification set

#### Scenario: A claim is identifiable without reading its prose

- **WHEN** a finding asserts a defect, a limitation, deferred work, or a fix
- **THEN** it is identifiable as a claim from its metadata alone
- **AND** a consumer need not parse its text to know the assertion can decay

### Requirement: A claim whose code has changed is reported as unverified

The system SHALL report claims whose anchored content has changed since the claim was
recorded, as a set distinct from the anchor-repair sets it already reports.

The existing buckets answer a different question. Broken asks whether the anchored path
still exists; drifted asks whether its bytes moved. Neither asks whether the assertion is
still true, which is the only question a claim raises.

A claim in this set SHALL NOT be presented as resolved, nor as false — only as
unverified. The system cannot know whether the change fixed it, worsened it, or missed
it entirely; asserting any of those would replace one stale fact with another.

#### Scenario: A claim is surfaced when its code moves

- **WHEN** a claim's anchored file has changed since the claim was recorded
- **THEN** it is reported as unverified
- **AND** it is distinguishable from a broken or merely drifted anchor

#### Scenario: A rule drifting is not reported as unverified

- **WHEN** a rule's anchored file has changed
- **THEN** it is not reported as unverified
- **AND** its ordinary drift is reported as it is today

#### Scenario: An unverified claim is not called resolved

- **WHEN** a claim is reported as unverified
- **THEN** the report does not assert whether the claim still holds

### Requirement: A claim is served with its verification status

The system SHALL mark a claim's verification status wherever findings are served as
guidance, so a consumer acting on a claim knows whether it was confirmed against the
current code or predates it.

Serving an unverified claim indistinguishably from a confirmed one is the failure this
change exists to prevent: a record stating that work remains, read as current fact, is
acted on — and the action can be worse than inaction when the code has moved on.

#### Scenario: A consumer can tell a confirmed claim from an unverified one

- **WHEN** findings are requested for a set of paths
- **AND** one of the returned findings is a claim whose code has changed
- **THEN** the response marks it unverified
- **AND** a claim confirmed against current code is marked differently

### Requirement: Re-verifying a claim is an explicit outcome, not a re-attach

The system SHALL let a claim be recorded as still true, no longer true, or partially
true, and SHALL NOT treat re-attaching its anchor as evidence of any of them.

Re-attach means "this applied to my change". It refreshes the recorded content hash,
which would clear the unverified mark as a side effect while asserting nothing about
whether the claim holds — the precise silent failure this change removes. A claim that
is no longer true SHALL be superseded by one describing the current state, so the
record's history stays readable.

A claim found **partially** true SHALL be expressible as such. The failure that motivated
this was a successor claiming a defect was FIXED when the fix covered only part of it,
leaving a residue that read as outstanding work and was not.

#### Scenario: Clearing an unverified mark requires a verification outcome

- **WHEN** a claim is unverified
- **AND** its anchor is re-attached without a verification outcome
- **THEN** it remains unverified

#### Scenario: A claim that is no longer true is superseded, not deleted

- **WHEN** a claim is verified as no longer true
- **THEN** a finding describing the current state supersedes it
- **AND** the superseded claim stops being served as guidance

#### Scenario: A partially-fixed claim is recordable as such

- **WHEN** a fix addresses part of what a claim describes
- **THEN** the outcome distinguishes the fixed part from the residue
- **AND** the residue does not read as untouched outstanding work
