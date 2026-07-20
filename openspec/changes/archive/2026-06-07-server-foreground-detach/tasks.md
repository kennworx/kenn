## 1. setsid-only detachment

- [x] 1.1 Rewrite `runtime::daemonize` (Unix) to `chdir("/")` + `nix::unistd::
      setsid()` with no fork; document why forking breaks Metal.
- [x] 1.2 Point the spawned child's stdout/stderr at `<state_dir>/server.log` in
      `cmd_server::spawn_daemon_and_wait` (the setsid-only child inherits them).
- [x] 1.3 Remove the `daemonize` crate dependency from `kenn-server`.

## 2. Verification

- [x] 2.1 `kenn server start` (daemon mode) → `/v1/embeddings` returns a real
      vector (was 503 before).
- [x] 2.2 `kenn embed` completes through the auto-spawned daemon.
- [x] 2.3 `cargo clippy -p kenn-server -p kenn-cli --all-targets` zero warnings.
- [x] 2.4 `cargo fmt --all`.
