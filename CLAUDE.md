## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

## 5. Rust: Check Warnings with Clippy

**Use `cargo clippy --workspace --all-targets` to check warnings, not `cargo check`.**

The workspace opts into `clippy::pedantic`, so user-visible editor warnings include the pedantic group. Plain `cargo check` is too lenient and gives a misleading "clean" answer.

- Before claiming code is clean, run clippy and confirm zero warnings.
- After any non-trivial Rust change, re-run clippy. `cargo clippy --fix --allow-dirty` auto-applies most fixes; finish the rest manually.
- Real correctness flags (`cast_possible_truncation`, `cast_sign_loss`, etc.) warrant a fix. Cosmetic pedantic flags on intentional code (e.g., `match_same_arms` on a documentation-style mapping table) can be `#[allow(...)]`'d with a justification comment.

## 6. Rust: CRAP Gate

**Run `just crap-ci` after non-trivial Rust changes.** CRAP = `cyclomatic² × (1 − coverage)³ + cyclomatic`. The gate threshold is 30, configured in `.cargo-crap.toml`; the runner is slow because it instruments the test suite under `llvm-cov`.

Two ways the gate fails:
- **Regression** — an existing baselined entry's CRAP got worse.
- **New over-threshold** — a function newly above 30.

Either is a real signal. Fix either by **adding test coverage** or **reducing cyclomatic complexity** (split big functions into small single-purpose helpers — the orchestrator stays untested but its branch count drops). Both routes are legitimate; pick whichever is closer to honest.

**Don't blindly run `just crap-baseline` to silence the gate** — that bakes in real complexity debt as accepted. Refresh the baseline only when you've decided the pre-existing entries are genuinely grandfathered, and say so in the commit message.

Workflow: run `just crap-ci` before claiming a Rust change is done. If it fails on functions YOU touched, fix them. If it fails on pre-existing functions because the baseline is stale, that's a baseline-refresh conversation — surface it, don't paper over it.

## 7. Rust: Format last with `cargo fmt --all`

**The final step of any Rust change, after clippy and CRAP both pass.** The workspace uses default `rustfmt` settings (no `rustfmt.toml`).

- Do NOT run `cargo fmt` while iterating. Formatting mid-work churns the diff and obscures what's actually changing under clippy / CRAP feedback.
- Once all logic is in place and `cargo clippy --workspace --all-targets` + `just crap-ci` are green, run `cargo fmt --all` as the very last step before staging the commit.
- If the run touches files you didn't edit, that's formatting drift from elsewhere. Commit those files under their own focused message (e.g. `workspace: cargo fmt --all`); don't bundle drift with logic changes.
- This rule overrides §3's "don't touch adjacent formatting" for `.rs` files: `rustfmt` output is canonical, and the diff that matters is "what `rustfmt` would produce."
- **Then run clippy once more.** Formatting is not warning-neutral: re-wrapping a call across more lines pushed a function from 99 to 101 lines and tripped `clippy::too_many_lines` *after* a green gate, so the reported "zero warnings" was stale by the time it was said. The order is clippy → CRAP → fmt → **clippy**.

## 8. C#: Format with `dotnet format`

**The final step of any change under `indexers/kenn-dotnet/` (the .NET sidecar), after the xunit suite passes.** There is no `rustfmt.toml`-equivalent — the project uses the SDK default style, and `dotnet format` is canonical.

- The two projects have no shared `.sln`, so format each:
  ```
  dotnet format indexers/kenn-dotnet/kenn-dotnet.csproj
  dotnet format indexers/kenn-dotnet.tests/kenn-dotnet.tests.csproj
  ```
  CI/pre-commit can assert conformance with `--verify-no-changes` (non-zero exit if anything would change).
- Like §7: do NOT run it mid-iteration. Run it last, after `dotnet test indexers/kenn-dotnet.tests` is green.
- If it touches files you didn't edit, that's pre-existing drift — commit those under their own focused message (e.g. `kenn-dotnet: dotnet format`); don't bundle drift with logic changes.
- This rule overrides §3's "don't touch adjacent formatting" for `.cs` files: `dotnet format` output is canonical, and the diff that matters is "what `dotnet format` would produce."

