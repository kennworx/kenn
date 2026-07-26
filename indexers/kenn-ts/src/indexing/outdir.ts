import * as path from "node:path";

/**
 * One project's compiled-output mapping: its `outDir` and the `rootDir` the
 * sources were compiled from, both absolute.
 */
export interface OutDirPair {
  outDir: string;
  rootDir: string;
}

/**
 * Maps a compiled declaration file back to the source it was generated from.
 *
 * In a workspace, package `b` importing package `a` resolves through `a`'s
 * `package.json` `types` field to `a/dist/index.d.ts` — the BUILD OUTPUT, not
 * `a/src/index.ts`. That declaration is a different file, so it gets a different
 * symbol key from the source definition, and every cross-package reference in
 * the monorepo lands on a symbol that is neither the real definition nor
 * first-party. The result is a package graph with no internal edges between
 * workspace packages at all.
 *
 * The mapping is DERIVED, never guessed: `outDir` and `rootDir` are declared in
 * the package's own `tsconfig.json`, so `<outDir>/p.d.ts` → `<rootDir>/p.ts` is
 * exactly the inverse of what the compiler was told to do. Repos that enable
 * `declarationMap` publish this same mapping as `.d.ts.map`; deriving it from
 * the config means it also works for the (common) case where they don't.
 */
export class OutDirMap {
  private readonly pairs: OutDirPair[];

  constructor(pairs: OutDirPair[]) {
    // Deepest `outDir` first: nested projects must win over an ancestor whose
    // outDir would also match the path.
    this.pairs = [...pairs].sort(
      (a, b) => b.outDir.split(path.sep).length - a.outDir.split(path.sep).length,
    );
  }

  /**
   * The source files `absFile` could have been compiled from, most likely first,
   * or `[]` when it is not a declaration file under a known `outDir`. A `.js`
   * under an outDir is emitted output with no symbol of its own, so only
   * declaration files map.
   *
   * Why a LIST. The extension is not invertible one-to-one:
   * - `.d.mts` / `.d.cts` are what a dual ESM+CJS build emits (tsup, unbuild,
   *   modern `tsc`). Matching only `.d.ts` left those unmapped, so every
   *   cross-package reference into such a package kept keying to build output —
   *   the very defect this class exists to remove, just spelled differently.
   * - `Button.tsx` compiles to `Button.d.ts`. Mapping `.d.ts` to `.ts` alone
   *   produced a path that does not exist, and the reference silently lost its
   *   edge. Every `.tsx`-authored component in a workspace package was affected.
   *
   * Candidates are ordered the way `tsc` resolves: the exact-extension inverse
   * first, then `.tsx` for the `.d.ts` case. Still no filesystem access — the
   * caller keeps only a candidate that was actually indexed, so a mapping with
   * no corresponding source degrades to "no edge" rather than to a wrong one.
   */
  toSource(absFile: string): string[] {
    // `.d.mts` → `.mts`, `.d.cts` → `.cts` are exact inverses of the emit.
    // `.d.ts` is the ambiguous one: `.ts` OR `.tsx`.
    const suffixes: ReadonlyArray<readonly [string, readonly string[]]> = [
      [".d.mts", [".mts"]],
      [".d.cts", [".cts"]],
      [".d.ts", [".ts", ".tsx"]],
    ];
    const match = suffixes.find(([decl]) => absFile.endsWith(decl));
    if (!match) return [];
    const [declExt, sourceExts] = match;
    for (const { outDir, rootDir } of this.pairs) {
      const rel = path.relative(outDir, absFile);
      if (rel.startsWith("..") || path.isAbsolute(rel)) continue;
      const stem = rel.slice(0, rel.length - declExt.length);
      return sourceExts.map((ext) => path.join(rootDir, stem + ext));
    }
    return [];
  }
}

/**
 * Whether `absFile` is a TypeScript declaration file, by any of the extensions a
 * build can emit. `isExternalPath` uses this to keep compiler output out of the
 * first-party symbol space: testing only `.d.ts` let a workspace-internal
 * `dist/index.d.mts` through as INTERNAL, which is worse than a missing edge —
 * the build output becomes a first-party symbol competing with its own source.
 */
export function isDeclarationFile(absFile: string): boolean {
  return absFile.endsWith(".d.ts") || absFile.endsWith(".d.mts") || absFile.endsWith(".d.cts");
}
