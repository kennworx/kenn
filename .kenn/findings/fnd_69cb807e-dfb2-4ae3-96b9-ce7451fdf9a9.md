---
id: fnd_69cb807e-dfb2-4ae3-96b9-ce7451fdf9a9
tags:
- directive
- polarity:do
- docker
parent_ids: []
created_at: 2026-07-23T09:06:47.769227Z
---
Windows docker indexing translates host paths onto the container /work mount (crates/kenn-indexer/src/docker.rs — ContainerMount / MountStrategy::Translate). When touching an indexer driver or the docker runtime: (1) Every absolute path ARGUMENT a driver passes to a containerized indexer MUST go through container_arg(self.mount.as_ref(), path). A bare .arg(abs_path) is valid under the POSIX same-path mount but breaks the Windows /work mount — the Linux container cannot see a Windows drive path. Discovery still uses the real host root; only the args handed to the indexer are translated. Each driver passes a DIFFERENT set of absolute args (dotnet --workspace+--projects, python --cwd+--output+--target-only, etc.) — audit them all, do not assume the root is the only one. (2) EXCEPTION: the SCIP --output file path and the ScipOutcome::Scip { path } returned for read-back stay HOST paths — kenn reads the .scip back on the host after the container writes it through the /work bind mount, so translate the --output ARG but keep the variable host-side. (3) SCIP project_root reconciliation (reconcile_container_root, pipeline/ingest.rs) is GATED on the runtime signal ScipDriver::container_mount(), NEVER unconditional: an unconditional /work-to-host rebase masks a genuine project_root/workspace mismatch — the scip_documents_outside_the_root test uses /work as a sentinel unrelated root, and only a real Translate mount may rebase. (4) container_mount() in docker.rs is the single predicate (docker && cfg!(windows)) feeding BOTH the launcher MountStrategy and each driver mount, so they can never disagree.