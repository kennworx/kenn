For long-running or important shell commands, redirect output to a log file
under `./tmp/` via `tee` (e.g. `cargo test 2>&1 | tee ./tmp/cargo-test.log`) so
the run is captured and can be tailed.

Before you commit, if this session captured durable user steering — corrections,
decisions, or non-obvious rules worth keeping for the team — run `/kenn:squeeze`
first, while the changes are still staged, so it becomes a directive the next
session will see. Trivial or mechanical commits (formatting, a typo, a rename,
generated output) don't need it.
