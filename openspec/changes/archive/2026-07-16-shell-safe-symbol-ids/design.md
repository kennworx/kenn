## Context

`pub_id` is kenn's public symbol handle — the argument to `kenn get` / `find` /
`relations` and the MCP resolver. Today it is minted per-language: Rust uses a
`::`-joined module path; TypeScript/Go/Python use SCIP descriptors (backtick-quoted
names, `/` `#` `().` separators); C# uses fully-qualified method signatures. An
audit shows all three languages in this repo emit ids that cannot be passed as a
single unquoted POSIX-shell argument (backticks, `()`, `<>`, spaces).

Both formation paths funnel through one interning call,
`registry.intern_with_pub_id(language, wire, pub_id)` — from the JSONL join
(`transform_jsonl/stream.rs:237`, ts/csharp/swift) and the SCIP transform
(`transform/document/walk.rs:130`, rust/go/python). That funnel is where a single
renderer can own the id grammar for all six languages.

**Nothing decodes the `pub_id`.** An earlier draft claimed `edge.rs:329` parses the
id's trailing `().`; it does not — it reads `occ.symbol`, the raw SCIP wire symbol,
via `splitn(5, ' ')`, upstream of the `pub_id`. A grep of `pub_id` consumers shows
the id is barely parsed structurally, and only on safe separators/prefixes
(`ends_with`, `rsplit(['.','/'])`, `strip_prefix`). So the transform must be
**injective + deterministic**, but **not reversible** — which lets us choose a
readable delimiter grammar instead of percent-encoding.

## Goals / Non-Goals

**Goals:**
- Every code language's `pub_id` is a valid single unquoted POSIX-shell argument.
- The id stays readable/typeable (agents read it from the atlas and pass it back).
- Uniqueness + cross-run determinism (same code → byte-identical ids).
- One enforcement point so no future indexer can reintroduce a bad char.

**Non-Goals:**
- Reversibility (nothing decodes the id).
- Unifying the per-language grammars (`::` vs SCIP) beyond shell-safety.
- Changing the prefix scheme (`rs:`/`ts:`/…) or renaming.
- Working around the id in the atlas markdown generator.

## Decisions

### D1 — A safe-delimiter grammar (replaces percent-encoding)

*Shipped vs. target: the safe alphabet, the structural swaps (`<>`→`~`, `()`→`+`,
`,` args), and the dropped backticks/lifetimes/kind-marker are shipped in
`pubid.rs`. The `_word_` type constructors, the `=` fn-return marker, and the
reversible `_x<NN>_` floor are the richer **target**, not shipped — the shipped
renderer instead uses a lossy `floor` (see "Residual escape" below). Per D6, the
target is reached by richer real symbols from the sidecar + Rust rendering, never
by teaching a sidecar about shells.*

**Safe alphabet** — literal, unquoted-shell-safe in every position an id uses (the
always-present `rs:`/`ts:`/… prefix neutralizes every word-start caveat on `# ~ -`):

```
A–Z  a–z  0–9   . _ / : @ -        name + structural chars
+  ~  ,                            grammar delimiters
=                                  function-return marker (_fn_+arg=ret)
```

**Structural roles**: `:` prefix · `/` path (SCIP namespace segments) · `.` dotted
names (C# namespaces/types) · `#` type/member separator (an *internal* delimiter
only — never a trailing kind suffix) · `::` Rust path · `@` version salt (in the
*intern key* only, not the emitted id — so `@` is free for the `$` mapping below).

**Kind markers (dropped, not carried).** A symbol's own kind lives in the store's
`kind` column, not in a trailing punctuation marker on the id. So SCIP languages
**drop the trailing kind marker of the last segment — both** the term `.`
(`Foo#config.` → `Foo#config`) **and** a bare type's `#` (`IdRegistry#` →
`IdRegistry`). `#` and `.` survive only as *internal separators*: `EdgeFrame#edge_kind`
keeps the type→member `#`, a dotted name keeps its `.`. C# already emits no trailing
marker (`PubId.cs`). A *callable* still carries `+` (`walk()` → `walk+`), so the
call-vs-value distinction is intact; only the type-vs-value one is dropped.

