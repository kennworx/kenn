import { afterAll, expect, test } from "bun:test";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import type { Frame, PackageFrame } from "../../frames";
import { indexWorkspace } from "../src/indexing/core";
import { JsonlSink } from "../src/wire/sink";

// One workspace holding two in-repo projects:
//   packages/web-app  → package.json `name: "@acme/web"` (name ≠ directory)
//   libs/plain        → package.json with NO `name`      (basename fallback)
// The npm `name` is the source of truth, so the first is named by its declared
// name (not its `web-app` directory); the second, lacking a name, falls back to
// its directory basename.
const ws = fs.mkdtempSync(path.join(os.tmpdir(), "kenn-ts-pkg-"));
afterAll(() => fs.rmSync(ws, { recursive: true, force: true }));

function packagesOf(root: string): PackageFrame[] {
  const frames: Frame[] = [];
  const sink = new JsonlSink((chunk) => {
    for (const line of chunk.split("\n")) {
      if (line.trim()) frames.push(JSON.parse(line) as Frame);
    }
  });
  indexWorkspace(root, [], sink);
  sink.flush();
  return frames.filter((f): f is PackageFrame => f.type === "package");
}

function writeProject(rel: string, pkg: Record<string, unknown>): void {
  const dir = path.join(ws, rel);
  fs.mkdirSync(path.join(dir, "src"), { recursive: true });
  fs.writeFileSync(path.join(dir, "package.json"), JSON.stringify(pkg));
  fs.writeFileSync(path.join(dir, "src", "app.ts"), "export class App {}\n");
}

test("a package's name is its package.json `name` (npm source of truth), not its directory", () => {
  writeProject(path.join("packages", "web-app"), { name: "@acme/web", version: "1.0.0" });
  writeProject(path.join("libs", "plain"), { version: "1.0.0" }); // no `name`
  fs.writeFileSync(
    path.join(ws, "tsconfig.json"),
    JSON.stringify({ compilerOptions: { strict: false }, include: ["**/*.ts"] }),
  );

  const names = packagesOf(ws).map((p) => p.name);
  // The declared name wins over the `web-app` directory.
  expect(names).toContain("@acme/web");
  expect(names).not.toContain("web-app");
  expect(names).not.toContain("packages/web-app");
  // A manifest with no `name` falls back to the directory basename.
  expect(names).toContain("plain");
});
