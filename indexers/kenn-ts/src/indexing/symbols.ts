import * as path from "node:path";

import ts from "typescript";

import type { EndStats, Ref, SymbolFrame } from "../../../frames";
import type { EdgeFrame } from "../../../frames";
import type { IdRegistry } from "./ids";
import { wireKind } from "./kind";
import { moduleKey, symbolKey } from "./key";
import { rangeOf } from "./range";
import type { JsonlSink } from "../wire/sink";

export interface FileCtx {
  sf: ts.SourceFile;
  checker: ts.TypeChecker;
  moduleRel: string;
  fileId: Ref;
  pkgRef: Ref;
  isTest: boolean;
  ids: IdRegistry;
  sink: JsonlSink;
  stats: EndStats;
}

type NameableName = ts.Identifier | ts.StringLiteralLike | ts.PrivateIdentifier;

function isNameable(name: ts.PropertyName): name is NameableName {
  return (
    ts.isIdentifier(name) ||
    ts.isStringLiteralLike(name) ||
    ts.isPrivateIdentifier(name)
  );
}

function docOf(ctx: FileCtx, sym: ts.Symbol): string | undefined {
  const s = ts.displayPartsToString(sym.getDocumentationComment(ctx.checker)).trim();
  return s.length > 0 ? s : undefined;
}

function signatureOf(ctx: FileCtx, sym: ts.Symbol, decl: ts.Declaration): string | undefined {
  try {
    if (ts.isFunctionLike(decl)) {
      const sig = ctx.checker.getSignatureFromDeclaration(decl);
      if (sig) return ctx.checker.signatureToString(sig);
    }
    const t = ctx.checker.getTypeOfSymbolAtLocation(sym, decl);
    return ctx.checker.typeToString(t);
  } catch {
    return undefined;
  }
}

function arityOf(decl: ts.Declaration): { nargs?: number; targs?: number } {
  const out: { nargs?: number; targs?: number } = {};
  if (ts.isFunctionLike(decl)) out.nargs = decl.parameters.length;
  const tps = (decl as { typeParameters?: ts.NodeArray<ts.TypeParameterDeclaration> })
    .typeParameters;
  if (tps && tps.length > 0) out.targs = tps.length;
  return out;
}

/** Emit a SymbolFrame for a named declaration; returns its Ref (0 if unresolved). */
function emitNamed(
  ctx: FileCtx,
  nameNode: ts.Node,
  decl: ts.Declaration,
  parentId: Ref,
): Ref {
  const sym = ctx.checker.getSymbolAtLocation(nameNode);
  if (!sym) return 0;
  const name = sym.getName();
  const kind = wireKind(sym, decl);
  const key = symbolKey(name, kind, decl, ctx.moduleRel);
  const canonical = ctx.ids.symbolId(key);

  // Declaration merging (merged interface/namespace, function overloads): emit
  // one frame per site with `partial: true` and distinct ids sharing (key,pkg).
  // The first site takes the canonical id (edge target); later sites alloc.
  const isPartial = (sym.getDeclarations() ?? []).length > 1;
  const isFirst = ctx.ids.needFull(key);
  if (!isFirst && !isPartial) return canonical;
  const id = isFirst ? canonical : ctx.ids.alloc();

  const frame: SymbolFrame = {
    type: "symbol",
    id,
    key,
    kind,
    name,
    file: ctx.fileId,
    range: rangeOf(ctx.sf, nameNode),
    body: rangeOf(ctx.sf, decl),
  };
  if (ctx.pkgRef) frame.pkg = ctx.pkgRef;
  if (parentId) frame.parent = parentId;
  if (isPartial) frame.partial = true;
  if (ctx.isTest) frame.test = true;
  const sig = signatureOf(ctx, sym, decl);
  if (sig) frame.sig = sig;
  const doc = docOf(ctx, sym);
  if (doc) frame.doc = doc;
  const { nargs, targs } = arityOf(decl);
  if (nargs !== undefined) frame.nargs = nargs;
  if (targs !== undefined) frame.targs = targs;
  ctx.sink.push(frame);
  ctx.stats.symbols += 1;

  if (isFirst && parentId) {
    const edge: EdgeFrame = {
      type: "edge",
      edge_kind: "defined_in",
      source: canonical,
      target: parentId,
    };
    ctx.sink.push(edge);
    ctx.stats.edges += 1;
  }
  return canonical;
}

/** Walk a declaration node, emitting its symbol and recursing into members. */
function visit(ctx: FileCtx, node: ts.Node, parentId: Ref): void {
  if (ts.isModuleDeclaration(node)) {
    const id = emitNamed(ctx, node.name, node, parentId);
    if (node.body && ts.isModuleBlock(node.body)) {
      node.body.forEachChild((c) => visit(ctx, c, id || parentId));
    }
    return;
  }
  if (ts.isClassDeclaration(node) || ts.isClassExpression(node)) {
    const id = node.name ? emitNamed(ctx, node.name, node, parentId) : parentId;
    node.members.forEach((m) => visit(ctx, m, id));
    return;
  }
  if (ts.isInterfaceDeclaration(node)) {
    const id = emitNamed(ctx, node.name, node, parentId);
    node.members.forEach((m) => visit(ctx, m, id));
    return;
  }
  if (ts.isEnumDeclaration(node)) {
    const id = emitNamed(ctx, node.name, node, parentId);
    node.members.forEach((m) => {
      if (isNameable(m.name)) emitNamed(ctx, m.name, m, id);
    });
    return;
  }
  if (ts.isTypeAliasDeclaration(node)) {
    emitNamed(ctx, node.name, node, parentId);
    return;
  }
  if (ts.isFunctionDeclaration(node)) {
    if (node.name) emitNamed(ctx, node.name, node, parentId);
    return;
  }
  if (
    ts.isMethodDeclaration(node) ||
    ts.isMethodSignature(node) ||
    ts.isGetAccessorDeclaration(node) ||
    ts.isSetAccessorDeclaration(node) ||
    ts.isPropertyDeclaration(node) ||
    ts.isPropertySignature(node)
  ) {
    if (isNameable(node.name)) emitNamed(ctx, node.name, node, parentId);
    return;
  }
  if (ts.isVariableStatement(node)) {
    node.declarationList.declarations.forEach((d) => {
      if (ts.isIdentifier(d.name)) emitNamed(ctx, d.name, d, parentId);
    });
    return;
  }
}

/** Emit the module symbol (D13) and all top-level declarations of a file. */
export function emitFileSymbols(ctx: FileCtx): Ref {
  const { sf } = ctx;
  let moduleId: Ref = 0;
  if (ts.isExternalModule(sf)) {
    const key = moduleKey(ctx.moduleRel);
    moduleId = ctx.ids.symbolId(key);
    if (ctx.ids.needFull(key)) {
      const frame: SymbolFrame = {
        type: "symbol",
        id: moduleId,
        key,
        kind: "module",
        name: path.basename(ctx.moduleRel),
        file: ctx.fileId,
        range: [0, 0, 0, 0],
      };
      if (ctx.pkgRef) frame.pkg = ctx.pkgRef;
      if (ctx.isTest) frame.test = true;
      ctx.sink.push(frame);
      ctx.stats.symbols += 1;
    }
  }
  const containerId = moduleId || ctx.pkgRef;
  if (containerId) {
    const edge: EdgeFrame = {
      type: "edge",
      edge_kind: "contains",
      source: containerId,
      target: ctx.fileId,
    };
    ctx.sink.push(edge);
    ctx.stats.edges += 1;
  }
  sf.forEachChild((n) => visit(ctx, n, moduleId));
  return moduleId;
}
