## MODIFIED Requirements

### Requirement: The server binds the resolved address from global config

The HTTP listener SHALL bind to the address resolved with the
precedence `KENN_SERVER_ADDR` env var > `[server].addr` in the
global config > built-in default `127.0.0.1:41873`. The same
resolved address SHALL be used by auto-spawn clients to probe.

When **multiple daemons start concurrently** (e.g. several MCP instances
cold-start and each auto-spawns), the bind is the arbiter: exactly **one**
daemon binds the resolved address and the rest fail with `EADDRINUSE`. A
daemon that loses the bind SHALL exit without writing a PID file and
without corrupting the winner's state. Auto-spawn **clients** treat a lost
bind race as non-fatal — they converge on the winner via `/healthz` (see
`embedding-producer`); operator-run `kenn server start` still surfaces the
conflict with a non-zero exit so a human sees it.

#### Scenario: default address

- **GIVEN** no `KENN_SERVER_ADDR` env var and no `[server].addr` in global config
- **WHEN** `kenn server start` runs
- **THEN** the server binds `127.0.0.1:41873`

#### Scenario: env var overrides config

- **GIVEN** `[server].addr = "127.0.0.1:41873"` in global config
- **AND** `KENN_SERVER_ADDR=127.0.0.1:9999` in the environment
- **WHEN** `kenn server start` runs
- **THEN** the server binds `127.0.0.1:9999`

#### Scenario: bind conflict surfaces cleanly (operator-run)

- **GIVEN** another process already holds the resolved address
- **WHEN** `kenn server start` runs interactively
- **THEN** the server exits non-zero with a message naming the address and `EADDRINUSE`
- **AND** does not write a PID file

#### Scenario: concurrent auto-spawn resolves to one daemon

- **GIVEN** two processes auto-spawn a daemon at the same resolved address simultaneously
- **WHEN** both attempt to bind
- **THEN** exactly one binds and becomes the live daemon (writes the PID file)
- **AND** the other fails to bind, exits, and does not write or overwrite the PID file
- **AND** both spawning clients reach the winner via `/healthz`
