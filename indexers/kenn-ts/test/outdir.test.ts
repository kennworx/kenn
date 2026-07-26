import { describe, expect, test } from "bun:test";
import * as path from "node:path";

import { isDeclarationFile, OutDirMap } from "../src/indexing/outdir";

const ws = path.resolve("/ws");
const core = { outDir: path.join(ws, "packages/core/dist"), rootDir: path.join(ws, "packages/core/src") };

describe("OutDirMap", () => {
  test("maps a built declaration back to the source it came from", () => {
    // The whole point: `import … from "@scope/core"` resolves through the
    // package's `types` field into dist, so without this the reference keys on
    // a path the source definition never had.
    const m = new OutDirMap([core]);
    // `.d.ts` is ambiguous — `.ts` first (tsc's own resolution order), `.tsx`
    // second, and the caller keeps whichever was actually indexed.
    expect(m.toSource(path.join(ws, "packages/core/dist/okf/knowledge-base.d.ts"))).toEqual([
      path.join(ws, "packages/core/src/okf/knowledge-base.ts"),
      path.join(ws, "packages/core/src/okf/knowledge-base.tsx"),
    ]);
  });

  test("leaves anything outside a known outDir alone", () => {
    const m = new OutDirMap([core]);
    // Real source, already correct.
    expect(m.toSource(path.join(ws, "packages/core/src/index.ts"))).toEqual([]);
    // A genuine external declaration must keep its identity, or it would be
    // rewritten into a workspace path that does not exist.
    expect(m.toSource(path.join(ws, "node_modules/@types/react/index.d.ts"))).toEqual([]);
    // Emitted JS under the outDir is not a declaration and has no symbol.
    expect(m.toSource(path.join(ws, "packages/core/dist/index.js"))).toEqual([]);
  });

  test("prefers the deepest outDir when projects nest", () => {
    const outer = { outDir: path.join(ws, "dist"), rootDir: path.join(ws, "src") };
    const inner = {
      outDir: path.join(ws, "dist/pkg"),
      rootDir: path.join(ws, "packages/pkg/src"),
    };
    // Registration order must not decide the winner.
    for (const pairs of [[outer, inner], [inner, outer]]) {
      const m = new OutDirMap(pairs);
      expect(m.toSource(path.join(ws, "dist/pkg/a.d.ts"))[0]).toBe(
        path.join(ws, "packages/pkg/src/a.ts"),
      );
    }
  });

  test("no configured pairs is a no-op", () => {
    expect(new OutDirMap([]).toSource(path.join(ws, "packages/core/dist/x.d.ts"))).toEqual([]);
  });

  // A dual ESM+CJS build (tsup, unbuild, modern tsc) emits `.d.mts`/`.d.cts`
  // alongside `.d.ts`. Matching only `.d.ts` left those unmapped, so every
  // cross-package reference into such a package kept keying to build output —
  // the exact defect OutDirMap exists to remove, spelled differently.
  //
  // Each maps to its own extension, which is the precise inverse of the emit,
  // never a guess: an `.mts` source cannot have produced a `.d.cts`.
  test("maps the declaration extensions a dual ESM/CJS build emits", () => {
    const m = new OutDirMap([core]);
    expect(m.toSource(path.join(ws, "packages/core/dist/index.d.mts"))).toEqual([
      path.join(ws, "packages/core/src/index.mts"),
    ]);
    expect(m.toSource(path.join(ws, "packages/core/dist/index.d.cts"))).toEqual([
      path.join(ws, "packages/core/src/index.cts"),
    ]);
  });

  // `Button.tsx` compiles to `Button.d.ts`, so mapping to `.ts` alone produced a
  // path that does not exist and the reference silently lost its edge. Every
  // `.tsx`-authored component exported from a workspace package was affected.
  test("offers .tsx as a candidate for an ambiguous .d.ts", () => {
    const m = new OutDirMap([core]);
    const got = m.toSource(path.join(ws, "packages/core/dist/Button.d.ts"));
    expect(got).toContain(path.join(ws, "packages/core/src/Button.tsx"));
    // Order matters: `.ts` is tried first, matching how tsc resolves.
    expect(got[0]).toBe(path.join(ws, "packages/core/src/Button.ts"));
  });

  // A path ending in `.d.ts` must not be shortened by the `.mts` arm, and vice
  // versa — a naive `replace(/\.d\.[mc]?ts$/)` on `x.d.ts` would leave `x.d`.
  test("strips exactly the declaration suffix", () => {
    const m = new OutDirMap([core]);
    expect(m.toSource(path.join(ws, "packages/core/dist/a.b.d.ts"))[0]).toBe(
      path.join(ws, "packages/core/src/a.b.ts"),
    );
  });
});

describe("isDeclarationFile", () => {
  // `isExternalPath` uses this to keep compiler output out of the first-party
  // symbol space. A `.d.ts`-only test let a workspace-internal
  // `dist/index.d.mts` through as INTERNAL — worse than a lost edge, because the
  // build output then becomes a first-party symbol competing with its source.
  test("covers every declaration extension a build emits", () => {
    expect(isDeclarationFile("/ws/dist/index.d.ts")).toBe(true);
    expect(isDeclarationFile("/ws/dist/index.d.mts")).toBe(true);
    expect(isDeclarationFile("/ws/dist/index.d.cts")).toBe(true);
  });

  test("is not fooled by ordinary sources", () => {
    for (const p of [
      "/ws/src/index.ts",
      "/ws/src/index.mts",
      "/ws/src/index.cts",
      "/ws/src/Button.tsx",
      "/ws/dist/index.js",
      // Not a declaration: the `d` is part of the name.
      "/ws/src/mod.ts",
    ]) {
      expect(isDeclarationFile(p)).toBe(false);
    }
  });
});
