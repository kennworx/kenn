//! `Config` — the workspace-local `kenn.toml` schema and its loader.

use serde::{Deserialize, Serialize};

use crate::docker::DockerConfig;
use crate::error::ConfigError;
use crate::index::IndexConfig;
use crate::ingest::IngestConfig;
use crate::language::{LanguageConfig, Runtime};
use crate::layout::LayoutConfig;
use crate::lifecycle::LifecycleConfig;
use crate::mcp::McpConfig;
use crate::metrics::MetricsConfig;
use crate::staleness::StalenessConfig;
use crate::tests_config::TestsConfig;
use crate::vectors::VectorsConfig;
use crate::visualize::VisualizeConfig;
use crate::workspace::WorkspaceConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub language: LanguageConfig,
    #[serde(default)]
    pub tests: TestsConfig,
    #[serde(default)]
    pub ingest: IngestConfig,
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    #[serde(default)]
    pub staleness: StalenessConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub visualize: VisualizeConfig,
    #[serde(default)]
    pub layout: LayoutConfig,
    #[serde(default)]
    pub vectors: VectorsConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub docker: DockerConfig,
    /// The XML↔SQL bridge. Top-level, not under `[language.xml]`: the bridge
    /// runs after both producers join and belongs to neither.
    #[serde(default)]
    pub xml_sql: crate::XmlSqlConfig,
}

