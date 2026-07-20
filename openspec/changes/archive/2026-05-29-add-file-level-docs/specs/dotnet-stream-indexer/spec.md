## ADDED Requirements

### Requirement: FileFrame carries file-level comment trivia

The producer SHALL emit on each in-source `FileFrame` an optional `doc` field: a JSON array of strings carrying the file's comment trivia in source order, **one entry per comment trivia token** (not a merged block). Entries SHALL be drawn from two slots: (1) the leading trivia of the compilation unit's first token (the file header), and (2) each namespace declaration's leading trivia. Each `SingleLineCommentTrivia` SHALL be its own entry; each `MultiLineCommentTrivia` SHALL be one entry preserved verbatim including internal newlines. Per-token granularity is required so the consumer can filter a license line without discarding an adjacent purpose line. The producer SHALL NOT filter, classify, or drop any comment (license-boilerplate filtering is a consumer concern). When a file has no such trivia, `doc` SHALL be omitted (or empty) and no doc is emitted.

When the first token of the compilation unit is itself the `namespace` keyword (no usings or types precede it), slots (1) and (2) reference the same leading trivia; the producer SHALL emit each comment token exactly once (deduplicated by trivia span) rather than twice.

The extraction SHALL read syntax trivia, NOT `GetDocumentationCommentXml()`, because plain `//` / `/* */` headers are not returned by the documentation API and namespace declarations return empty documentation XML regardless of any `///` present.

#### Scenario: File-header comments are captured as separate entries

- **WHEN** a C# file begins with consecutive `//` lines before its first `using`/`namespace`/type
- **THEN** the file's `FileFrame.doc` array contains one entry per `//` line, in order

#### Scenario: A multi-line block comment is one entry

- **WHEN** the file header is a single `/* … */` block spanning multiple lines
- **THEN** it appears as one `FileFrame.doc` entry with its internal newlines preserved

#### Scenario: Namespace-leading comment is captured

- **WHEN** a comment sits immediately above a `namespace` declaration (e.g. after the using directives)
- **THEN** the `FileFrame.doc` array contains it as an entry

#### Scenario: A comment above a namespace with no preceding usings is not double-counted

- **WHEN** a file's first token is the `namespace` keyword and a comment block precedes it
- **THEN** each comment token appears exactly once in `FileFrame.doc` (not duplicated across the first-token and namespace slots)

#### Scenario: A file with no comment trivia emits no doc

- **WHEN** a C# file has no file-header and no namespace-leading comments
- **THEN** `FileFrame.doc` is omitted or empty and the consumer writes no file_docs row

#### Scenario: Producer does not filter license headers

- **WHEN** a file begins with a copyright/license header
- **THEN** the producer still emits it verbatim in `FileFrame.doc` (filtering happens on the consumer)
