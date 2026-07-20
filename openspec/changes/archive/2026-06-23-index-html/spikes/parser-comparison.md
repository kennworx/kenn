# Spike — HTML parser comparison

Goal: pick the HTML parser for `index-html`. Hard requirement: **all WHATWG
quirks** (void/open-only tags, valueless & unquoted attributes, optional/implied
close tags, raw-text `<script>`/`<style>`). Soft requirement: positions to
**line** granularity (the graph is line-based — `get_source` returns
start/end_line; `find_at_location` takes a line; the CSS scan already converts
offset→line). Byte-precise spans are *not* required.

## The structural fact that frames it

Every HTML quirk exists because HTML5 is specified with tree-builder recovery —
you [cannot correctly tokenize HTML without building the tree](https://blog.cloudflare.com/html-parsing-1/).
So "all quirks" ⇒ a real WHATWG tree builder. That immediately splits the field:

- **Full WHATWG tree:** `html5ever` (the reference; Servo) and `swc_html_parser`.
- **Token-level, no full tree:** `lol_html` — [aborts on ambiguous nesting](https://docs.rs/lol_html/)
  rather than guess (good for streaming rewrite, wrong shape for an indexer).
- **Approximate tree:** `tree-sitter-html` — error-tolerant CST via a C external
  scanner; handles syntactic quirks but not full WHATWG tree repair, and breaks
  the pure-Rust house style.

## Measured: dependency weight (isolated crates, `cargo tree` + cold build)

| Parser | Unique transitive crates | Cold build |
|---|---:|---:|
| tree-sitter-html | 10 | ~13s |
| html5ever (+ markup5ever_rcdom) | 24 | ~26s |
| lol_html | 38 | ~36s |
| swc_html_parser (+ swc_common) | 78 | ~95s |

## Measured: binary footprint (stripped release, LTO, panic=abort, real usage)

Each parser exercised in a real `main` (so the linker keeps its code), minus a
no-dep baseline of **296 KB**:

| Parser | Binary | **Δ vs baseline** |
|---|---:|---:|
| tree-sitter-html | 396 KB | **+99 KB** |
| swc_html_parser | 732 KB | **+420 KB** |
| lol_html | 749 KB | **+437 KB** |
| html5ever | 865 KB | **+549 KB** |

Resolved versions: html5ever 0.39.0, markup5ever_rcdom 0.39.0, lol_html 3.0.0,
swc_html_parser 23.0.0 + swc_common 23.0.2, tree-sitter 0.26.9 +
tree-sitter-html 0.23.2.

## The inversion

Crate count and binary size **disagree** for the two finalists:

```
   crate count:  html5ever (24) ≪ swc (78)        → "swc is heavy"
   binary size:  swc (+420K) <  html5ever (+549K)  → the opposite
```

LTO + dead-code elimination strip the unreachable SWC ecosystem; only the
reachable HTML parser links. So "swc is bloat" is **false** for the shipped
artifact. But the ~130 KB spread among the Rust candidates is noise against
kenn's multi-MB binary — binary size does not decide it.

## Decision: html5ever (fallback swc_html_parser)

| Axis | Verdict |
|---|---|
| Binary size | swc ≈ html5ever (both noise for kenn) — tie |
| Cold build | html5ever ~26s ≪ swc ~95s — **html5ever** |
| Dependency surface | 24 ≪ 78 crates (audit / supply-chain) — **html5ever** |
| Version stability | html5ever 0.39 ≪ swc v23 fast major churn — **html5ever** |
| WHATWG completeness | both full — tie |
| Positions | swc free AST > html5ever TokenSink plumbing — swc |
| House style | pure-Rust lean = **html5ever** |

**html5ever** wins on build time, dependency surface, and version stability — not
on binary size (it is in fact the largest of the three Rust options). The cost is
a position-tracking `TokenSink`; cheap at line granularity. **swc_html_parser** is
the fallback if that plumbing proves painful — its binary cost is fine.

`tree-sitter-html` is dramatically smallest (+99 KB) but only approximates the
WHATWG tree and adds a C grammar — out under the all-quirks requirement.

## Quirk test corpus (run when implementing)

Pass = the right attr name + value + a line that maps back to the file, with
correct sibling/child nesting.

```
VOID/OPEN     <br> <img src=a> <input>           → following siblings not swallowed
VALUELESS     <input disabled> <option selected> → name extracted, value=""
UNQUOTED      <a href=/foo> <div class=btn>       → value ends at whitespace/>
SELF-CLOSE    <div/> <script/>                    → slash ignored; <div/> stays OPEN
OPTIONAL CLOSE <li>a<li>b   <p>x<p>y              → SIBLINGS (affects id enclosure)
IMPLIED TAGS  <table><tr><td>                     → tbody auto-inserted (nesting)
RAW TEXT      <script>if(a<b){}</script>          → "<b" is TEXT; raw span for Tier 3
COMMENTS      <!-- class=x -->                     → NO edge (precision)
DUP ATTR      <div class=a class=b>               → first wins
FOREIGN       <svg viewBox=".."><rect/>           → case-preserving attrs, self-close
MALFORMED     <div class="btn"> …EOF              → recover, still extract
TEMPLATING    class="{{x}}"  class="${y}"          → opaque text, grade Fuzzy/skip
CASE          <DIV CLASS=Btn>                      → tag/attr fold; value "Btn" preserved
```

The four that actually separate parsers: **raw-text** (kills naive scans + Tier 3
prerequisite), **optional-close/implied** (only full WHATWG nests right),
**comments** (precision), **malformed recovery**.

Spike harnesses: `tmp/parser-spike.sh` (deps) and `tmp/parser-size.sh` (binary).
