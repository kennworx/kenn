import { expect, test } from "bun:test";
import * as path from "node:path";

import type { EdgeFrame, Frame, SymbolFrame } from "../../frames";
import { indexWorkspace } from "../src/indexing/core";
import { JsonlSink } from "../src/wire/sink";

const FIXTURES = path.join(import.meta.dir, "fixtures");

function runFixtures(): Frame[] {
  const frames: Frame[] = [];
  const sink = new JsonlSink((chunk) => {
    for (const line of chunk.split("\n")) {
      if (line.trim()) frames.push(JSON.parse(line) as Frame);
    }
  });
  indexWorkspace(FIXTURES, [], sink);
  sink.flush();
  return frames;
}

const frames = runFixtures();
const symbols = frames.filter((f): f is SymbolFrame => f.type === "symbol");
const edges = frames.filter((f): f is EdgeFrame => f.type === "edge");
const edgeKinds = new Set(edges.map((e) => e.edge_kind));

test("top-level function is kind 'function' with type-param count", () => {
  const fn = symbols.find((s) => s.name === "identity");
  expect(fn?.kind).toBe("function");
  expect(fn?.targs).toBe(1);
});

test("enum members are kind 'enum_member'", () => {
  const members = symbols.filter((s) => s.kind === "enum_member").map((s) => s.name);
  expect(members).toEqual(expect.arrayContaining(["Red", "Green"]));
});

test("merged interface emits partial sites sharing one key", () => {
  const shape = symbols.filter((s) => s.name === "Shape" && s.kind === "interface");
  expect(shape.length).toBe(2);
  expect(shape.every((s) => s.partial === true)).toBe(true);
  expect(new Set(shape.map((s) => s.key)).size).toBe(1);
});

test("file-level comment blocks captured raw on FileFrame.doc", () => {
  const file = frames.find((f) => f.type === "file" && f.path.endsWith("sample.ts"));
  const doc = file && file.type === "file" ? (file.doc ?? []) : [];
  expect(doc.length).toBe(2);
  expect(doc[0]).toContain("Copyright");
  expect(doc[1]).toContain("@fileoverview");
});

test("full edge taxonomy present", () => {
  for (const k of ["calls", "type_use", "field_access", "implements", "imports", "defined_in", "contains"]) {
    expect(edgeKinds.has(k as EdgeFrame["edge_kind"])).toBe(true);
  }
});

test("field access classifies read vs write", () => {
  const fa = edges.filter((e) => e.edge_kind === "field_access");
  const ops = new Set(fa.map((e) => e.field_op));
  expect(ops.has("write")).toBe(true);
  expect(ops.has("read")).toBe(true);
});

test("a module symbol anchors the file (D13)", () => {
  const mod = symbols.find((s) => s.kind === "module" && s.name === "sample.ts");
  expect(mod).toBeDefined();
});
