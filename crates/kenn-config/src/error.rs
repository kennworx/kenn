//! `ConfigError` — every failure mode for `kenn.toml` loading.

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid address `{value}` from {source_name}: {error}")]
    Addr {
        source_name: &'static str,
        value: String,
        error: std::net::AddrParseError,
    },
    #[error(
        "[xml_sql].rules[{index}] needs an `attribute`: an element name alone \
         ({element}) identifies no table"
    )]
    XmlSqlRuleWithoutAttribute { index: usize, element: String },
    #[error("[language.{language}] command must be a non-empty array of tokens")]
    EmptyCommand { language: &'static str },
    #[error("[language.{language}] runtime = \"docker\" requires a non-empty `image`")]
    DockerImageRequired { language: &'static str },
    #[error(
        "[language.{language}] `image` is set but runtime is not \"docker\" — \
         set runtime = \"docker\" or remove `image`"
    )]
    ImageWithoutDocker { language: &'static str },
    #[error("[language.python].targets[{index}] must be relative, got `{value}`")]
    AbsoluteTarget { index: usize, value: String },
    #[error("[language.python].targets contains duplicate entry `{value}`")]
    DuplicateTarget { value: String },
    #[error("[{scope}].excludes[{index}] `{pattern}` is not a valid glob: {reason}")]
    InvalidGlob {
        scope: &'static str,
        index: usize,
        pattern: String,
        reason: String,
    },
}
