//! Author a `kenn.toml` from detection results (tasks 3.1–3.4).
//!
//! Mutates a [`toml_edit::DocumentMut`] in place so comments and untouched keys
//! survive — the base is the commented starter template on a fresh init, or the
//! user's existing config under `--force`. A typed `Config` round-trip is
//! deliberately avoided: it would emit every default (`command = […]`, all
//! excludes, `[mcp]`) and freeze them into the file.

use kenn_config::TextConfig;
use toml_edit::{value, Array, DocumentMut, Item, Table, Value};

use super::detect::{Availability, Classified};

/// Apply the classification to `doc`: enable each available language, route each
/// degraded one to the text fallback, and seed `[tests] paths` only when the
/// existing list is empty.
pub fn apply(doc: &mut DocumentMut, classified: &[Classified]) {
    let degraded: Vec<&Classified> = classified
        .iter()
        .filter(|c| matches!(c.availability, Availability::Degraded { .. }))
        .collect();

    for c in classified {
        match &c.availability {
            Availability::Enabled => {
                enable_language(doc, c.spec.name);
                // On --force, a language that was degraded before but is now
                // available may have left its source globs in the text include.
                // Drop them so the config doesn't both enable the language and
                // list its sources in the fallback.
                prune_text_include(doc, c.spec.source_globs);
            }
            Availability::Containerized { image } => {
                enable_docker_language(doc, c.spec.name, image);
                prune_text_include(doc, c.spec.source_globs);
            }
            Availability::Degraded { .. } => {} // routed to text below
        }
    }
    if !degraded.is_empty() {
        route_to_text(doc, &degraded);
    }
    seed_tests(doc, classified);
}

/// Whether an availability yields a full symbol graph — either the local tool
/// (`Enabled`) or a container (`Containerized`). Both seed test globs and have
/// their source globs pruned from the text include; only `Degraded` falls to
/// text.
fn is_indexed(a: &Availability) -> bool {
    matches!(
        a,
        Availability::Enabled | Availability::Containerized { .. }
    )
}

/// Remove `globs` from `[language.text] include`, if present. Used when a
/// language becomes enabled to strip globs an earlier degrade added.
fn prune_text_include(doc: &mut DocumentMut, globs: &[&str]) {
    let Some(include) = doc
        .get_mut("language")
        .and_then(|l| l.get_mut("text"))
        .and_then(|t| t.get_mut("include"))
        .and_then(toml_edit::Item::as_array_mut)
    else {
        return;
    };
    include.retain(|v| !v.as_str().is_some_and(|s| globs.contains(&s)));
}

/// Set `[language.<name>].enabled = true`, creating the section if absent. The
/// `command` key is left as the template's commented default — the default
/// already resolves the tool on `PATH`, so writing it would only pin a name.
#[expect(
    clippy::indexing_slicing,
    reason = "toml_edit Index/IndexMut are panic-free — missing keys read as None and \
              writes auto-vivify tables; the idiomatic API, not slice indexing"
)]
fn enable_language(doc: &mut DocumentMut, name: &str) {
    doc["language"][name]["enabled"] = value(true);
}

/// Enable `[language.<name>]` for a container fallback (task 5.1): `enabled =
/// true`, `runtime = "docker"`, and the digest-pinned `image`. Mirrors
/// `enable_language` but pins the runtime so the index run drives the tool
/// inside `image` (see the `docker-indexer-runtime` change) rather than PATH.
#[expect(
    clippy::indexing_slicing,
    reason = "toml_edit Index/IndexMut are panic-free (see enable_language)"
)]
fn enable_docker_language(doc: &mut DocumentMut, name: &str, image: &str) {
    doc["language"][name]["enabled"] = value(true);
    doc["language"][name]["runtime"] = value("docker");
    doc["language"][name]["image"] = value(image);
}

/// Enable `[language.text]` and, for each degraded language, add its source
/// globs to `include` and its excludes to the excludes union. Degraded source
/// stays searchable by FTS + embeddings with no symbol graph.
#[expect(
    clippy::indexing_slicing,
    reason = "toml_edit Index/IndexMut are panic-free (see enable_language)"
)]
fn route_to_text(doc: &mut DocumentMut, degraded: &[&Classified]) {
    let text = &mut doc["language"]["text"];
    text["enabled"] = value(true);

    let mut include = as_array(text.get("include"));
    for c in degraded {
        for g in c.spec.source_globs {
            push_unique(&mut include, g);
        }
    }
    text["include"] = value(include);

    // The user-set list REPLACES the defaults, so write the union explicitly —
    // text's defaults miss vendored/build trees like `vendor/**` or `obj/**`.
    // Seed from any existing list so `--force` preserves a user's own excludes.
    let mut excludes = as_array(text.get("excludes"));
    for e in TextConfig::DEFAULT_EXCLUDES {
        push_unique(&mut excludes, e);
    }
    for c in degraded {
        for e in c.spec.excludes {
            push_unique(&mut excludes, e);
        }
    }
    text["excludes"] = value(excludes);
}