## 9. A Test Is Not a Guard Until You've Seen It Fail

**Break the code, watch the test go red, restore. Then claim it guards something.**

(How to *run* them is §11 — `just test`, never bare `cargo test --workspace`.)

§4 says "write a test that reproduces it, then make it pass." That is necessary
and not sufficient: a test can pass for reasons unrelated to the property it
names. Every one of these passed on first write and guarded nothing —

- a reflection test reading `MethodBody.LocalVariables` for a type that a static
  call never puts there;
- a spawn test whose assertion (`/^\d+\.\d+\.\d+/`) also matched the *test
  runner's* own `--version` output;
- a "behavioral" test that could not fail, because the environment it scrubbed
  was not the one the code under test consulted;
- an assertion on the intermediate struct that carries a value, not on the field
  the bug was in.

Each was found only by mutation. So, before saying a test guards a fix:

1. Revert the fix (or negate the invariant) with `perl -0pi -e` / an edit.
2. Run the test. **It must fail, and for the stated reason.**
3. Restore, and confirm green.

Corollary: **fix one finding per edit.** Two fixes in one edit hide each other —
removing a duplicate `Console.Error.WriteLine` while adding `logger.LogError`
looked like one change and silently made the diagnostic suppressible by an
env var.

Corollary: **when a mutation survives, suspect the fixture before the test.** The
usual cause is that the setup never reaches the branch the test names, so the
assertion is true for an unrelated reason. Four in one session, all this shape:

- a fixture that registered `src/order.rs` in the *store* but never created it on
  disk, so the existence rung it was meant to order against was unreachable;
- a link written `../notes` in a test for *bare-name* handling — the slash routed
  it to the file branch, so the symbol lookup under test never ran;
- a `HashSet` mock that distinguished `docs` from `docs/` where the real
  `Path::exists` does not, rejecting the bad input by accident;
- a `RawLink { target: "[[gone]]" }` the extractor can never produce, since it
  strips the brackets before the code under test sees it.

A mock that is *stricter* than the thing it stands for is the most dangerous
kind: it makes the guard pass for a reason production would not reproduce.

If a property genuinely cannot be asserted from the test harness (see the
`dotnet exec` / MSBuildLocator case in `just probe-smoke`), say so in a comment
where the test would have gone, and put the real check where the artifact lives.
Do not leave a proxy standing in for it.

## 10. Dogfood the kenn CLI — Symbol Questions Go Through the Graph

**This project IS a code-intelligence tool. Using `rg` for a question the graph
answers is both a process failure and a correctness risk.**

**The gate:** before typing `rg`/`grep` with an *identifier* as the pattern — a
function, type, method, field, const, or env/config name — run the kenn query
FIRST. Reaching for `rg` on an identifier is the smell.

| Question | Command |
|---|---|
| Where is `X` defined? | `kenn find X` → `pub_id` |
| Who calls `X`? | `kenn list callers <pub_id>` |
| Where is `X` used / read / written? | `kenn list usages <pub_id>` |
| What does `X` call? | `kenn list callees <pub_id>` |
| Who implements / overrides `X`? | `kenn list implementers\|overrides <pub_id>` |
| What does this file import? | `kenn list imports <pub_id>` |
| Show `X`'s source | `kenn get <pub_id>` |
| Workspace shape / health | `kenn overview` / `kenn check` |

Build it first if needed (`just build-cli` → `./build/kenn`); run it in-sandbox
via Bash directly.

**Grep cannot prove absence — the graph can.** `rg` only tells you "not in the
files I happened to search." `kenn list usages` enumerates every edge in the
workspace, so "exactly 2 usages, neither a config override" is a sound claim;
"I grepped and didn't find it" is not. NEVER make a negative or exhaustive
claim — "nothing sets this", "it's wired nowhere", "this is dead code" — from a
text search.

