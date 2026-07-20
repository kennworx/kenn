import { expect, test } from "bun:test";
import * as path from "node:path";

import { version as packageVersion } from "../package.json";

const MAIN = path.join(import.meta.dir, "..", "src", "main.ts");

// `bun <file> args` — deliberately not `bun run <file> args`. Under `run`, bun
// may claim leading flags for itself, and its own `--version` / `--help` output
// would satisfy the assertions below without main.ts ever executing.
function run(...args: string[]): { code: number; stdout: string; stderr: string } {
  const proc = Bun.spawnSync(["bun", MAIN, ...args]);
  return {
    code: proc.exitCode,
    stdout: proc.stdout.toString(),
    stderr: proc.stderr.toString(),
  };
}

// `kenn init` probes each indexer with `--version` to decide whether the
// language is indexable. A non-zero exit here silently degrades TypeScript to
// the generic text fallback, so this is the contract that must not regress.
test("--version exits 0 and prints a bare version on stdout", () => {
  const { code, stdout, stderr } = run("--version");
  expect(code).toBe(0);
  expect(stdout.trim()).toMatch(/^\d+\.\d+\.\d+$/);
  expect(stderr).toBe("");
  // Guard against bun answering instead of main.ts: bun's version is a semver too.
  expect(stdout.trim()).not.toBe(Bun.version);
});

test("--version needs no workspace and emits no JSONL frames", () => {
  const { stdout } = run("--version");
  expect(stdout).not.toContain('"type"');
});

// One source of truth: a hardcoded literal here would drift from package.json
// the first time either is bumped, and the wire's tool_version with it.
test("--version is package.json's version, not a hardcoded copy", () => {
  const { stdout } = run("--version");
  expect(stdout.trim()).toBe(packageVersion);
});

test("the meta frame's tool_version matches --version", () => {
  const fixtures = path.join(import.meta.dir, "fixtures");
  const meta = JSON.parse(run("index", "--workspace", fixtures).stdout.split("\n")[0]!);
  expect(meta.tool_version).toBe(run("--version").stdout.trim());
});

// parseArgs is strict: before this was handled, any unknown option killed the
// process with a raw TypeError stack trace against the bundled source.
test("an unknown option is a usage error, not a crash", () => {
  const { code, stderr } = run("-V");
  expect(code).toBe(2);
  expect(stderr).toContain("kenn-ts:");
  expect(stderr).not.toContain("TypeError");
  expect(stderr).not.toContain("at main");
});

test("an unknown option keeps stdout clean for the JSONL channel", () => {
  const { stdout } = run("--nope");
  expect(stdout).toBe("");
});

test("--help still exits 0 and prints kenn-ts's own help", () => {
  const { code, stdout } = run("--help");
  expect(code).toBe(0);
  expect(stdout).toContain("kenn-ts index --workspace");
});

// The success path through main(): wrapping parseArgs in try/catch moved
// `values`/`positionals` behind a destructure, and indexer.test.ts calls
// indexWorkspace directly without ever entering main().
test("index --workspace streams a meta frame and exits 0", () => {
  const fixtures = path.join(import.meta.dir, "fixtures");
  const { code, stdout } = run("index", "--workspace", fixtures);
  expect(code).toBe(0);

  const first = JSON.parse(stdout.split("\n")[0]!);
  expect(first.type).toBe("meta");
  expect(first.tool).toBe("kenn-ts");
  expect(first.language).toBe("typescript");
});

test("index without --workspace is a usage error", () => {
  const { code, stderr } = run("index");
  expect(code).toBe(2);
  expect(stderr).toContain("--workspace is required");
});
