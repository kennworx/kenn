// Heuristic test-file detection. Mirrors the kenn default test globs
// (`**/*.test.ts`, `**/*.spec.ts`, `tests/**`, `__tests__/**`). The driver
// may later pass workspace-configured patterns; this is the built-in default.
const TEST_RE =
  /(?:\.test\.|\.spec\.|(?:^|\/)tests?\/|(?:^|\/)__tests__\/)/i;

export function isTestPath(relPath: string): boolean {
  return TEST_RE.test(relPath.replaceAll("\\", "/"));
}
