import * as fs from "node:fs";
import * as path from "node:path";

import type { PackageFrame, Ref } from "../../../frames";
import type { IdRegistry } from "./ids";
import type { JsonlSink } from "../wire/sink";

function isExternalDir(dir: string, root: string): boolean {
  const rel = path.relative(root, dir);
  return rel.startsWith("..") || rel.split(path.sep).includes("node_modules");
}

/**
 * Resolves a file's owning package by walking up to the nearest `package.json`
 * (name + version), emitting one `PackageFrame` per `(name, version)`.
 * node_modules / out-of-workspace packages are flagged `external`.
 */
export class Packages {
  private dirToPkg = new Map<string, Ref>();
  private byNameVersion = new Map<string, Ref>();

  constructor(
    private readonly root: string,
    private readonly ids: IdRegistry,
    private readonly sink: JsonlSink,
  ) {}

  /** Package ref for the file's directory (0 if none found). */
  forFile(absFile: string): Ref {
    return this.walk(path.dirname(absFile));
  }

  private walk(dir: string): Ref {
    const cached = this.dirToPkg.get(dir);
    if (cached !== undefined) return cached;
    const pj = path.join(dir, "package.json");
    let result: Ref;
    if (fs.existsSync(pj) && fs.statSync(pj).isFile()) {
      result = this.fromPackageJson(pj, dir);
    } else {
      const parent = path.dirname(dir);
      result = parent === dir ? 0 : this.walk(parent);
    }
    this.dirToPkg.set(dir, result);
    return result;
  }

  private fromPackageJson(pj: string, dir: string): Ref {
    let name = "";
    let version: string | undefined;
    try {
      const j = JSON.parse(fs.readFileSync(pj, "utf8")) as {
        name?: unknown;
        version?: unknown;
      };
      if (typeof j.name === "string") name = j.name;
      if (typeof j.version === "string") version = j.version;
    } catch {
      // unreadable/invalid package.json → fall through to basename fallback
    }
    // The package.json `name` is the npm-canonical identity — the source of truth
    // for a project's name, in-workspace or external. Only when it is absent (or
    // the manifest is unreadable) do we fall back to the directory basename. A
    // misleading `name` (a product name on a subproject, a stale starter-kit
    // template) is the *repo's* data, reported faithfully — not something this
    // indexer rewrites to the directory behind the user's back.
    if (!name) name = path.basename(dir);

    const nv = `${name}@${version ?? ""}`;
    const existing = this.byNameVersion.get(nv);
    if (existing !== undefined) return existing;

    const id = this.ids.alloc();
    this.byNameVersion.set(nv, id);
    const frame: PackageFrame = { type: "package", id, name, manager: "npm" };
    if (version) frame.version = version;
    if (isExternalDir(dir, this.root)) frame.external = true;
    this.sink.push(frame);
    return id;
  }
}