Trade-off: without the trailing `#`/`.`, a same-scope type `Foo` and value `Foo`
(e.g. a TS `type Foo` + `const Foo`) collapse to one id. Rare, and accepted — the
`kind` column still separates them downstream. The earlier asymmetry (drop `.`,
keep `#`) was the real defect: it left types alone spelled with a trailing quirk.
Namespace/path markers are **not** unified — C# stays dotted (`Kenn.Dotnet.Cli`),
SCIP keeps `/`. Net: `.` is only ever a dotted-name separator.

**Grammar delimiters** replace the callable/generic markers:

| role | delimiter | example |
|------|-----------|---------|
| callable marker + parameter separator | `+` | `Run(A, B)` → `Run+A+B`; `walk()` → `walk+` |
| open / descend a constructor's arg list | `~` | `From<ExitCodes>` → `From~ExitCodes` |
| sibling args within one list | `,` | `HashMap<K, V>` → `HashMap~K,V` |

**Type constructors → safe words.** Every type-former becomes a word + `~`, so its
punctuation never appears — this is what covers the full type-expression space (not
just `() <>`) and resolves the tuple-vs-callable `()` clash. The word depends on the
type *kind*, which only the indexer's semantic model knows (see D4):

| construct | example | → |
|-----------|---------|---|
| generic (named type + args) | `List<T>` | `List~T` — the type's own name, not a reserved word |
| array / jagged / multidim | `int[]` · `int[][]` · `int[,]` | `_array_~int` · `_array_~_array_~int` · `_array2_~int` |
| sized array (Rust) | `[u8; 4]` | `_array_~u8,4` |
| optional / nullable (sugar) | C#/Swift `int?` · Swift `String!` | `_optional_~int` · `_optional_~String` |
| pointer | `int*` | `_ptr_~int` |
| reference | `&str` · `ref int` · `&mut T` | `_ref_~str` · `_ref_~int` · `_refmut_~T` |
| out / in param | `out int` · `in int` | `_out_~int` · `_in_~int` |
| tuple | `(int, string)` | `_tuple_~int,string` |
| fn / closure *(per audit)* | `fn(int)->bool` · `fn()->void` | `_fn_+int=bool` · `_fn_=void` |
| union (TS) *(per audit)* | `A \| B` | `_union_~A,B` |
| object type (TS) *(per audit)* | `{ x: number }` | `_obj_~x,y` |

Rows marked *(per audit)* are not yet grounded — TS unions / object types stay in
type *aliases* (named), not in ids, and fn-types are rare; the renderer adds these
words only if the fixture audit (task 1.1) actually finds them. The unmarked rows are
the grounded C# param-type / generic cases.

