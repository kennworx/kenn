/**
 * JSONL wire format for kenn streaming indexers.
 *
 * Canonical reference for every producer (C#, future TS/Rust/Go/Python) and
 * every consumer. Mirror these shapes exactly.
 *
 * Spec: openspec/specs/dotnet-stream-indexer/spec.md (after wire-pkg-and-stubs).
 *
 * The wire is purely numeric: every cross-reference is a `Ref` (u32 id)
 * assigned by the producer at first sight. Identity is split across two
 * layers:
 *
 *   - The wire carries STRUCTURE — packages, files, stubs, full symbols,
 *     edges. Strings on the wire are language-naked, intra-package
 *     descriptors (`Models.Order#Save(int)`); the language prefix is
 *     declared once on `MetaFrame.language` and not repeated per symbol.
 *   - Identity / display strings (`pub_id`, fenced signatures, etc.) are
 *     assembled by the consumer from these structural pieces.
 *
 * Stubs are explicit: a `StubFrame` carries the minimum a consumer needs
 * to allocate a `short_id` and intern by `(key, pkg)`. A `SymbolFrame`
 * always denotes a fully-known record. When a producer emits both forms
 * for a single logical symbol, the same wire `id` is used for both — the
 * consumer keys upgrade-vs-dedup off wire-id collision.
 *
 * Partial declarations (C# `partial class`, Rust `impl` blocks) emit one
 * `SymbolFrame` per declaration site with `partial: true` and DISTINCT
 * wire ids sharing the same `(key, pkg)`. The consumer's dedup logic on
 * `(key, pkg)` collapse appends additional declaration sites without
 * dropping per-site edges. Older `PartialDefFrame` is gone.
 */

// ─────────────────────────────────────────────────────────────────────────────
// Envelope
// ─────────────────────────────────────────────────────────────────────────────

/** Discriminated union of every JSONL line. */
export type Frame =
  | MetaFrame
  | FileFrame
  | PackageFrame
  | StubFrame
  | SymbolFrame
  | EdgeFrame
  | ErrorFrame
  | EndFrame;

export type FrameType = Frame["type"];

// ─────────────────────────────────────────────────────────────────────────────
// Cross-references and primitives
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Run-local numeric id assigned by the producer. SINGLE id space across
 * files, packages, and symbols — an edge `target` may point to either a
 * file or a symbol, and the consumer dispatches by looking up the id in
 * its file/symbol tables.
 *
 * Ids are assigned monotonically starting at 1. `0` is reserved (means
 * "no reference"). Ids are NOT stable across runs.
 */
export type Ref = number;

/** 0-based [start_line, start_col, end_line, end_col]. */
export type Range = readonly [number, number, number, number];

/** SCIP-style symbol kind label. Producer chooses the most specific. */
export type SymbolKind =
  | "namespace"
  | "module"
  | "class"
  | "struct"
  | "interface"
  | "enum"
  | "enum_member"
  | "delegate"
  | "type"
  | "constructor"
  | "destructor"
  | "method"
  | "function"
  | "accessor"
  | "property"
  | "field"
  | "const"
  | "event"
  | "symbol";

/** Edge taxonomy. */
export type EdgeKind =
  | "defined_in"          // child symbol → enclosing module/namespace/type
  | "contains"            // module/namespace/package → file
  | "calls"               // caller → callee (range = invocation site)
  | "type_use"            // user → referenced type
  | "field_access"        // reader/writer → field/property (carries field_op)
  | "implements"          // concrete type → interface/base
  | "overrides"           // override → base method / interface member
  | "instantiates"        // generic type → type argument
  | "generic_constraint"  // type parameter → constraint type
  | "imports"             // module → module
  | "corresponds_to"      // symbol ↔ equivalent symbol
  | "extends_type";       // augmenting symbol (C# extension method) → extended type

export type FieldOp = "read" | "write";

export type Language = "csharp" | "typescript" | "rust" | "go" | "python";

export type ErrorSeverity = "error" | "warning";

// ─────────────────────────────────────────────────────────────────────────────
// Frame shapes
// ─────────────────────────────────────────────────────────────────────────────

/** First frame, exactly once. */
export interface MetaFrame {
  type: "meta";
  v: 1;
  project_root: string;
  tool: string;
  tool_version: string;
  language: Language;
  /** ISO 8601 UTC timestamp when the producer wrote this frame.
   *  Format: `YYYY-MM-DDTHH:mm:ss.sssZ` (millisecond precision). */
  ts: string;
}

