## ADDED Requirements

### Requirement: EmbeddingGemma queries are embedded with the model's query task prompt

When the producing model is EmbeddingGemma-family, the producer SHALL prepend
the model's query task-instruction prompt (`task: search result | query: `) to
**query-kind** embeds before tokenization. Document-kind embeds SHALL be sent
raw — the document prompt is deferred (measured as adding nothing over
query-only), so corpus embedding output is byte-identical to the unprompted
behavior and stored vectors need no invalidation.

The prompt SHALL be applied inside the producer boundary, keyed on the model id —
a producer configured for a **non-EmbeddingGemma** model SHALL send raw text with
no prompt for either kind. The prompt SHALL NOT be stored in `embeddable_text`;
only the bytes fed to the tokenizer carry it.

#### Scenario: query and document of the same text embed differently

- **GIVEN** an EmbeddingGemma producer
- **WHEN** the same string is embedded once as a query and once as a document
- **THEN** the two vectors differ (the query carried the task prompt; the
  document did not)

#### Scenario: document embedding is unchanged by this feature

- **GIVEN** an EmbeddingGemma producer
- **WHEN** a code symbol or finding is embedded as a document
- **THEN** the raw `embeddable_text` is tokenized with no prompt, producing the
  same vector as before the query prompt existed (existing indexes reuse their
  vectors with zero re-embeds)

#### Scenario: a non-EmbeddingGemma model receives no prompt

- **GIVEN** a producer configured for a non-EmbeddingGemma model id (e.g. a
  remote ollama model)
- **WHEN** any text is embedded as either kind
- **THEN** the raw text is sent with no task prompt prepended

### Requirement: the embed kind is explicit at the producer boundary

The embedding-producer boundary SHALL carry an explicit embed kind — query versus
document — distinct from scheduler priority. Corpus embedding SHALL use the
document kind and free-text query embedding SHALL use the query kind. Prompt
selection SHALL derive from this kind, not from the interactive-vs-bulk priority.

#### Scenario: corpus and query paths carry distinct kinds

- **WHEN** a code symbol is embedded at index time
- **THEN** it is embedded with the document kind
- **AND WHEN** a free-text query is embedded
- **THEN** it is embedded with the query kind
