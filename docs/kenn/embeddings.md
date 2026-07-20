# Embeddings

Where vectors come from, and how to point kenn at a non-default
provider (ollama, lm-studio, hosted OpenAI).

## Default — local kenn server

With nothing configured, the first kenn process that needs an
embedding probes `127.0.0.1:41873`; if no daemon is listening, it
fork-execs `kenn server start --idle-timeout 600` and waits for
`/healthz`. The daemon loads EmbeddingGemma-300M (GGUF, q8_0) on
the first `/v1/embeddings` request, unloads after 60 s of no
embed calls, exits after 10 minutes of no requests at all.

See [server.md](server.md) for lifecycle details.

## Encoding: base64 by default

kenn's server defaults to `encoding_format: "base64"` when the
request omits the field — a deliberate deviation from OpenAI's
`"float"` default. base64 carries the raw f32-LE bytes (~3×
smaller than a JSON-float array, and bit-exact when decoded as
f32). Third-party clients that want float arrays must request
them explicitly:

```json
{ "model": "embeddinggemma-300M", "input": "...", "encoding_format": "float" }
```

kenn's own client (`RemoteEmbedder`) always sends
`encoding_format: "base64"` explicitly so it works uniformly
against kenn, ollama, lm-studio, and OpenAI (which default to
`"float"`).

## Pointing kenn at an external OpenAI-compatible provider

Any provider speaking the OpenAI `/v1/embeddings` API works.
The client opts into base64 encoding for ~3× smaller wire
payloads and transparently accepts either base64 strings or
float arrays in the response (so providers that ignore the
field still work).

Set `KENN_EMBED_URL` to the **base URL** (don't include
`/v1/...` — the client appends that itself); a trailing slash is
tolerated. Set `KENN_EMBED_MODEL` to the id the provider serves.

### ollama

```sh
ollama pull nomic-embed-text                # one-time, on the ollama side
ollama serve                                # if not already running
export KENN_EMBED_URL=http://localhost:11434
export KENN_EMBED_MODEL=nomic-embed-text
```

…or persistently in `~/.config/kenn/kenn.toml`:

```toml
[embeddings]
url = "http://localhost:11434"
model = "nomic-embed-text"
```

When `embeddings.url` is set, kenn will **never** auto-spawn its
own server — your ollama instance is the source of truth.

### lm-studio

Same pattern:

```toml
[embeddings]
url = "http://localhost:1234"
model = "text-embedding-nomic-embed-text-v1.5"
```

(Replace `model` with whatever id you've loaded in lm-studio's
Local Server panel.)

### Hosted OpenAI

```toml
[embeddings]
url = "https://api.openai.com"
model = "text-embedding-3-small"
```

…but kenn does **not** currently send an `Authorization` header.
Hosted OpenAI is more of a forward-compatibility story than a v1
target — auth lands when a real shared-deployment story does.

## Vector-sidecar manifest

When kenn writes embeddings to disk (the `.kenn/vectors/`
committed sidecar), it stamps a `manifest.toml`:

```toml
format_version = 1

[embedding_model]
id = "embeddinggemma-300M"

[vector]
dim = 768
quant = "int8-sym-pervec"
norm = "l2"

[fingerprint]
hash = "xxh3-64"
text = "sig-lf-doc/v1"
```

Reconciliation reuses committed vectors only when
`embedding_model.id` matches the configured model. **Provider
URL is intentionally not stamped** — the vectors are a function
of the model, not where it came from. The same id served by
ollama, lm-studio, or kenn's own server is treated as
compatible. Runtime drift between providers for the same id is
accepted as noise; if you care, version the id explicitly
(`embeddinggemma-300M-ollama`).

## Switching models

If you change `[embeddings].model`, the next embedding pass will
detect the mismatch against the existing `embedding_model.id`
and **refuse** with a clear error:

```
embedding model changed: sidecar stamped with `model-a`, current is `model-b`.
Wipe `.kenn/vectors/` (committed) to re-embed from scratch.
```

This is the deliberate v1 behavior: mass automatic re-embed on
model swap is risky (old segments would coexist with new), so
the operator explicitly wipes. A future
`embedding-model-update` change will automate it.

## Failure handling

Per design D13 (see the `extract-kenn-server` proposal),
remote-provider failures **always degrade**: unreachable
endpoint, non-2xx response, malformed body, timeout — all map to
`Ok(None)` upstream and search falls back to lexical-only. The
producer logs the underlying cause at WARN (`RUST_LOG=kenn_embed=warn`
shows it).

The in-process `LlamaEmbedder` (the fallback path) is stricter:
load failures degrade, but inference failures bubble as errors.
The asymmetry is deliberate — remote providers are moving
targets outside kenn's control; the local model is kenn's own
responsibility.

A future `KENN_EMBED_STRICT=1` mode could opt remote callers
back into hard-failure semantics; deferred until asked for.

## Diagnosing with `kenn doctor`

A misconfigured or crashed embedder degrades search to lexical-only
(above) — but that degradation is otherwise silent. `kenn doctor`
probes the **actually-selected** backend by embedding a trivial
string, so it reflects the real runtime path (including a remote
daemon that fork-execs and fails), not just what the config asks for:

```
$ kenn doctor
embedder: healthy
  model:   embeddinggemma-300M
  backend: remote (HTTP)
  dim:     768
  latency: 740 ms
```

It reports one of three outcomes, with distinct exit codes:

- **healthy** (exit 0) — dimension, latency, and the active backend
  (in-process llama.cpp, or a remote HTTP endpoint).
- **disabled** (exit 0) — no model configured; search is lexical-only.
  Not an error, just a configuration.
- **failed** (exit non-zero) — the raw backend error text (e.g. the
  macOS daemon fork+Metal failure), so you can see *why* embeddings
  aren't being produced.

Run it whenever semantic search seems to be returning only lexical
matches, or after changing the embedding provider.