/**
 * Per-source-file metadata. First sighting only; `id` is reused implicitly
 * by anything that references this file (edges via `target`, symbols /
 * stubs via `file` — note: file is on `defs`, not on `SymbolFrame`).
 *
 * Boolean flags omit when false (default). Producers SHOULD NOT emit
 * `"test": false` or `"external": false` — let the field's absence
 * speak.
 */
export interface FileFrame {
  type: "file";
  id: Ref;
  path: string;
  /** xxh64 of UTF-8 bytes as 16 lowercase hex chars. */
  content_hash: string;
  test?: boolean;
  external?: boolean;
  /** File-level comment trivia: one entry per comment token (file
   *  header + each namespace-leading comment), in source order. Omitted
   *  when the file has none. Raw comment text — license-boilerplate
   *  filtering is a consumer concern. */
  doc?: string[];
}

/**
 * Package metadata. One frame per logical `(name, version)` package.
 * Producers MUST intern producer-side by `(name, version)` so multi-target
 * compilations of the same package emit one frame, not many.
 *
 * Cross-language mapping of "package":
 *   - C#:         assembly / .csproj output
 *   - Rust:       crate
 *   - TypeScript: npm package
 *   - Go:         module / package path
 *   - Python:     distribution / module
 *
 * `external: true` marks packages outside the workspace (BCL, NuGet,
 * crates.io, npm). Workspace-local packages omit it (or set false).
 *
 * Producers MUST emit a `PackageFrame` BEFORE any `SymbolFrame` /
 * `StubFrame` referencing it via `pkg`.
 */
export interface PackageFrame {
  type: "package";
  id: Ref;
  name: string;
  version?: string;
  manager?: string;     // "nuget" | "cargo" | "npm" | "go" | "pypi" — short ecosystem label
  external?: boolean;
}

/**
 * Forward-ref / external-symbol stub. Carries the minimum a consumer
 * needs to allocate a short id and intern by `(key, pkg)`.
 *
 * Two producer scenarios:
 *
 *   1. Internal forward ref. Producer needs to reference a symbol it
 *      hasn't fully walked yet. Emit a `StubFrame` with `id`, then later
 *      emit a `SymbolFrame` carrying the SAME `id` once the declaration
 *      has been walked. The consumer keys upgrade off wire-id collision.
 *
 *   2. External symbol (BCL, NuGet, third-party). Producer has no source
 *      declaration at all. Emit `StubFrame` exactly once; never follow
 *      with a `SymbolFrame`. Consumer derives `external: true` from the
 *      resolved package's `external` flag.
 *
 * Producers MUST use the same `id` across the stub and the eventual
 * `SymbolFrame` upgrade. Producers MUST NOT emit a `SymbolFrame` for a
 * symbol they only have partial info on — use `StubFrame` for that.
 */
export interface StubFrame {
  type: "stub";
  id: Ref;
  kind: SymbolKind;
  name: string;
  /** Cross-run-stable, language-naked, intra-package path
   *  (`Models.Order#Save(int)`). The consumer assembles `pub_id` as
   *  `<lang_prefix>:<key>` from `MetaFrame.language`. */
  key: string;
  pkg?: Ref;
}

/**
 * Full symbol declaration. Every emitted SymbolFrame is a complete
 * record — producers use `StubFrame` instead when only partial info is
 * available.
 *
 * Locals (method-local vars, lambda params, range variables, anonymous
 * types) are NEVER emitted.
 *
 * Identity:
 *   - `key` is language-naked, intra-package, and serves as the
 *     cross-run-stable moniker. The consumer's `pub_id` is
 *     `<lang_prefix>:<key>` assembled from `MetaFrame.language`.
 *   - `(key, pkg)` is the consumer's dedup intern key. Two SymbolFrames
 *     with matching `(key, pkg)` and `partial: true` are appended as
 *     additional declaration sites; with `partial: false` (default) the
 *     second is treated as a duplicate (multi-target source-shared
 *     emission) and its outgoing edges are skipped.
 *
 * Boolean flags omit when false; numeric flags omit when 0.
 */
