import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";

const DEFAULT_EXCLUDE_DIRS = new Set([
  "node_modules",
  "bin",
  "obj",
  "target",
  ".git",
]);

/**
 * Linked git worktrees that are descendants of `root` (excluding `root`
 * itself). Discovered from git, not from path patterns — matches the SCIP
 * discovery rule. Empty when not a git repo.
 */
function worktreeDirs(root: string): Set<string> {
  const r = spawnSync("git", ["-C", root, "worktree", "list", "--porcelain"], {
    encoding: "utf8",
  });
  if (r.status !== 0 || !r.stdout) return new Set();
  const out = new Set<string>();
  const sep = path.sep;
  for (const line of r.stdout.split("\n")) {
    if (!line.startsWith("worktree ")) continue;
    const dir = path.resolve(line.slice("worktree ".length).trim());
    if (dir !== root && dir.startsWith(root + sep)) out.add(dir);
  }
  return out;
}

/**
 * Discover `tsconfig.json` files under `root`, skipping the explicit-exclude
 * dirs and any linked-worktree directories.
 */
export function discoverTsconfigs(root: string): string[] {
  const excludedWorktrees = worktreeDirs(root);
  const found: string[] = [];

  const walk = (dir: string): void => {
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const full = path.join(dir, e.name);
      if (e.isDirectory()) {
        if (DEFAULT_EXCLUDE_DIRS.has(e.name)) continue;
        if (excludedWorktrees.has(full)) continue;
        walk(full);
      } else if (e.isFile() && e.name === "tsconfig.json") {
        found.push(full);
      }
    }
  };

  walk(root);
  found.sort();
  return found;
}