/// Seed `[tests] paths` from the enabled languages' test globs, but only when
/// the existing list is empty or absent. `[tests] paths` is authoritative with
/// no fallback, so an empty list means nothing counts as test code; a user's
/// non-empty list is never touched. Degraded languages contribute nothing —
/// the text producer records every chunk as non-test.
#[expect(
    clippy::indexing_slicing,
    reason = "toml_edit Index/IndexMut are panic-free (see enable_language)"
)]
fn seed_tests(doc: &mut DocumentMut, classified: &[Classified]) {
    let existing = doc
        .get("tests")
        .and_then(|t| t.get("paths"))
        .and_then(|p| p.as_array());
    if existing.is_some_and(|a| !a.is_empty()) {
        return;
    }

    let mut paths = Array::new();
    for c in classified {
        if is_indexed(&c.availability) {
            for g in c.spec.test_globs {
                push_unique(&mut paths, g);
            }
        }
    }
    if paths.is_empty() {
        return;
    }
    // Insert an explicit `[tests]` table when the config lacks one, so it
    // renders as a section rather than an inline `tests = { … }` at the top.
    if doc.get("tests").is_none() {
        doc.insert("tests", Item::Table(Table::new()));
    }
    doc["tests"]["paths"] = value(paths);
}

/// The array at `item`, or a fresh empty one.
fn as_array(item: Option<&toml_edit::Item>) -> Array {
    item.and_then(|i| i.as_array()).cloned().unwrap_or_default()
}

