## ADDED Requirements

### Requirement: Search dataset id columns follow the store naming convention

The search dataset's identity columns SHALL be named so each states its contract:
- `id` — the **volatile** numeric join key (was `short_id`), rewritten every run, resolved against the graph dataset matching the row's kind (symbol or file).
- `pub_id` — the symbol's **stable, API-visible** public id (unchanged), e.g. `cs:Foo`; empty for non-symbol rows. This is the same `pub_id` meaning used elsewhere in the store.
- `embed_key` — the **internal** composite key used to reconcile and reuse committed embeddings across runs (was `id` / `stable_id`): `name:<lang>:<pub_id>`, `doc:<lang>:<pub_id>`, or `filedoc:<lang>:<path>`. Not API-visible.

#### Scenario: Search row columns use the convention

- **WHEN** the search store is built
- **THEN** each row's volatile join key is `id`, the symbol's public id is `pub_id`, and the internal embedding-reconciliation key is `embed_key`

#### Scenario: embed_key drives reconciliation, volatile id drives join

- **GIVEN** a row whose `embed_key` and text fingerprint are unchanged since the last run
- **WHEN** the search store is rebuilt
- **THEN** its committed embedding is reused even though its `id` (volatile join key) may have changed