export interface SymbolFrame {
  type: "symbol";
  id: Ref;
  /** Owning package. `0` / omitted means cross-package or unknown. */
  pkg?: Ref;
  /** Intra-package descriptor. See `StubFrame.key` for format. */
  key: string;
  kind: SymbolKind;
  /** Simple identifier as it appears in source (`"Save"`). */
  name: string;
  /** Direct enclosing symbol's id. `0` / omitted means top-level. */
  parent?: Ref;
  /** id of the file containing the primary declaration. `0` / omitted
   *  for symbols whose declaration site is unknown. */
  file?: Ref;
  /** Identifier-span of the primary declaration site. Required. */
  range: Range;
  /** Full declaration span of the primary declaration site (whole
   *  function/class/interface/method through its closing brace, excluding
   *  leading trivia). Omitted when the declaration span is unavailable. */
  body?: Range;
  /** True when this is one of multiple declaration sites of a partial
   *  symbol (C# `partial class`, etc.). Producer emits one frame per
   *  site with distinct ids; consumer dedup-appends. */
  partial?: boolean;
  /** Argument count (methods only). */
  nargs?: number;
  /** Generic type-parameter count. */
  targs?: number;
  /** True when the declaration is in a test file / test path. */
  test?: boolean;
  /** Bare signature line (no code fence, no language hint).
   *  Code-fence wrapping is a presentation choice for the consumer. */
  sig?: string;
  /** Doc-comment XML / markdown. */
  doc?: string;
}

/**
 * One edge. Both endpoints are `Ref`s that the producer guarantees were
 * introduced (at least as stubs / package frames) before this frame.
 *
 * `range` is typically present for `calls` / `type_use` / `field_access` /
 * `instantiates` and omitted for structural edges (`defined_in`, `contains`,
 * `implements`, `overrides`, `generic_constraint`, `imports`,
 * `corresponds_to`).
 *
 * `field_op` is required for `edge_kind: "field_access"`, omitted otherwise.
 */
export interface EdgeFrame {
  type: "edge";
  edge_kind: EdgeKind;
  source: Ref;
  target: Ref;
  range?: Range;
  field_op?: FieldOp;
}

/**
 * Producer error/warning. Severity `"error"` bumps `EndFrame.stats.errors`;
 * warnings do not. Producer keeps running unless the condition is fatal.
 */
export interface ErrorFrame {
  type: "error";
  severity: ErrorSeverity;
  source: string;        // "msbuild" | "indexer" | "roslyn" | ...
  message: string;
  /** Workspace-relative file or project path, when applicable. Note: this
   *  is a STRING path, not a Ref, because errors may reference paths that
   *  never made it onto the wire as files. */
  path?: string;
  range?: Range;
  code?: string;         // vendor code, e.g. "MSB1234"
}

/** Last frame, exactly once. */
export interface EndFrame {
  type: "end";
  stats: EndStats;
  /** ISO 8601 UTC timestamp when the producer wrote this frame.
   *  Format: `YYYY-MM-DDTHH:mm:ss.sssZ` (millisecond precision). Pair
   *  with `MetaFrame.ts` to compute producer wall time. */
  ts: string;
}

export interface EndStats {
  files: number;
  symbols: number;
  edges: number;
  /** Aggregate count of `error`-severity ErrorFrames. Warnings excluded. */
  errors: number;
}

// ─────────────────────────────────────────────────────────────────────────────
// Type guards
// ─────────────────────────────────────────────────────────────────────────────

export const isMeta     = (f: Frame): f is MetaFrame    => f.type === "meta";
export const isFile     = (f: Frame): f is FileFrame    => f.type === "file";
export const isPackage  = (f: Frame): f is PackageFrame => f.type === "package";
export const isStub     = (f: Frame): f is StubFrame    => f.type === "stub";
export const isSymbol   = (f: Frame): f is SymbolFrame  => f.type === "symbol";
export const isEdge     = (f: Frame): f is EdgeFrame    => f.type === "edge";
export const isError    = (f: Frame): f is ErrorFrame   => f.type === "error";
export const isEnd      = (f: Frame): f is EndFrame     => f.type === "end";

// ─────────────────────────────────────────────────────────────────────────────
// Wire-format constants
// ─────────────────────────────────────────────────────────────────────────────

export const DEFAULT_FLUSH_BYTES = 1 << 20;   // 1 MiB
export const DEFAULT_FLUSH_FRAMES = 4096;
export const DEFAULT_BATCH_SIZE = 10_000;
export const WIRE_VERSION = 1 as const;

/** Reserved Ref value meaning "no reference". */
export const REF_NONE: Ref = 0;
