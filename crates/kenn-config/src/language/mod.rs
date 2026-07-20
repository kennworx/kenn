//! `[language.*]` sections — per-language indexer config.
//!
//! Each language gets its own submodule with the struct, defaults, and
//! per-language tests. `LanguageConfig` aggregates the per-language
//! sub-configs.

mod csharp;
mod css;
mod go;
mod html;
mod markdown;
mod python;
mod runtime;
mod rust;
mod swift;
mod text;
mod typescript;

pub use csharp::CsharpConfig;
pub use css::{CssConfig, SassConfig};
pub use go::GoConfig;
pub use html::HtmlConfig;
pub use markdown::{MarkdownConfig, MarkdownRoot};
pub use python::PythonConfig;
pub use runtime::Runtime;
pub use rust::RustConfig;
pub use swift::SwiftConfig;
pub use text::TextConfig;
pub use typescript::TypescriptConfig;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageConfig {
    #[serde(default)]
    pub csharp: CsharpConfig,
    #[serde(default)]
    pub rust: RustConfig,
    #[serde(default)]
    pub typescript: TypescriptConfig,
    #[serde(default)]
    pub python: PythonConfig,
    #[serde(default)]
    pub go: GoConfig,
    #[serde(default)]
    pub markdown: MarkdownConfig,
    #[serde(default)]
    pub css: CssConfig,
    #[serde(default)]
    pub html: HtmlConfig,
    #[serde(default)]
    pub swift: SwiftConfig,
    #[serde(default)]
    pub text: TextConfig,
}
