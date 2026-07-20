import ts from "typescript";

import type { Range } from "../../../frames";

/** 0-based [startLine, startCol, endLine, endCol] for a node, excluding leading trivia. */
export function rangeOf(sf: ts.SourceFile, node: ts.Node): Range {
  const start = sf.getLineAndCharacterOfPosition(node.getStart(sf));
  const end = sf.getLineAndCharacterOfPosition(node.getEnd());
  return [start.line, start.character, end.line, end.character];
}
