## Context

kenn indexes code, markdown (`index-markdown`), and stylesheets (`index-css`).
HTML — the markup that connects those — is unindexed. Unlike code, HTML
contributes almost no callable symbols; its graph value is **edges that connect
otherwise-isolated islands**: CSS classes, stylesheets, scripts, documents, and
the ids JS/CSS reference.

```
        ┌──────────┐  uses_css_class  ┌──────────┐
        │   HTML   │─────────────────▶│   CSS    │   already mined; edge dropped today
        │ document │  imports(<link>) │  classes │
        │   node   │─────────────────▶│          │
        └────┬─────┘                  └──────────┘
             │ imports(<script src>)        ▲ corresponds_to
             ▼                               │
        ┌──────────┐   <a href> / #frag   ┌──┴───────┐
        │  JS/TS   │◀────links_to────────│ html_id  │  (HTML owns the id)
        └──────────┘   (markdown resolver) └──────────┘
```

Two grounding facts from the current code:

1. **The class-usage scan already covers HTML, then discards it.** The CSS usage
   walk is extension-agnostic (`css/discover.rs:93`) and grades `class=` as
   `Exact`, but edge attribution bails when the file has no code-graph node
   (`css/ingest.rs:197` — `continue`). HTML files are scanned and dropped.
2. **House parser style is purpose-built, pure-Rust** — `pulldown-cmark`
   (markdown), `lightningcss` (CSS). Raw-text scanning is reserved for
   cross-language usage mining, not for a language's own files.

## Goals / Non-Goals

**Goals:**

- Make an HTML file a first-class `document` node so the already-mined `class=`
  edges land and `check_css` stops reporting HTML-only classes as dead.
- Resolve HTML's connective edges by **reusing** existing machinery: the
  markdown link resolver (`<a href>`), the `Imports` edge (`<link>`/`<script>`),
  the CSS class registry (`class=`), the CSS extractor (inline `<style>`).
- Model ids correctly: HTML owns the id; CSS/JS correspond to it.
- Handle real-world HTML quirks (void/open-only tags, valueless/unquoted attrs,
  optional/implied close, raw-text `<script>`/`<style>`) via a true WHATWG parse.

**Non-Goals:**

- Inline `<script>` / event-handler / inline-`style=` JS extraction (needs the
  separate JS/TS indexer pipeline — deferred).
- Per-element nodes. Only the document and id'd elements become nodes; other
  elements are not nodes.
- Byte-precise positions. The graph is line-based (`get_source` → start/end_line,
  `find_at_location` → line; the CSS scan already does `offset_to_line`). Line
  granularity is the bar.

## Decisions

### D1 — Tiers, with the keystone first

```
TIER 0  KEYSTONE  Language::Html + html/htm + HTML file → document node
                  → the node every edge attaches to; precondition for check_css fix
TIER 1  GLUE      <a href> → LinksTo/LinksToFile (markdown resolve.rs)
                  <link>,<script> → Imports (HTML→CSS/JS)
TIER 2  IDS+ASSET id= → html_id node; href=#frag → LinksTo; html_id↔css_id → CorrespondsTo
                  <img>/asset → LinksTo (attachment stub); <a href> doc → LinksToFile
TIER 3  EMBEDDED  inline <style> → existing CSS extractor (offset-rebased)
                  (inline <script> deferred — separate pipeline)
```

The keystone gates everything and is cheap; each later tier hangs off the
document node. Tiers map to task phases, not separate changes.

### D2 — IDs: HTML owns them; correspondence, not a usage edge (Option C)

Classes and ids are asymmetric, and modelling that asymmetry is *correct*, not a
shortcut:

```
   CLASSES  CSS defines .btn → HTML/JS USE it          → uses_css_class (directional)
   IDS      HTML defines id="root" ◀▶ CSS #root, JS getElementById → corresponds_to
```

So: `id="…"` creates an `html_id` node; a CSS `#root` selector (`css_id`, from
`index-css`) and the `html_id` join via the existing `corresponds_to` edge **when
both exist**. There is **no `uses_css_id` edge**. Payoffs:

- `<div id="root">` with no CSS rule (React mount point) is a lone `html_id` —
  normal, not dead.
- a lone `css_id` with no `html_id` is a *dead-selector* signal for `check_css`.

