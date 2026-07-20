import ts from "typescript";

import type { SymbolKind } from "../../../frames";

/** Backtick-escape a name that is not a simple identifier (SCIP descriptor rule). */
function esc(name: string): string {
  if (/^[\w$+-]+$/.test(name)) return name;
  return "`" + name.replaceAll("`", "``") + "`";
}

/** Descriptor suffix for a wire kind: `/` namespace, `#` type, `().` method, `.` term. */
function suffixForKind(kind: SymbolKind): string {
  switch (kind) {
    case "namespace":
    case "module":
      return "/";
    case "class":
    case "struct":
    case "interface":
    case "enum":
    case "type":
    case "delegate":
      return "#";
    case "method":
    case "function":
    case "constructor":
    case "destructor":
    case "accessor":
      return "().";
    default:
      return ".";
  }
}

/** Descriptor segment for an enclosing declaration node, or undefined if it introduces no scope. */
function containerSegment(node: ts.Node): string | undefined {
  if (ts.isModuleDeclaration(node)) return esc(node.name.getText()) + "/";
  if (ts.isInterfaceDeclaration(node)) return esc(node.name.text) + "#";
  if (ts.isEnumDeclaration(node)) return esc(node.name.text) + "#";
  if (ts.isTypeAliasDeclaration(node)) return esc(node.name.text) + "#";
  if ((ts.isClassDeclaration(node) || ts.isClassExpression(node)) && node.name) {
    return esc(node.name.text) + "#";
  }
  if (ts.isFunctionDeclaration(node) && node.name) return esc(node.name.text) + "().";
  if (ts.isMethodDeclaration(node) && ts.isIdentifier(node.name)) {
    return esc(node.name.text) + "().";
  }
  return undefined;
}

/** The descriptor key of a module-file: the relative path as a namespace segment. */
export function moduleKey(moduleRel: string): string {
  return esc(moduleRel) + "/";
}

/**
 * Cross-run-stable, intra-package descriptor key for a symbol: the module
 * prefix, the enclosing named scopes, and the symbol's own segment. Two
 * symbols in different files never collide (module prefix); a type and a
 * value of the same name differ by suffix (`#` vs `.`).
 */
export function symbolKey(
  name: string,
  kind: SymbolKind,
  decl: ts.Declaration,
  moduleRel: string,
): string {
  const segs: string[] = [esc(name) + suffixForKind(kind)];
  let node: ts.Node | undefined = decl.parent;
  while (node && !ts.isSourceFile(node)) {
    const seg = containerSegment(node);
    if (seg) segs.unshift(seg);
    node = node.parent;
  }
  segs.unshift(moduleKey(moduleRel));
  return segs.join("");
}
