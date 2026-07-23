## REMOVED Requirements

### Requirement: The Docker indexer runtime is unsupported on Windows

**Reason**: Superseded by this change. That requirement was an explicit deferral —
its stated blocker was that Windows path translation was unimplemented. This
change implements it (`MountStrategy::Translate`: the workspace bind-mounts at
`/work` with every absolute path argument translated, and `metadata.project_root`
reconciled back at ingest), so docker IS now the supported — and default —
indexing runtime on Windows via Docker Desktop.

**Migration**: None for users. A Windows host with Docker Desktop running now gets
containerized indexing automatically: `kenn init` probes for a local indexer
first and, when none is present, authors `runtime = "docker"` without `--docker`.
Without a runnable daemon, missing-toolchain languages degrade to text as before.
The replacing behavior lives in the `docker-indexer-runtime` capability
("kenn synthesizes the container invocation" + "kenn init on Windows probes local
indexers first, then defaults to docker").
