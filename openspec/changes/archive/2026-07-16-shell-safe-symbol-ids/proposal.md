## Why

A symbol's `pub_id` is the identifier agents and users hand to kenn as a lookup
argument — `kenn get <pub_id>`, `get-source`, `find`, `relations` (the MCP query
tool resolves a `pub_id` directly). But the id grammar was never designed for
shell use, and an audit of the live index shows **every code language emits
shell-hostile ids**:

- **typescript** — all 206 internal ids are backtick-quoted (SCIP wraps the
  file-path module descriptor in backticks): `` ts:`indexers/frames.ts`/walk(). ``
- **csharp** — 292/566 carry `()`, 84 carry `<>`, and **61 carry spaces** — full
  method signatures with fully-qualified parameter types, commas, and spaces:
  `cs:…IndexCommand#Run(Kenn…IndexOptions, Kenn…JsonlSink, …)`, `#.ctor()`
- **rust** — 96 carry backticks + `<>` (trait-impl / generic descriptors):
  `` rs:kenn-cli::exit::ExitCode::`From<ExitCodes>` ``

None of these can be passed as a single unquoted POSIX-shell argument; backticks
are command substitution, `()` is a subshell, spaces split args. It surfaced when
the `atlas` feature emitted `pub_id`s for `kenn get`. This is a cross-language
data-format problem, so it warrants its own change.

## What Changes

- **Establish a shell-safety invariant for `pub_id`**: every code language emits a
  `pub_id` that is safe as a single unquoted POSIX-shell argument — and stays
  readable/typeable, since agents read these from the atlas.
- **Replace the shell-hostile grammar with a safe-delimiter grammar** (not
  percent-encoding — nothing decodes the id, so we only need injective + stable,
  which frees us to pick readable delimiters):
  - `+` — callable marker and parameter separator (`Run(A, B)` → `Run+A+B`)
  - `~` — opens/descends a generic argument list (`From<ExitCodes>` → `From~ExitCodes`)
  - `,` — separates sibling args within one generic list (`HashMap<K, V>` → `HashMap~K,V`)
  - type constructors whose native syntax uses a shell metacharacter — array `[]`,
    pointer `*`, nullable `?`, tuple `()`, brace `{}`, union `|`, ref/`ref`/`out`/`in`,
    fn — render as reserved safe words + `~` (`_array_~int`, `_tuple_~int,string`,
    `_optional_~int`, `_ref_~str`, `_fn_+int=bool`), so glob (`* ? [ ]`) and
    brace-expansion (`{ }`) chars never reach the shell
  - SCIP backtick quoting + Rust lifetimes are dropped; `$`→`@`; operator glyphs →
    safe words by a general rule (covers Swift custom operators); a residual `_x<NN>_`
    word escape makes the transform **total**, so the backstop can't fire on real input.
    Values render bare (kind suffix only for callable `+` / type `#` / namespace `/`);
    `=` marks a function's return (`_fn_+arg=ret`). Structural markers are unchanged.
- **Render type references readably**: an *external* (referenced-package) type is
  spelled by its leaf name; an *internal* type stays qualified. A **per-language**
  leaf registry disambiguates: if two distinct external FQNs contend for the same
  leaf, all contenders fall back to FQN. C# keeps its parameter types (they are the
  overload disambiguator — no arity/hash); Swift keys on base name + argument
  labels.
- **One shared renderer, fed by structured descriptors**: each indexer emits its
  descriptor as external-tagged parts; a single language-agnostic renderer in
  kenn-indexer applies the grammar + leaf-registry + a hostile-char backstop. The
  leaf registry is per-language and resolved locally within each language's own
  ingest — no global barrier, languages stay parallel.
- **A conformance test** that scans all emitted ids per language against the
  shell-hostile set and fails on any hit, over fixtures covering all six languages.
- **Migration**: a data-format change → `reindex --force`. Low blast radius —
  findings anchor to file paths (not ids) and vectors key on a content fingerprint
  (not ids), so neither needs a reconcile pass.

## Non-premise (corrected from an earlier draft)

`edge.rs`'s callable heuristic is **not** a `pub_id` parser. It reads `occ.symbol`
— the raw SCIP wire symbol — via `splitn(5, ' ')` on the SCIP grammar, upstream of
and independent from the kenn `pub_id`. The grammar change never touches it, so
`edge.rs` needs **no change** and callable/reference classification is unaffected.
The care this change needs comes from uniqueness/stability, not a downstream parser.

## Capabilities

### New Capabilities
- `shell-safe-symbol-ids`: the cross-language invariant that a `pub_id` is a valid
  single unquoted POSIX-shell argument, the safe-delimiter grammar (`+`/`~`/`,`)
  that carries callable/generic structure, the external-leaf rendering with
  per-language FQN-on-contention, and the per-language conformance verification.

### Modified Capabilities
<!-- The id GRAMMAR lives across the indexers and the SCIP/JSONL transforms rather
     than in one spec's requirements; the concrete formation changes are Impact +
     tasks. If review prefers a delta on `code-intel-data-model` (which describes
     the symbol/pub_id shape), that surfaces there. -->

## Impact

- **Shared renderer** (new, language-agnostic, the enforcement point):
  kenn-indexer applies the `+`/`~`/`,` grammar, the external-leaf registry, and the
  hostile-char backstop. Reached by both formation paths —
  `transform_jsonl/stream.rs` (ts/csharp/swift) and `transform/document/walk.rs`
  (rust/go/python) — via the shared funnel `registry.intern_with_pub_id`.
- **Structured descriptors** (per indexer): `indexers/kenn-ts` (`key.ts`),
  `kenn-dotnet` (C#), the SCIP transformer (rust/go/python), `kenn-swift` emit
  descriptor parts tagged with each type-ref's `external` flag instead of a
  pre-baked hostile string. The `external` flag is already on every symbol.
- **`pub_id` consumers to inventory** (most parse only on safe separators):
  `markdown/walk.rs:279` (`ends_with(suffix)` — the one to check), `css/ingest.rs`,
  `html/links.rs` (`strip_prefix`). NOT `edge.rs` (see Non-premise).
- **Data-format / migration**: `reindex --force`. Findings anchors (file-path
  based) and vector ids (fingerprint based) are unaffected.
- **Out of scope**: the atlas markdown rendering (renders fine once ids are
  shell-safe — do not work around it in the md generator); any prefix/renaming
  scheme beyond the grammar above.
