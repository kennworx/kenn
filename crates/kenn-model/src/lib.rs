//! source-data-model: schema, public ID format, and wire types shared by
//! producer (scip-indexing-pipeline) and consumer (mcp-server).

pub mod edge;
pub mod id;
pub mod kind;
pub mod kind_classifier;

pub mod language;
pub mod location;
pub mod record;
pub mod shell_safe;
pub mod short_id;
pub mod tests_config;

pub use edge::{
    EdgeKind, EdgeProperties, EdgeRecord, FieldOp, ImportKind, IsomorphismSource, LinkGrade,
};
pub use id::{IdTransformer, ParsedId, PublicId};
pub use kind::Kind;
pub use language::{Language, ProjectFile};
pub use location::{format_location, parse_location, Range};
pub use record::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisAnchoredCommunityRecord,
    AnalysisFlatCommunityRecord, AnalysisGodNodeRecord, AnalysisNodeMembershipRecord, DefRecord,
    FileDocsRecord, FileRecord, GodNodeFilter, PackageRecord, ShortId, SymbolDocsRecord,
    SymbolRecord,
};
pub use short_id::{compose as compose_short_id, counter_of, partition_of};
pub use tests_config::TestsConfig;
