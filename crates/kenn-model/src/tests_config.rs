//! `kenn.toml` `[tests]` section parsing and matching.

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TestsConfigError {
    #[error("toml parse: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid glob `{pattern}`: {source}")]
    BadGlob {
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestsConfig {
    #[serde(default)]
    pub paths: Vec<String>,
}

impl TestsConfig {
    pub fn from_toml(input: &str) -> Result<Self, TestsConfigError> {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            tests: TestsConfig,
        }
        // Accepts either a bare `paths = [...]` document or a parent doc with `[tests]`.
        if let Ok(bare) = toml::from_str::<TestsConfig>(input) {
            if !bare.paths.is_empty() {
                return Ok(bare);
            }
        }
        let wrap: Wrap = toml::from_str(input)?;
        Ok(wrap.tests)
    }

    pub fn matcher(&self) -> Result<TestsMatcher, TestsConfigError> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.paths {
            let glob = Glob::new(pattern).map_err(|source| TestsConfigError::BadGlob {
                pattern: pattern.clone(),
                source,
            })?;
            builder.add(glob);
        }
        let set = builder
            .build()
            .map_err(|source| TestsConfigError::BadGlob {
                pattern: self.paths.join(","),
                source,
            })?;
        Ok(TestsMatcher { set })
    }
}

#[derive(Debug, Clone)]
pub struct TestsMatcher {
    set: GlobSet,
}

impl TestsMatcher {
    /// True if `path` (workspace-relative) matches any configured glob.
    /// Empty pattern list never matches (default-off).
    #[must_use]
    pub fn is_test_file(&self, path: &str) -> bool {
        self.set.is_match(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(patterns: &[&str]) -> TestsMatcher {
        TestsConfig {
            paths: patterns.iter().map(|s| (*s).into()).collect(),
        }
        .matcher()
        .unwrap()
    }

    #[test]
    fn representative_globs_classify_correctly() {
        let m = matcher(&[
            "tests/**",
            "**/*Test.cs",
            "**/*_test.go",
            "**/test_*.py",
            "**/*.test.ts",
            "**/*.spec.ts",
        ]);
        assert!(m.is_test_file("tests/foo.rs"));
        assert!(m.is_test_file("Models/OrderTest.cs"));
        assert!(m.is_test_file("internal/conn_test.go"));
        assert!(m.is_test_file("pkg/test_utils.py"));
        assert!(m.is_test_file("src/api.test.ts"));
        assert!(m.is_test_file("src/api.spec.ts"));
        assert!(!m.is_test_file("src/api.ts"));
        assert!(!m.is_test_file("Models/Order.cs"));
        assert!(!m.is_test_file("internal/conn.go"));
    }

    #[test]
    fn empty_paths_never_matches() {
        let m = matcher(&[]);
        assert!(!m.is_test_file("tests/foo.rs"));
        assert!(!m.is_test_file("any/thing.go"));
    }

    #[test]
    fn parses_from_full_toml_document() {
        let toml = r#"
[tests]
paths = ["tests/**", "**/*_test.go"]
"#;
        let cfg = TestsConfig::from_toml(toml).unwrap();
        assert_eq!(
            cfg.paths,
            vec!["tests/**".to_string(), "**/*_test.go".to_string()]
        );
    }

    #[test]
    fn parses_from_bare_table() {
        let toml = r#"paths = ["tests/**"]"#;
        let cfg = TestsConfig::from_toml(toml).unwrap();
        assert_eq!(cfg.paths, vec!["tests/**".to_string()]);
    }

    #[test]
    fn empty_section_yields_default_off() {
        let cfg = TestsConfig::from_toml("").unwrap();
        assert!(cfg.paths.is_empty());
        assert!(!cfg.matcher().unwrap().is_test_file("tests/foo.rs"));
    }
}
