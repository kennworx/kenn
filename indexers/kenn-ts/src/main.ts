import { parseArgs } from "node:util";

import {
  WIRE_VERSION,
  type EndFrame,
  type MetaFrame,
} from "../../frames";
import { indexWorkspace } from "./indexing/core";
import { JsonlSink } from "./wire/sink";
import { version as packageVersion } from "../package.json";

const TOOL = "kenn-ts";
/// One source, so `--version` and the wire's `tool_version` cannot drift from
/// the package metadata. kenn-dotnet derives its version from the assembly for
/// the same reason.
const TOOL_VERSION = packageVersion;

function nowIso(): string {
  return new Date().toISOString();
}

function printHelp(): void {
  process.stdout.write(
    `kenn-ts — TypeScript streaming indexer (kenn JSONL wire)

Usage:
  kenn-ts index --workspace <dir> [--tsconfigs <path>...] [options]

Options:
  --workspace <dir>     Workspace root to index (required for 'index').
  --tsconfigs <path>    tsconfig.json to index; repeatable. If omitted,
                        discovered under the workspace.
  --flush-bytes <n>     Stdout flush threshold in bytes.
  --flush-frames <n>    Stdout flush threshold in frames.
  --version             Print the tool version.
  -h, --help            Show this help.
`,
  );
}

function main(): number {
  let parsed;
  try {
    parsed = parseArgs({
      args: Bun.argv.slice(2),
      options: {
        workspace: { type: "string" },
        tsconfigs: { type: "string", multiple: true },
        "flush-bytes": { type: "string" },
        "flush-frames": { type: "string" },
        version: { type: "boolean" },
        help: { type: "boolean", short: "h" },
      },
      allowPositionals: true,
    });
  } catch (err) {
    // parseArgs is strict: any unknown option throws. Without this the process
    // dies with a raw TypeError stack against the bundled source, which tells a
    // caller nothing and looks like a crash rather than a usage error.
    const msg = err instanceof Error ? err.message : String(err);
    process.stderr.write(`${TOOL}: ${msg}\nRun \`${TOOL} --help\` for usage.\n`);
    return 2;
  }
  const { values, positionals } = parsed;

  // Answered before `index` is required, so a caller can probe whether this
  // indexer is runnable without handing it a workspace. Printed bare, matching
  // `kenn-dotnet --version`, so both are parsed the same way.
  if (values.version) {
    process.stdout.write(`${TOOL_VERSION}\n`);
    return 0;
  }
  if (values.help) {
    printHelp();
    return 0;
  }
  if (positionals[0] !== "index") {
    printHelp();
    return positionals.length === 0 ? 0 : 1;
  }

  const workspace = values.workspace;
  if (!workspace) {
    process.stderr.write("kenn-ts: --workspace is required\n");
    return 2;
  }

  const flushBytes = values["flush-bytes"]
    ? Number(values["flush-bytes"])
    : undefined;
  const flushFrames = values["flush-frames"]
    ? Number(values["flush-frames"])
    : undefined;

  const sink = new JsonlSink(
    (chunk) => process.stdout.write(chunk),
    flushBytes,
    flushFrames,
  );

  const meta: MetaFrame = {
    type: "meta",
    v: WIRE_VERSION,
    project_root: workspace,
    tool: TOOL,
    tool_version: TOOL_VERSION,
    language: "typescript",
    ts: nowIso(),
  };
  sink.push(meta);

  const stats = indexWorkspace(workspace, values.tsconfigs ?? [], sink);

  const end: EndFrame = { type: "end", stats, ts: nowIso() };
  sink.push(end);
  sink.flush();
  return 0;
}

// `process.exitCode`, NEVER `process.exit(main())`. stdout is a PIPE when kenn
// drives this indexer, and writes to a pipe are asynchronous: once a chunk
// exceeds the pipe buffer (~64KB) it is queued on the event loop, and
// process.exit() discards whatever is still queued. That silently TRUNCATED the
// final frames on every large workspace — the JSONL stream ended mid-token and
// ingest failed with "EOF while parsing a string", losing symbols. Small
// fixtures never caught it because their whole output fit in the pipe buffer and
// so was written synchronously. Setting exitCode lets the runtime drain stdout
// and exit on its own; indexing is synchronous, so nothing else holds the loop.
process.exitCode = main();
