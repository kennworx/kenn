import ts from "typescript";

import type { SymbolKind } from "../../../frames";

/** Is this variable declaration a `const` (vs `let`/`var`)? */
function isConstDecl(decl: ts.Declaration): boolean {
  if (!ts.isVariableDeclaration(decl)) return false;
  const list = decl.parent;
  return ts.isVariableDeclarationList(list) && (list.flags & ts.NodeFlags.Const) !== 0;
}

/**
 * Map a `ts.Symbol` (with its declaration for `const`/`let` disambiguation)
 * to the most specific wire `SymbolKind`. `function` and `enum_member` are
 * the TS-relevant additions; everything else reuses existing kinds.
 */
export function wireKind(sym: ts.Symbol, decl?: ts.Declaration): SymbolKind {
  const f = sym.getFlags();
  if (f & ts.SymbolFlags.Module) return "namespace";
  if (f & ts.SymbolFlags.Class) return "class";
  if (f & ts.SymbolFlags.Interface) return "interface";
  if (f & ts.SymbolFlags.Enum) return "enum";
  if (f & ts.SymbolFlags.EnumMember) return "enum_member";
  if (f & ts.SymbolFlags.TypeAlias) return "type";
  if (f & (ts.SymbolFlags.GetAccessor | ts.SymbolFlags.SetAccessor)) return "accessor";
  if (f & ts.SymbolFlags.Constructor) return "constructor";
  if (f & ts.SymbolFlags.Method) return "method";
  if (f & ts.SymbolFlags.Function) return "function";
  if (f & ts.SymbolFlags.Property) return "property";
  if (f & ts.SymbolFlags.Variable) {
    return decl && isConstDecl(decl) ? "const" : "symbol";
  }
  return "symbol";
}