/// Append `s` unless it is already present, so repeated authoring is idempotent.
fn push_unique(arr: &mut Array, s: &str) {
    let present = arr.iter().any(|v| v.as_str() == Some(s));
    if !present {
        arr.push(Value::from(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::detect::SPECS;

    const TEMPLATE: &str = include_str!("../../assets/starter_kenn.toml");

    fn template() -> DocumentMut {
        TEMPLATE.parse().expect("template parses")
    }

    fn classify(name: &str, availability: Availability) -> Classified {
        let spec = SPECS.iter().find(|s| s.name == name).unwrap();
        Classified { spec, availability }
    }

    fn degraded(name: &str) -> Classified {
        classify(
            name,
            Availability::Degraded {
                command: String::new(),
                hint: String::new(),
                reason: String::new(),
                not_executable: true,
            },
        )
    }

    fn containerized(name: &str, image: &str) -> Classified {
        classify(
            name,
            Availability::Containerized {
                image: image.to_string(),
            },
        )
    }

    /// Every authored document must still load as a `Config`.
    fn reparse(doc: &DocumentMut) -> kenn_config::Config {
        kenn_config::Config::from_toml(&doc.to_string()).expect("authored config parses")
    }

    #[test]
    fn enabled_language_is_flagged_without_a_command_key() {
        let mut doc = template();
        apply(&mut doc, &[classify("rust", Availability::Enabled)]);

        let rust = &doc["language"]["rust"];
        assert_eq!(rust["enabled"].as_bool(), Some(true));
        assert!(
            rust.get("command").is_none(),
            "the default resolves on PATH; init must not pin a command"
        );
        assert!(reparse(&doc).language.rust.enabled);
    }

    #[test]
    fn degraded_language_routes_to_text_not_its_own_section() {
        let mut doc = template();
        apply(&mut doc, &[degraded("go")]);

        assert_eq!(
            doc["language"]["go"]["enabled"].as_bool(),
            Some(false),
            "a degraded language stays disabled"
        );
        let text = &doc["language"]["text"];
        assert_eq!(text["enabled"].as_bool(), Some(true));
        let include: Vec<_> = text["include"].as_array().unwrap().iter().collect();
        assert!(
            include.iter().any(|v| v.as_str() == Some("**/*.go")),
            "go source globs land in the text fallback"
        );
    }

    #[test]
    fn degraded_text_excludes_are_the_union_with_vendor_trees() {
        let mut doc = template();
        apply(&mut doc, &[degraded("go")]);

        let ex: Vec<String> = doc["language"]["text"]["excludes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        // text's own defaults, plus Go's vendored/build trees.
        assert!(ex.iter().any(|e| e == "**/.git/**"), "{ex:?}");
        assert!(ex.iter().any(|e| e == "vendor/**"), "{ex:?}");
        assert!(ex.iter().any(|e| e == "**/testdata/**"), "{ex:?}");
    }

    #[test]
    fn rust_enabled_and_go_degraded_together() {
        let mut doc = template();
        apply(
            &mut doc,
            &[classify("rust", Availability::Enabled), degraded("go")],
        );
        let cfg = reparse(&doc);
        assert!(cfg.language.rust.enabled);
        assert!(!cfg.language.go.enabled);
        assert!(cfg.language.text.enabled);
        assert!(cfg.language.text.include.iter().any(|g| g == "**/*.go"));
    }

    #[test]
    fn containerized_language_gets_docker_runtime_and_image_not_text() {
        let mut doc = template();
        apply(
            &mut doc,
            &[containerized("go", "ghcr.io/kennworx/kenn-go@sha256:abc")],
        );
        // The authored TOML pins the docker runtime + image on the language.
        assert_eq!(doc["language"]["go"]["enabled"].as_bool(), Some(true));
        assert_eq!(doc["language"]["go"]["runtime"].as_str(), Some("docker"));
        assert_eq!(
            doc["language"]["go"]["image"].as_str(),
            Some("ghcr.io/kennworx/kenn-go@sha256:abc")
        );
        // It still loads as a Config, and is NOT routed to the text fallback.
        let cfg = reparse(&doc);
        assert!(cfg.language.go.enabled);
        assert!(
            !cfg.language.text.include.iter().any(|g| g == "**/*.go"),
            "a containerized language is not a text-fallback language"
        );
    }

    #[test]
    fn containerized_language_seeds_its_test_globs() {
        let mut doc = template();
        apply(
            &mut doc,
            &[containerized(
                "rust",
                "ghcr.io/kennworx/kenn-rust@sha256:abc",
            )],
        );
        let paths: Vec<String> = doc["tests"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(paths.iter().any(|p| p == "**/*_test.rs"), "{paths:?}");
    }

    #[test]
    fn enabling_a_language_strips_its_stale_text_include() {
        // Simulate a config from an earlier degrade: text enabled with **/*.go.
        let mut doc: DocumentMut =
            "[language.text]\nenabled = true\ninclude = [\"**/*.go\", \"**/*.yaml\"]\n"
                .parse()
                .unwrap();
        // Go is now available → enabled; its stale include must be dropped, but
        // an unrelated include (**/*.yaml) must remain.
        apply(&mut doc, &[classify("go", Availability::Enabled)]);

        let include: Vec<String> = doc["language"]["text"]["include"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(!include.contains(&"**/*.go".to_string()), "{include:?}");
        assert!(include.contains(&"**/*.yaml".to_string()), "{include:?}");
    }

    #[test]
    fn tests_paths_seeded_from_enabled_languages_over_empty_template() {
        // The template ships an EMPTY [tests] paths (single source of truth is
        // the detection table), so init seeds it from the enabled languages.
        let mut doc = template();
        apply(&mut doc, &[classify("rust", Availability::Enabled)]);
        let paths: Vec<String> = doc["tests"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(paths.iter().any(|p| p == "**/*_test.rs"), "{paths:?}");
    }

    #[test]
    fn a_user_populated_tests_paths_is_never_modified() {
        let mut doc: DocumentMut =
            "[tests]\npaths = [\"custom/**\"]\n[language.rust]\nenabled = false\n"
                .parse()
                .unwrap();
        apply(&mut doc, &[classify("rust", Availability::Enabled)]);
        let paths: Vec<String> = doc["tests"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            paths,
            vec!["custom/**".to_string()],
            "a user's list is untouched"
        );
    }

    #[test]
    fn tests_paths_seeded_from_enabled_when_absent() {
        // A user config with no [tests] section: seed from enabled languages.
        let mut doc: DocumentMut = "[language.rust]\nenabled = false\n".parse().unwrap();
        apply(&mut doc, &[classify("rust", Availability::Enabled)]);
        let paths: Vec<String> = doc["tests"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(paths.iter().any(|p| p == "**/*_test.rs"), "{paths:?}");
    }

    #[test]
    fn degraded_language_contributes_no_test_globs() {
        let mut doc: DocumentMut = "[nothing]\n".parse().unwrap();
        apply(&mut doc, &[degraded("go")]);
        // No enabled language ⇒ no [tests] seeded (go is degraded, inert).
        assert!(
            doc.get("tests").is_none(),
            "degraded langs seed no test globs"
        );
    }

    #[test]
    fn authoring_preserves_template_comments() {
        let mut doc = template();
        apply(&mut doc, &[classify("rust", Availability::Enabled)]);
        let rendered = doc.to_string();
        assert!(
            rendered.contains("# command = [\"rust-analyzer\"]"),
            "doc-comments survive authoring"
        );
    }

    #[test]
    fn apply_is_idempotent() {
        let mut once = template();
        apply(&mut once, &[degraded("go")]);
        let mut twice = template();
        apply(&mut twice, &[degraded("go")]);
        apply(&mut twice, &[degraded("go")]);
        assert_eq!(
            once.to_string(),
            twice.to_string(),
            "re-authoring is stable"
        );
    }
}
