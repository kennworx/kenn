---
id: fnd_c60ad3db-25dc-4a17-b41a-fea64a337d3b
tags:
- directive
- polarity:dont
- verification
parent_ids: []
created_at: 2026-07-26T10:14:25.497883Z
---
Swift emitter determinism: never iterate a Dictionary or Set when the order reaches the wire. Swift reseeds its hasher PER PROCESS, so `for usr in defs.keys` and `for pair in moduleImports` visited elements in a different order on every run — and both orders decided identity. `keyFor` hands the unsalted key to whichever USR arrives FIRST and salts the losers, so two runs of one binary over an unchanged tree disagreed about which of three `Contained` types was `ArgumentParser.Contained` and which carried a `#<digest>` suffix; module stubs separately took ids in Set order and swapped 5704/5705. Nine atlas documents differed between runs; now only the header timestamp does.

Sort by USR, NOT by source position: a USR is a stable identity, so inserting a declaration above a collision cannot move the unsalted key onto a different symbol. A line-ordered sort is deterministic per run but still churns published ids on unrelated edits.

THE TESTING TRAP, which cost the most time here. The in-process test (`testEmitIsDeterministicAcrossRuns`, two emitter runs over one store) catches the Set bug — a Swift Set built twice in one process can iterate differently, since order depends on internal layout, not just the seed. It does NOT catch the Dictionary bug: one process builds the same Dictionary the same way twice, so only a RESEED exposes it, and reverting `defs.keys.sorted()` leaves that test green. `just swift-determinism` is the guard for that half — two separate sidecar processes over one store, diffed. Mutation-check any determinism fix against BOTH; a green unit test here does not mean the emitter is order-independent.

And when the pipeline still differs after a sidecar fix, check the RUNTIME before re-reading the code: a repo with `runtime = "docker"` for a language runs the sidecar from `ghcr.io/kennworx/kenn-<lang>:local`, not `build/kenn-<lang>`. Rebuilding the local binary changes nothing until `just build-image <lang>` reruns. That mismatch made a correct fix look like a failed one for several iterations.