**Reserved `_word_` forms.** Built-in structural formers are wrapped `_like_this_`
so they occupy a name-space ordinary types don't — otherwise a native `int?`
(`Optional~int`) would collide with a library type literally named `Optional<int>`
and the two would silently merge. Wrapped, a collision needs a user type named exactly
`_optional_` (near-zero). A former that *is* a named library type (Rust `Option<T>`,
C# `Func<…>`, `System.Nullable<T>`) is just a generic → renders by its name (`Option~T`),
subject to D3 leaf/FQN.

**Function types** render args as `+`-slots and the return after `=`, always present
(void included): `_fn_+<arg>+…=<ret>` — `fn(int)->bool` → `_fn_+int=bool`,
`fn()->void` → `_fn_=void`. No arrow is possible (every arrow glyph uses `<`/`>`,
shell redirects) and `~` reads as *generic*, so `=` marks the return — one meaning,
unambiguous. (`=` is freed from escape duty by the `_x<NN>_` floor below.) `=` stays
shell-safe even under zsh's `MAGIC_EQUAL_SUBST`, which only expands `identifier=value`
arguments — every id carries `:` / `+` / `#` before any `=`, so it never matches an
identifier.

**Why constructors and not literal punctuation:** `* ? [ ]` are glob metacharacters
and `{ }` are brace-expansion metacharacters. Unquoted, `int[]` either expands to a
filename or hard-fails in zsh (`no matches found`), and `{a,b}` *splits the id into
two shell words* (deadly next to our `,`). None can ever be literal — hence the words.

**Formation drops:** SCIP backtick quoting (`key.ts` `esc()`, the Rust generic
carryover — the wrapped text is already delimited); Rust lifetimes (`'a`, `'_` rarely
affect identity).

**Operator-glyph names → safe words**, by a *general* glyph→token rule (not a fixed
table, so Swift custom operators like `<*>` are covered): `+`→`op_add`, `<<`→`op_shl`,
`<*>`→`op_lt_star_gt`. (C# already emits `op_Addition`; Rust uses trait methods
`add`/`shl`; so this is mainly Swift.) The rule must stay injective against a real
identifier literally named `op_add`.

**`$` → `@`.** `$` is a valid JS/TS identifier char (`$mount`), hostile to the shell,
and not a constructor — so it maps to the safe sigil `@` (free, per the structural
note above).

**Residual escape — the floor.** *Shipped:* every maximal run of shell-hostile
bytes collapses to a single `_` (`Some Note`→`Some_Note`, `a<>b`→`a_b`), after the
per-language structural swaps run. It is lossy (distinct hostile chars merge, a
small collision risk) but reads like a name, and its output always satisfies
`shell_safe::is_safe`, which the store's writer asserts — so the backstop is a
can't-happen invariant, not a failure mode. *Target (not shipped):* a reversible
`_x<NN>_` per-byte escape (`^`→`_x5e_`) joined to the `_word_` family, which would
also free `=` for the function-return marker; deferred with the rest of the rich
grammar.

**Composition — order preserved, delimiters swapped in place.** This change does not
reorder id components; each language keeps its existing grammar (Rust `::`-paths,
C#'s `scope#name<generics>(params)`, the SCIP `/`#`().` suffixes). We only replace the
hostile markers with safe ones at their current positions (`()`→`+`, `<>`→`~`,
constructors→words) and drop backticks/lifetimes. So the callable/generic placement is
inherited from today's formation (e.g. C# `#name~generics+params`), not newly defined —
unifying grammars across languages stays a non-goal.

**Nesting note:** `~`/`,` do not encode tree depth, so `Vec<Option<T>>` and a
hypothetical `Vec<Option, T>` both flatten to `Vec~Option,T`. Still **injective over
real symbols** because a type's arity is fixed (`Vec` is arity-1, so only one reading
is a real type). Readability degrades past ~2 nesting levels; uniqueness never does.

### D2 — No parser update needed; `edge.rs` is unaffected

The callable/type markers are read from the SCIP wire symbol, not the `pub_id` (see
Context). `edge.rs` needs no change; classification is unchanged by construction.
The only obligation is the inventory (task 1.3): confirm every `pub_id` consumer
parses only on characters the grammar leaves literal — the one to actually verify is
`markdown/walk.rs:279` (`ends_with(suffix)`).

### D3 — External type references render by leaf name; internal stay qualified

*Status: future (not in the shipped lossy floor). Recorded as the target for
readable C# ids; per D6 it is done Rust-side off a richer kenn-dotnet symbol.*

A type reference inside a descriptor (a C# param type, any language's generic arg)
is spelled by its **leaf name if the type is external** (from a referenced
package — kenn's uniform `external` flag), and **fully qualified if internal**.
This kills the bulk of the verbosity (`System.Collections.Generic.Dictionary` →
`Dictionary`) while keeping local types precise, and internal-vs-external asymmetry
means an internal `User` (`Acme.Models.User`) never collides with an external `User`
(`User`).

C# keeps its parameter types (they *are* the overload disambiguator — no arity/hash
reduction). Swift keys a function on base name + **argument labels**, each label its
own `+` slot with its trailing colon: `greet(name:)` → `greet+name:`,
`move(from:to:)` → `move+from:+to:`. Type-only Swift overloads (same labels, different
types — `f(x: Int)` vs `f(x: String)`) are rare but legal, and labels alone would merge
them; so when a label set is shared by ≥2 overloads, kenn **appends the leaf param
type(s) as extra `+` slots** after the labels (`f+x:+Int` vs `f+x:+String`), mirroring
C# and guaranteeing they never intern to one row. Whether they occur at all is a
fixture-audit item (task 1.1); the fallback rule stands regardless.

### D4 — Per-language leaf registry, resolved locally (no global barrier)

*Status: future. The shipped renderer uses the lossy `floor` (D1), not leaf-name
shortening, so no contention arises yet. When D3's leaf-shortening lands it stays
Rust-side per D6; this records how contention resolves then.*

Contention — two distinct external FQNs wanting the same leaf — is resolved by a
**per-language** registry: within one language's id-space, a leaf claimed by ≥2
distinct FQNs falls back to **FQN for all claimants** (symmetric, so the result is
independent of processing order → re-indexing unchanged code is byte-identical).

Per-language is correct *and* minimal: ids are prefix-scoped, so a `cs:` leaf and an
`rs:` leaf can never collide — a global registry would over-qualify across languages
for nothing. Each language's Rust renderer (D6) resolves its own leaves in a **local
two-pass** over that language's symbols. No cross-language synchronization; languages
stay parallel.

### D6 — Shell-safety lives in Rust ingestion; indexers emit real symbols

The division of labour is a hard line, not a convenience:

- **Indexers emit real, language-native code symbols.** The external tools
  (rust-analyzer, scip-go, scip-python) we don't own; our own sidecars
  (kenn-ts, kenn-dotnet, kenn-swift) follow the same rule — each emits the
  *true* symbol for its language (SCIP descriptors with backticks, a C# signature
  with `int[]`, a Swift `<` operator) and knows **nothing** about shells.
  Shell-safety is not their job and must never leak into them.
- **Rust ingestion is the single owner of shell-safety.**
  `pubid::render(Language, raw)` (`crates/kenn-indexer/src/pubid.rs`) takes the raw
  language-native symbol and renders the shell-safe `pub_id`. It is the only place
  in the system that knows a shell exists.

Rendering is still **per-language** — but the per-language *rules* live in Rust,
dispatched on `Language`, never pushed down into a binary. This is required, not
stylistic: the same byte means different things in different languages, and only a
language-specific rule renders it correctly. Proof (swift-argument-parser fixture):
`<` is an *operator name* in Swift (`InputOrigin.Element.<(_:_:)`) but *opens a
generic* in Rust/C#; a uniform `<`→`~` would corrupt the Swift operator into a fake
generic. Likewise `()` (C# call vs Swift argument-label list vs tuple type), `,`, and
spaces differ per language. So `render` dispatches:

- **rust** — drop backticks, `<>`→`~`, drop lifetimes.
- **typescript** — drop backticks, `()`→`+`.
- **csharp** — swap `()`/`<>` for `+`/`~`, then floor.
- **swift** — operator glyphs → `op_*` words, `()`→argument labels.
- **go/python** — already clean; drop the trailing kind marker, then floor.
- **markdown/css/html** — structural (paths/selectors/anchors); the floor alone.

The **only** shared pieces are `kenn_model::shell_safe` (what "safe" *means*) and the
readable `floor` (pubid.rs) that every render passes through. Neither does structural
interpretation, so neither can mis-represent a construct.

**Corollary for richer rendering.** If a language's raw symbol is too lossy for Rust
to render well — e.g. `render_csharp` cannot tell an array from a tuple from the
string kenn-dotnet currently emits — the fix is to make **kenn-dotnet emit a richer
*real* symbol** (more structural detail in its own natural notation), which
`render_csharp` then interprets. The sidecar still never learns about shells; Rust
still owns the transform. The rich `_word_`/leaf grammar (D1/D3/D4) is reached this
way, or it is not reached at all.

### D5 — Conformance test is the invariant's teeth

A test scans every emitted `pub_id` (per language, over fixtures covering all six)
against the hostile-char set and fails on any hit — the invariant is enforced at CI,
not aspirational.

## Risks / Trade-offs

- **A D3/D4 rendering change moves ids beyond just the grammar.** Leaf-vs-FQN and
  contention resolution change id shape. → Intended; it is a reindex-generation bump
  anyway (Migration), and it is what makes ids usable.
- **Over-qualification churn.** Introducing a same-leaf external type re-qualifies
  that leaf to FQN across that language. → Bounded (per-language, and only real
  same-leaf clashes trigger it); nothing internal keys on id stability.
- **Nesting flattening** (D1) is injective but not human-parseable past ~2 levels. →
  Accepted; real generics are shallow, uniqueness holds.
- **A missed `pub_id` consumer** degrades quietly. → Lower risk than first assumed
  (edge.rs is not one); the inventory (1.3) + the backstop + the conformance test are
  the net.
- **Reserved-namespace assumption.** The `_word_` formers (`_array_`…`_obj_`) and the
  `_x<NN>_` floor — ~15 tokens — each collide only with a user symbol literally named
  `_array_` / `_x5e_` / etc. Near-zero individually, but it is now a *load-bearing*
  assumption across the whole family, not a one-off. → Accepted for readability; the
  airtight alternative (a reserved sigil that cannot begin an identifier) was rejected
  as less readable.

## Migration Plan

**No users yet**, so this is a plain reindex — no version-bump / release-gating
ceremony (deferred until real users exist; a `STORE_SCHEMA_VERSION` bump would be the
mechanism then).

1. Land the structured-descriptor emit + shared renderer.
2. `kenn index --force` rebuilds the graph with the new ids.
3. No reconcile pass needed — **confirmed (5.1)**: findings `.anchor.jsonl` records
   key on file paths (`{"op":"attach","anchor":"crates/…/x.rs","ts":…}`), so anchors
   survive. The vector sidecar keys on a content fingerprint + short_id, and `pub_id`
   — though a `knowledge` column — is **not** part of `embeddable_text`: the code
   recipe embeds **doc prose only**, and the fingerprint hashes `doc` alone
   (`finalize.rs build_name_rows`: `text_fingerprint(&doc)`; `name_text` is the split
   signature, also independent of the id). So a `pub_id` change invalidates neither
   vectors nor anchors, and **no `CODE_TEXT_RECIPE` bump is required**.

## Stability contract

- Determinism: within any run, ids are a pure function of the indexed code — the
  per-language registry resolves contention symmetrically, so parallel ingest cannot
  vary the result. Re-indexing unchanged code yields byte-identical ids.
- Cross-version: an external type's short name is stable until another external type
  with the same leaf enters/leaves that language's index (a real dependency change),
  at which point that leaf re-qualifies to FQN. Findings (path-anchored) and vectors
  (fingerprint-keyed) do not depend on id stability; only a stale copied id is
  affected, and it self-corrects against the regenerated atlas.

## Open Questions

- **Constructor-kind tags — RESOLVED by the live 6-language audit (task 1.1).** Indexed
  this repo (rust/ts/cs) + cloned fixtures for the rest (swift-argument-parser, samber/lo,
  pallets/click):

  | lang | total | hostile chars found | transform |
  |------|-------|---------------------|-----------|
  | rust | 6754 | backtick 96, `<>` 96, lifetimes | drop backticks, `<>`→`~`, drop lifetimes |
  | ts | 206 | backtick 206, `()` 54 | drop backticks, `()`→`+` |
  | **csharp** | 566 | `()` 292, `<>` 84, space 61, `?` 10, `[]` 2, `<Main>$` | **full `_word_` machinery** |
  | go | 2186 | **none** | none |
  | swift | 4979 | `()` 2275 (labels + operators `==`/`<`) | `()`→labels/`+`, operator-words |
  | python | 2393 | **none** | none |

  Conclusions: **the `_word_` type-constructors are C#-only** (only C# spells arrays,
  nullable, named tuples). kenn-dotnet has the semantic model that *knows* those kinds,
  so it can carry them in the **real** symbol it emits; the `_word_` rendering itself is
  Rust-side in `render_csharp` (D6). No SCIP/JSONL language spells raw `&`/`[]`/`*`/tuple,
  so there is **no constructor-kind extraction problem**. rust/ts need only backtick-drop + `<>`→`~` + `()`→`+`; swift needs
  `()`→labels + operator-words; **go and python need nothing**. Swift already appends a
  `#<hash>` overload disambiguator, so the type-only-overload fallback (D3) is likely
  redundant on the swift path.
- **SCIP two-pass (scope now tiny)** — D4's leaf registry only matters where external
  type refs appear *inside* an id, which the audit shows is **C#-only**. The SCIP
  languages have no in-id type refs to resolve, so no SCIP two-pass is needed; the
  registry lives entirely on the C# path — `render_csharp` resolves it (D6) over the
  whole compilation kenn-dotnet emits, a local two-pass over the C# symbols. This
  removes the earlier SCIP-transform buffering concern.
- **Primitive aliasing** — external leaf of `System.Int32` is `Int32`; whether to map
  to the C# keyword `int` is an optional prettiness table (same for other langs).
- **Operator table coverage** — enumerate the Swift operator glyphs → `op_*` set on
  the fixtures (task 1.1).
- **Go/Python/Swift audit** — confirm emitted ids against the hostile set on
  fixtures (not in this repo's index today), including Swift type-only overloads.
