import * as fs from "node:fs";
import * as path from "node:path";

import ts from "typescript";

import type { EndStats, ErrorFrame, FileFrame } from "../../../frames";
import type { JsonlSink } from "../wire/sink";
import { emitFileEdges } from "./edges";
import { extractFileDoc } from "./file-doc";
import { IdRegistry } from "./ids";
import { Packages } from "./packages";
import { emitFileSymbols } from "./symbols";
import { isTestPath } from "./test-path";
import { discoverTsconfigs } from "./tsconfig";

interface WalkedFile {
  sf: ts.SourceFile;
  checker: ts.TypeChecker;
  moduleRel: string;
  fileId: number;
  moduleId: number;
}

type SourceCache = Map<string, ts.SourceFile>;

/** Build a program for one tsconfig, sharing parsed source files across projects. */
function buildProgram(tsconfigPath: string, cache: SourceCache): ts.Program {
  const cfg = ts.readConfigFile(tsconfigPath, ts.sys.readFile);
  const parsed = ts.parseJsonConfigFileContent(
    cfg.config ?? {},
    ts.sys,
    path.dirname(tsconfigPath),
  );
  // A target repo's tsconfig must never be able to write to OUR protocol
  // stream. `traceResolution` makes the compiler emit module-resolution
  // diagnostics through its `trace` callback, which defaults to console.log —
  // i.e. stdout, which IS the JSONL wire. Measured on a real repo (zod sets it
  // in packages/bench/tsconfig.json): line 1 became "Found 'package.json' at
  // …" and the whole run failed with "json on line 1: expected value".
  //
  // Overridden rather than silenced at the host, because any future option that
  // writes diagnostics should be answered the same way: the target does not get
  // to choose what goes on our stdout.
  parsed.options.traceResolution = false;
  const host = ts.createCompilerHost(parsed.options);
  // Belt and braces: if some other path still requests a trace, send it to
  // stderr where kenn already captures driver output.
  host.trace = (s: string) => process.stderr.write(`${s}\n`);
  const original = host.getSourceFile.bind(host);
  host.getSourceFile = (fileName, languageVersion, onError, shouldCreate) => {
    const cached = cache.get(fileName);
    if (cached) return cached;
    const sf = original(fileName, languageVersion, onError, shouldCreate);
    if (sf) cache.set(fileName, sf);
    return sf;
  };
  return ts.createProgram(parsed.fileNames, parsed.options, host);
}

/** xxh64 of the file's on-disk UTF-8 bytes, 16 lowercase hex chars (matches the C# producer). */
function contentHash(absPath: string): string {
  const bytes = fs.readFileSync(absPath);
  return Bun.hash.xxHash64(bytes).toString(16).padStart(16, "0");
}

export function indexWorkspace(
  workspace: string,
  tsconfigArgs: string[],
  sink: JsonlSink,
): EndStats {
  const root = path.resolve(workspace);
  const configs =
    tsconfigArgs.length > 0
      ? tsconfigArgs.map((c) => path.resolve(root, c))
      : discoverTsconfigs(root);

  const ids = new IdRegistry();
  const packages = new Packages(root, ids, sink);
  const sourceCache: SourceCache = new Map();
  const stats: EndStats = { files: 0, symbols: 0, edges: 0, errors: 0 };
  const walked: WalkedFile[] = [];

  // Pass 1: files + definitions (so internal symbols are full before edges).
  for (const cfg of configs) {
    let program: ts.Program;
    try {
      program = buildProgram(cfg, sourceCache);
    } catch (e) {
      const err: ErrorFrame = {
        type: "error",
        severity: "error",
        source: "indexer",
        message: `failed to load project ${cfg}: ${String(e)}`,
        path: path.relative(root, cfg),
      };
      sink.push(err);
      stats.errors += 1;
      continue;
    }

    const checker = program.getTypeChecker();
    const inSet = new Set(program.getRootFileNames());
    for (const sf of program.getSourceFiles()) {
      if (!inSet.has(sf.fileName) || sf.isDeclarationFile) continue;
      const rel = path.relative(root, sf.fileName);
      const { id, isNew } = ids.internFile(rel);
      if (!isNew) continue;

      const isTest = isTestPath(rel);
      const frame: FileFrame = {
        type: "file",
        id,
        path: rel,
        content_hash: contentHash(sf.fileName),
      };
      if (isTest) frame.test = true;
      const doc = extractFileDoc(sf);
      if (doc.length > 0) frame.doc = doc;
      sink.push(frame);
      stats.files += 1;

      const pkgRef = packages.forFile(sf.fileName);
      const moduleId = emitFileSymbols({
        sf,
        checker,
        moduleRel: rel,
        fileId: id,
        pkgRef,
        isTest,
        ids,
        sink,
        stats,
      });
      walked.push({ sf, checker, moduleRel: rel, fileId: id, moduleId });
    }
  }

  // Pass 2: edges (internal targets resolve to full symbols; externals stub).
  for (const w of walked) {
    emitFileEdges({
      sf: w.sf,
      checker: w.checker,
      moduleRel: w.moduleRel,
      moduleId: w.moduleId,
      root,
      ids,
      packages,
      sink,
      stats,
    });
  }

  return stats;
}
