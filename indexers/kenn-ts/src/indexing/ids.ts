import type { Ref } from "../../../frames";

type SymState = "stub" | "full";

/**
 * Producer-assigned numeric ids. Single monotonic id space shared by files,
 * packages, and symbols (per the wire). Starts at 1; 0 is reserved.
 *
 * Symbols intern by their descriptor `key` (cross-program stable — `ts.Symbol`
 * objects are per-program, so the string key is the canonical identity). State
 * tracks stub-vs-full so a forward/external reference emits a `StubFrame` once
 * and a later definition upgrades to a `SymbolFrame` reusing the same `Ref`.
 */
export class IdRegistry {
  private nextId: Ref = 1;
  private fileIds = new Map<string, Ref>();
  private symIds = new Map<string, Ref>();
  private symState = new Map<string, SymState>();

  internFile(relPath: string): { id: Ref; isNew: boolean } {
    const existing = this.fileIds.get(relPath);
    if (existing !== undefined) return { id: existing, isNew: false };
    const id = this.nextId++;
    this.fileIds.set(relPath, id);
    return { id, isNew: true };
  }

  /** Stable id for a symbol key (interns on first sight). */
  symbolId(key: string): Ref {
    let id = this.symIds.get(key);
    if (id === undefined) {
      id = this.nextId++;
      this.symIds.set(key, id);
    }
    return id;
  }

  /** True if a full `SymbolFrame` should be emitted now (not already full). */
  needFull(key: string): boolean {
    if (this.symState.get(key) === "full") return false;
    this.symState.set(key, "full");
    return true;
  }

  /** True if a `StubFrame` should be emitted now (no stub/full emitted yet). */
  needStub(key: string): boolean {
    if (this.symState.has(key)) return false;
    this.symState.set(key, "stub");
    return true;
  }

  /** Current emission state of a key without interning it. */
  peekState(key: string): SymState | undefined {
    return this.symState.get(key);
  }

  /** Has a full SymbolFrame been emitted for this key? */
  hasFull(key: string): boolean {
    return this.symState.get(key) === "full";
  }

  alloc(): Ref {
    return this.nextId++;
  }
}
