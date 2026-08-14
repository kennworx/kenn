//! `kenn.toml` configuration loader.
//!
//! Two distinct schemas live here:
//!
//! - [`Config`] — the **workspace-local** `kenn.toml` checked into a repo.
//!   Owns workspace, language, indexing, and other per-repo settings.
//! - [`GlobalConfig`] — the **per-user** `kenn.toml` at the OS-standard
//!   config path. Owns user-wide server/embeddings settings. The two never
//!   overlap; embedding and server settings are never per-repo.

mod config;
mod docker;
mod error;
mod global;
mod index;
mod ingest;
mod language;
mod layout;
mod lifecycle;
mod mcp;
mod metrics;
mod staleness;
mod tests_config;
mod vectors;
mod visualize;
mod workspace;
mod xml_sql;

pub use config::Config;
pub use docker::DockerConfig;
pub use error::ConfigError;
pub use global::{EmbeddingsConfig, GlobalConfig, ServerConfig};
pub use index::{IndexAnalysisOptions, IndexConfig};
pub use ingest::IngestConfig;
pub use language::{
    CsharpConfig, CssConfig, GoConfig, HtmlConfig, LanguageConfig, MarkdownConfig, MarkdownRoot,
    PythonConfig, Runtime, RustConfig, SassConfig, SqlConfig, SwiftConfig, TextConfig,
    TypescriptConfig, XmlConfig,
};
pub use layout::LayoutConfig;
pub use lifecycle::LifecycleConfig;
pub use mcp::McpConfig;
pub use metrics::MetricsConfig;
pub use staleness::StalenessConfig;
pub use tests_config::TestsConfig;
pub use vectors::VectorsConfig;
pub use visualize::VisualizeConfig;
pub use workspace::WorkspaceConfig;
pub use xml_sql::{TableRole, TableRule, XmlSqlConfig};
