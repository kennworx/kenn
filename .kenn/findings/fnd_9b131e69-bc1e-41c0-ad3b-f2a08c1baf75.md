---
id: fnd_9b131e69-bc1e-41c0-ad3b-f2a08c1baf75
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-24T14:31:31.259302Z
---
The stable .kenn/atlas pointer symlinks to the RESOLVED run directory, never through `live`. `live` is a pointer FILE (that is what makes the atomic flip work unprivileged on Windows), so it is not traversable — the original `.kenn/atlas -> local/live/atlas` dangled the moment that landed, and every repo indexed before the fix still carries the corpse. refresh_atlas_pointer (atlas/producer.rs) resolves first and links to the concrete run: relative when the bundle is under the pointer dir (so it survives the repo being moved), absolute when a `derived_root = "global"` store puts runs in an XDG cache outside it. THE SUBTLE PART: detection uses symlink_metadata, not exists() or metadata — both FOLLOW the link, so a dangling pointer reads as ABSENT and a naive create-if-missing leaves it broken forever. Only a symlink is removed; a real directory a user put at that path is left alone. The call sits in the SHARED writer (aggregate.rs, off AtlasContext.pointer_dir) so `kenn index` and the MCP reindex path both refresh it — per the CLI/workflow parity rule. It is best-effort and its failure is logged at DEBUG, never warn: creating a symlink is exactly what Windows withholds without Developer Mode, and the `atlas: <path>` line every index prints remains the contract. The pointer is a convenience for HUMANS; agents should keep deriving the path from that line.