impl Config {
    pub fn load_from_path(path: &std::path::Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Self = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Load config or return defaults if no file exists at `path`.
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, ConfigError> {
        if path.exists() {
            Self::load_from_path(path)
        } else {
            Ok(Self::default())
        }
    }

    /// A deterministic hash of the indexing-affecting config — the
    /// `[language.*]` blocks that determine *what* gets indexed (enabled
    /// languages, roots, `usage_sources`, per-language excludes). Folded
    /// into the reindex staleness key so flipping a language on (or
    /// otherwise changing the language config) forces a reindex even when
    /// git state is unchanged.
    ///
    /// Only `language` is hashed — `[staleness]`, `[layout]`, and
    /// `[embeddings]` do not change *what* is indexed, so changing them
    /// (e.g. `git_aware_skip`) must not itself force a reindex.
    ///
    /// Total / non-panicking: a serialization error (not expected for the
    /// plain-data `LanguageConfig`) falls back to hashing the `Debug`
    /// rendering, which is just as deterministic for staleness purposes.
    #[must_use]
    pub fn indexing_signature(&self) -> u64 {
        let bytes = serde_json::to_vec(&self.language)
            .unwrap_or_else(|_| format!("{:?}", self.language).into_bytes());
        xxhash_rust::xxh64::xxh64(&bytes, 0)
    }

    /// Post-load validation. Rejects:
    /// * empty `command` arrays on any `[language.*]` block;
    /// * `runtime = "docker"` without an `image`, or an `image` set while
    ///   runtime is not docker;
    /// * absolute or duplicate entries in `python.targets`;
    /// * invalid glob patterns in any `*.excludes` and `[workspace].excludes`.
    fn validate(&self) -> Result<(), ConfigError> {
        for (lang, cmd) in [
            ("csharp", &self.language.csharp.command),
            ("rust", &self.language.rust.command),
            ("typescript", &self.language.typescript.command),
            ("python", &self.language.python.command),
            ("go", &self.language.go.command),
            ("swift", &self.language.swift.command),
        ] {
            if cmd.is_empty() {
                return Err(ConfigError::EmptyCommand { language: lang });
            }
        }
        for (lang, runtime, image) in [
            (
                "csharp",
                self.language.csharp.runtime,
                &self.language.csharp.image,
            ),
            (
                "rust",
                self.language.rust.runtime,
                &self.language.rust.image,
            ),
            (
                "typescript",
                self.language.typescript.runtime,
                &self.language.typescript.image,
            ),
            (
                "python",
                self.language.python.runtime,
                &self.language.python.image,
            ),
            ("go", self.language.go.runtime, &self.language.go.image),
            (
                "swift",
                self.language.swift.runtime,
                &self.language.swift.image,
            ),
        ] {
            let has_image = image.as_deref().is_some_and(|s| !s.is_empty());
            match runtime {
                Runtime::Docker if !has_image => {
                    return Err(ConfigError::DockerImageRequired { language: lang });
                }
                Runtime::Local if has_image => {
                    return Err(ConfigError::ImageWithoutDocker { language: lang });
                }
                _ => {}
            }
        }
        let py = &self.language.python;
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (i, t) in py.targets.iter().enumerate() {
            if std::path::Path::new(t).is_absolute() {
                return Err(ConfigError::AbsoluteTarget {
                    index: i,
                    value: t.clone(),
                });
            }
            if !seen.insert(t.as_str()) {
                return Err(ConfigError::DuplicateTarget { value: t.clone() });
            }
        }
        self.validate_globs()?;
        self.validate_xml_sql()?;
        Ok(())
    }

    /// Every configured glob, from one place: an invalid pattern is a config
    /// error rather than a producer failure at walk time.
    fn validate_globs(&self) -> Result<(), ConfigError> {
        for (scope, patterns) in [
            ("workspace", &self.workspace.excludes),
            ("rust", &self.language.rust.excludes),
            ("typescript", &self.language.typescript.excludes),
            ("csharp", &self.language.csharp.excludes),
            ("python", &self.language.python.excludes),
            ("markdown", &self.language.markdown.excludes),
            ("swift", &self.language.swift.excludes),
            ("text", &self.language.text.excludes),
        ] {
            for (i, pat) in patterns.iter().enumerate() {
                globset::Glob::new(pat).map_err(|e| ConfigError::InvalidGlob {
                    scope,
                    index: i,
                    pattern: pat.clone(),
                    reason: e.to_string(),
                })?;
            }
        }
        // Markdown search-root globs are also globs; validate them too.
        for (i, root) in self.language.markdown.roots.iter().enumerate() {
            globset::Glob::new(&root.glob).map_err(|e| ConfigError::InvalidGlob {
                scope: "markdown.roots",
                index: i,
                pattern: root.glob.clone(),
                reason: e.to_string(),
            })?;
        }
        // Text-fallback include globs are the producer's primary input; validate
        // them at load so a bad glob is a clear config error, not a runtime
        // producer failure.
        for (i, pat) in self.language.text.include.iter().enumerate() {
            globset::Glob::new(pat).map_err(|e| ConfigError::InvalidGlob {
                scope: "text.include",
                index: i,
                pattern: pat.clone(),
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }

    /// A bridge rule naming an element or a role but no attribute identifies no
    /// table — the attribute is what holds the name. Rejected loudly rather than
    /// ignored, because a silently inert rule looks configured: someone would
    /// conclude the bridge cannot see their schema rather than that the rule is
    /// malformed.
    fn validate_xml_sql(&self) -> Result<(), ConfigError> {
        for (i, rule) in self.xml_sql.rules.iter().enumerate() {
            if rule.attribute.trim().is_empty() {
                return Err(ConfigError::XmlSqlRuleWithoutAttribute {
                    index: i,
                    element: rule.element.clone().unwrap_or_default(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{CsharpConfig, PythonConfig, RustConfig, SwiftConfig, TypescriptConfig};
    use std::path::PathBuf;

    /// Empty TOML produces all-defaults. Touches every supported
    /// language and every workspace/test field to catch silent shape
    /// regressions.
    #[test]
    fn defaults_when_empty() {
        let c = Config::from_toml("").unwrap();
        // All languages opt-in by default.
        assert!(!c.language.csharp.enabled);
        assert!(!c.language.rust.enabled);
        assert!(!c.language.typescript.enabled);
        assert!(!c.language.python.enabled);
        assert!(!c.language.csharp.provision_directory_build_props);
        // Default launcher tokens.
        assert_eq!(c.language.csharp.command, vec!["kenn-dotnet".to_string()]);
        assert_eq!(c.language.rust.command, vec!["rust-analyzer".to_string()]);
        assert_eq!(c.language.typescript.command, vec!["kenn-ts".to_string()]);
        assert_eq!(c.language.python.command, vec!["scip-python".to_string()]);
        assert!(c.language.python.targets.is_empty());
        // Per-language excludes default to that language's constant
        // regardless of whether the language is enabled (the field
        // is materialized at deserialization).
        assert_eq!(
            c.language.python.excludes,
            PythonConfig::DEFAULT_EXCLUDES
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
        assert!(c.workspace.excludes.is_empty());
    }

    /// Enabling only Rust does NOT bring Python's excludes into the
    /// Rust field — proves per-language scoping at the config layer.
    #[test]
    fn rust_only_workspace_does_not_inherit_python_excludes() {
        let c = Config::from_toml("[language.rust]\nenabled = true\n").unwrap();
        assert!(c.language.rust.excludes.contains(&"target/**".to_string()));
        // Python's constants are populated on the Python field but
        // never appear in the Rust field.
        assert!(!c
            .language
            .rust
            .excludes
            .contains(&"__pycache__/**".to_string()));
    }

    /// Legacy cross-language `[exclude]` section is rejected by
    /// `deny_unknown_fields`. Replaced by per-language `excludes`.
    #[test]
    fn legacy_exclude_section_errors() {
        let toml = "[exclude]\nglobs = [\"foo/**\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::Toml(_) => {}
            other => panic!("expected Toml deny_unknown_fields error, got {other:?}"),
        }
    }

    /// Pairwise disjointness of per-language `DEFAULT_EXCLUDES` —
    /// catches accidental duplication if a future language's defaults
    /// overlap an existing language's.
    #[test]
    fn default_excludes_constants_are_disjoint() {
        let sets = [
            ("rust", RustConfig::DEFAULT_EXCLUDES),
            ("typescript", TypescriptConfig::DEFAULT_EXCLUDES),
            ("csharp", CsharpConfig::DEFAULT_EXCLUDES),
            ("python", PythonConfig::DEFAULT_EXCLUDES),
            ("swift", SwiftConfig::DEFAULT_EXCLUDES),
        ];
        for (i, (name_a, set_a)) in sets.iter().enumerate() {
            for (name_b, set_b) in sets.iter().skip(i + 1) {
                for pat in *set_a {
                    assert!(
                        !set_b.contains(pat),
                        "{pat:?} appears in both {name_a} and {name_b} DEFAULT_EXCLUDES",
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_absolute_target_with_index() {
        let toml = "[language.python]\ntargets = [\"src/a\", \"/abs/path\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::AbsoluteTarget { index, value } => {
                assert_eq!(index, 1);
                assert_eq!(value, "/abs/path");
            }
            other => panic!("expected AbsoluteTarget, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_target() {
        let toml = "[language.python]\ntargets = [\"src\", \"src\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::DuplicateTarget { value } => assert_eq!(value, "src"),
            other => panic!("expected DuplicateTarget, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_glob_in_python_excludes() {
        let toml = "[language.python]\nexcludes = [\"good/**\", \"bad[unclosed\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::InvalidGlob {
                scope,
                index,
                pattern,
                ..
            } => {
                assert_eq!(scope, "python");
                assert_eq!(index, 1);
                assert_eq!(pattern, "bad[unclosed");
            }
            other => panic!("expected InvalidGlob, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_glob_in_workspace_excludes() {
        let toml = "[workspace]\nexcludes = [\"bad[unclosed\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::InvalidGlob { scope, pattern, .. } => {
                assert_eq!(scope, "workspace");
                assert_eq!(pattern, "bad[unclosed");
            }
            other => panic!("expected InvalidGlob, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_glob_in_text_include() {
        let toml = "[language.text]\ninclude = [\"good/**\", \"bad[unclosed\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::InvalidGlob {
                scope,
                index,
                pattern,
                ..
            } => {
                assert_eq!(scope, "text.include");
                assert_eq!(index, 1);
                assert_eq!(pattern, "bad[unclosed");
            }
            other => panic!("expected InvalidGlob, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_staleness_metrics_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.lifecycle.gc_keep, 2);
        assert!(c.staleness.git_aware_skip);
        assert_eq!(c.metrics.regression_threshold_pct, 10);
    }

    /// End-to-end round-trip — every section parses + the per-language
    /// `excludes` flows through to the resolved Config.
    #[test]
    fn parses_full_document() {
        let toml = r#"
[workspace]
root = "/repo"
excludes = ["sensitive/**"]

[language.csharp]
enabled = true
command = ["/usr/local/bin/kenn-dotnet"]
provision_directory_build_props = true

[language.python]
enabled = true
command = ["bunx", "@sourcegraph/scip-python"]
project_name = "myproj"
project_version = "0.1.0"
excludes = ["worked/**"]

[tests]
paths = ["tests/**"]
"#;
        let c = Config::from_toml(toml).unwrap();
        assert_eq!(c.workspace.root, Some(PathBuf::from("/repo")));
        assert_eq!(c.workspace.excludes, vec!["sensitive/**".to_string()]);
        assert_eq!(
            c.language.csharp.command,
            vec!["/usr/local/bin/kenn-dotnet".to_string()]
        );
        assert!(c.language.csharp.provision_directory_build_props);
        assert!(c.language.python.enabled);
        assert_eq!(
            c.language.python.command,
            vec!["bunx".to_string(), "@sourcegraph/scip-python".to_string()]
        );
        assert_eq!(c.language.python.project_name.as_deref(), Some("myproj"));
        assert_eq!(c.language.python.project_version.as_deref(), Some("0.1.0"));
        assert_eq!(c.language.python.excludes, vec!["worked/**".to_string()]);
        assert_eq!(c.tests.paths, vec!["tests/**".to_string()]);
    }

    /// Flipping a language `enabled` flag changes `indexing_signature`,
    /// while changing a non-language section (`[staleness]`) does not —
    /// the signature tracks *what* is indexed, not freshness policy.
    #[test]
    fn indexing_signature_tracks_language_not_staleness() {
        let base = Config::default();
        let base_sig = base.indexing_signature();

        // Same config → same signature (deterministic).
        assert_eq!(base_sig, Config::default().indexing_signature());

        // Enabling a language changes the signature.
        let mut lang_changed = Config::default();
        lang_changed.language.html.enabled = true;
        assert_ne!(
            base_sig,
            lang_changed.indexing_signature(),
            "enabling a language must change the indexing signature"
        );

        // Flipping a language's runtime to docker changes the signature — a
        // host↔container swap re-indexes (the output is a different producer).
        let mut runtime_changed = Config::default();
        runtime_changed.language.rust.runtime = Runtime::Docker;
        assert_ne!(
            base_sig,
            runtime_changed.indexing_signature(),
            "flipping runtime must change the indexing signature"
        );

        // Changing only `[staleness]` does NOT change the signature.
        let mut staleness_changed = Config::default();
        staleness_changed.staleness.git_aware_skip = !base.staleness.git_aware_skip;
        assert_eq!(
            base_sig,
            staleness_changed.indexing_signature(),
            "a staleness-only change must not perturb the indexing signature"
        );
    }

    #[test]
    fn rejects_empty_command_array() {
        for (lang, toml) in [
            ("csharp", "[language.csharp]\ncommand = []\n"),
            ("rust", "[language.rust]\ncommand = []\n"),
            ("typescript", "[language.typescript]\ncommand = []\n"),
            ("python", "[language.python]\ncommand = []\n"),
        ] {
            let err = Config::from_toml(toml).unwrap_err();
            match err {
                ConfigError::EmptyCommand { language } => assert_eq!(language, lang),
                other => panic!("expected EmptyCommand for {lang}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_docker_runtime_without_image() {
        let err = Config::from_toml("[language.rust]\nruntime = \"docker\"\n").unwrap_err();
        match err {
            ConfigError::DockerImageRequired { language } => assert_eq!(language, "rust"),
            other => panic!("expected DockerImageRequired, got {other:?}"),
        }
        // An empty image string counts as no image.
        Config::from_toml("[language.go]\nruntime = \"docker\"\nimage = \"\"\n")
            .expect_err("empty image under docker is rejected");
    }

    #[test]
    fn rejects_image_without_docker_runtime() {
        let err =
            Config::from_toml("[language.rust]\nimage = \"ghcr.io/x@sha256:a\"\n").unwrap_err();
        match err {
            ConfigError::ImageWithoutDocker { language } => assert_eq!(language, "rust"),
            other => panic!("expected ImageWithoutDocker, got {other:?}"),
        }
    }

    #[test]
    fn accepts_docker_runtime_with_image() {
        let c = Config::from_toml(
            "[language.rust]\nruntime = \"docker\"\nimage = \"ghcr.io/kenn/ra@sha256:a\"\n",
        )
        .expect("docker + image is valid");
        assert_eq!(c.language.rust.runtime, Runtime::Docker);
        assert_eq!(
            c.language.rust.image.as_deref(),
            Some("ghcr.io/kenn/ra@sha256:a")
        );
    }
}
