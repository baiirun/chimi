pub mod backend;
mod capability;
mod compiler;
#[cfg(feature = "native-backend")]
#[doc(hidden)]
pub mod concurrency_lab;
mod decimal;
pub mod diagnostic;
mod doc;
mod doc_examples;
mod doc_site;
mod duration;
mod executable;
pub mod execution;
mod formatter;
#[cfg(feature = "frontend-benchmarks")]
#[doc(hidden)]
pub mod frontend_benchmark;
mod graphemes;
mod hints;
mod ids;
mod inspection;
mod json;
pub mod line_index;
mod lint;
mod manifest;
mod numeric;
mod occurrences;
mod operations;
#[cfg(feature = "native-backend")]
#[doc(hidden)]
pub mod process_lab;
pub mod project;
mod random_source;
mod resolution;
mod semantic;
pub mod source;
mod standard;
mod standard_tree;
mod syntax;
pub mod testing;
mod trace_view;
mod typing;

pub use backend::{BackendOperationProfile, BackendProfile, BackendProgram, BackendValueProfile};
pub use compiler::{
    CheckReport, Compiler, CompilerSession, GenericParameterKind, IncrementalStatistics,
    IncrementalTypeReport, ModuleDiagnostics, ModuleOrigin, NameReport, ProjectPathError,
    ProjectSource, ReferenceInfo, SymbolHandle, SymbolInfo, TypeArenaStats, TypeReport,
    ValuePropertyStatus,
};
pub use doc::{ItemDocumentation, ModuleDocumentation};
pub use doc_site::{DocPage, render_site};
pub use formatter::FormatReport;
pub use hints::TypeHint;
pub use inspection::{
    CallableInspection, ClausePosition, InspectedToken, InspectionError, InspectionReport,
    NeedInspection, ReferenceInspection, SymbolInspection, SyntaxFrame, ValuePropertyInspection,
};
pub use occurrences::{Occurrence, SymbolOccurrences};
pub use operations::StandardTreeVerification;
pub use trace_view::{Locate, TraceError, render_trace};
