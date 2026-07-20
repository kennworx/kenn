//! `[visualize]` section — defaults applied by `kenn visualize`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualizeConfig {
    /// Default anchor layout algorithm for bare `kenn visualize`. One
    /// of `"spectral"`, `"force"`, `"stress"`, `"linlog"`. When unset,
    /// the binary falls back to `"spectral"`. Explicit `--algo` on the
    /// CLI overrides this.
    #[serde(default)]
    pub layout: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn layout_optional() {
        let toml = r#"
[visualize]
layout = "force"
"#;
        let c = Config::from_toml(toml).unwrap();
        assert_eq!(c.visualize.layout.as_deref(), Some("force"));
    }
}
