import ts from "typescript";

/**
 * Leading file-level comment blocks (raw, unfiltered — license filtering is a
 * consumer concern). Contiguous `//` lines coalesce into one block; a blank
 * line breaks the block; each `/* *\/` / `/** *\/` is its own block; a leading
 * `#!` shebang line is skipped. Returns [] when the file has no leading comment.
 */
export function extractFileDoc(sf: ts.SourceFile): string[] {
  const text = sf.getFullText();
  let start = 0;
  if (text.startsWith("#!")) {
    const nl = text.indexOf("\n");
    start = nl < 0 ? text.length : nl + 1;
  }

  const ranges = ts.getLeadingCommentRanges(text, start) ?? [];
  const blocks: string[] = [];
  let cur: string[] = [];
  let prevLine = -2;

  const flush = (): void => {
    if (cur.length > 0) {
      blocks.push(cur.join("\n"));
      cur = [];
    }
  };

  for (const r of ranges) {
    const raw = text.slice(r.pos, r.end);
    if (r.kind === ts.SyntaxKind.MultiLineCommentTrivia) {
      flush();
      blocks.push(raw);
      prevLine = -2;
      continue;
    }
    const line = sf.getLineAndCharacterOfPosition(r.pos).line;
    if (prevLine >= 0 && line > prevLine + 1) flush();
    cur.push(raw);
    prevLine = line;
  }
  flush();
  return blocks;
}
