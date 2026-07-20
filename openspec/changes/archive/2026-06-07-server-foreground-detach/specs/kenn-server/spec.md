## ADDED Requirements

### Requirement: The per-user daemon detaches without forking

When `kenn server start` runs in daemon mode, the server SHALL detach into its
own session using `setsid` only, without a fork-without-exec. The spawning
parent SHALL fork-exec the server as a fresh process and redirect its stdio to
the server log. The daemon's embedder (llama.cpp / Metal) SHALL be able to
create a compute context and serve `/v1/embeddings`.

#### Scenario: daemonized server serves embeddings

- **GIVEN** `kenn server start` invoked in daemon mode on macOS
- **WHEN** a client POSTs to `/v1/embeddings`
- **THEN** the server returns embedding vectors (not a 503 context-creation
  failure)
- **AND** `kenn embed` completes against the daemon

#### Scenario: daemon outlives the launching shell and logs

- **WHEN** the launching `kenn server start` returns
- **THEN** the daemon keeps running detached from the controlling terminal
- **AND** its stdout/stderr are written to `<state_dir>/server.log`
