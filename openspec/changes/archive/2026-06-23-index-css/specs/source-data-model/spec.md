## ADDED Requirements

### Requirement: Stylesheet public IDs use `css:`/`sass:` prefixes with path/selector native form

The public-ID scheme SHALL include two stylesheet language prefixes: `css:` for
`.css` files and `sass:` for Sass files (`.scss` and `.sass` — one language with
two syntaxes). The native-ID portion SHALL be `<lang>:<relpath>#<type>:<name>`,
where `<type>` is one of `class`, `id`, or `var` and `<name>` is the atomic token
(class `.btn` → `class:btn`, id `#app` → `id:app`, custom property `--brand` →
`var:--brand`). The `<type>` segment is REQUIRED: a class and an id of the same
name in the same file (`.hero` and `#hero`) MUST produce distinct IDs. The
stylesheet file itself is a node with ID `<lang>:<relpath>`. This extends the
existing `<lang>:<native-id>` scheme additively and SHALL NOT change existing
code-language IDs. The `css`/`sass` split SHALL be source-provenance only: both
languages use the same stylesheet node kinds and feed one unified class registry.

#### Scenario: A CSS class has a typed `css:` public ID

- **WHEN** `.btn-primary` is defined in `src/button.css`
- **THEN** its public ID is `css:src/button.css#class:btn-primary`

#### Scenario: A same-named class and id do not collide

- **WHEN** a file defines both `.hero` and `#hero`
- **THEN** the class ID is `…#class:hero` and the id ID is `…#id:hero`
- **AND** the two nodes are distinct

#### Scenario: A Sass class has a `sass:` public ID

- **WHEN** `.btn-primary` is defined in `src/button.scss`
- **THEN** its public ID is `sass:src/button.scss#class:btn-primary`
- **AND** its node kind is `css_class` (kinds are shared across css/sass)

#### Scenario: Code IDs are unchanged

- **WHEN** stylesheet indexing is enabled
- **THEN** existing `cs:` / `ts:` / `rs:` / `go:` / `py:` / `md:` IDs are
  unaffected

### Requirement: Stylesheet node kinds

The kind enum SHALL include `css_class`, `css_id`, and `css_var`. Each is a
symbol-space node (so usage and dependency edges target it unambiguously), carries
the language value of its source file (`css` or `sass`), and carries its native
ID as `pub_id`. Each stylesheet **file** SHALL additionally be a scope node of
kind `module` (a stylesheet is a module of style rules): its selectors are
`defined_in` it and it `contains` them, and it is the valid endpoint for
`imports` edges (below). A `FileRecord` with the matching language (`css` or
`sass`) is also emitted for the files table and change detection.

#### Scenario: A class is a `css_class` node regardless of source language

- **WHEN** a class is indexed from a `.css` file and another from a `.scss` file
- **THEN** both nodes have kind `css_class`
- **AND** their languages are `css` and `sass` respectively

#### Scenario: A stylesheet file is a module node owning its selectors

- **WHEN** `src/button.css` defines `.btn`
- **THEN** a `module` node `css:src/button.css` exists
- **AND** `.btn` is `defined_in` it (and it `contains` `.btn`)

### Requirement: Edge-kind enum includes `uses_css_class` and `extends_rule`

The edge-kind enum SHALL include `uses_css_class` (a code file/symbol references a
CSS class) and `extends_rule` (`@extend .class` / `composes … from`, rule →
class). In v1 `extends_rule` targets SHALL be existing `css_class` nodes;
`@extend %placeholder` and `@include`/mixin (which would require placeholder/mixin
node kinds) are out of scope. `@import`/`@use`/`@forward` SHALL reuse the existing
`imports` edge kind, whose endpoints are the stylesheet **module** nodes (so the
reuse is consistent with `imports` being a module-to-module relation). These
additions are additive; existing edge kinds retain their meaning.

#### Scenario: A class usage and an @extend use the new kinds

- **WHEN** a component uses a class and a rule `@extend`s a placeholder
- **THEN** the first edge has kind `uses_css_class` and the second `extends_rule`

#### Scenario: An @use is a module-to-module imports edge

- **WHEN** `a.scss` contains `@use './b'`
- **THEN** an `imports` edge connects module `sass:a.scss` → module `sass:b.scss`
