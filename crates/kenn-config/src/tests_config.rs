//! `[tests]` section — globs identifying test code.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestsConfig {
    /// Globs identifying test code. Authoritative — there are no
    /// built-in fallback patterns. Configure in `kenn.toml`; the
    /// starter file written by `kenn init` ships with a sensible set.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Regexes matched against a C# project's assembly name; a match marks the
    /// whole project as test code — every symbol in it emits `test = true`
    /// (`.NET` only; the kenn-dotnet indexer reads these). Fits a repo whose
    /// test assemblies share a naming convention, e.g. all end in `Test`.
    #[serde(default)]
    pub assembly_regex: Vec<String>,
}
