import * as path from "node:path";

import ts from "typescript";

import type {
  EdgeFrame,
  EdgeKind,
  EndStats,
  FieldOp,
  Range,
  Ref,
  StubFrame,
} from "../../../frames";
import type { IdRegistry } from "./ids";
import { wireKind } from "./kind";
import { moduleKey, symbolKey } from "./key";
import type { Packages } from "./packages";
import { rangeOf } from "./range";
import type { JsonlSink } from "../wire/sink";

export interface EdgeCtx {
  sf: ts.SourceFile;
  checker: ts.TypeChecker;
  moduleRel: string;
  moduleId: Ref;
  root: string;
  ids: IdRegistry;
  packages: Packages;
  sink: JsonlSink;
  stats: EndStats;
}

function isExternal(declSf: ts.SourceFile, root: string): boolean {
  const rel = path.relative(root, declSf.fileName);
  return (
    rel.startsWith("..") ||
    rel.split(path.sep).includes("node_modules") ||
    declSf.isDeclarationFile
  );
}

function symAt(ctx: EdgeCtx, node: ts.Node): ts.Symbol | undefined {
  return ctx.checker.getSymbolAtLocation(node);
}

function pushEdge(
  ctx: EdgeCtx,
  edgeKind: EdgeKind,
  source: Ref,
  target: Ref,
  range?: Range,
  fieldOp?: FieldOp,
): void {
  if (!source || !target || source === target) return;
  const e: EdgeFrame = { type: "edge", edge_kind: edgeKind, source, target };
  if (range) e.range = range;
  if (fieldOp) e.field_op = fieldOp;
  ctx.sink.push(e);
  ctx.stats.edges += 1;
}

/**
 * Resolve a referenced symbol to a Ref. External symbols (node_modules / lib
 * `.d.ts`) emit a `StubFrame` once. Internal symbols link only when already
 * emitted as a full symbol — locals/params resolve to 0 (no edge).
 */
function ensureRef(ctx: EdgeCtx, symRaw: ts.Symbol | undefined): Ref {
  if (!symRaw) return 0;
  let sym = symRaw;
  if (sym.flags & ts.SymbolFlags.Alias) {
    try {
      sym = ctx.checker.getAliasedSymbol(sym);
    } catch {
      /* keep original */
    }
  }
  const decl = sym.declarations?.[0];
  if (!decl) return 0;
  const name = sym.getName();
  if (!name || name.startsWith("__")) return 0;
  const declSf = decl.getSourceFile();
  const rel = path.relative(ctx.root, declSf.fileName);
  const kind = wireKind(sym, decl);
  const key = symbolKey(name, kind, decl, rel);

  if (isExternal(declSf, ctx.root)) {
    const id = ctx.ids.symbolId(key);
    if (ctx.ids.needStub(key)) {
      const stub: StubFrame = { type: "stub", id, kind, name, key };
      const pkg = ctx.packages.forFile(declSf.fileName);
      if (pkg) stub.pkg = pkg;
      ctx.sink.push(stub);
    }
    return id;
  }
  return ctx.ids.hasFull(key) ? ctx.ids.symbolId(key) : 0;
}

/** Resolve an import/export module specifier to the imported module's Ref. */
function moduleRef(ctx: EdgeCtx, spec: ts.StringLiteralLike): Ref {
  const sym = symAt(ctx, spec);
  const decl = sym?.declarations?.[0];
  if (!decl) return 0;
  const declSf = decl.getSourceFile();
  const rel = path.relative(ctx.root, declSf.fileName);
  const key = moduleKey(rel);
  if (isExternal(declSf, ctx.root)) {
    const id = ctx.ids.symbolId(key);
    if (ctx.ids.needStub(key)) {
      const stub: StubFrame = {
        type: "stub",
        id,
        kind: "module",
        name: path.basename(rel),
        key,
      };
      const pkg = ctx.packages.forFile(declSf.fileName);
      if (pkg) stub.pkg = pkg;
      ctx.sink.push(stub);
    }
    return id;
  }
  return ctx.ids.hasFull(key) ? ctx.ids.symbolId(key) : 0;
}

function typeParamsOf(node: ts.Node): ts.NodeArray<ts.TypeParameterDeclaration> | undefined {
  return (node as { typeParameters?: ts.NodeArray<ts.TypeParameterDeclaration> })
    .typeParameters;
}

function declNameNode(node: ts.Node): ts.Node | undefined {
  if (
    (ts.isClassDeclaration(node) ||
      ts.isClassExpression(node) ||
      ts.isFunctionDeclaration(node) ||
      ts.isInterfaceDeclaration(node) ||
      ts.isEnumDeclaration(node) ||
      ts.isTypeAliasDeclaration(node) ||
      ts.isModuleDeclaration(node)) &&
    node.name
  ) {
    return node.name;
  }
  if (
    (ts.isMethodDeclaration(node) ||
      ts.isPropertyDeclaration(node) ||
      ts.isGetAccessorDeclaration(node) ||
      ts.isSetAccessorDeclaration(node) ||
      ts.isEnumMember(node)) &&
    (ts.isIdentifier(node.name) || ts.isStringLiteralLike(node.name))
  ) {
    return node.name;
  }
  if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name)) return node.name;
  return undefined;
}

/** Id of `node` if it is a full-emitted symbol declaration, else 0. */
function declId(ctx: EdgeCtx, node: ts.Node): Ref {
  const nameNode = declNameNode(node);
  if (!nameNode) return 0;
  const sym = symAt(ctx, nameNode);
  if (!sym) return 0;
  const key = symbolKey(sym.getName(), wireKind(sym, node as ts.Declaration), node as ts.Declaration, ctx.moduleRel);
  return ctx.ids.hasFull(key) ? ctx.ids.symbolId(key) : 0;
}