**Why `corresponds_to` (an overload, but a defensible one):** the edge's existing
meaning is "the same logical entity expressed in two languages" (a TS DTO ↔ its
C# twin). An `id="root"` and a CSS `#root` selector *are* exactly that — one
identifier declared in HTML and in CSS-selector syntax — so the symmetric
correspondence reading is honest. It is deliberately **not** a "CSS uses the
element" claim (that would be directional, and there is no generic reference edge
to carry it without inventing one). Consequence to accept: `list_correspondences`
now surfaces HTML↔CSS id co-declarations alongside cross-language type
equivalents; both are "same entity, two languages," so the mixing is consistent.

**Enclosing reuse:** `index-css` attributes usage to "enclosing symbol, else
file." HTML has no functions, but an id'd element is a node — so `class=` usage
inside `<div id="card">…</div>` attributes to the `card` html_id node, falling
back to the document node. Same enclosing-or-file fallback, no new logic.

Alternatives rejected: (B) keep `css_id` as the sole node and point HTML at it —
semantically backwards (the definition lives in HTML); (A) a `uses_css_id` edge —
wrong relation (id reference is correspondence, not usage), and adds an edge kind.

### D3 — Parser: html5ever (measured spike)

"All HTML quirks" requires a real WHATWG tree builder — you [cannot correctly
tokenize HTML without the tree](https://blog.cloudflare.com/html-parsing-1/).
That narrows the field to `html5ever` and `swc_html_parser`; `lol_html` is
token-level (no tree, aborts on ambiguous nesting) and `tree-sitter-html` only
approximates the tree and breaks the pure-Rust house style. Full data in
[spikes/parser-comparison.md](spikes/parser-comparison.md). Measured summary:

| Parser | Δ binary | Crates | Cold build | WHATWG | Positions |
|---|---:|---:|---:|---|---|
| **html5ever** | +549 KB | 24 | ~26s | ✅ full | TokenSink plumbing |
| swc_html_parser | +420 KB | 78 | ~95s | ✅ full | free (span AST) |
| lol_html | +437 KB | 38 | ~36s | ⚠️ aborts | free |
| tree-sitter-html | +99 KB | 10 | ~13s | ⚠️ partial | free |

The spike's surprise: **binary size inverts the crate-count story** — swc
compiles *smaller* than html5ever (LTO strips the unreachable SWC ecosystem), so
"swc is bloat" is false for the shipped artifact. But the ~130 KB spread among
the Rust candidates is noise against kenn's multi-MB binary, so binary size does
**not** decide it.

**Decision: `html5ever`** — on the axes that do differ materially: ~3.6× faster
cold builds, a third the dependency surface (audit/supply-chain), and stable
`0.39` versioning vs SWC's fast major churn (already at v23). It's the WHATWG
reference, and pure-Rust like the rest of the indexer. The price is a
position-tracking `TokenSink` (swc would hand positions free) — bounded, and at
line granularity, cheap.

**Fallback: `swc_html_parser`** — if the `TokenSink` line/nesting plumbing proves
painful. Its binary cost is genuinely fine; the cost is build time + churn.

### D4 — Pipeline placement

HTML ingest is a parallel producer (like CSS) during the ingest phase. The
connective steps — `<a href>` resolution, `html_id`↔`css_id` correspondence, and
`class=`/`id=` attribution — are **gated on the post-code/CSS-registry barrier**,
mirroring how `index-css` gates usage resolution: the class registry and code
file nodes must exist before HTML edges can resolve against them.

### D5 — Class-usage ownership: the HTML parser owns it; the raw scan steps aside

There is a sharp interaction the keystone creates. Today the extension-agnostic
usage scan mines `class=` from HTML text and then **drops** it at
`css/ingest.rs:197` (`continue` — no node). The moment HTML files become document
nodes, that `continue` no longer fires, so the raw scan would emit a
`uses_css_class` edge **and** the HTML parser would emit one → **double edges**.

Decision: **the HTML parser owns class-usage for HTML files; the raw usage scan
excludes indexed-HTML extensions.** The parser path is strictly better — it scopes
`class=` to real element attributes, dropping the phantom edges the raw scan emits
from `<!-- class=x -->` and `<script>el.className="y"</script>`. So the keystone
does not merely make the crude edges land; the parser path *replaces* the crude
edges with precise, deduplicated ones, attributed to the enclosing `html_id` or
the document node. Net effect: precision up, no double-counting, and the
`check_css` fix follows from HTML being indexed — it does **not** depend on
`usage_sources` globbing the HTML (the HTML indexer is the source of truth).

### D6 — Inline `<style>` node identity (Tier 3)

An inline `<style>` block is not a stylesheet *file*, but a CSS selector is a CSS
node wherever its text lives, and `index-css`'s identity scheme
`<lang>:<relpath>#<type>:<name>` and node kinds (`CssClass`/`CssId`/`CssVar`) are
already **shared across languages**. So an inline-style selector reuses
`Kind::CssClass`/`CssId`/`CssVar` with a native id under the **`css:` prefix**,
the HTML file as relpath: `css:<relpath>#class:<name>` (and `#id:`/`#var:`). It
registers into the **same shared class registry**, so a `class=` anywhere can
resolve to an inline-defined class. Positions rebase by the block's base offset
(the markdown fenced-code pattern).

**Why `css:` and not `html:` (a collision fix).** An inline `#hero` selector and
the file's `id="hero"` element (an `html_id`, D2) both want a `#id:` typed id. Under
a shared `html:` prefix they would mint the *same* pub-id
(`html:page.html#id:hero`) for two distinct nodes — a collision. Putting the CSS
selector under `css:` (`css:page.html#id:hero`) keeps it distinct from the
`html_id` (`html:page.html#id:hero`), so the two **correspond** (D2) instead of
colliding — exactly as an *external* stylesheet's `#hero` already corresponds.
Provenance is preserved by the relpath; the prefix just states the node's kind of
language. The ShortId is still minted in the HTML producer's id space — sound
because `fetch_symbol` keys on the language column + pub-id, not the ShortId
partition.

### D7 — The link edge is chosen by the target's table, not by syntax

`LinksTo`/`Embeds` hydrate their target from the **symbols** table; `LinksToFile`
hydrates from the **files** table — a distinct kind because file and symbol
ShortIds collide and the kind tells the reader which table to read
(`reader/code_links.rs:70`). So the edge for an HTML reference is forced by *what
the target resolves to*:

```
   target = indexed file (other .html doc)   → LinksToFile   (files table)
   target = indexed CSS/JS (<link>,<script>) → Imports       (files table)
   target = html_id / section node           → LinksTo       (symbols table)
   target = attachment STUB (<img>, asset)   → LinksTo        (symbols table)
```

An `attachment` is a leaf **stub node in the symbol space** (kenn does not index
binary assets), so `<img src>` is `LinksTo` to that stub — **not** `LinksToFile`
(that was an early error: the asset isn't in the files table). We deliberately do
**not** reuse markdown's `Embeds` for `<img>`: both are symbol-targeting and valid,
but using plain `LinksTo` drops one concept. Trade-off accepted: the graph won't
mark HTML images as transclusion, so a "find all embeds" query won't see them —
a minor, conscious inconsistency with markdown in exchange for fewer edge kinds.

## Risks / Trade-offs

- **html5ever gives no positions on its DOM** → implement a `TokenSink` that
  tracks source line and assembles the element nesting (line granularity is all
  the graph needs). If it proves heavy, fall back to swc (D3).
- **Whole-document scope for class usage on big pages** → usage attributes to
  the enclosing id'd element when present, else the document node; acceptable,
  refine later if noisy.
- **Tier 3 offset rebasing** → inline `<style>` positions are relative to the
  block; add the block's base offset before feeding lightningcss. Same pattern
  markdown uses for fenced code.
- **New dependency surface** → 24 crates, pure-Rust, measured; consistent with
  existing parser deps. No version bump to kenn's own wire formats.

## Open Questions

- **Inline `<script>`** — deferred. Revisit once there's demand; it needs the
  JS/TS jsonl/stream indexer to accept fragments with offset mapping.
- **JS `getElementById("x")` → `html_id`** — a future `uses`-style edge via a
  string scan (mirrors the CSS class scan). Out of scope here; `html_id` is
  defined so the edge has a target when it lands.
- **`href` → code-symbol links** — markdown resolves fenced refs to code symbols
  (`code_resolve.rs`); whether HTML `href` should ever resolve to a symbol is
  left to the markdown-resolver reuse, not specified here.