**The trap that keeps biting:** judge by the *question*, not the token's shape.
An env-var name (`NUGET_PACKAGES`), a config key, or a const *looks* like a
string literal — but "where is this wired / who constructs it" is a **usages**
question. Grepping one likely file and declaring it unwired is how you produce a
confidently wrong answer twice in a row.

**When `rg` genuinely is right:** the text isn't a symbol at all — prose in docs
and comments, message strings, filenames and paths, and *values* in
TOML/JSON/YAML. The symbol graph doesn't index those.

**Staleness:** the index reflects the last `kenn index`, so it lags *uncommitted*
edits. This is NOT a licence to fall back to grep: for code you haven't edited
this session it is accurate; if you have moved or renamed symbols, run
`kenn index --force` — or say plainly that the answer may lag.

## 11. Rust: Run Tests With `just test`

**`just test`. Never bare `cargo test --workspace`.**

The recipe is `KENN_EMBED_IN_PROCESS=1 cargo test --workspace`, and that variable
is load-bearing, not decoration. Without it an early test binary reaches the
embedder selector, finds no in-process forcing, and auto-spawns a local `kenn
server` daemon (`kenn-embed/src/selector.rs`, `try_spawn_local_server`) with a
**600-second** lifetime. On macOS that daemon holds the Metal device, so the
later `kenn-store/tests/hybrid_search.rs` suite cannot initialise its own
embedder and its vector-dependent tests fail.

`hybrid_search` sets the flag for *itself*, which cannot evict a daemon another
binary already started — only setting it workspace-wide stops the spawn.
Serializing the tests would not help either: the daemon outlives whichever
binary spawned it, so ordering does not remove the contention. Removing the
daemon does.

| command | result |
|---|---|
| `cargo test --workspace` | 7 failures in `hybrid_search`, **0** `ggml_metal_device_init` |
| `just test` | 60 suites green, 13 `ggml_metal_device_init` |

**If `paraphrase_query_retrieves_code_symbol`, `store_finding_surfaces_near_duplicate`,
or the embed-pass tests fail, check the invocation before you read any code.**
This has been misdiagnosed twice — once as "a flake", once as "parallel test
binaries", which is also wrong: cargo runs test *binaries* sequentially.

### The commands

```sh
just test                       # everything — the default before any commit
just test sql::parse            # filter; ARGS pass straight through to cargo
just test -- --nocapture        # so do trailing test-harness args
```

A single package or target is fine for a fast inner loop, but **carry the flag
yourself** — the recipe is the only thing that sets it:

```sh
KENN_EMBED_IN_PROCESS=1 cargo test -p kenn-store --test hybrid_search
```

`cargo test -p <crate> --lib` needs no flag: unit tests never load the model.
That is the normal mid-refactor loop, and it is what makes bare `cargo` feel
safe right up until the suite that does load it runs.

### The other suites

| recipe | what it runs |
|---|---|
| `just test-indexers` | dotnet + swift + `probe-smoke` + `index-stability`, then the TypeScript sidecar via `bun test` |
| `just test-indexer-dotnet` / `-swift` | one sidecar's own suite |
| `cd indexers/kenn-ts && bun test` | the TypeScript sidecar alone — no recipe wraps it, and it is `bun`, never `npx` |
| `just embed-smoke` | the real `EmbeddingGemma` weights (macOS; first run downloads ~300MB) |
| `just crap-ci` | the CRAP gate — see §6; runs the suite under `llvm-cov` itself |
| `just probe-smoke` | every sidecar against an unreachable toolchain |

Sidecar suites are NOT part of `just test`, which is Rust only. A change under
`indexers/` needs its own suite run (§8).

### Where this sits in the sequence

Unit tests (`cargo test -p <crate> --lib`) are the mid-work loop. Before a
commit, the full order is:

```
just test  →  cargo clippy --workspace --all-targets  →  just crap-ci
           →  cargo fmt --all  →  cargo clippy once more   (§7)
```