function isAssignmentOperator(kind: ts.SyntaxKind): boolean {
  return kind >= ts.SyntaxKind.FirstAssignment && kind <= ts.SyntaxKind.LastAssignment;
}

function fieldOp(node: ts.PropertyAccessExpression): FieldOp {
  let cur: ts.Node = node;
  let parent: ts.Node | undefined = node.parent;
  while (parent) {
    if (
      ts.isBinaryExpression(parent) &&
      parent.left === cur &&
      isAssignmentOperator(parent.operatorToken.kind)
    ) {
      return "write";
    }
    if (
      (ts.isPostfixUnaryExpression(parent) || ts.isPrefixUnaryExpression(parent)) &&
      (parent.operator === ts.SyntaxKind.PlusPlusToken ||
        parent.operator === ts.SyntaxKind.MinusMinusToken)
    ) {
      return "write";
    }
    if (!ts.isParenthesizedExpression(parent)) break;
    cur = parent;
    parent = parent.parent;
  }
  return "read";
}

function emitTypeArgs(
  ctx: EdgeCtx,
  args: ts.NodeArray<ts.TypeNode> | undefined,
  src: Ref,
): void {
  if (!args) return;
  for (const a of args) {
    if (ts.isTypeReferenceNode(a)) {
      pushEdge(ctx, "instantiates", src, ensureRef(ctx, symAt(ctx, a.typeName)), rangeOf(ctx.sf, a));
    }
  }
}

function emitConstraints(ctx: EdgeCtx, node: ts.Node, ownerId: Ref): void {
  const tps = typeParamsOf(node);
  if (!tps || !ownerId) return;
  for (const tp of tps) {
    if (tp.constraint && ts.isTypeReferenceNode(tp.constraint)) {
      pushEdge(
        ctx,
        "generic_constraint",
        ownerId,
        ensureRef(ctx, symAt(ctx, tp.constraint.typeName)),
      );
    }
  }
}

function emitHeritage(
  ctx: EdgeCtx,
  node: ts.ClassLikeDeclaration | ts.InterfaceDeclaration,
  ownerId: Ref,
): void {
  if (!ownerId || !node.heritageClauses) return;
  for (const clause of node.heritageClauses) {
    for (const t of clause.types) {
      pushEdge(ctx, "implements", ownerId, ensureRef(ctx, symAt(ctx, t.expression)));
      emitTypeArgs(ctx, t.typeArguments, ownerId);
    }
  }
}

function emitOverride(ctx: EdgeCtx, method: ts.MethodDeclaration, methodId: Ref): void {
  if (!methodId || !ts.isIdentifier(method.name)) return;
  const cls = method.parent;
  if (!ts.isClassLike(cls) || !cls.heritageClauses) return;
  const memberName = method.name.text;
  for (const clause of cls.heritageClauses) {
    for (const t of clause.types) {
      const baseType = ctx.checker.getTypeAtLocation(t);
      const baseMember = baseType.getProperty(memberName);
      if (baseMember) pushEdge(ctx, "overrides", methodId, ensureRef(ctx, baseMember));
    }
  }
}

function walk(ctx: EdgeCtx, node: ts.Node, sourceId: Ref): void {
  let src = sourceId;
  const id = declId(ctx, node);
  if (id) src = id;

  if (id) emitConstraints(ctx, node, id);
  if (ts.isClassLike(node) || ts.isInterfaceDeclaration(node)) {
    emitHeritage(ctx, node, id || src);
  }
  if (ts.isMethodDeclaration(node) && ts.getCombinedModifierFlags(node) & ts.ModifierFlags.Override) {
    emitOverride(ctx, node, id || src);
  }

  if (ts.isCallExpression(node)) {
    pushEdge(ctx, "calls", src, ensureRef(ctx, symAt(ctx, node.expression)), rangeOf(ctx.sf, node.expression));
    emitTypeArgs(ctx, node.typeArguments, src);
  } else if (ts.isNewExpression(node)) {
    pushEdge(ctx, "calls", src, ensureRef(ctx, symAt(ctx, node.expression)), rangeOf(ctx.sf, node.expression));
    emitTypeArgs(ctx, node.typeArguments, src);
  } else if (ts.isPropertyAccessExpression(node)) {
    const isCallee = ts.isCallExpression(node.parent) && node.parent.expression === node;
    if (!isCallee) {
      pushEdge(ctx, "field_access", src, ensureRef(ctx, symAt(ctx, node)), rangeOf(ctx.sf, node.name), fieldOp(node));
    }
  } else if (ts.isTypeReferenceNode(node)) {
    pushEdge(ctx, "type_use", src, ensureRef(ctx, symAt(ctx, node.typeName)), rangeOf(ctx.sf, node.typeName));
    emitTypeArgs(ctx, node.typeArguments, src);
  } else if (ts.isImportDeclaration(node)) {
    if (node.moduleSpecifier && ts.isStringLiteralLike(node.moduleSpecifier)) {
      pushEdge(ctx, "imports", ctx.moduleId, moduleRef(ctx, node.moduleSpecifier));
    }
  } else if (
    ts.isExportDeclaration(node) &&
    node.moduleSpecifier &&
    ts.isStringLiteralLike(node.moduleSpecifier)
  ) {
    pushEdge(ctx, "imports", ctx.moduleId, moduleRef(ctx, node.moduleSpecifier));
  }

  node.forEachChild((c) => walk(ctx, c, src));
}

export function emitFileEdges(ctx: EdgeCtx): void {
  ctx.sf.forEachChild((c) => walk(ctx, c, ctx.moduleId));
}
